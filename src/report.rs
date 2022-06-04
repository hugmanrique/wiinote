use std::error::Error;
use std::fmt::{Display, Formatter};
use std::io::Cursor;

use anyhow::Result;
use bitflags::bitflags;
use bluer::l2cap;
use bytes::Buf;
use tokio::io::{AsyncWriteExt, BufWriter};

#[derive(Debug, Clone)]
pub enum ReportError {
    Incomplete,
    InvalidTransHeader(u8),
    UnknownReportType(u8),
}

impl Display for ReportError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::Incomplete => write!(f, "Incomplete record contents"),
            Self::InvalidTransHeader(id) => write!(f, "Invalid transaction header {}", id),
            Self::UnknownReportType(id) => write!(f, "Unknown report type {}", id),
        }
    }
}

impl Error for ReportError {}

bitflags! {
    pub struct Buttons: u16 {
        // The status of the power button is not reported.
        const UP = 1 << 11;
        const DOWN = 1 << 10;
        const LEFT = 1 << 8;
        const RIGHT = 1 << 9;

        const A = 1 << 3;
        const B = 1 << 2;

        const PLUS = 1 << 12;
        const HOME = 1 << 7;
        const MINUS = 1 << 4;
        const ONE = 1 << 1;
        const TWO = 1 << 0;
    }
}

impl Buttons {
    pub fn from_common(src: &mut Cursor<&[u8]>) -> Self {
        Self::from_bits_truncate(src.get_u16())
    }
}

bitflags! {
    pub struct Lights: u8 {
        const ONE = 1 << 0;
        const TWO = 1 << 1;
        const THREE = 1 << 2;
        const FOUR = 1 << 3;
    }
}

impl Lights {
    /// Represents the given value as a linear scale on
    /// the four Wiimote lights.
    pub fn scale(value: u8) -> Self {
        let bits = value >> 6; // 0..4
        let mut enabled = 1;
        for i in 0..=bits {
            enabled |= 1 << i;
        }

        Lights::from_bits(enabled).unwrap()
    }
}

#[derive(Debug)]
pub enum InputReport {
    /// Received in response to a `OutputReport::RequestStatus`, or when
    /// an extension is plugged or unplugged. In the latter case,
    /// the user is responsible for resetting the DRM by sending
    /// an `OutputReport::SetDrm` report.
    Status {
        buttons: Buttons,
        lights: Lights,
        battery: u8,
        plugged_ext: bool,
        speaker_enabled: bool,
        ir_enabled: bool,
    },
    /// Data reporting mode used for input reports.
    /// Upon connection, the DRM defaults to `0x30`.
    Drm { buttons: Buttons, mode: u8 },
    /// An `OutputReport` failed or explicit acknowledgement was requested.
    Result {
        buttons: Buttons,
        response_to: u8,
        /// Error identifier, `0` if success.
        code: u8,
    },
}

impl InputReport {
    const TRANS_HEADER: u8 = 0xA1;

    pub fn parse(src: &mut Cursor<&[u8]>) -> Result<Self> {
        // All input reports contain the transaction header, the type ID,
        // and the button statuses.
        if src.remaining() < 3 {
            return Err(ReportError::Incomplete.into());
        }

        let trans_header = src.get_u8();
        if trans_header != Self::TRANS_HEADER {
            return Err(ReportError::InvalidTransHeader(trans_header).into());
        }

        match src.get_u8() {
            0x20 => {
                ensure_readable(src, 6)?;
                let buttons = Buttons::from_common(src);
                let status = src.get_u8();
                src.advance(2); // unknown, always zero

                Ok(Self::Status {
                    buttons,
                    lights: Lights::from_bits(status >> 4).unwrap(),
                    plugged_ext: status & (1 << 1) != 0,
                    speaker_enabled: status & (1 << 2) != 0,
                    ir_enabled: status & (1 << 3) != 0,
                    battery: src.get_u8(),
                })
            }
            0x22 => {
                ensure_readable(src, 4)?;
                Ok(Self::Result {
                    buttons: Buttons::from_common(src),
                    response_to: src.get_u8(),
                    code: src.get_u8(),
                })
            }
            // Data reports; we are only interested in the button states.
            id @ (0x30..=0x37 | 0x3D..=0x3F) => {
                let len = match id {
                    0x30 => 2,
                    0x31 => 5,
                    0x32 => 10,
                    0x33 => 17,
                    0x34..=0x37 | 0x3D..=0x3F => 21,
                    _ => unreachable!(),
                };
                ensure_readable(src, len)?;

                let buttons = if id != 0x3D {
                    Buttons::from_common(src)
                } else {
                    src.advance(2);
                    Buttons::empty()
                };
                // Skip non-button data
                src.advance(len - 2);

                Ok(Self::Drm { buttons, mode: id })
            }
            id => Err(ReportError::UnknownReportType(id).into()),
        }
    }

    /// The currently pressed buttons.
    pub fn buttons(&self) -> Buttons {
        match *self {
            Self::Status { buttons, .. } => buttons,
            Self::Drm { buttons, .. } => buttons,
            Self::Result { buttons, .. } => buttons,
        }
    }
}

fn ensure_readable(src: &mut Cursor<&[u8]>, len: usize) -> Result<()> {
    if src.remaining() >= len {
        Ok(())
    } else {
        Err(ReportError::Incomplete.into())
    }
}

pub enum OutputReport {
    /// Enables/disables the LED lights.
    SetLights(Lights),
    /// Requests a data reporting mode.
    SetDrm { lights: Lights, mode: u8 },
    /// Requests a status report (`InputReport::Status`) from the Wiimote.
    RequestStatus { lights: Lights },
}

impl OutputReport {
    const TRANS_HEADER: u8 = 0xA2;

    pub async fn write(&self, dest: &mut BufWriter<l2cap::Stream>) -> Result<()> {
        dest.write_u8(Self::TRANS_HEADER).await?;
        match *self {
            Self::SetLights(enabled) => {
                dest.write_u8(0x11).await?;
                dest.write_u8(enabled.bits() << 4).await?;
            }
            Self::SetDrm { lights, mode } => {
                dest.write_u8(0x12).await?;
                // Disable continuous reporting; only receive input reports
                // when data has changed.
                dest.write_u8(lights.bits() << 4).await?;
                dest.write_u8(mode).await?;
            }
            Self::RequestStatus { lights } => {
                dest.write_u8(0x15).await?;
                dest.write_u8(lights.bits() << 4).await?;
            }
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Lights;

    #[test]
    fn light_scale() {
        assert_eq!(Lights::scale(0), Lights::ONE);
        assert_eq!(Lights::scale(63), Lights::ONE);
        assert_eq!(Lights::scale(127), Lights::ONE | Lights::TWO);
        assert_eq!(
            Lights::scale(191),
            Lights::ONE | Lights::TWO | Lights::THREE
        );
        assert_eq!(Lights::scale(u8::MAX), Lights::all());
    }
}

use std::io;
use std::io::Cursor;

use anyhow::Result;
use bluer::l2cap::{SocketAddr, Stream};
use bluer::{AddressType, Device};
use bytes::{Buf, BytesMut};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufWriter};

use crate::report::{InputReport, ReportError};
use crate::OutputReport;

/// Sends and receives reports from a Wiimote.
pub struct Connection {
    device: Device,
    _control_stream: Stream,
    stream: BufWriter<Stream>,
    buffer: BytesMut,
}

pub const CONTROL_PSM: u16 = 0x11;
pub const DATA_PSM: u16 = 0x13;

impl Connection {
    /// Connects to the Wiimote without pairing.
    pub async fn connect(device: Device) -> Result<Self> {
        // Establish HID connection with remote by opening the control and
        // data L2CAP streams. The former is essentially unused, but it must
        // be open for communication.
        let addr = device.address();
        let control_sa = SocketAddr::new(addr, AddressType::BrEdr, CONTROL_PSM);
        let data_sa = SocketAddr::new(addr, AddressType::BrEdr, DATA_PSM);

        let control_stream = Stream::connect(control_sa).await?;
        let data_stream = Stream::connect(data_sa).await?;

        Ok(Self::new(device, control_stream, data_stream))
    }

    pub fn new(device: Device, control_stream: Stream, data_stream: Stream) -> Self {
        let mtu = data_stream.as_ref().recv_mtu().unwrap_or(8192);
        Self {
            device,
            _control_stream: control_stream,
            stream: BufWriter::new(data_stream),
            buffer: BytesMut::with_capacity(mtu as usize),
        }
    }

    /// Read a single `InputReport` from the underlying stream.
    ///
    /// # Returns
    /// On success, the received report is returned. If the L2CAP stream is
    /// closed cleanly, it returns `None`. Otherwise, an error is returned.
    pub async fn read_report(&mut self) -> Result<Option<InputReport>> {
        loop {
            // Attempt to parse report from buffered data.
            if let Some(report) = self.parse_report()? {
                return Ok(Some(report));
            }

            // Not enough buffered data to read a report, read more from
            // the stream.
            if self.stream.read_buf(&mut self.buffer).await? == 0 {
                // The stream was closed, check if we have the partial
                // contents of a report.
                if self.buffer.is_empty() {
                    return Ok(None);
                } else {
                    Err(io::Error::from(io::ErrorKind::ConnectionReset))?;
                }
            }
        }
    }

    fn parse_report(&mut self) -> Result<Option<InputReport>> {
        let mut buf = Cursor::new(&self.buffer[..]);
        match InputReport::parse(&mut buf) {
            Ok(report) => {
                // The `parse` function has advanced the cursor until
                // the end of the report. Discard the parsed data from
                // the buffer.
                let len = buf.position() as usize;
                self.buffer.advance(len);

                Ok(Some(report))
            }
            Err(err) => match err.downcast_ref::<ReportError>() {
                // Not enough data has been buffered
                Some(ReportError::Incomplete) => Ok(None),
                _ => Err(err),
            },
        }
    }

    pub async fn write(&mut self, report: &OutputReport) -> Result<()> {
        report.write(&mut self.stream).await?;

        // Ensure the report is written to the socket.
        self.stream.flush().await.map_err(|err| err.into())
    }

    pub fn device(&self) -> &Device {
        &self.device
    }
}

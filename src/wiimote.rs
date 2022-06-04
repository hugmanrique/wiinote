use std::time::Duration;

use anyhow::Result;
use bluer::Device;
use time::interval_at;
use tokio::time;
use tokio::time::Instant;

use crate::connection::Connection;
use crate::report::{Buttons, InputReport, Lights};
use crate::{Keyboard, OutputReport};

/// Indicates the metric to display using the Wiimote lights.
#[derive(Eq, PartialEq)]
enum LightsMode {
    /// Display the battery level.
    Battery,
    /// Display the connection strength level.
    Connection,
}

pub struct Wiimote {
    connection: Connection,
    mode: LightsMode,
    /// The light states, as written in the last `OutputReport` sent.
    prev_lights: Lights,
    /// We are awaiting a `InputReport::Status` in response to a heartbeat.
    awaiting_status: bool,
}

impl Wiimote {
    pub fn new(connection: Connection) -> Self {
        Self {
            connection,
            // Default to battery level, the connection strength is probably
            // high immediately after pairing.
            mode: LightsMode::Battery,
            prev_lights: Lights::all(),
            awaiting_status: false,
        }
    }

    pub async fn run(&mut self, keyboard: &mut Keyboard) -> Result<()> {
        // Writing immediately to the socket results in a "Transport endpoint
        // is not connected" error. Delay the initial heartbeat report.
        let start_send = Instant::now() + Duration::from_secs(1);
        let mut heartbeat = interval_at(start_send, Duration::from_secs(10));

        loop {
            // Listen for the shutdown signal while reading a report.
            let maybe_report = tokio::select! {
                res = self.connection.read_report() => res?,
                _ = heartbeat.tick() => {
                    self.send_heartbeat().await?;
                    continue;
                },
                _ = tokio::signal::ctrl_c() => return Ok(()),
            };

            let report: InputReport = match maybe_report {
                Some(report) => report,
                None => return Ok(()), // the peer closed the socket
            };

            let buttons = report.buttons();
            keyboard.update(buttons)?;

            match buttons {
                Buttons::ONE => self.set_mode(LightsMode::Battery).await?,
                Buttons::TWO => self.set_mode(LightsMode::Connection).await?,
                _ => {}
            };

            if let InputReport::Status { battery, .. } = report {
                if self.awaiting_status {
                    self.awaiting_status = false;

                    // The status report was requested by `send_heartbeat()`,
                    // update the remote lights.
                    if self.mode == LightsMode::Battery {
                        println!("battery: {}", battery);
                        self.set_lights(Lights::scale(battery)).await?;
                    }
                } else {
                    // An extension was plugged or unplugged, reset the DRM.
                    self.connection
                        .write(&OutputReport::SetDrm {
                            lights: self.prev_lights,
                            mode: 0x30, // Core Buttons (default)
                        })
                        .await?;
                }
            }
        }
    }

    async fn set_mode(&mut self, mode: LightsMode) -> Result<()> {
        if self.mode != mode {
            return Ok(());
        }
        self.mode = mode;
        self.send_heartbeat().await
    }

    async fn send_heartbeat(&mut self) -> Result<()> {
        self.connection
            .write(&OutputReport::RequestStatus {
                lights: self.prev_lights,
            })
            .await?;
        self.awaiting_status = true;

        // Update the remote lights
        match self.mode {
            LightsMode::Connection => {
                // Technically, RSSI is a measure of the received intensity,
                // not connection quality. This is good enough for the Wiimote.
                // The scale goes from -80 to 0, where 0 indicates the greatest
                // signal strength.
                let rssi = self.device().rssi().await?.unwrap_or(0);
                println!("rssi: {}", rssi);
                let level = (rssi * u8::MAX as i16 / -80) as u8;

                self.set_lights(Lights::scale(!level)).await
            }
            LightsMode::Battery => {
                // The status report sent in response to the heartbeat
                // contains the battery level. The value is read in `run`
                // and the lights are updated correspondingly.
                Ok(())
            }
        }
    }

    async fn set_lights(&mut self, enabled: Lights) -> Result<()> {
        self.prev_lights = enabled;
        self.connection
            .write(&OutputReport::SetLights(enabled))
            .await
    }

    fn device(&self) -> &Device {
        self.connection.device()
    }
}

use std::time::Duration;

use anyhow::Result;
use bluer::Device;
use tokio::time;

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
}

impl Wiimote {
    pub fn new(connection: Connection) -> Self {
        Self {
            connection,
            // Default to battery level, the connection strength is probably
            // high immediately after pairing.
            mode: LightsMode::Battery,
            prev_lights: Lights::all(),
        }
    }

    pub async fn run(&mut self, keyboard: &mut Keyboard) -> Result<()> {
        let mut heartbeat = time::interval(Duration::from_secs(10));
        loop {
            // Listen for the shutdown signal while reading a report.
            let maybe_report = tokio::select! {
                res = self.connection.read_report() => res?,
                _ = heartbeat.tick() => {
                    self.send_heartbeat().await?;
                    continue;
                }
                _ = tokio::signal::ctrl_c() => {
                    return Ok(());
                }
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

            // Update the remote lights upon receiving a status report,
            // which can requested by `send_heartbeat()`.
            // todo: reset the DRM when an extension is plugged.
            if self.mode == LightsMode::Battery {
                if let InputReport::Status { battery, .. } = report {
                    self.set_lights(Lights::from_level(battery)).await?;
                }
            }
        }
    }

    async fn set_mode(&mut self, mode: LightsMode) -> Result<()> {
        self.mode = mode;
        self.send_heartbeat().await
    }

    async fn send_heartbeat(&mut self) -> Result<()> {
        self.connection
            .write(&OutputReport::RequestStatus {
                lights: self.prev_lights,
            })
            .await?;

        // Update the remote lights
        match self.mode {
            LightsMode::Connection => {
                // Technically, RSSI is a measure of the received intensity,
                // not connection quality. This is good enough for the Wiimote.
                // The scale goes from -80 to 0, where 0 indicates the greatest
                // signal strength.
                let rssi = self.device().rssi().await?.unwrap_or(0);
                let level = rssi * u8::MAX as i16 / -80;

                self.set_lights(Lights::from_level(level as u8)).await
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

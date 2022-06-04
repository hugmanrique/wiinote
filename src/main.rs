use crate::keyboard::Keyboard;


use crate::connection::Connection;
use crate::report::OutputReport;
use crate::wiimote::Wiimote;
use anyhow::{anyhow, Result};
use bluer::{AdapterEvent, Device};
use futures::{pin_mut, StreamExt};



mod connection;
mod keyboard;
mod report;
mod wiimote;

#[tokio::main]
async fn main() -> Result<()> {
    let device = find_wiimote()
        .await?
        .ok_or_else(|| anyhow!("Cannot find Wiimote Bluetooth device"))?;

    println!("Creating keyboard device");
    let mut keyboard = Keyboard::default()?;

    println!("Connecting to {}", device.address());
    let connection = Connection::connect(device).await?;

    let mut wiimote = Wiimote::new(connection);
    wiimote.run(&mut keyboard).await
}

async fn find_wiimote() -> Result<Option<Device>> {
    let session = bluer::Session::new().await?;
    let adapter = session.default_adapter().await?;
    adapter.set_powered(true).await?;

    println!("Discovering using Bluetooth adapter {}\n", adapter.name());
    let events = adapter.discover_devices().await?;
    pin_mut!(events);

    while let Some(event) = events.next().await {
        if let AdapterEvent::DeviceAdded(addr) = event {
            let device = adapter.device(addr)?;
            let alias = device.alias().await?;

            if alias == "Nintendo RVL-CNT-01" || alias == "Nintendo RVL-CNT-01-TR" {
                return Ok(Some(device));
            }
        }
    }
    Ok(None)
}

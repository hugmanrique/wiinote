use anyhow::Result;
use bluer::agent::{Agent, ReqError};
use std::io;

use bluer::{Adapter, AdapterEvent, Device, Session};
use clap::Parser;
use futures::{future, pin_mut, StreamExt};

use crate::connection::Connection;
use crate::keyboard::Keyboard;
use crate::report::OutputReport;
use crate::wiimote::Wiimote;

mod connection;
mod keyboard;
mod report;
mod wiimote;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// Initiates pairing with the Wiimote prior to connecting.
    ///
    /// On disconnection, the Wiimote attempts to re-establish
    /// the connection to the last paired host if any button is
    /// pressed. Otherwise, the Wiimote must be placed in discoverable
    /// mode to re-connect.
    ///
    /// Pairing is required when connecting to a `RVL-CNT-01-TR`
    /// Wiimote model that was placed in discoverable mode by
    /// pressing the 1 and 2 buttons.
    #[clap(long, takes_value = false)]
    pair: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Args = Args::parse();

    let session = Session::new().await?;
    let adapter = session.default_adapter().await?;
    adapter.set_powered(true).await?;

    println!("Opening keyboard device");
    let mut keyboard = Keyboard::default()?;

    // `discover()` returns `None` when the application is shutdown.
    while let Some(connection) = discover(&session, &adapter, args.pair).await? {
        let addr = connection.device().address();
        println!("Device connected: {}", addr);

        let mut wiimote = Wiimote::new(connection);
        if let Err(err) = wiimote.run(&mut keyboard).await {
            println!("Device disconnected: {}", addr);

            if err.downcast_ref::<io::Error>().is_none() {
                return Err(err);
            }
            // The Wiimote disconnected, restart discovery session.
        }
    }
    Ok(())
}

/// Initiates a device discovery session, and opens a connection
/// to the first Wiimote found. If `pair` is `true`, pairs the
/// remote prior to connecting.
///
/// # Returns
/// On success, the Wiimote connection is returned. If the program is
/// shutdown, it returns `None`. Otherwise, an error is returned.
async fn discover(session: &Session, adapter: &Adapter, pair: bool) -> Result<Option<Connection>> {
    assert!(adapter.is_powered().await?);

    println!("Discovering using Bluetooth adapter {}\n", adapter.name());
    let discover = adapter.discover_devices_with_changes().await?;
    pin_mut!(discover);
    loop {
        let event = tokio::select! {
            Some(event) = discover.next() => event,
            _ = tokio::signal::ctrl_c() => break,
            else => break,
        };

        let addr = match event {
            AdapterEvent::DeviceAdded(addr) => addr,
            _ => continue,
        };

        let device = adapter.device(addr)?;
        let alias = device.alias().await?;
        if alias == "Nintendo RVL-CNT-01" || alias == "Nintendo RVL-CNT-01-TR" {
            // All known devices are included in the device stream, even those that
            // are not in range. Ensure that the found Wiimote is in range.
            // Note that if a known device later comes in range, the device is
            // discovered again.
            if device.rssi().await?.is_none() {
                continue;
            }

            // Stop the discovery session.
            drop(discover);

            if pair {
                crate::pair(session, adapter, &device).await?
            }
            let connection = Connection::connect(device).await?;
            return Ok(Some(connection));
        }
    }
    Ok(None)
}

/// Pairs the given Wiimote with the host.
async fn pair(session: &Session, adapter: &Adapter, device: &Device) -> Result<()> {
    if device.is_paired().await? {
        return Ok(());
    }

    println!("Pairing {}", device.address());
    // todo: how do we know if the wiimote is being connected by pressing
    //       the 1 + 2 buttons or the sync button? The PIN code is different.

    // Connecting to the Wiimote to initiate the pairing process
    // requires authenticating using a PIN code. If the Wiimote
    // was placed in discoverable mode by holding the 1 and 2
    // buttons, the PIN code is the Bluetooth address of the
    // Wiimote in little endian. When the sync button is pressed,
    // the PIN code is the Bluetooth address of the host (in
    // little endian).
    let remote_addr = device.address();
    let addr = adapter.address().await?;

    // The `bluer` library takes the PIN as a string. Often, the
    // PIN code is not a valid UTF-8 string, so we must do some
    // *unsafe stuff*.
    let pin_bytes: Vec<u8> = addr.iter().copied().rev().collect();
    let pin_code = unsafe { String::from_utf8_unchecked(pin_bytes) }; // todo: ensure safety

    let agent = Agent {
        request_pin_code: Some(Box::new(move |request| {
            Box::pin(if request.device == remote_addr {
                future::ready(Ok(pin_code.clone()))
            } else {
                // Reject authentication requests for other devices.
                future::err(ReqError::Rejected)
            })
        })),
        ..Default::default()
    };
    // The agent is unregistered when the handle is dropped.
    let _handle = session.register_agent(agent);

    device.pair().await.map_err(|err| err.into())
}

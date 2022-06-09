use std::io;
use std::path::PathBuf;

use anyhow::{bail, Result};
use bluer::agent::{Agent, ReqError};
use bluer::l2cap::{SocketAddr, StreamListener};
use bluer::{Adapter, AdapterEvent, Address, AddressType, Device, Session};
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
    /// Searches and connects to a Wiimote placed in discoverable
    /// mode by pressing the sync button on its back.
    ///
    /// Otherwise, listens for connections from an already paired
    /// Wiimote.
    #[clap(long, takes_value = false)]
    discover: bool,
    /// Initiates pairing with the Wiimote prior to connecting
    /// (assumes `--discover`).
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
    #[clap(long, parse(from_os_str), value_name = "FILE")]
    paired_devices: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Args = Args::parse();

    if !args.discover && args.pair {
        // todo: enforce discover flag instead.
        bail!("Cannot enable pairing in non-discovery mode");
    }

    let session = Session::new().await?;
    let adapter = session.default_adapter().await?;
    adapter.set_powered(true).await?;

    println!("Opening keyboard device");
    let mut keyboard = Keyboard::default()?;

    // Have we connected to a Wiimote successfully?
    let mut have_conn = false;
    loop {
        // Once a Wiimote was paired in a discovery session,
        // the re-connection process via `listen()` is faster.
        let should_discover = args.discover && (!args.pair || !have_conn);
        let maybe_connection = if should_discover {
            discover(&session, &adapter, args.pair).await?
        } else {
            listen(&session, &adapter).await?
        };

        let connection = match maybe_connection {
            Some(connection) => connection,
            // `discover()` and `listen()` return `None` when the
            // application is shutdown.
            None => break,
        };

        have_conn = true;
        let addr = connection.device().address();
        println!("Device connected: {}", addr);

        let mut wiimote = Wiimote::new(connection);
        if let Err(err) = wiimote.run(&mut keyboard).await {
            println!("Device disconnected: {}", addr);

            if err.downcast_ref::<io::Error>().is_none() {
                // The returned error was not caused by I/O.
                return Err(err);
            }
            // The Wiimote disconnected, restart search process.
        }
    }
    Ok(())
}

/*/// Process the connection to the Wiimote.
///
/// # Returns
/// If the Wiimote is disconnected gracefully, returns `Ok`.
/// Otherwise, an error is returned.
async fn handle(connection: Connection) -> Result<()> {
    let addr = connection.device().address();
    println!("Device connected: {}", addr);

    let mut wiimote = Wiimote::new(connection);
    let result = wiimote.run(&mut keyboard).await;
    println!("Device disconnected: {}", addr);

    result
}*/

/// Initiates a device discovery session, and opens a connection
/// to the first Wiimote found. If `pair` is `true`, pairs the
/// remote prior to connecting.
///
/// # Returns
/// On success, the Wiimote connection is returned. If the program is
/// shutdown, it returns `None`. Otherwise, an error is returned.
async fn discover(session: &Session, adapter: &Adapter, pair: bool) -> Result<Option<Connection>> {
    assert!(adapter.is_powered().await?);

    println!("Discovering using Bluetooth adapter {}", adapter.name());
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

/// Opens the control and data L2CAP sockets and listens for
/// connections.
///
/// # Returns
/// On success, the Wiimote connection is returned. If the program is
/// shutdown, it returns `None`. Otherwise, an error is returned.
async fn listen(session: &Session, adapter: &Adapter) -> Result<Option<Connection>> {
    println!("Listening on Bluetooth adapter {}", adapter.name());

    // Register agent to accept authentication requests.
    // todo: only accept those coming from a paired device => remove condition from below.
    let agent = Agent {
        request_authorization: Some(Box::new(|_| Box::pin(async { Ok(()) }))),
        authorize_service: Some(Box::new(|_| Box::pin(async { Ok(()) }))),
        ..Default::default()
    };
    let agent_handle = session.register_agent(agent).await?;

    let control_sa = SocketAddr::new(Address::any(), AddressType::BrEdr, connection::CONTROL_PSM);
    let data_sa = SocketAddr::new(Address::any(), AddressType::BrEdr, connection::DATA_PSM);

    let control_listener = StreamListener::bind(control_sa).await?;
    let data_listener = StreamListener::bind(data_sa).await?;

    loop {
        let ((control_stream, control_sa), (data_stream, data_sa)) = tokio::select! {
            res = async {
                // Wait for both listeners to accept a connection. The tasks are
                // IO-bound, so each task can interleave their processing on
                // the current thread.
                let control_future = control_listener.accept();
                let data_future = data_listener.accept();
                future::try_join(control_future, data_future).await
            } => res?,
            _ = tokio::signal::ctrl_c() => break
        };

        println!(
            "Device connected to both channels: {:?}, {:?}",
            control_sa, data_sa
        );

        if control_sa.addr != data_sa.addr {
            // Each listener accepted a connection from a different remote.
            continue;
        }

        // Reject connections from non-paired devices.
        let device = adapter.device(control_sa.addr)?;
        if !device.is_paired().await? {
            println!("not paired");
            continue;
        }

        drop(agent_handle); // unregister the agent

        let connection = Connection::new(device, control_stream, data_stream);
        return Ok(Some(connection));
    }
    Ok(None)
}

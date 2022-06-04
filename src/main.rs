use anyhow::Result;
use bluer::agent::{Agent, ReqError};
use bluer::l2cap::{SocketAddr, StreamListener};
use bluer::{Adapter, AdapterEvent, AddressType, Device, Session};
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
    /// mode (by pressing the sync button on its back) to re-connect.
    ///
    /// Pairing is required when connecting to a Wiimote that
    /// was placed in discoverable mode by pressing the 1 and
    /// 2 buttons continuously.
    #[clap(long, takes_value = false)]
    pair: bool,
    /// Searches and connects to a Wiimote placed in discoverable
    /// mode by pressing the sync button on its back.
    ///
    /// Otherwise, listens for connections by an already paired
    /// Wiimote.
    #[clap(long, takes_value = false)]
    discover: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Args = Args::parse();

    let session = Session::new().await?;
    let adapter = session.default_adapter().await?;
    adapter.set_powered(true).await?;

    println!("Opening keyboard device");
    let mut keyboard = Keyboard::default()?;

    loop {
        let maybe_connection = if args.discover {
            discover(&session, &adapter, args.pair).await?
        } else {
            listen(&adapter).await?
        };

        let connection = match maybe_connection {
            Some(connection) => connection,
            // `discover()` and `listen()` return `None` when the
            // application is shutdown.
            None => break,
        };

        println!("Device connected: {}", connection.device().address());
        let mut wiimote = Wiimote::new(connection);
        wiimote.run(&mut keyboard).await?;
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
    let discover = adapter.discover_devices().await?;
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

        // todo: ensure device is in range. The docs recommend using the `rssi` property.
        //       Don't throw if an error occurs, instead `continue` with next device.
        let device = adapter.device(addr)?;
        let alias = device.alias().await?;
        if alias == "Nintendo RVL-CNT-01" || alias == "Nintendo RVL-CNT-01-TR" {
            // We found a Wiimote, stop the discovery session.
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
async fn listen(adapter: &Adapter) -> Result<Option<Connection>> {
    let adapter_addr = adapter.address().await?;
    println!(
        "Listening on Bluetooth adapter {} with address {}\n",
        adapter.name(),
        adapter_addr
    );

    let control_sa = SocketAddr::new(adapter_addr, AddressType::BrEdr, connection::CONTROL_PSM);
    let data_sa = SocketAddr::new(adapter_addr, AddressType::BrEdr, connection::DATA_PSM);

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

        if control_sa != data_sa {
            // Each listener accepted a connection from a different remote.
            continue;
        }

        // Create the `Connection` directly: the Wiimote connected to the
        // host, so it is already paired.
        // todo: ensure this is indeed the case
        // todo: keep list of paired devices and reject if not in list?
        let device = adapter.device(control_sa.addr)?;
        let connection = Connection::new(device, control_stream, data_stream);
        return Ok(Some(connection));
    }
    Ok(None)
}

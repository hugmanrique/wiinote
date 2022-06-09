mod keyboard;

use crate::keyboard::Keyboard;
use anyhow::Result;
use clap::Parser;
use futures_util::stream::TryStreamExt;
use std::path::PathBuf;
use xwiimote::event::EventKind;
use xwiimote::{Address, Channels, Device, Monitor};

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// Searches and connects to a Wiimote placed in discoverable
    /// mode by pressing the sync button on its back.
    ///
    /// Otherwise, listens for connections from an already paired
    /// Wii Remote.
    #[clap(long, takes_value = false)]
    discover: bool,
    /// Opens the Wii Remote device at the given location.
    #[clap(parse(from_os_str), value_name = "FILE")]
    device: Option<PathBuf>,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let args: Args = Args::parse();

    println!("Opening keyboard device");
    let mut keyboard = Keyboard::try_default()?;

    if let Some(path) = args.device {
        let address = Address::from(path);
        connect(&address, &mut keyboard).await?
    } else {
        while let Some(address) = find_device(args.discover).await? {
            connect(&address, &mut keyboard).await?;
        }
        // The monitor never returns `None` in discovery mode.
        eprintln!("No connected devices found");
    }
    Ok(())
}

async fn find_device(discover: bool) -> Result<Option<Address>> {
    if discover {
        println!("Discovering devices");
    } else {
        println!("Enumerating connected devices");
    }

    let mut monitor = Monitor::new(discover)?;
    monitor.try_next().await.map_err(|err| err.into())
}

async fn connect(address: &Address, keyboard: &mut Keyboard) -> Result<()> {
    let mut device = Device::connect(address)?;
    let name = device.kind()?;

    device.open(Channels::CORE, false)?;
    println!("Device connected: {}", name);

    let mut event_stream = device.events()?;
    while let Some(event) = event_stream.try_next().await? {
        match event.kind {
            EventKind::Key(key, state) => {
                keyboard.update(&key, &state)?;
            }
            _ => {}
        }
    }
    println!("Device disconnected: {}", name);
    Ok(())
}

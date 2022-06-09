mod keyboard;

use crate::keyboard::Keyboard;
use anyhow::Result;
use clap::Parser;
use futures_util::stream::TryStreamExt;
use num_traits::FromPrimitive;
use std::path::PathBuf;
use std::time::Duration;
use xwiimote::event::{Event, EventKind, Key};
use xwiimote::{Address, Channels, Device, Led, Monitor};

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// Searches and connects to a Wii Remote placed in discoverable
    /// mode after failing to connect to an already plugged-in
    /// Wii Remote.
    ///
    /// If the connection is dropped, the program restarts the
    /// discovery session until a new Wii Remote is found.
    ///
    /// When not set, the program exits if no plugged-in Wii Remote
    /// is found.
    #[clap(long, takes_value = false)]
    discover: bool,
    /// Opens the Wii Remote device at the given location.
    ///
    /// If not present, connects to the first Wii Remote found;
    /// see the `--discover` option for more.
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

/// Initiates the connection to the given address.
///
/// # Returns
/// On success, the function blocks until the device is disconnected
/// gracefully, returning `Ok`. Otherwise, an error is raised.
async fn connect(address: &Address, keyboard: &mut Keyboard) -> Result<()> {
    let mut device = Device::connect(address)?;
    let name = device.kind()?;

    device.open(Channels::CORE, true)?;
    println!("Device connected: {}", name);

    handle(&mut device, keyboard).await?;
    println!("Device disconnected: {}", name);
    Ok(())
}

/// The metrics that can be displayed in a [`LightDisplay`].
enum LightsMetric {
    /// Display the battery level.
    Battery,
    /// Display the connection strength level.
    Connection,
}

/// The set of lights of a Wii Remote, used as a display.
struct LightDisplay<'a> {
    device: &'a Device,
    metric: LightsMetric,
    interval: tokio::time::Interval,
}

impl<'a> LightDisplay<'a> {
    pub fn new(device: &'a Device) -> Self {
        Self {
            device,
            // Default to battery level, the connection strength is
            // probably high immediately after pairing.
            metric: LightsMetric::Battery,
            interval: tokio::time::interval(Duration::from_secs(20)),
        }
    }

    pub async fn tick(&mut self) -> tokio::time::Instant {
        self.interval.tick().await
    }

    /// Updates the Wii Remote lights according to the current metric.
    pub async fn update(&self) -> Result<()> {
        let level = match self.metric {
            LightsMetric::Battery => self.device.battery()?,
            LightsMetric::Connection => {
                // Technically, RSSI is a measure of the received intensity,
                // not connection quality. This is good enough for the Wii Remote.
                // The scale goes from -80 to 0, where 0 indicates the greatest
                // signal strength.
                let rssi = 0; // todo
                !((rssi * u8::MAX as i16 / -80) as u8)
            }
        };

        // `level` is a value from 0 to u8::MAX.
        let last_ix = 1 + (level >> 6); // 1..=4
        for ix in 1..=4 {
            let light = Led::from_u8(ix).unwrap();
            self.device.set_led(light, ix <= last_ix)?;
        }

        Ok(())
    }

    /// Updates the displayed metric.
    pub async fn set_metric(&mut self, metric: LightsMetric) -> Result<()> {
        self.metric = metric;
        self.update().await
    }
}

/// Process the connection to the Wii Remote.
///
/// # Returns
/// If the device is disconnected gracefully, returns `Ok`. Otherwise,
/// an error is returned.
async fn handle(device: &mut Device, keyboard: &mut Keyboard) -> Result<()> {
    let mut event_stream = device.events()?;
    let mut display = LightDisplay::new(device);

    loop {
        let maybe_event = tokio::select! {
            res = event_stream.try_next() => res?,
            _ = display.tick() => {
                display.update().await?;
                continue;
            }
        };

        let event: Event = match maybe_event {
            Some(event) => event,
            None => return Ok(()), // connection closed
        };

        if let EventKind::Key(key, state) = event.kind {
            match key {
                Key::One => display.set_metric(LightsMetric::Battery).await?,
                Key::Two => display.set_metric(LightsMetric::Connection).await?,
                _ => keyboard.update(&key, &state)?,
            };
        }
    }
}

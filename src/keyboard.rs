use crate::report::Buttons;
use anyhow::Result;
use uinput::event::keyboard::{Key, Misc};
use uinput::{event, Device};

static DEV_NAME: &str = "Wiinote";

pub struct Keyboard(Device);

impl Keyboard {
    pub fn default() -> Result<Self> {
        let events = [
            event::Keyboard::Key(Key::Up),
            event::Keyboard::Key(Key::Down),
            event::Keyboard::Key(Key::Left),
            event::Keyboard::Key(Key::Right),
            event::Keyboard::Key(Key::Enter),
            event::Keyboard::Misc(Misc::VolumeUp),
            event::Keyboard::Key(Key::Esc),
            event::Keyboard::Misc(Misc::VolumeDown),
        ];

        let mut builder = uinput::default()?.name(DEV_NAME)?;
        for event in events {
            builder = builder.event(event)?;
        }

        Ok(Self(builder.create()?))
    }

    pub fn update(&mut self, buttons: Buttons) -> Result<()> {
        let events = buttons.iter().filter_map(|(_, button)| button.key_event());
        for event in events {
            self.0.click(&event)?;
        }

        self.0.synchronize().map_err(|err| err.into())
    }
}

impl Buttons {
    pub fn key_event(&self) -> Option<event::Keyboard> {
        Some(match *self {
            Buttons::UP => event::Keyboard::Key(Key::Up),
            Buttons::DOWN => event::Keyboard::Key(Key::Down),
            Buttons::LEFT => event::Keyboard::Key(Key::Left),
            Buttons::RIGHT => event::Keyboard::Key(Key::Right),
            Buttons::A => event::Keyboard::Key(Key::Enter),
            Buttons::B => event::Keyboard::Key(Key::Left),
            Buttons::PLUS => event::Keyboard::Misc(Misc::VolumeUp),
            Buttons::HOME => event::Keyboard::Key(Key::Esc),
            Buttons::MINUS => event::Keyboard::Misc(Misc::VolumeDown),
            // todo: add events for button combinations. This also requires changing `update()`
            _ => return None,
        })
    }
}

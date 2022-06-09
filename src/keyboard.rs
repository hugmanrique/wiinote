use anyhow::Result;
use uinput::event;
use uinput::event::keyboard;
use xwiimote::event::{Key, KeyState};

static DEV_NAME: &str = "Wiinote";

pub struct Keyboard(uinput::Device);

impl Keyboard {
    pub fn try_default() -> Result<Self> {
        let events = [
            event::Keyboard::Key(keyboard::Key::Up),
            event::Keyboard::Key(keyboard::Key::Down),
            event::Keyboard::Key(keyboard::Key::Left),
            event::Keyboard::Key(keyboard::Key::Right),
            event::Keyboard::Key(keyboard::Key::Enter),
            event::Keyboard::Misc(keyboard::Misc::VolumeUp),
            event::Keyboard::Key(keyboard::Key::Esc),
            event::Keyboard::Misc(keyboard::Misc::VolumeDown),
        ];

        let mut builder = uinput::default()?.name(DEV_NAME)?;
        for event in events {
            builder = builder.event(event)?;
        }

        Ok(Self(builder.create()?))
    }

    pub fn update(&mut self, button: &Key, state: &KeyState) -> Result<()> {
        if let Some(key) = key_event(&button) {
            match *state {
                KeyState::Down => self.0.press(&key)?,
                KeyState::Up => self.0.release(&key)?,
                _ => {}
            };
            self.0.synchronize().map_err(|err| err.into())
        } else {
            Ok(()) // The button is not matched to any key, ignore.
        }
    }
}

/// Converts the Wii Remote key to a keyboard event.
pub fn key_event(key: &Key) -> Option<event::Keyboard> {
    Some(match *key {
        Key::Up => event::Keyboard::Key(keyboard::Key::Up),
        Key::Down => event::Keyboard::Key(keyboard::Key::Down),
        Key::Left => event::Keyboard::Key(keyboard::Key::Left),
        Key::Right => event::Keyboard::Key(keyboard::Key::Right),
        Key::A => event::Keyboard::Key(keyboard::Key::Enter),
        Key::B => event::Keyboard::Key(keyboard::Key::Left),
        Key::Plus => event::Keyboard::Misc(keyboard::Misc::VolumeUp),
        Key::Home => event::Keyboard::Key(keyboard::Key::Esc),
        Key::Minus => event::Keyboard::Misc(keyboard::Misc::VolumeDown),
        _ => return None,
    })
}

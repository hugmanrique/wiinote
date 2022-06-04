# wiinote

Use a [Wiimote](https://en.wikipedia.org/wiki/Wii_Remote) as a slide clicker.

Follows a structure similar to the Tokio project's [mini-redis](https://github.com/tokio-rs/mini-redis) example.
Special thanks to David Herrmann et al. for documenting the [Wiimote HID protocol](https://raw.githubusercontent.com/dvdhrm/xwiimote/master/doc/PROTOCOL).

## Build

```shell
cargo build --release
```

You'll need the following things to build wiinote:
- Rust >= 1.61.0
- libdbus-1-dev >= 1.12.20
- libudev-dev >= 248.3

## Setup

```bash
modprobe uinput
systemctl enable bluetooth.service

# Allow non-root user to access device file
groupadd -f uinput
gpasswd -a $USER uinput
cat >/lib/udev/rules.d/40-input.rules <<EOL
KERNEL=="uinput", SUBSYSTEM=="misc", GROUP="uinput", MODE="0660"
EOL

# Reload udev rules
udevadm control --reload-rules && udevadm trigger
```

## License
[MIT](LICENSE) @ [Hugo Manrique](https://hugmanrique.me)
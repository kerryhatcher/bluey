# Seeed XIAO ESP32-C6 (scanner)

The [Seeed XIAO ESP32-C6](https://wiki.seeedstudio.com/xiao_esp32c6_getting_started/) runs the
bluey scanner firmware in [`firmware/`](../firmware). It listens for radio traffic and keeps a table
of what it hears, which the `bluey` CLI reads over USB. It can scan for one of three things at a time:

| Mode | Hears | CLI name |
|------|-------|----------|
| BLE (default) | Bluetooth LE advertisements: phones, AirTags, AirPods, SmartTags, Tile, sensors | `ble` |
| WiFi | WiFi access points (2.4 GHz only) | `wifi` |
| 802.15.4 | Zigbee and Thread frames, channels 11 to 26 | `802154` |

The README's [Firmware](../README.md#firmware) and [802.15.4](../README.md#802154-zigbee-and-thread)
sections cover the details of each mode. This page is about the board.

## Hardware

| | |
|---|---|
| Chip | ESP32-C6 (RISC-V, single core, with BLE, WiFi 6 and 802.15.4) |
| Target | `riscv32imac-unknown-none-elf` |
| Logic level | 3.3 V |
| USB | USB-C, using the chip's built-in USB Serial/JTAG controller |
| Flash | Chip's external flash, with an `nvs` partition the firmware uses to save the mode |

The firmware talks to the computer through the built-in USB Serial/JTAG controller. It appears as a
serial port (`/dev/ttyACM*`), and carries both the binary messages to the CLI and the log lines.

## Build and flash

The firmware has its own cargo config, so build it from inside `firmware/`. The C6 uses the
standard Rust RISC-V toolchain, pinned by `firmware/rust-toolchain.toml`.

```
cargo install espflash
cd firmware
cargo run --release                                                   # BLE and WiFi
cargo run --release --no-default-features --features ieee802154       # BLE and 802.15.4
```

The default build is BLE plus WiFi. The `ieee802154` build is BLE plus 802.15.4. A single image cannot
have WiFi and 802.15.4 together, because the radio library does not allow it.

To flash one board when several are plugged in:

```
espflash flash --chip esp32c6 -p /dev/ttyACM1 target/riscv32imac-unknown-none-elf/release/bluey-firmware
```

The `runner` in `firmware/.cargo/config.toml` is `espflash flash --chip esp32c6`, so `cargo run`
flashes the board it finds. On Linux, your user needs access to the serial port (the `dialout` group
on most distributions).

## Using it

Use the CLI rather than a serial monitor. The firmware sends binary snapshots, so a plain monitor shows
garbage.

```
bluey devices                 # shows the board, its mode and what it can scan for
bluey mode wifi -p /dev/ttyACM1
bluey monitor
```

The mode is saved in flash, so the board comes back in the same mode after a reset or reflash.
Changing mode clears the tables.

## Limits

- **One radio, one mode at a time.** Each board scans for a single thing. Use several boards to watch several things.
- **Table sizes.** Up to 64 BLE devices, 64 WiFi access points, and 64 802.15.4 nodes. Entries not heard for 2 minutes are dropped.
- **802.15.4 only listens.** It never sends, so sleeping or rarely transmitting devices can be missed.
- **Find My accessories can't be told apart**, and phones change address about every 15 minutes.

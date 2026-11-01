# bluey

ESP32-C6 firmware for doing blue things. It scans for Bluetooth LE advertisements
and logs a table of the devices it hears nearby: phones, Apple Find My accessories
(AirTags), AirPods, Samsung SmartTags, Tile and so on.

Written in Rust (`no_std`) with [`esp-hal`](https://github.com/esp-rs/esp-hal),
Embassy and the [`trouble-host`](https://github.com/embassy-rs/trouble) BLE stack.
Built for a Seeed XIAO ESP32-C6, but any ESP32-C6 board should work.

## Output

Every 10 seconds the firmware prints one line per device, sorted by address:

```
---- 3 devices ----
7A:D3:F9:09:57:82 ( rpa) rssi= -65 [ -80.. -63] n=101  seen  22s ago: Apple Nearby Info (iPhone/iPad/Mac/Watch)
CA:4B:97:87:FD:C3 (static) rssi= -74 [ -82.. -71] n=19   seen   1s ago: Apple Find My (AirTag/accessory)
```

| Field | Meaning |
|-------|---------|
| address | Device address, most significant byte first |
| `(static)`, `(public)`, `(rpa)`, `(nrpa)` | Address type. `rpa`/`nrpa` rotate periodically (phones, AirTags) |
| `rssi=` | Last signal strength in dBm, then `[min..max]` since first heard |
| `n=` | Advertisements heard |
| `seen … ago` | Time since the device was last heard |

Devices not heard for 2 minutes are dropped from the table.

## Build and flash

Requires Rust (stable) and [`espflash`](https://github.com/esp-rs/espflash):

```
rustup target add riscv32imac-unknown-none-elf
cargo install espflash
cargo run --release   # builds, flashes and opens the serial monitor
```

The board is flashed and monitored over its USB port, so no external probe is needed.
On Linux your user needs access to `/dev/ttyACM0` (the `dialout` group on most distros).

## Layout

- `src/bin/main.rs` sets up BLE, runs the scanner and prints the report.
- `src/adv.rs` parses advertising data and classifies the device.
- `src/table.rs` is the table of devices heard so far.

## Limitations

- **Find My detection is approximate.** AirTags and other Find My accessories send the
  same advertisement, so they can't be told apart.
- **Rotating addresses.** Phones change address about every 15 minutes, so one phone can
  show up as several devices over time.
- **Legacy advertisements only.** BLE 5 extended advertising isn't handled.
- The ESP32-C6 controller ignores the duplicate filter, so duplicates are merged in the
  device table instead.

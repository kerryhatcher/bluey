# bluey

ESP32-C6 firmware for doing blue things, and a command line tool to read it. The device
scans for Bluetooth LE advertisements and WiFi access points, and keeps a table of what it
hears: phones, Apple Find My accessories (AirTags), AirPods, Samsung SmartTags, Tile,
routers and so on. The CLI reads that table over USB and logs it.

Written in Rust with [`esp-hal`](https://github.com/esp-rs/esp-hal), Embassy and the
[`trouble-host`](https://github.com/embassy-rs/trouble) BLE stack. Built for a Seeed XIAO
ESP32-C6, but any ESP32-C6 board should work.

## Layout

This is a cargo workspace.

| Path | What it is |
|------|------------|
| `crates/bluey-core` | Shared by both sides, `no_std`: the messages the device sends, their framing on the serial port, and BLE advertisement parsing |
| `crates/bluey-cli` | The `bluey` command line tool |
| `firmware/` | The ESP32-C6 firmware |

The firmware targets RISC-V and has its own cargo config, so it is built from inside
`firmware/`. A plain `cargo build` at the root builds only the host crates.

## Firmware

Requires Rust (stable, via the `rust-toolchain.toml`) and [`espflash`](https://github.com/esp-rs/espflash):

```
cargo install espflash
cd firmware
cargo run --release   # builds and flashes
```

The board is flashed and read over its USB port, so no external probe is needed. On Linux
your user needs access to `/dev/ttyACM0` (the `dialout` group on most distros).

Every 10 seconds the device sends a snapshot of everything it has heard recently, as a
binary message on the USB serial port (and its log lines the same way), so a plain serial
monitor shows garbage. Use the CLI.

## CLI

```
cargo install --path crates/bluey-cli    # or: cargo run -p bluey-cli -- <command>

bluey ports                   # list serial ports; the ESP32 is marked
bluey monitor                 # show snapshots and device logs as they arrive
bluey snapshot [--json]       # wait for the next snapshot, print it, exit
bluey log -o hears.jsonl      # append every snapshot to a file, one JSON object per line
```

The port is found automatically if one ESP32 is plugged in, otherwise pick it with `--port`.

`monitor` prints one line per device, sorted by address:

```
---- 2 devices ----
6B:E8:AD:69:A4:CE (   rpa) rssi= -79 [ -80.. -79] n=4    seen  17s ago: Apple Nearby Info (iPhone/iPad/Mac/Watch)
FE:56:EE:B4:8F:83 (static) rssi= -80 [ -80.. -79] n=2    seen   0s ago: Apple Find My (AirTag/accessory)
---- 1 wifi access points ----
F4:92:BF:92:56:C9 ch1  rssi= -61 [ -61.. -59] n=5    seen   6s ago: HatchNet Wpa2Personal
```

| Field | Meaning |
|-------|---------|
| address | BLE address, or the access point's BSSID |
| `(static)`, `(public)`, `(rpa)`, `(nrpa)` | BLE address type. `rpa`/`nrpa` rotate periodically (phones, AirTags) |
| `rssi=` | Last signal strength in dBm, then `[min..max]` since first heard |
| `n=` | Advertisements heard (BLE) or scans that found it (WiFi) |
| `seen … ago` | Time since last heard |

Devices not heard for 2 minutes are dropped from the device's table.

Each line of `bluey log` is a snapshot with the time it was received, and every device with
its first and last seen times as wall-clock timestamps:

```json
{"time":"2026-11-01T03:54:09Z","uptime_ms":30386,"ble":[{"addr":"6B:E8:AD:69:A4:CE","addr_type":"rpa","kind":"Apple Nearby Info (iPhone/iPad/Mac/Watch)","name":null,"rssi":-79,"rssi_min":-80,"rssi_max":-79,"count":4,"first_seen":"2026-11-01T03:53:39.362Z","last_seen":"2026-11-01T03:54:01.43Z"}],"wifi":[]}
```

## Development

```
cargo test                                  # host crates
cargo clippy --workspace --exclude bluey-firmware --all-targets
cd firmware && cargo clippy --release       # firmware
```

## Limitations

- **Find My detection is approximate.** AirTags and other Find My accessories send the
  same advertisement, so they can't be told apart.
- **Rotating addresses.** Phones change address about every 15 minutes, so one phone can
  show up as several devices over time.
- **BLE and WiFi share one radio.** Scanning both at once starves both: BLE sightings drop
  sharply and many WiFi scans come back empty. The tables keep what was found earlier.
- **WiFi sees access points only**, not the phones and laptops connected to them. 2.4 GHz only.
- **Legacy BLE advertisements only.** BLE 5 extended advertising isn't handled.
- The ESP32-C6 controller ignores the duplicate filter, so duplicates are merged in the
  device table instead.
- The CLI only listens. The device streams snapshots; it can't be asked for anything yet.

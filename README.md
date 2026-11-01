# bluey

ESP32-C6 firmware for doing blue things, and a command line tool to read it. The device
scans for Bluetooth LE advertisements, WiFi access points or Zigbee and Thread devices, and
keeps a table of what it hears: phones, Apple Find My accessories (AirTags), AirPods,
Samsung SmartTags, Tile, routers, smart home sensors and so on. The CLI reads that table over
USB and logs it.

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
| `vendor/esp-radio` | `esp-radio` with two small fixes for 802.15.4, see `BLUEY-PATCHES.md` in it |

The firmware targets RISC-V and has its own cargo config, so it is built from inside
`firmware/`. A plain `cargo build` at the root builds only the host crates.

## Firmware

Requires Rust (stable, via the `rust-toolchain.toml`) and [`espflash`](https://github.com/esp-rs/espflash).

```
cargo install espflash
cd firmware
cargo run --release                                          # BLE and WiFi
cargo run --release --no-default-features --features ieee802154   # BLE and 802.15.4
```

**There are two builds.** The radio library can't have WiFi and 802.15.4 in the same image, so
each device can scan for BLE and *one* of the other two, depending on which build is on it.
`bluey devices` shows what each one can do. To get all three, flash at least two devices
differently. To flash a particular device when several are plugged in, use
`espflash flash --chip esp32c6 -p /dev/ttyACM1 <path to the binary>`; the binary is
`target/riscv32imac-unknown-none-elf/release/bluey-firmware`.

The board is flashed and read over its USB port, so no external probe is needed. On Linux
your user needs access to `/dev/ttyACM0` (the `dialout` group on most distros).

The device has one radio, so it scans for **one thing at a time**: Bluetooth LE devices,
WiFi access points or 802.15.4 devices. It switches when the CLI tells it to. Switching
shuts one radio down before starting the other, and clears the tables. A new device starts in
BLE mode.

Every 10 seconds, and whenever the CLI asks, the device sends a snapshot of what it has heard
as a binary message on the USB serial port (its log lines go the same way), so a plain serial
monitor shows garbage. Use the CLI.

The mode is saved in flash when it changes, so a device comes back in the same mode after a
reset or power cycle, and reflashing the firmware keeps it. It is stored in the first sector
of the `nvs` partition, which nothing else on the device uses.

## CLI

```
cargo install --path crates/bluey-cli    # or: cargo run -p bluey-cli -- <command>

bluey ports                   # list serial ports; the ESP32s are marked
bluey devices                 # each device: port, id, mode, what it has heard
bluey mode wifi               # switch devices to WiFi (or `ble`, or `802154`)
bluey monitor                 # show snapshots and device logs as they arrive
bluey snapshot [--json]       # print a snapshot from each device and exit
bluey log -o hears.jsonl      # append every snapshot to a file, one JSON object per line
```

### Several devices

Every command works on **all** the ESP32s that are plugged in, unless you pick some with
`--port`. A port can be followed by the mode to put that device in first, so one device
can scan for each thing, with nothing sharing a radio:

```
bluey monitor -p /dev/ttyACM0:ble -p /dev/ttyACM1:wifi -p /dev/ttyACM2:802154
bluey log -o hears.jsonl -p /dev/ttyACM0:ble -p /dev/ttyACM1:wifi -p /dev/ttyACM2:802154
bluey mode wifi -p /dev/ttyACM0       # change one while others keep going
```

Devices are told apart by their chip MAC address (the `device` field in the log), which
stays the same if a device ends up on a different port.

### Output

`monitor` prints one line per device or access point, sorted by address:

```
---- ttyACM0 [ble]: 2 devices ----
6B:E8:AD:69:A4:CE (   rpa) rssi= -79 [ -80.. -79] n=4    seen  17s ago: Apple Nearby Info (iPhone/iPad/Mac/Watch)
FE:56:EE:B4:8F:83 (static) rssi= -80 [ -80.. -79] n=2    seen   0s ago: Apple Find My (AirTag/accessory)
---- ttyACM1 [wifi]: 1 wifi access points ----
F4:92:BF:92:56:C9 ch1  rssi= -61 [ -61.. -59] n=5    seen   6s ago: HatchNet Wpa2Personal
---- ttyACM2 [802154]: 2 802.15.4 devices ----
PAN 9297 3148                    (short) ch25 rssi= -75 [ -75.. -74] n=2    seen  36s ago: Zigbee
PAN BEEF 7357                    (short) ch15 rssi= -61 [ -62.. -61] n=18   seen  73s ago: Thread network="MyHome"
```

| Field | Meaning |
|-------|---------|
| address | BLE address, the access point's BSSID, or for 802.15.4 the PAN (network) and the device's address |
| `(static)`, `(public)`, `(rpa)`, `(nrpa)` | BLE address type. `rpa`/`nrpa` rotate periodically (phones, AirTags) |
| `rssi=` | Last signal strength in dBm, then `[min..max]` since first heard |
| `n=` | Advertisements heard (BLE), scans that found it (WiFi) or frames heard (802.15.4) |
| `seen … ago` | Time since last heard |

Entries not heard for 2 minutes are dropped from the device's table.

Each line of `bluey log` is a snapshot from one device, with the time it was received, and
every entry with its first and last seen times as wall-clock timestamps:

```json
{"time":"2026-11-01T03:54:09Z","port":"/dev/ttyACM0","device":"90:64:9B:AA:29:00","mode":"ble","uptime_ms":30386,"ble":[{"addr":"6B:E8:AD:69:A4:CE","addr_type":"rpa","kind":"Apple Nearby Info (iPhone/iPad/Mac/Watch)","name":null,"rssi":-79,"rssi_min":-80,"rssi_max":-79,"count":4,"first_seen":"2026-11-01T03:53:39.362Z","last_seen":"2026-11-01T03:54:01.43Z"}],"wifi":[]}
```

## Development

```
cargo test                                  # host crates
cargo clippy --workspace --exclude bluey-firmware --all-targets
cd firmware && cargo clippy --release       # firmware
```

## 802.15.4 (Zigbee and Thread)

In this mode the device listens to every frame on each of the 16 channels (11 to 26) in turn,
half a second at a time, so a full sweep takes eight seconds. It only listens: it never
sends anything. It lists who it hears by network (PAN) and address.

Zigbee and Thread are both built on 802.15.4, so which one a device is speaking is a guess:

- Frames with MAC level encryption are Thread (Zigbee encrypts higher up).
- Beacons say which they are. Thread beacons also carry the **network name**.
- Otherwise it looks at the start of the data: a compressed IPv6 header is Thread, a Zigbee
  network header is Zigbee. Anything else is `unknown`.

Matter isn't a separate radio mode: it runs over WiFi or Thread, so a Matter device on Thread
shows up here as a Thread device, and one on WiFi as part of its access point.

## Limitations

- **One radio per device.** A device scans for one thing at a time: doing several at once
  starved each of them. Use several devices to watch several things.
- **WiFi and 802.15.4 are separate firmware builds**, so a device can switch between BLE and
  one of them without being reflashed, but not to the other.
- **802.15.4 is a listener only.** A device that never sends (a sleeping sensor) isn't seen,
  and one that sends rarely may be missed while the sweep is on another channel. Devices that
  don't use a long lived network, or that only reply to a request, show up less than routers.
- **Find My detection is approximate.** AirTags and other Find My accessories send the
  same advertisement, so they can't be told apart.
- **Rotating addresses.** Phones change address about every 15 minutes, so one phone can
  show up as several devices over time.
- **WiFi sees access points only**, not the phones and laptops connected to them. 2.4 GHz only.
- **Legacy BLE advertisements only.** BLE 5 extended advertising isn't handled.
- The ESP32-C6 controller ignores the duplicate filter, so duplicates are merged in the
  device table instead.

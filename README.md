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
| `firmware/` | The ESP32-C6 scanner firmware |
| `dashboard/` | The ESP32-S3 dashboard firmware: a screen and a web page, see [Dashboard](#dashboard-esp32-s3) |
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
bluey log                     # record everything heard into a GeoPackage, to map and plot later
bluey poll [--every 5]        # like `log`, but asks for a snapshot every few seconds, until stopped
bluey gps                     # show what a USB GPS reports
bluey dashboard               # pass it all on to a dashboard device with a screen
```

### Several devices

Every command works on **all** the ESP32s that are plugged in, unless you pick some with
`--port`. A port can be followed by the mode to put that device in first, so one device
can scan for each thing, with nothing sharing a radio:

```
bluey monitor -p /dev/ttyACM0:ble -p /dev/ttyACM1:wifi -p /dev/ttyACM2:802154
bluey log -p /dev/ttyACM0:ble -p /dev/ttyACM1:wifi -p /dev/ttyACM2:802154
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

## Recording

`bluey log` adds everything the scanners hear to a [GeoPackage](https://www.geopackage.org/), a
SQLite file that GIS software understands. It lives in the XDG data folder, so it is in the same
place wherever you run the command:

```
~/.local/share/bluey/bluey.gpkg        # or $XDG_DATA_HOME/bluey/bluey.gpkg
```

It is only ever added to, so it builds up over time (`--truncate` never touches it), and several
runs and several scanners can all go into the one file. Use `--gpkg FILE` for a different file.
`bluey dashboard` records too if you add `--record`.

There is one layer, `observations`, with a row for every device in every snapshot (so about every
10 seconds for each scanner):

| Column | |
|--------|-|
| `geom` | The point where the computer was, WGS 84. Empty if the GPS had no fix |
| `time` | When the snapshot arrived, UTC |
| `scanner`, `port` | The scanner's chip MAC (which doesn't change with its port) and its serial port |
| `kind` | `ble`, `wifi` or `802154` |
| `device` | BLE address, WiFi BSSID, or 802.15.4 `PAN:address` |
| `label` | BLE name, WiFi SSID or Thread network name, if it has one |
| `category` | What kind of device (BLE), the security (WiFi) or Zigbee/Thread (802.15.4) |
| `detail` | BLE address type, or 802.15.4 address type |
| `channel` | WiFi and 802.15.4 channel |
| `rssi` | Signal strength in dBm, the last reading before the snapshot |
| `rssi_min`, `rssi_max` | Weakest and strongest since the scanner first heard it |
| `count` | Advertisements (BLE), scans (WiFi) or frames (802.15.4) heard |
| `first_seen`, `last_seen` | When the scanner first and last heard it, UTC |
| `latitude`, `longitude`, `altitude_m`, `hdop`, `satellites` | The GPS reading, as plain columns for tools that don't do geometry |

Without a GPS the rows are still recorded, just with no position. They are the signal and the
time, which is still useful for plotting.

**On a map:** open the file in QGIS (Layer, Add Layer, Add Vector Layer). Style the points by `rssi`
to see where a signal was strong, and use the temporal controller on `time` to play it back.

**With SQL**, from `sqlite3 ~/.local/share/bluey/bluey.gpkg` or any SQLite tool:

```sql
-- How strong one device was over time, and where
SELECT time, rssi, latitude, longitude FROM observations
WHERE kind = 'wifi' AND device = 'F4:92:BF:92:56:C9' ORDER BY time;

-- Where each WiFi network was loudest
SELECT label, max(rssi) AS best, latitude, longitude FROM observations
WHERE kind = 'wifi' AND geom IS NOT NULL GROUP BY device;
```

**In Python:** `geopandas.read_file(path, layer="observations")` gives a GeoDataFrame with real
datetimes, ready for `matplotlib`.

Things to know when reading it:

- A scanner reports a device once per snapshot, with its latest signal. It doesn't record every
  advertisement, so the time resolution is about 10 seconds.
- The position is the computer's, not the scanner's, so it is only as close as they are.
- Phones and AirTags change their address about every 15 minutes, so one of them shows up as
  many `device` values over a day.
- **It holds your location and the addresses of the devices around you.** Keep it somewhere you
  trust, and delete rows you don't need (`DELETE FROM observations WHERE time < '2026-01-01'`).

## Development

```
cargo test                                  # host crates
cargo clippy --workspace --exclude bluey-firmware --all-targets
cd firmware && cargo clippy --release       # firmware
```

### Position from a GPS

If a USB GPS is plugged in, `bluey log` puts its position on every row, so you can map where
things were heard. It looks for one among the USB serial ports that aren't bluey devices, and
accepts any receiver that sends NMEA over serial (most USB pucks do), at 4800, 9600, 38400 or
115200 baud. To name one instead, or to turn the search off:

```
bluey log --gps /dev/ttyUSB0          # speed is detected
bluey log --gps /dev/ttyUSB0:4800     # or given
bluey log --no-gps
```

Searching opens each candidate port and listens for a second or two. That is harmless for a
GPS, but a board that restarts when its port is opened (an Arduino, say) would restart, so
use `--gps` or `--no-gps` if you have one plugged in. `bluey gps` shows the position once a
second, which is a quick way to see whether the receiver has a fix yet. That takes a minute or
two from a cold start, and a clear view of the sky.

In the GeoPackage the position is the row's `geom` and the GPS columns. The JSON output, if you
ask for it (`bluey log -o FILE` appends one JSON object per snapshot per line, and
`bluey snapshot --json` prints one), has a `gps` object instead. Without a fix it only says so, and
how many satellites the receiver is using:

```json
"gps":{"fix":true,"satellites":6,"latitude":48.1173,"longitude":11.516667,"altitude_m":545.4,"hdop":0.9,"speed_mps":0.0,"course_deg":84.4,"age_ms":43}
"gps":{"fix":false,"satellites":0,"age_ms":120}
```

`hdop` is the receiver's own estimate of how good the position is: lower is better, and under
2 is good. `age_ms` is how long ago the receiver last reported, and a reading older than five
seconds counts as no fix. Records have no `gps` field when no GPS is in use. Note that a log
with positions records where you were, so think before sharing it.

## Dashboard (ESP32-S3)

A second kind of device, a [Seeed XIAO ESP32S3](https://wiki.seeedstudio.com/xiao_esp32s3_getting_started/)
with an Adafruit 1.14" 240x135 ST7789 display. It scans for nothing. It shows what the scanners
heard, and the GPS position, in two ways:

- **On its screen**: the Wi-Fi network and address, the position, and one line per scanner with
  what it is scanning for, how much it has heard and how long ago.
- **On a web page**, served from a Wi-Fi network it makes itself. Join the network with your phone
  and open the dashboard (a "sign in to network" prompt usually offers it, or go to
  <http://192.168.4.1/>). It lists everything each scanner has heard, strongest signal first, and
  can be filtered.

It can't reach the scanners itself. The computer is the go-between: `bluey dashboard` reads the
scanners and the GPS as usual and passes it all on to the dashboard over its USB cable, so the
computer has to stay plugged in and running.

```
bluey dashboard                          # scanners and GPS -> dashboard
bluey dashboard --record                 # and record them too, like `bluey log`
bluey dashboard -p /dev/ttyACM0:ble -p /dev/ttyACM1:wifi
```

`bluey devices` lists a dashboard as one, and the other commands leave it alone. The CLI finds it
by asking every device what it is.

### Wiring

The display shares the SPI bus with the Wio-SX1262 LoRa module, which is left idle. The pins match
the `remotectl` project, so the same wiring works.

| Display pin | XIAO pin | GPIO |
|-------------|----------|------|
| V+ | 3V3 | |
| G | GND | |
| CL (clock) | D8 | 7 |
| DA (data) | D10 | 9 |
| CS | D6 | 43 |
| DC | D7 | 44 |
| BL (backlight) | not connected, so always on | |

The display has no reset pin. If the picture is shifted or cropped, adjust `DISPLAY_OFFSET_X` and
`DISPLAY_OFFSET_Y` in `dashboard/src/bin/main.rs`.

### Build and flash

The ESP32-S3 is Xtensa, which needs Espressif's fork of Rust: `espup install`, which also gives
`~/export-esp.sh`. The access point's password is compiled in and the build fails without it.

```
source ~/export-esp.sh
cd dashboard
BLUEY_AP_PASSWORD='at least 8 characters' cargo build --release
espflash flash --chip esp32s3 ../target/xtensa-esp32s3-none-elf/release/bluey-dashboard
```

| Variable | |
|----------|-|
| `BLUEY_AP_PASSWORD` | Required. The Wi-Fi password (WPA2, 8 to 63 characters). It protects the position and the list of what's around you, so don't leave it as something obvious. |
| `BLUEY_AP_SSID` | The network's name. Default `bluey`. |

The board has to be in its ROM bootloader to be flashed if it is running firmware that uses
the USB port for something else (as the `remotectl` firmware does): hold **B** (the tiny button
beside the USB-C port) while plugging it in, flash, then tap **R**. Once the dashboard firmware is
on it, it appears as a serial port like the scanners do and `espflash` can reset it itself.

Flashing this **replaces whatever is on the board**, including `remotectl`.

### Limits

- Up to 4 phones at once, 8 scanners.
- The web page and the Wi-Fi network are plain HTTP over WPA2. Anyone with the password sees the
  position, so treat it like one.
- It only shows what the computer sends. If `bluey dashboard` stops, the screen and page keep the
  last snapshots and show how old they are, and the position is dropped after 30 seconds.
- The scanners' USB cables carry their data to the computer, not to the dashboard.

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

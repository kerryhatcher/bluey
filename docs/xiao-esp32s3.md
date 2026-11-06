# Seeed XIAO ESP32-S3 (dashboard)

The [Seeed XIAO ESP32-S3](https://wiki.seeedstudio.com/xiao_esp32s3_getting_started/) runs the
dashboard firmware in [`dashboard/`](../dashboard). It scans for nothing. It shows what the scanners
have heard, and the GPS position, on a small screen and on a web page. The
[README's Dashboard section](../README.md#dashboard-esp32-s3) covers how it fits with `bluey dashboard`.

The dashboard has two outputs:

- **Screen**: the Wi-Fi network and address, the position, and one line per scanner with what it
  is scanning for, how much it has heard and how long ago.
- **Web page**, served from a Wi-Fi network the board makes itself, listing everything each scanner
  has heard, strongest signal first, with a filter.

## Hardware

| | |
|---|---|
| Chip | ESP32-S3 (Xtensa, dual core, with WiFi and BLE) |
| Target | `xtensa-esp32s3-none-elf` |
| Logic level | 3.3 V |
| USB | USB-C, using the chip's built-in USB Serial/JTAG controller |
| Buttons | **B** (beside the USB-C port) is held while plugging in to enter the ROM bootloader. **R** resets. |
| Display | Adafruit 1.14" 240x135 ST7789 SPI display, no reset pin |

The dashboard's serial link is the USB port. It is the same port the CLI reads from, and where the
logs go.

### Wiring

The display shares the SPI bus with the Wio-SX1262 LoRa module, which is left idle (its chip-select
is held high). The pins match the `remotectl` project, so the same wiring works.

| Display pin | XIAO pin | GPIO |
|-------------|----------|------|
| V+ | 3V3 | |
| G | GND | |
| CL (clock) | D8 | 7 |
| DA (data) | D10 | 9 |
| CS | D6 | 43 |
| DC | D7 | 44 |
| BL (backlight) | not connected, so always on | |

The LoRa module's chip-select is GPIO 41, set high in `dashboard/src/bin/main.rs`.

The display has no reset pin. If the picture is shifted or cropped, adjust `DISPLAY_OFFSET_X` (52)
and `DISPLAY_OFFSET_Y` (40) in `dashboard/src/bin/main.rs`.

## Build and flash

The ESP32-S3 is Xtensa, so it needs Espressif's fork of Rust. Install it with `espup install`, which
also creates `~/export-esp.sh`. The access point's password is compiled in, and the build fails
without it.

```
source ~/export-esp.sh
cd dashboard
BLUEY_AP_PASSWORD='at least 8 characters' cargo build --release
espflash flash --chip esp32s3 ../target/xtensa-esp32s3-none-elf/release/bluey-dashboard
```

| Variable | |
|----------|-|
| `BLUEY_AP_PASSWORD` | Required. The Wi-Fi password (WPA2, 8 to 63 characters). It protects the position and the list of what's around you, so don't use something obvious. |
| `BLUEY_AP_SSID` | The network's name. Default `bluey`. |

If the board is running firmware that uses the USB port for something else (as `remotectl` does), put
it in the ROM bootloader first: hold **B** while plugging it in, flash, then tap **R**. Once the
dashboard firmware is on it, the board appears as a serial port and `espflash` can reset it itself.

**Flashing replaces whatever is on the board**, including `remotectl`.

## Network

| | |
|---|---|
| Network name | `bluey` (or `BLUEY_AP_SSID`) |
| Security | WPA2, password `BLUEY_AP_PASSWORD` |
| Channel | 6 |
| Board address | 192.168.4.1 |
| Phone addresses | DHCP, 192.168.4.50 to 192.168.4.200 |
| Web page | <http://192.168.4.1/> on port 80 |

Phones that join get a "sign in to network" prompt, which opens the dashboard. Requests to the usual
connectivity-check addresses are redirected to the dashboard.

## Limits

- Up to 4 phones at once, and 8 scanners.
- The web page and the Wi-Fi network are plain HTTP over WPA2. Anyone with the password sees the
  position, so treat it like one.
- The dashboard only shows what the computer sends. If `bluey dashboard` stops, the screen and page
  keep the last snapshots and show how old they are. The position is dropped after 30 seconds.
- The scanners' USB cables carry their data to the computer, not to the dashboard.

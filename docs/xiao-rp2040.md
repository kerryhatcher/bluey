# Seeed XIAO RP2040

A [Seeed XIAO RP2040](https://wiki.seeedstudio.com/XIAO-RP2040/) is a small board with a
Raspberry Pi RP2040 microcontroller, in the same thumbnail-sized form factor as the other XIAO
boards used in this project (the ESP32-C6 scanners and the ESP32-S3 dashboard, see the
[README](../README.md)).

This board is not part of the bluey firmware or CLI yet. This page records what is known about
it, so that it can be used later.

## Hardware

| | |
|---|---|
| Chip | Raspberry Pi RP2040 (dual Arm Cortex-M0+, 264 KB SRAM) |
| Flash | 2 MB (on the board) |
| Logic level | 3.3 V |
| USB | USB-C, USB 2.0 full speed (12 Mbps) |
| Buttons | **BOOT** (BOOTSEL), held while plugging in to enter the UF2 bootloader; **RESET** |
| LEDs | One RGB LED |

The board has 11 numbered pads (D0 to D10) plus 3V3, 5V, GND and the battery pads on the
back. The RP2040 GPIO each pad connects to:

| Pad | GPIO | Notes |
|-----|------|-------|
| D0 | GP26 | ADC0 |
| D1 | GP27 | ADC1 |
| D2 | GP28 | ADC2 |
| D3 | GP29 | ADC3 |
| D4 | GP6 | I2C SDA |
| D5 | GP7 | I2C SCL |
| D6 | GP0 | UART TX |
| D7 | GP1 | UART RX |
| D8 | GP2 | SPI SCK |
| D9 | GP4 | SPI MISO |
| D10 | GP3 | SPI MOSI |

Check the pinout against the Seeed wiki for your board revision before wiring anything to it.

## Connected to this computer

| | |
|---|---|
| Serial port | `/dev/ttyACM0` |
| By-id path | `/dev/serial/by-id/usb-Seeed_XIAO_RP2040_4250305539333104-if00` |
| USB ID | `2e8a:000a` (Raspberry Pi vendor ID, RP2040 product ID) |
| Manufacturer / product | Seeed / XIAO RP2040 |
| Serial number | `4250305539333104` |
| USB driver | `cdc_acm` (a plain USB serial port) |
| Firmware | **Unknown.** It is running something that exposes a serial port. Nothing in this repository flashes it. |

The board appears as a serial port because its firmware provides one. To see what the firmware
sends, you have to open the port, which may cause the board to restart. Don't send it anything
until you know what the firmware expects.

Reading the serial port needs access to `/dev/ttyACM0`, which is the `dialout` group on most
distributions.

## Flashing

The RP2040 has a UF2 bootloader. Two ways in:

1. **Hold BOOT and plug the board in** (or press RESET while holding BOOT). It mounts as a USB
   drive named `RPI-RP2`. Copy a `.uf2` file onto that drive, and the board reboots into the
   new firmware.
2. **Use `picotool`** or the Arduino IDE / MicroPython tools, which put the board into the
   bootloader for you.

Flashing replaces whatever firmware is on the board. Check what is on it first (see above) if
you want to keep it.

## Using it with bluey

Nothing yet. `bluey ports` does not know this board, and the CLI only talks to the ESP32s. If you
want the RP2040 to take part, it needs its own firmware and a message format agreed with
`bluey-core`, so write that down here first.

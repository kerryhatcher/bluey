#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use bluey_dashboard::{link, logger, net, output, screen, state, web, wifi};
use embassy_executor::Spawner;
use embassy_futures::select::select;
use embassy_time::{Duration, Timer};
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_hal_bus::spi::ExclusiveDevice;
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::delay::Delay;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::rng::Rng;
use esp_hal::spi::Mode;
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;
use esp_hal::usb::usb_serial_jtag::UsbSerialJtag;
use log::{error, info};
use mipidsi::Builder;
use mipidsi::interface::SpiInterface;
use mipidsi::models::ST7789;
use mipidsi::options::{ColorInversion, Orientation, Rotation};

extern crate alloc;

// This creates a default app-descriptor required by the esp-idf bootloader.
esp_bootloader_esp_idf::esp_app_desc!();

// The Adafruit 1.14" 240x135 TFT (ST7789) sits in the middle of the controller's 240x320 RAM.
// These are for the native portrait orientation. If the border doesn't line up with the
// edge of the panel, adjust them.
const DISPLAY_WIDTH: u16 = 135;
const DISPLAY_HEIGHT: u16 = 240;
const DISPLAY_OFFSET_X: u16 = 52;
const DISPLAY_OFFSET_Y: u16 = 40;

/// The screen is redrawn at least this often (so ages count up) and at most every `REDRAW_GAP`.
const REDRAW_PERIOD: Duration = Duration::from_secs(1);
const REDRAW_GAP: Duration = Duration::from_millis(250);

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 73744);
    esp_alloc::heap_allocator!(size: 96 * 1024);

    // Logs go to the CLI as messages on the USB serial port.
    logger::init();

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);
    info!("bluey dashboard starting");

    // The CLI talks to us over the USB serial port. Output goes through esp-println, so
    // only the receiving half is needed here.
    let (usb_rx, _usb_tx) = UsbSerialJtag::new(peripherals.USB_DEVICE).into_async().split();
    spawner.spawn(output::task().expect("output task"));
    spawner.spawn(link::receive(usb_rx).expect("link task"));
    spawner.spawn(link::announce().expect("announce task"));

    // The access point, with DHCP, DNS and the web server on it.
    let rng = Rng::new();
    let seed = (u64::from(rng.random()) << 32) | u64::from(rng.random());
    let stack = wifi::start(&spawner, peripherals.WIFI, seed);
    spawner.spawn(net::dhcp_task(stack).expect("dhcp task"));
    spawner.spawn(net::dns_task(stack).expect("dns task"));
    for id in 0..web::WEB_TASKS {
        spawner.spawn(web::web_task(id, stack).expect("web task"));
    }

    // The Wio-SX1262 shares the display's SPI bus. It is not used, so keep it deselected.
    let _lora_nss = Output::new(peripherals.GPIO41, Level::High, OutputConfig::default());

    // The display: CL=D8 (SCK), DA=D10 (MOSI), CS=D6, DC=D7. It has no reset pin.
    let display_cs = Output::new(peripherals.GPIO43, Level::High, OutputConfig::default());
    let display_dc = Output::new(peripherals.GPIO44, Level::Low, OutputConfig::default());
    let spi = Spi::new(
        peripherals.SPI2,
        SpiConfig::default().with_frequency(Rate::from_mhz(10)).with_mode(Mode::_0),
    )
    .expect("spi config")
    .with_sck(peripherals.GPIO7)
    .with_mosi(peripherals.GPIO9);
    let display_spi = ExclusiveDevice::new(spi, display_cs, Delay::new()).expect("display cs");

    let mut display_buf = [0u8; 512];
    let di = SpiInterface::new(display_spi, display_dc, &mut display_buf);
    let mut display = Builder::new(ST7789, di)
        .display_size(DISPLAY_WIDTH, DISPLAY_HEIGHT)
        .display_offset(DISPLAY_OFFSET_X, DISPLAY_OFFSET_Y)
        .orientation(Orientation::new().rotate(Rotation::Deg0))
        .invert_colors(ColorInversion::Inverted)
        .init(&mut Delay::new())
        .expect("display init");
    display.clear(Rgb565::BLACK).expect("display clear");
    info!("display initialized");

    loop {
        if let Err(e) = screen::draw(&mut display, &state::get()) {
            error!("display draw error: {e:?}");
        }
        select(Timer::after(REDRAW_PERIOD), state::CHANGED.wait()).await;
        Timer::after(REDRAW_GAP).await;
    }
}

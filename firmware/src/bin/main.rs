#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

#[cfg(feature = "wifi")]
use alloc::string::String;
use bluey_core::adv::{AddrClass, summarize};
use bluey_core::framing::{Decoder, Item};
use bluey_core::proto::{Command, Message, Mode, PROTOCOL_VERSION, Snapshot};
use bluey_firmware::nodes::NodeTable;
use bluey_firmware::output;
use bluey_firmware::settings::Settings;
use bluey_firmware::table::DeviceTable;
use bluey_firmware::wifi::AccessPointTable;
use bt_hci::controller::ExternalController;
use bt_hci::param::{AddrKind, FilterDuplicates, LeAdvReportsIter};
use core::cell::{Cell, RefCell};
use core::convert::Infallible;
use critical_section::Mutex;
use embassy_executor::Spawner;
use embassy_futures::join::join4;
use embassy_futures::select::{Either, select};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer};
use embedded_io_async::Read;
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::peripherals::{BT, FLASH};
#[cfg(feature = "wifi")]
use esp_hal::peripherals::WIFI;
#[cfg(feature = "ieee802154")]
use esp_hal::peripherals::IEEE802154;
use esp_hal::timer::timg::TimerGroup;
use esp_hal::usb::usb_serial_jtag::{UsbSerialJtag, UsbSerialJtagRx};
use esp_hal::Async;
use esp_radio::ble::controller::BleConnector;
#[cfg(feature = "ieee802154")]
use bluey_core::ieee802154;
#[cfg(feature = "ieee802154")]
use esp_radio::ieee802154::{Config as RadioConfig, Ieee802154};
#[cfg(feature = "wifi")]
use esp_radio::wifi::scan::{ScanConfig as WifiScanConfig, ScanTypeConfig};
#[cfg(feature = "wifi")]
use esp_radio::wifi::sta::StationConfig;
#[cfg(feature = "wifi")]
use esp_radio::wifi::{Config as WifiConfig, WifiController};
use log::{info, warn};
use trouble_host::prelude::*;

extern crate alloc;

const CONNECTIONS_MAX: usize = 1;
const L2CAP_CHANNELS_MAX: usize = 1;

/// How often a snapshot of the tables is sent.
const REPORT_PERIOD: Duration = Duration::from_secs(10);

/// Entries not heard from for this long are dropped from the tables.
const FORGET_AFTER: Duration = Duration::from_secs(120);

/// How long to listen on each 802.15.4 channel before moving to the next.
#[cfg(feature = "ieee802154")]
const CHANNEL_DWELL: Duration = Duration::from_millis(500);

/// The channels 802.15.4 uses in the 2.4 GHz band, which is all the ESP32-C6 can do.
#[cfg(feature = "ieee802154")]
const IEEE802154_CHANNELS: core::ops::RangeInclusive<u8> = 11..=26;

/// How often the WiFi access points are scanned for.
#[cfg(feature = "wifi")]
const WIFI_SCAN_PERIOD: Duration = Duration::from_secs(5);

static DEVICES: Mutex<RefCell<DeviceTable>> = Mutex::new(RefCell::new(DeviceTable::new()));
static ACCESS_POINTS: Mutex<RefCell<AccessPointTable>> = Mutex::new(RefCell::new(AccessPointTable::new()));

static NODES: Mutex<RefCell<NodeTable>> = Mutex::new(RefCell::new(NodeTable::new()));

static CURRENT_MODE: Mutex<Cell<Mode>> = Mutex::new(Cell::new(Mode::Ble));
/// The host asked for this mode.
static MODE_REQUEST: Signal<CriticalSectionRawMutex, Mode> = Signal::new();
/// Something wants a snapshot sent right away.
static SNAPSHOT_REQUEST: Signal<CriticalSectionRawMutex, ()> = Signal::new();

fn current_mode() -> Mode {
    critical_section::with(|cs| CURRENT_MODE.borrow(cs).get())
}

/// Adds every advertisement the controller reports to the device table.
struct AdvRecorder;

impl EventHandler for AdvRecorder {
    fn on_adv_reports(&self, reports: LeAdvReportsIter) {
        let now = Instant::now();
        for report in reports.flatten() {
            let Ok(addr) = <[u8; 6]>::try_from(report.addr.raw()) else {
                continue;
            };
            let class = AddrClass::new(report.addr_kind == AddrKind::RANDOM, &addr);
            let summary = summarize(report.data);
            critical_section::with(|cs| {
                DEVICES.borrow_ref_mut(cs).record(addr, class, report.rssi, &summary, now);
            });
        }
    }
}

/// Scans for Bluetooth LE devices and adds them to the device table. Only returns on failure.
#[allow(
    clippy::large_stack_frames,
    reason = "the BLE host resources are large, and this runs once in main's task"
)]
async fn run_ble(bt: BT<'_>) {
    let transport = match BleConnector::new(bt, Default::default()) {
        Ok(transport) => transport,
        Err(e) => return warn!("failed to start BLE: {e:?}"),
    };
    let ble_controller = ExternalController::<_, 1>::new(transport);
    let mut resources: HostResources<_, DefaultPacketPool, CONNECTIONS_MAX, L2CAP_CHANNELS_MAX> =
        HostResources::new();
    let stack = trouble_host::new(ble_controller, &mut resources).build();
    let mut runner = stack.runner();
    let mut scanner = Scanner::new(stack.central());

    let config = ScanConfig {
        active: true,
        // The ESP32-C6 controller ignores this, so duplicates are merged in the device table.
        filter_duplicates: FilterDuplicates::Disabled,
        ..Default::default()
    };

    let handler = AdvRecorder;
    let scan = async {
        // Scanning stops when the session is dropped, so hold it for as long as we run.
        let _session = loop {
            match scanner.scan(&config).await {
                Ok(session) => break session,
                Err(_) => {
                    warn!("failed to start scan, retrying");
                    Timer::after(Duration::from_secs(1)).await;
                }
            }
        };
        core::future::pending::<()>().await;
    };
    match select(runner.run_with_handler(&handler), scan).await {
        Either::First(result) => warn!("BLE host stopped: {:?}", result.err()),
        Either::Second(()) => {}
    }
}

/// Scans for WiFi access points and adds them to the access point table. Only returns on failure.
#[cfg(feature = "wifi")]
#[allow(
    clippy::large_stack_frames,
    reason = "the config and log formatting need some stack, and this runs once in main's task"
)]
async fn run_wifi(wifi: WIFI<'_>) {
    let mut controller = match WifiController::new(wifi, Default::default()) {
        Ok(controller) => controller,
        Err(e) => return warn!("failed to start WiFi: {e:?}"),
    };
    // The scan results are only used for the table, so the station never connects anywhere.
    if let Err(e) = controller.set_config(&WifiConfig::Station(StationConfig::default())) {
        return warn!("failed to configure WiFi: {e:?}");
    }

    let config = WifiScanConfig::default()
        .with_show_hidden(true)
        .with_scan_type(ScanTypeConfig::Active {
            min: esp_hal::time::Duration::from_millis(40),
            max: esp_hal::time::Duration::from_millis(120),
        });
    loop {
        match controller.scan_async(&config).await {
            Ok(access_points) => {
                let now = Instant::now();
                critical_section::with(|cs| {
                    let mut table = ACCESS_POINTS.borrow_ref_mut(cs);
                    for ap in &access_points {
                        let auth = ap.auth_method.map_or(String::from("unknown"), |m| alloc::format!("{m:?}"));
                        table.record(ap.bssid, ap.ssid.as_str(), ap.channel, &auth, ap.signal_strength, now);
                    }
                });
            }
            Err(e) => warn!("WiFi scan failed: {e:?}"),
        }
        Timer::after(WIFI_SCAN_PERIOD).await;
    }
}

/// The radio has received a frame.
#[cfg(feature = "ieee802154")]
static FRAME_RECEIVED: Signal<CriticalSectionRawMutex, ()> = Signal::new();

#[cfg(feature = "ieee802154")]
fn frame_received() {
    FRAME_RECEIVED.signal(());
}

/// Listens for Zigbee and Thread devices, moving through the channels, and adds what it hears
/// to the node table. Never returns.
#[cfg(feature = "ieee802154")]
#[allow(
    clippy::large_stack_frames,
    reason = "the frame buffers are large, and this runs once in main's task"
)]
async fn run_ieee802154(radio: IEEE802154<'_>) {
    let mut radio = Ieee802154::new(radio);
    radio.set_rx_available_callback_fn(frame_received);

    for channel in IEEE802154_CHANNELS.cycle() {
        radio.set_config(RadioConfig {
            channel,
            // Listen to everything on the channel, not just frames for us, and don't answer any.
            promiscuous: true,
            rx_when_idle: true,
            ..Default::default()
        });
        // Not `start_receive`, which does nothing when already listening, so the channel wouldn't change.
        radio.restart_receive();

        let end = Instant::now() + CHANNEL_DWELL;
        loop {
            while let Some(received) = radio.raw_received() {
                record_frame(&received.data, received.channel);
            }
            if matches!(select(FRAME_RECEIVED.wait(), Timer::at(end)).await, Either::Second(())) {
                break;
            }
        }
    }
}

/// Adds a received frame to the node table.
#[cfg(feature = "ieee802154")]
fn record_frame(data: &[u8], channel: u8) {
    // The first byte is the length of what follows, which ends with two bytes where the
    // checksum would be. The radio puts the signal strength in the first of them.
    let len = usize::from(data[0]);
    if len < 3 || len >= data.len() {
        return;
    }
    let rssi = data[len - 1] as i8;
    let Some(frame) = ieee802154::parse(&data[1..len - 1]) else {
        return;
    };
    // Frames with no sender, like acknowledgements, say nothing about who is there.
    let Some(src) = frame.src else { return };
    let sender = ieee802154::classify(&frame);
    critical_section::with(|cs| {
        NODES.borrow_ref_mut(cs).record(
            frame.pan_id,
            src,
            sender.protocol,
            sender.network_name.as_deref(),
            channel,
            rssi,
            Instant::now(),
        );
    });
}

#[cfg(all(feature = "wifi", feature = "ieee802154"))]
compile_error!("WiFi and 802.15.4 can't both be built in; use `--no-default-features --features ieee802154`");

/// The modes this firmware can switch to. WiFi and 802.15.4 can't both be built in.
const SUPPORTED: &[Mode] = &[
    Mode::Ble,
    #[cfg(feature = "wifi")]
    Mode::Wifi,
    #[cfg(feature = "ieee802154")]
    Mode::Ieee802154,
];

/// The peripherals the radios use.
struct Radios {
    bt: BT<'static>,
    #[cfg(feature = "wifi")]
    wifi: WIFI<'static>,
    #[cfg(feature = "ieee802154")]
    ieee802154: IEEE802154<'static>,
}

/// Waits until the host asks for a mode other than `current`.
async fn mode_change(current: Mode) -> Mode {
    loop {
        let requested = MODE_REQUEST.wait().await;
        if requested != current {
            return requested;
        }
    }
}

/// Runs whichever radio the current mode needs, one at a time, and swaps them on request.
///
/// Switching drops the running scan, which shuts its radio down, before the other one starts.
/// The mode is saved then, while no radio is busy, and restored after a reset.
async fn run_radio(mut radios: Radios, flash: FLASH<'static>) -> Infallible {
    let mut settings = Settings::new(flash);
    // A mode saved by a firmware with a different radio is no use here.
    let mut mode = settings.load_mode().filter(|m| SUPPORTED.contains(m)).unwrap_or(Mode::Ble);
    loop {
        critical_section::with(|cs| {
            DEVICES.borrow_ref_mut(cs).clear();
            ACCESS_POINTS.borrow_ref_mut(cs).clear();
            NODES.borrow_ref_mut(cs).clear();
            CURRENT_MODE.borrow(cs).set(mode);
        });
        info!("scanning for {}", mode.label());
        // The host wants to see that the switch happened.
        SNAPSHOT_REQUEST.signal(());

        let changed = match mode {
            Mode::Ble => select(run_ble(radios.bt.reborrow()), mode_change(mode)).await,
            #[cfg(feature = "wifi")]
            Mode::Wifi => select(run_wifi(radios.wifi.reborrow()), mode_change(mode)).await,
            #[cfg(feature = "ieee802154")]
            Mode::Ieee802154 => select(run_ieee802154(radios.ieee802154.reborrow()), mode_change(mode)).await,
            // Commands for modes we don't have are turned away, so this isn't reached.
            #[allow(unreachable_patterns)]
            _ => Either::Second(mode_change(mode).await),
        };
        match changed {
            Either::First(()) => {
                // The scan failed. Give it a moment, then try again.
                Timer::after(Duration::from_secs(2)).await;
            }
            Either::Second(requested) => {
                mode = requested;
                settings.save_mode(mode);
            }
        }
    }
}

/// Reads commands from the host and acts on them.
async fn receive_commands(mut rx: UsbSerialJtagRx<'static, Async>) -> Infallible {
    let mut decoder = Decoder::<Command>::new();
    let mut buf = [0u8; 64];
    loop {
        let n = match rx.read(&mut buf).await {
            Ok(n) => n,
            Err(_) => {
                Timer::after(Duration::from_millis(100)).await;
                continue;
            }
        };
        for item in decoder.push(&buf[..n]) {
            // Anything else on the line isn't for us.
            let Item::Message(command) = item else { continue };
            match command {
                Command::SetMode(mode) if !SUPPORTED.contains(&mode) => {
                    warn!("this firmware can't scan for {}", mode.label());
                    SNAPSHOT_REQUEST.signal(());
                }
                Command::SetMode(mode) if mode != current_mode() => MODE_REQUEST.signal(mode),
                Command::SetMode(_) | Command::Snapshot => SNAPSHOT_REQUEST.signal(()),
            }
        }
    }
}

/// Sends the tables to the host every `REPORT_PERIOD`, or sooner if asked.
#[allow(
    clippy::large_stack_frames,
    reason = "the log formatting needs some stack, and this runs once in main's task"
)]
async fn report(device: [u8; 6]) -> Infallible {
    loop {
        select(Timer::after(REPORT_PERIOD), SNAPSHOT_REQUEST.wait()).await;
        let now = Instant::now();
        let (mode, ble, wifi, ieee802154) = critical_section::with(|cs| {
            let mut devices = DEVICES.borrow_ref_mut(cs);
            let mut access_points = ACCESS_POINTS.borrow_ref_mut(cs);
            let mut nodes = NODES.borrow_ref_mut(cs);
            devices.prune(now, FORGET_AFTER);
            access_points.prune(now, FORGET_AFTER);
            nodes.prune(now, FORGET_AFTER);
            (CURRENT_MODE.borrow(cs).get(), devices.snapshot(), access_points.snapshot(), nodes.snapshot())
        });

        output::send(&Message::Snapshot(Snapshot {
            protocol: PROTOCOL_VERSION,
            device,
            mode,
            supported: SUPPORTED.to_vec(),
            uptime_ms: now.as_millis(),
            ble,
            wifi,
            ieee802154,
        }));
    }
}

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(_spawner: Spawner) -> ! {
    // generator version: 2.0.0
    // generator parameters: -o esp32c6 -o unstable-hal -o alloc -o ble-trouble -o embassy -o log -o esp-backtrace

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 65536);
    // The WiFi driver needs more heap than BLE alone.
    esp_alloc::heap_allocator!(size: 72 * 1024);

    bluey_firmware::logger::init();

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);

    info!("Embassy initialized!");

    // Output goes through esp-println; this is only for reading commands.
    let (usb_rx, _usb_tx) = UsbSerialJtag::new(peripherals.USB_DEVICE).into_async().split();
    let device = <[u8; 6]>::try_from(esp_hal::efuse::base_mac_address().as_bytes()).unwrap_or_default();

    // None of these return.
    let (never, ..) = join4(
        run_radio(
            Radios {
                bt: peripherals.BT,
                #[cfg(feature = "wifi")]
                wifi: peripherals.WIFI,
                #[cfg(feature = "ieee802154")]
                ieee802154: peripherals.IEEE802154,
            },
            peripherals.FLASH,
        ),
        receive_commands(usb_rx),
        report(device),
        output::run(),
    )
    .await;
    match never {}
}

#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use alloc::string::String;
use bluey_core::adv::{AddrClass, summarize};
use bluey_core::proto::{Message, PROTOCOL_VERSION, Snapshot};
use bluey_firmware::output;
use bluey_firmware::table::DeviceTable;
use bluey_firmware::wifi::AccessPointTable;
use bt_hci::controller::ExternalController;
use bt_hci::param::{AddrKind, FilterDuplicates, LeAdvReportsIter};
use core::cell::RefCell;
use critical_section::Mutex;
use embassy_futures::join::join5;
use embassy_executor::Spawner;
use embassy_time::{Duration, Instant, Timer};
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::timer::timg::TimerGroup;
use esp_radio::ble::controller::BleConnector;
use esp_radio::wifi::scan::{ScanConfig as WifiScanConfig, ScanTypeConfig};
use esp_radio::wifi::sta::StationConfig;
use esp_radio::wifi::{Config as WifiConfig, WifiController};
use log::{info, warn};
use trouble_host::prelude::*;

extern crate alloc;

const CONNECTIONS_MAX: usize = 1;
const L2CAP_CHANNELS_MAX: usize = 1;

/// How often the table of devices is printed.
const REPORT_PERIOD: Duration = Duration::from_secs(10);

/// Devices not heard from for this long are dropped from the table.
const FORGET_AFTER: Duration = Duration::from_secs(120);

static DEVICES: Mutex<RefCell<DeviceTable>> = Mutex::new(RefCell::new(DeviceTable::new()));

/// How often the WiFi access points are scanned for.
const WIFI_SCAN_PERIOD: Duration = Duration::from_secs(5);

static ACCESS_POINTS: Mutex<RefCell<AccessPointTable>> = Mutex::new(RefCell::new(AccessPointTable::new()));

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

/// Scans for WiFi access points and adds them to the access point table.
#[allow(
    clippy::large_stack_frames,
    reason = "the config and log formatting need some stack, and these run once in main's task"
)]
async fn scan_wifi(mut controller: WifiController<'_>) {
    // The scan results are only used for the table, so the station never connects anywhere.
    if let Err(e) = controller.set_config(&WifiConfig::Station(StationConfig::default())) {
        warn!("failed to start WiFi: {e:?}");
        return;
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

/// Sends the device and access point tables to the host every `REPORT_PERIOD`.
#[allow(
    clippy::large_stack_frames,
    reason = "the config and log formatting need some stack, and these run once in main's task"
)]
async fn report_devices() {
    loop {
        Timer::after(REPORT_PERIOD).await;
        let now = Instant::now();
        let ble = critical_section::with(|cs| {
            let mut table = DEVICES.borrow_ref_mut(cs);
            table.prune(now, FORGET_AFTER);
            table.snapshot()
        });
        let access_points = critical_section::with(|cs| {
            let mut table = ACCESS_POINTS.borrow_ref_mut(cs);
            table.prune(now, FORGET_AFTER);
            table.snapshot()
        });

        output::send(&Message::Snapshot(Snapshot {
            protocol: PROTOCOL_VERSION,
            uptime_ms: now.as_millis(),
            ble,
            wifi: access_points,
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

    // find more examples https://github.com/embassy-rs/trouble/tree/main/examples/esp32
    let wifi = WifiController::new(peripherals.WIFI, Default::default()).unwrap();
    let transport = BleConnector::new(peripherals.BT, Default::default()).unwrap();
    let ble_controller = ExternalController::<_, 1>::new(transport);
    let mut resources: HostResources<_, DefaultPacketPool, CONNECTIONS_MAX, L2CAP_CHANNELS_MAX> =
        HostResources::new();
    let stack = trouble_host::new(ble_controller, &mut resources).build();
    let mut runner = stack.runner();
    let mut scanner = Scanner::new(stack.central());

    let config = ScanConfig {
        active: true,
        // The radio is shared with WiFi, so leave it idle for part of each interval.
        interval: Duration::from_millis(1000),
        window: Duration::from_millis(200),
        // The ESP32-C6 controller ignores this, so duplicates are merged in the device table.
        filter_duplicates: FilterDuplicates::Disabled,
        ..Default::default()
    };

    info!("Scanning for BLE advertisements");
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
    let (result, ..) = join5(
        runner.run_with_handler(&handler),
        scan,
        scan_wifi(wifi),
        report_devices(),
        output::run(),
    )
    .await;
    panic!("BLE host stopped: {:?}", result.err());

    // for inspiration have a look at the examples at https://github.com/esp-rs/esp-hal/tree/esp-hal-v1.2.2/examples
}

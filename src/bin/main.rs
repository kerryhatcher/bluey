#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use alloc::string::String;
use bluey::adv::{AddrClass, Mac, summarize};
use bluey::table::DeviceTable;
use bt_hci::controller::ExternalController;
use bt_hci::param::{AddrKind, FilterDuplicates, LeAdvReportsIter};
use core::cell::RefCell;
use critical_section::Mutex;
use embassy_futures::join::join3;
use embassy_executor::Spawner;
use embassy_time::{Duration, Instant, Timer};
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::timer::timg::TimerGroup;
use esp_radio::ble::controller::BleConnector;
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

/// Prints the device table every `REPORT_PERIOD`.
async fn report_devices() {
    loop {
        Timer::after(REPORT_PERIOD).await;
        let now = Instant::now();
        let devices = critical_section::with(|cs| {
            let mut table = DEVICES.borrow_ref_mut(cs);
            table.prune(now, FORGET_AFTER);
            table.snapshot()
        });

        info!("---- {} devices ----", devices.len());
        for d in &devices {
            info!(
                "{} ({:>4}) rssi={:>4} [{:>4}..{:>4}] n={:<4} seen {:>3}s ago: {}{}",
                Mac(&d.addr),
                d.class.label(),
                d.rssi,
                d.rssi_min,
                d.rssi_max,
                d.count,
                (now - d.last_seen).as_secs(),
                d.kind.label(),
                d.name.as_deref().map_or(String::new(), |n| alloc::format!(" name={n:?}")),
            );
        }
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

    esp_println::logger::init_logger_from_env();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 65536);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);

    info!("Embassy initialized!");

    // find more examples https://github.com/embassy-rs/trouble/tree/main/examples/esp32
    let transport = BleConnector::new(peripherals.BT, Default::default()).unwrap();
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
    let (result, _, _) = join3(runner.run_with_handler(&handler), scan, report_devices()).await;
    panic!("BLE host stopped: {:?}", result.err());

    // for inspiration have a look at the examples at https://github.com/esp-rs/esp-hal/tree/esp-hal-v1.2.2/examples
}

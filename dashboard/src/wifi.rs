//! The Wi-Fi access point phones connect to, and the network stack on it.
//!
//! The ESP32-S3 is the router here: it has the fixed address [`AP_IP`], hands out addresses to
//! phones ([`crate::net::dhcp_task`]) and answers every name lookup with its own address
//! ([`crate::net::dns_task`]), so a phone that joins is sent straight to the dashboard.

use embassy_executor::Spawner;
use embassy_net::{Ipv4Address, Ipv4Cidr, Runner, Stack, StackResources, StaticConfigV4};
use esp_radio::wifi::ap::{AccessPointConfig, EventInfo};
use esp_radio::wifi::{AuthenticationMethodConfig, Config, Interface, WifiController};
use log::{info, warn};
use static_cell::StaticCell;

use crate::state;

/// The name of the network. Set it at build time with `BLUEY_AP_SSID`.
pub const SSID: &str = match option_env!("BLUEY_AP_SSID") {
    Some(ssid) => ssid,
    None => "bluey",
};

/// The password (WPA2, at least 8 characters). Required at build time as `BLUEY_AP_PASSWORD`,
/// because the dashboard shows where you are and what is around you.
const PASSWORD: &str = env!("BLUEY_AP_PASSWORD");

/// The channel to run on. 1, 6 and 11 are the ones that don't overlap with each other.
const CHANNEL: u8 = 6;

/// How many phones can be connected at once.
const MAX_CLIENTS: u16 = 4;

/// The dashboard's address, which is also the router and DNS server on this network.
pub const AP_IP: Ipv4Address = Ipv4Address::new(192, 168, 4, 1);

/// Sockets for the web server tasks, DHCP, DNS and slack.
const STACK_SOCKETS: usize = 8;

static RESOURCES: StaticCell<StackResources<STACK_SOCKETS>> = StaticCell::new();

/// Starts the access point and returns the network stack. It is usable straight away.
pub fn start(spawner: &Spawner, wifi: esp_hal::peripherals::WIFI<'static>, seed: u64) -> Stack<'static> {
    let controller = WifiController::new(wifi, Default::default()).expect("Failed to initialize Wi-Fi");
    let interface = Interface::access_point();

    let (stack, runner) = embassy_net::new(
        interface,
        embassy_net::Config::ipv4_static(StaticConfigV4 {
            address: Ipv4Cidr::new(AP_IP, 24),
            gateway: None,
            dns_servers: Default::default(),
        }),
        RESOURCES.init(StackResources::new()),
        seed,
    );

    spawner.spawn(access_point_task(controller).expect("access point task"));
    spawner.spawn(net_task(runner).expect("net task"));
    stack
}

#[embassy_executor::task]
async fn access_point_task(mut controller: WifiController<'static>) {
    let config = Config::AccessPoint(
        AccessPointConfig::default()
            .with_ssid(SSID.try_into().expect("BLUEY_AP_SSID too long"))
            .with_authentication(AuthenticationMethodConfig::Wpa2Personal(
                PASSWORD.try_into().expect("BLUEY_AP_PASSWORD invalid"),
            ))
            .with_channel(CHANNEL)
            .with_max_connections(MAX_CLIENTS),
    );
    controller.set_config(&config).expect("wifi config");
    info!("Wi-Fi: access point '{SSID}' is up, dashboard at http://{AP_IP}/");

    // Keep count of who is connected, for the screen.
    let mut clients: u8 = 0;
    loop {
        match controller.wait_for_access_point_connected_event_async().await {
            Ok(EventInfo::Connected(info)) => {
                clients = clients.saturating_add(1);
                info!("Wi-Fi: {:02X?} joined", info.mac);
            }
            Ok(EventInfo::Disconnected(info)) => {
                clients = clients.saturating_sub(1);
                info!("Wi-Fi: {:02X?} left", info.mac);
            }
            Err(e) => warn!("Wi-Fi: event error: {e:?}"),
        }
        state::set_clients(clients);
    }
}

#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, Interface>) {
    runner.run().await
}

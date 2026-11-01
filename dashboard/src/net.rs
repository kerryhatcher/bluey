//! DHCP and DNS for the phones that join the access point.

use core::net::Ipv4Addr;

use edge_dhcp::server::{Server, ServerOptions};
use edge_dhcp::{Options, Packet};
use embassy_net::Stack;
use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_net::{IpAddress, IpEndpoint};
use embassy_time::Instant;
use log::{info, warn};

use crate::wifi::AP_IP;

const DNS_PORT: u16 = 53;
const DHCP_SERVER_PORT: u16 = 67;
const DHCP_CLIENT_PORT: u16 = 68;

fn ap_ip() -> Ipv4Addr {
    let [a, b, c, d] = AP_IP.octets();
    Ipv4Addr::new(a, b, c, d)
}

/// Hands out addresses to phones: 192.168.4.50 to 192.168.4.200.
#[embassy_executor::task]
pub async fn dhcp_task(stack: Stack<'static>) {
    let mut rx_meta = [PacketMetadata::EMPTY; 4];
    let mut rx_buf = [0u8; 1024];
    let mut tx_meta = [PacketMetadata::EMPTY; 4];
    let mut tx_buf = [0u8; 1024];
    let mut socket = UdpSocket::new(stack, &mut rx_meta, &mut rx_buf, &mut tx_meta, &mut tx_buf);
    socket.bind(DHCP_SERVER_PORT).expect("bind DHCP port");

    let mut server: Server<fn() -> u64, 8> = Server::new(|| Instant::now().as_secs(), ap_ip());
    let mut gateway = [ap_ip()];
    let dns = [ap_ip()];
    let mut options = ServerOptions::new(ap_ip(), Some(&mut gateway));
    options.dns = &dns;
    // Tells phones (RFC 8910) that there's a page to show them.
    options.captive_url = Some("http://192.168.4.1/");

    let mut request_buf = [0u8; 600];
    let mut reply_buf = [0u8; 600];
    loop {
        let Ok((len, _from)) = socket.recv_from(&mut request_buf).await else {
            continue;
        };
        let request = match Packet::decode(&request_buf[..len]) {
            Ok(request) => request,
            Err(e) => {
                warn!("DHCP: bad packet: {e:?}");
                continue;
            }
        };
        let mut option_buf = Options::buf();
        let Some(reply) = server.handle_request(&mut option_buf, &options, &request) else {
            continue;
        };
        // Phones have no address yet and may not take a unicast reply, so broadcast,
        // unless the phone already has one and didn't ask for a broadcast.
        let destination = if !request.ciaddr.is_unspecified() && !request.broadcast {
            request.ciaddr
        } else {
            Ipv4Addr::BROADCAST
        };
        match reply.encode(&mut reply_buf) {
            Ok(bytes) => {
                let [a, b, c, d] = destination.octets();
                let endpoint = IpEndpoint::new(IpAddress::v4(a, b, c, d), DHCP_CLIENT_PORT);
                if let Err(e) = socket.send_to(bytes, endpoint).await {
                    warn!("DHCP: send failed: {e:?}");
                } else {
                    info!("DHCP: replied to {destination}");
                }
            }
            Err(e) => warn!("DHCP: can't encode the reply: {e:?}"),
        }
    }
}

/// Answers every name lookup with the dashboard's address. Phones check for internet by
/// looking up a well known name, find it points here, and offer to "sign in to the network",
/// which opens the dashboard.
#[embassy_executor::task]
pub async fn dns_task(stack: Stack<'static>) {
    let mut rx_meta = [PacketMetadata::EMPTY; 4];
    let mut rx_buf = [0u8; 1024];
    let mut tx_meta = [PacketMetadata::EMPTY; 4];
    let mut tx_buf = [0u8; 1024];
    let mut socket = UdpSocket::new(stack, &mut rx_meta, &mut rx_buf, &mut tx_meta, &mut tx_buf);
    socket.bind(DNS_PORT).expect("bind DNS port");

    let mut request = [0u8; 512];
    let mut reply = [0u8; 512];
    loop {
        let Ok((len, from)) = socket.recv_from(&mut request).await else {
            continue;
        };
        match edge_captive::reply(&request[..len], &AP_IP.octets(), core::time::Duration::from_secs(60), &mut reply) {
            Ok(n) => {
                if let Err(e) = socket.send_to(&reply[..n], from).await {
                    warn!("DNS: send failed: {e:?}");
                }
            }
            Err(e) => warn!("DNS: can't answer: {e:?}"),
        }
    }
}


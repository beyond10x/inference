//! Adversary cases for the egress watch (`story:cascade-offline-profile`): what the watch's own
//! reading of the process's sockets ([`connected_sockets`], the reading every poll of
//! [`EgressWatch`](inference_runtime::egress::EgressWatch) acts on) sees of a datagram sent to a
//! non-loopback address without a connect, and of an IPv4 address reached over an IPv6 socket.
//!
//! No packet leaves the host: the non-loopback address is the host's own source address towards
//! TEST-NET-1 (RFC 5737), found by connecting a UDP socket, which sends nothing, and the datagram
//! is sent to a socket of this process bound on it. A host without such an address skips the
//! cases.
#![cfg(target_os = "linux")]

use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use inference_runtime::egress::{connected_sockets, is_egress, unconnected_datagram_sockets};

static WATCHED: Mutex<()> = Mutex::new(());

fn watched() -> MutexGuard<'static, ()> {
    WATCHED.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The host's own non-loopback IPv4 source address, if it has a route off the host.
fn own_address() -> Option<Ipv4Addr> {
    let probe = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    probe.connect((Ipv4Addr::new(192, 0, 2, 1), 9)).ok()?;
    match probe.local_addr().ok()?.ip() {
        IpAddr::V4(address) if is_egress(IpAddr::V4(address)) => Some(address),
        _ => None,
    }
}

#[test]
fn a_datagram_sent_to_a_non_loopback_address_without_a_connect_is_seen_by_the_watch() {
    let _watched = watched();
    let Some(address) = own_address() else {
        eprintln!("no non-loopback IPv4 address: skipped");
        return;
    };
    let receiver = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).expect("a receiver binds");
    receiver
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("a read timeout");
    let target = SocketAddr::from((address, receiver.local_addr().expect("bound").port()));
    let sender = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).expect("a sender binds");
    sender
        .send_to(b"x", target)
        .expect("the datagram is sent to the non-loopback address");
    let mut buffer = [0_u8; 1];
    let (_, from) = receiver
        .recv_from(&mut buffer)
        .expect("the datagram arrives");
    assert_eq!(from.ip(), IpAddr::V4(address));

    // An unconnected socket shows no remote address in /proc, so the watch cannot see where the
    // datagram went; it sees the socket itself, unconnected and not bound to loopback, and the
    // watch fails the run on that (coordinator's amendment after the review fix).
    let port = sender.local_addr().expect("bound").port();
    let seen = unconnected_datagram_sockets().expect("the sockets of this process are readable");
    assert!(
        seen.iter().any(|local| local.port() == port),
        "a datagram went to {target} from an unconnected socket on port {port}, and the watch's \
         reading of the unconnected datagram sockets does not show it: {seen:?}"
    );
    drop(sender);
}

#[test]
fn an_ipv4_address_connected_over_an_ipv6_socket_is_seen_as_egress() {
    let _watched = watched();
    let Some(address) = own_address() else {
        eprintln!("no non-loopback IPv4 address: skipped");
        return;
    };
    let Ok(socket) = UdpSocket::bind("[::]:0") else {
        eprintln!("no IPv6 socket: skipped");
        return;
    };
    let mapped = SocketAddr::from((address.to_ipv6_mapped(), 9));
    socket
        .connect(mapped)
        .expect("a UDP socket connects; connecting sends nothing");

    let seen = connected_sockets().expect("the sockets of this process are readable");
    assert!(
        seen.contains(&mapped),
        "the udp6 row of the socket reads as {mapped}: {seen:?}"
    );
    assert!(is_egress(mapped.ip()), "{mapped} is egress");
    drop(socket);
}

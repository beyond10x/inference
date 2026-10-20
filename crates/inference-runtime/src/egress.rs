//! The egress watch: a run whose pipeline's deployment profile denies egress fails once a socket
//! of this process is connected to an address that is neither loopback nor unspecified, or holds
//! a datagram socket that is connected to nothing and not bound to loopback
//! (`docs/design/inference.md` A-04 at 1140, section 23.2 at 1120, the profile table at 1046).
//!
//! Loopback is not egress: a local worker reached on 127.0.0.1 or ::1 is compute the
//! `offline_strict` profile admits. Any other remote address is egress.
//!
//! A datagram socket that is not connected (UDP, UDP-Lite, an unprivileged ICMP ping socket or a
//! raw socket) sends each datagram to the address its `sendto` names, and the socket tables list
//! no remote address for it: where it sent is not observable here. One bound to a loopback
//! address can reach nothing but loopback; one bound to any other address, the unspecified one
//! included, can reach any address unseen. The watch counts that one as egress by what it can
//! do, not by what it did: such a socket fails the run whether or not it sent anything.
//!
//! [`EgressWatch::start`] polls the sockets of the current process on a thread of its own: it
//! reads the socket inodes under `/proc/self/fd` and matches them against the rows of the tables
//! [`TABLES`] names, TCP, UDP, UDP-Lite, ICMP and raw over IPv4 and IPv6 ([`connected_sockets`],
//! [`unconnected_datagram_sockets`]). On the first socket whose remote address is egress
//! ([`is_egress`]), or failing one, the first unconnected datagram socket not bound to loopback,
//! it asks the run to fail ([`RunRequests::fail`]) with a [`FailureReason`] naming the address,
//! remote or local, and no node, and stops polling. A process whose sockets it cannot read fails
//! the run the same way, naming the error: a watch that sees nothing does not pass for one that
//! saw nothing.
//!
//! The watch is a tripwire and a test instrument, not a sandbox. Polling misses a socket opened
//! and closed between two polls, and nothing is prevented: the bytes of a connection it sees may
//! already have left. It reads every socket of the process, so it cannot tell which node opened
//! one, and a socket another run or the application opened trips it too. A connected UDP socket
//! is seen though it sent nothing. Sockets of other families are not read: a packet socket
//! (`AF_PACKET`, which needs `CAP_NET_RAW`), SCTP, or a socket of another process the run asks to
//! send for it. Only Linux has it: on another operating system this module is absent.

use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use inference_core::policy::Egress;

use crate::RunRequests;
use crate::run::FailureReason;

/// How often [`EgressWatch::start`] polls the sockets of the process.
pub const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// How the sockets of a table carry bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// A connection: the socket sends only to the remote address it is connected to; a socket
    /// with no remote address listens.
    Stream,
    /// Datagrams: a socket with no remote address sends to whatever address each `sendto` names.
    Datagram,
}

/// The socket tables of `/proc/self/net` the watch reads, IPv4 and IPv6, with how their sockets
/// carry bytes. Unprivileged ICMP ping sockets appear only in `icmp` and `icmp6`.
pub const TABLES: [(&str, Transport); 10] = [
    ("/proc/self/net/tcp", Transport::Stream),
    ("/proc/self/net/tcp6", Transport::Stream),
    ("/proc/self/net/udp", Transport::Datagram),
    ("/proc/self/net/udp6", Transport::Datagram),
    ("/proc/self/net/udplite", Transport::Datagram),
    ("/proc/self/net/udplite6", Transport::Datagram),
    ("/proc/self/net/icmp", Transport::Datagram),
    ("/proc/self/net/icmp6", Transport::Datagram),
    ("/proc/self/net/raw", Transport::Datagram),
    ("/proc/self/net/raw6", Transport::Datagram),
];

/// What the watch tripped on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tripped {
    /// A socket connected to this remote address.
    Connected(SocketAddr),
    /// A datagram socket bound to this local address and connected to nothing.
    Unconnected(SocketAddr),
}

/// What the watch thread and its handle share.
#[derive(Debug, Default)]
struct Shared {
    state: Mutex<Watched>,
    stop: Condvar,
}

#[derive(Debug, Default)]
struct Watched {
    stopped: bool,
    polls: u64,
    tripped: Option<SocketAddr>,
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, Watched> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A running egress watch over one run; see the module documentation. Dropping it stops the
/// polling.
pub struct EgressWatch {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl EgressWatch {
    /// Starts watching for the run `requests` asks, polling every [`POLL_INTERVAL`], when
    /// `egress`, the egress of the run's pipeline profile, is [`Egress::Denied`]; none for any
    /// other egress, whose rules are not this watch's.
    ///
    /// # Errors
    ///
    /// The error of spawning the watch thread.
    pub fn start(egress: Egress, requests: RunRequests) -> io::Result<Option<Self>> {
        Self::every(egress, requests, POLL_INTERVAL)
    }

    /// [`EgressWatch::start`], polling every `interval`.
    ///
    /// # Errors
    ///
    /// The error of spawning the watch thread.
    pub fn every(
        egress: Egress,
        requests: RunRequests,
        interval: Duration,
    ) -> io::Result<Option<Self>> {
        if egress != Egress::Denied {
            return Ok(None);
        }
        let shared = Arc::new(Shared::default());
        let watched = Arc::clone(&shared);
        let thread = thread::Builder::new()
            .name("egress-watch".to_owned())
            .spawn(move || watch(&watched, &requests, interval))?;
        Ok(Some(Self {
            shared,
            thread: Some(thread),
        }))
    }

    /// How many times the watch has read the sockets of the process.
    #[must_use]
    pub fn polls(&self) -> u64 {
        self.shared.state().polls
    }

    /// The address that failed the run, once one did: the remote address of a connected socket,
    /// or the local address of an unconnected datagram socket.
    #[must_use]
    pub fn tripped(&self) -> Option<SocketAddr> {
        self.shared.state().tripped
    }
}

impl Drop for EgressWatch {
    fn drop(&mut self) {
        self.shared.state().stopped = true;
        self.shared.stop.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl fmt::Debug for EgressWatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.shared.state();
        formatter
            .debug_struct("EgressWatch")
            .field("polls", &state.polls)
            .field("tripped", &state.tripped)
            .finish_non_exhaustive()
    }
}

/// The watch thread: polls until stopped or until it failed the run.
fn watch(shared: &Shared, requests: &RunRequests, interval: Duration) {
    loop {
        let found = sockets().map(|sockets| first_egress(&sockets));
        let mut state = shared.state();
        if state.stopped {
            return;
        }
        state.polls += 1;
        let detail = match found {
            Ok(None) => None,
            Ok(Some(Tripped::Connected(remote))) => {
                state.tripped = Some(remote);
                Some(format!(
                    "egress denied by the deployment profile: a socket of this process is \
                     connected to {remote}, which is neither loopback nor unspecified"
                ))
            }
            Ok(Some(Tripped::Unconnected(local))) => {
                state.tripped = Some(local);
                Some(format!(
                    "egress denied by the deployment profile: a datagram socket of this process \
                     bound to {local} is connected to no address and not bound to loopback, so \
                     it can send to any address unseen"
                ))
            }
            Err(error) => Some(format!(
                "egress denied by the deployment profile, and the egress watch cannot read the \
                 sockets of this process: {error}"
            )),
        };
        if let Some(detail) = detail {
            drop(state);
            requests.fail(FailureReason {
                node_id: None,
                detail,
            });
            return;
        }
        let (state, _) = shared
            .stop
            .wait_timeout_while(state, interval, |state| !state.stopped)
            .unwrap_or_else(PoisonError::into_inner);
        if state.stopped {
            return;
        }
    }
}

/// Whether a connection to `address` is egress: any address but a loopback or an unspecified
/// one, an IPv4 address mapped into IPv6 read as the IPv4 address.
#[must_use]
pub fn is_egress(address: IpAddr) -> bool {
    let address = address.to_canonical();
    !(address.is_loopback() || address.is_unspecified())
}

/// The remote address of every socket of this process that has one, in the order of the tables
/// of [`TABLES`]; a listening or unconnected socket, whose remote address is unspecified, has
/// none. A table the kernel does not provide, such as `tcp6` without IPv6, holds no socket.
///
/// # Errors
///
/// The error of reading `/proc/self/fd` or a table that exists.
pub fn connected_sockets() -> io::Result<Vec<SocketAddr>> {
    Ok(sockets()?
        .into_iter()
        .filter(|socket| !socket.remote.ip().is_unspecified())
        .map(|socket| socket.remote)
        .collect())
}

/// The local address of every datagram socket of this process that is connected to no address
/// and not bound to a loopback address, the unspecified address included, in the order of the
/// tables of [`TABLES`]: a socket that can send to any address without the tables showing where.
///
/// # Errors
///
/// As [`connected_sockets`].
pub fn unconnected_datagram_sockets() -> io::Result<Vec<SocketAddr>> {
    Ok(sockets()?
        .into_iter()
        .filter(Socket::sends_unseen)
        .map(|socket| socket.local)
        .collect())
}

/// One socket of this process as a table lists it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Socket {
    transport: Transport,
    local: SocketAddr,
    remote: SocketAddr,
}

impl Socket {
    /// Whether the socket is a datagram socket connected to nothing and not bound to loopback.
    fn sends_unseen(&self) -> bool {
        self.transport == Transport::Datagram
            && self.remote.ip().is_unspecified()
            && !self.local.ip().to_canonical().is_loopback()
    }
}

/// What fails a run among `sockets`: the first connected to an egress address, or failing one,
/// the first that sends unseen.
fn first_egress(sockets: &[Socket]) -> Option<Tripped> {
    sockets
        .iter()
        .find(|socket| is_egress(socket.remote.ip()))
        .map(|socket| Tripped::Connected(socket.remote))
        .or_else(|| {
            sockets
                .iter()
                .find(|socket| socket.sends_unseen())
                .map(|socket| Tripped::Unconnected(socket.local))
        })
}

/// Every socket of this process the tables of [`TABLES`] list, in their order.
fn sockets() -> io::Result<Vec<Socket>> {
    let inodes = socket_inodes()?;
    let mut sockets = Vec::new();
    for (table, transport) in TABLES {
        let rows = match fs::read_to_string(table) {
            Ok(rows) => rows,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        sockets.extend(listed(&rows, transport, &inodes));
    }
    Ok(sockets)
}

/// The sockets among `inodes` that `rows`, a table of `transport` with its heading line, lists.
fn listed<'a>(
    rows: &'a str,
    transport: Transport,
    inodes: &'a HashSet<u64>,
) -> impl Iterator<Item = Socket> + 'a {
    rows.lines()
        .skip(1)
        .filter_map(row)
        .filter(|(_, _, inode)| inodes.contains(inode))
        .map(move |(local, remote, _)| Socket {
            transport,
            local,
            remote,
        })
}

/// The inode of every socket among the open descriptors of this process. A descriptor closed
/// while it is read is skipped.
fn socket_inodes() -> io::Result<HashSet<u64>> {
    let mut inodes = HashSet::new();
    for entry in fs::read_dir("/proc/self/fd")? {
        let Ok(target) = entry.and_then(|entry| fs::read_link(entry.path())) else {
            continue;
        };
        if let Some(inode) = target
            .to_str()
            .and_then(|target| target.strip_prefix("socket:["))
            .and_then(|target| target.strip_suffix(']'))
            .and_then(|inode| inode.parse().ok())
        {
            inodes.insert(inode);
        }
    }
    Ok(inodes)
}

/// The local address, the remote address and the inode of one row of a socket table: the
/// second, the third and the tenth whitespace-separated fields.
fn row(line: &str) -> Option<(SocketAddr, SocketAddr, u64)> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    let local = address(fields.get(1)?)?;
    let remote = address(fields.get(2)?)?;
    let inode = fields.get(9)?.parse().ok()?;
    Some((local, remote, inode))
}

/// An address as the socket tables write it: the address in hexadecimal, each 32-bit word as the
/// kernel holds it in memory, then a colon and the port in hexadecimal.
fn address(field: &str) -> Option<SocketAddr> {
    let (host, port) = field.split_once(':')?;
    let port = u16::from_str_radix(port, 16).ok()?;
    let mut bytes = Vec::with_capacity(16);
    for word in host.as_bytes().chunks(8) {
        let word = u32::from_str_radix(std::str::from_utf8(word).ok()?, 16).ok()?;
        bytes.extend_from_slice(&word.to_ne_bytes());
    }
    let ip = match bytes.len() {
        4 => IpAddr::V4(Ipv4Addr::from(<[u8; 4]>::try_from(bytes).ok()?)),
        16 => IpAddr::V6(Ipv6Addr::from(<[u8; 16]>::try_from(bytes).ok()?)),
        _ => return None,
    };
    Some(SocketAddr::new(ip, port))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_table_row_reads_as_the_remote_address_and_the_inode() {
        let loopback = u32::from_ne_bytes([127, 0, 0, 1]);
        let line = format!(
            "   0: 00000000:0000 {loopback:08X}:0035 07 00000000:00000000 00:00000000 00000000  \
             1000        0 4242 2 0000000000000000 0"
        );
        assert_eq!(
            row(&line),
            Some((
                SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)),
                SocketAddr::from((Ipv4Addr::LOCALHOST, 53)),
                4242
            ))
        );
        let words: String = Ipv6Addr::LOCALHOST
            .octets()
            .chunks(4)
            .map(|word| {
                format!(
                    "{:08X}",
                    u32::from_ne_bytes(word.try_into().expect("four bytes"))
                )
            })
            .collect();
        assert_eq!(
            address(&format!("{words}:1F90")),
            Some(SocketAddr::from((Ipv6Addr::LOCALHOST, 8080)))
        );
        assert_eq!(address("0100007F"), None);
        assert_eq!(address("XYZ:0035"), None);
    }

    /// An `icmp` table as a little-endian kernel writes it: a ping socket of inode 4242
    /// connected to 192.0.2.1, an unconnected one of inode 4243 and one of another process.
    const ICMP: &str = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when \
                        retrnsmt   uid  timeout inode ref pointer drops\n  \
                        12: 00000000:0002 010200C0:0000 01 00000000:00000000 00:00000000 \
                        00000000  1000        0 4242 2 0000000000000000 0\n  \
                        13: 00000000:0003 00000000:0000 07 00000000:00000000 00:00000000 \
                        00000000  1000        0 4243 2 0000000000000000 0\n  \
                        14: 00000000:0004 010200C0:0000 01 00000000:00000000 00:00000000 \
                        00000000  1000        0 9999 2 0000000000000000 0\n";

    #[test]
    #[cfg(target_endian = "little")]
    fn a_ping_socket_is_read_from_the_icmp_tables() {
        assert!(
            ["/proc/self/net/icmp", "/proc/self/net/icmp6"]
                .iter()
                .all(|icmp| TABLES.contains(&(*icmp, Transport::Datagram))),
            "both ICMP tables are read, as datagram sockets"
        );
        let inodes = HashSet::from([4242, 4243]);
        let listed: Vec<Socket> = listed(ICMP, Transport::Datagram, &inodes).collect();
        let remote = SocketAddr::from((Ipv4Addr::new(192, 0, 2, 1), 0));
        let unspecified = |port| SocketAddr::from((Ipv4Addr::UNSPECIFIED, port));
        assert_eq!(
            listed,
            [
                Socket {
                    transport: Transport::Datagram,
                    local: unspecified(2),
                    remote,
                },
                Socket {
                    transport: Transport::Datagram,
                    local: unspecified(3),
                    remote: unspecified(0),
                },
            ],
            "the sockets of this process only"
        );
        assert_eq!(first_egress(&listed), Some(Tripped::Connected(remote)));
        assert_eq!(
            first_egress(&listed[1..]),
            Some(Tripped::Unconnected(unspecified(3))),
            "an unconnected ping socket can send anywhere"
        );
    }

    #[test]
    fn an_unconnected_datagram_socket_trips_unless_bound_to_loopback_and_a_listener_never_does() {
        let socket = |transport, local: SocketAddr| Socket {
            transport,
            local,
            remote: SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)),
        };
        let any = SocketAddr::from((Ipv4Addr::UNSPECIFIED, 5000));
        let own = SocketAddr::from((Ipv4Addr::new(192, 0, 2, 7), 5000));
        let any6 = SocketAddr::from((Ipv6Addr::UNSPECIFIED, 5000));
        for local in [any, own, any6] {
            assert_eq!(
                first_egress(&[socket(Transport::Datagram, local)]),
                Some(Tripped::Unconnected(local)),
                "a datagram socket bound to {local}"
            );
            assert_eq!(
                first_egress(&[socket(Transport::Stream, local)]),
                None,
                "a listener on {local} sends to no one"
            );
        }
        for loopback in [
            SocketAddr::from((Ipv4Addr::LOCALHOST, 5000)),
            SocketAddr::from((Ipv6Addr::LOCALHOST, 5000)),
            SocketAddr::from((Ipv4Addr::LOCALHOST.to_ipv6_mapped(), 5000)),
        ] {
            assert_eq!(
                first_egress(&[socket(Transport::Datagram, loopback)]),
                None,
                "a datagram socket bound to {loopback} reaches loopback only"
            );
        }
    }
}

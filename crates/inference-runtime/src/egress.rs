//! The egress watch: a run whose pipeline's deployment profile denies egress fails once a socket
//! of this process is connected to an address that is neither loopback nor unspecified
//! (`docs/design/inference.md` A-04 at 1140, section 23.2 at 1120, the profile table at 1046).
//!
//! Loopback is not egress: a local worker reached on 127.0.0.1 or ::1 is compute the
//! `offline_strict` profile admits. Any other remote address is egress.
//!
//! [`EgressWatch::start`] polls the sockets of the current process on a thread of its own: it
//! reads the socket inodes under `/proc/self/fd` and matches them against the rows of
//! `/proc/self/net/tcp`, `tcp6`, `udp` and `udp6` ([`connected_sockets`]). On the first socket
//! whose remote address is egress ([`is_egress`]) it asks the run to fail
//! ([`RunRequests::fail`]) with a [`FailureReason`] naming the address and no node, and stops
//! polling. A process whose sockets it cannot read fails the run the same way, naming the error:
//! a watch that sees nothing does not pass for one that saw nothing.
//!
//! The watch is a tripwire and a test instrument, not a sandbox. Polling misses a connection
//! opened and closed between two polls, and nothing is prevented: the bytes of a connection it
//! sees may already have left. It reads every socket of the process, so it cannot tell which node
//! opened one, and a socket another run or the application opened trips it too. A connected UDP
//! socket is seen though it sent nothing. Only Linux has it: on another operating system this
//! module is absent.

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

/// The socket tables of `/proc/self/net` the watch reads, IPv4 and IPv6.
const TABLES: [&str; 4] = [
    "/proc/self/net/tcp",
    "/proc/self/net/tcp6",
    "/proc/self/net/udp",
    "/proc/self/net/udp6",
];

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

    /// The remote address that failed the run, once one did.
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
        let found = connected_sockets()
            .map(|remotes| remotes.into_iter().find(|remote| is_egress(remote.ip())));
        let mut state = shared.state();
        if state.stopped {
            return;
        }
        state.polls += 1;
        let detail = match found {
            Ok(None) => None,
            Ok(Some(remote)) => {
                state.tripped = Some(remote);
                Some(format!(
                    "egress denied by the deployment profile: a socket of this process is \
                     connected to {remote}, which is neither loopback nor unspecified"
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

/// The remote address of every socket of this process that has one, TCP and UDP over IPv4 and
/// IPv6, in the order of the tables `/proc/self/net/tcp`, `tcp6`, `udp` and `udp6`; a listening
/// or unconnected socket, whose remote address is unspecified, has none. A table the kernel does
/// not provide, such as `tcp6` without IPv6, holds no socket.
///
/// # Errors
///
/// The error of reading `/proc/self/fd` or a table that exists.
pub fn connected_sockets() -> io::Result<Vec<SocketAddr>> {
    let inodes = socket_inodes()?;
    let mut remotes = Vec::new();
    for table in TABLES {
        let rows = match fs::read_to_string(table) {
            Ok(rows) => rows,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        remotes.extend(
            rows.lines()
                .skip(1)
                .filter_map(row)
                .filter(|(remote, inode)| inodes.contains(inode) && !remote.ip().is_unspecified())
                .map(|(remote, _)| remote),
        );
    }
    Ok(remotes)
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

/// The remote address and the inode of one row of a socket table: the third and the tenth
/// whitespace-separated fields.
fn row(line: &str) -> Option<(SocketAddr, u64)> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    let remote = address(fields.get(2)?)?;
    let inode = fields.get(9)?.parse().ok()?;
    Some((remote, inode))
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
            Some((SocketAddr::from((Ipv4Addr::LOCALHOST, 53)), 4242))
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
}

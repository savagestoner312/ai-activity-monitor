//! Per-process network connection enumeration + cached reverse DNS, ported
//! from the Python collector's `psutil.Process(pid).net_connections()` +
//! `socket.gethostbyaddr()` calls.
#![cfg(any(windows, target_os = "macos"))]

use netstat2::{get_sockets_info, AddressFamilyFlags, ProtocolFlags, ProtocolSocketInfo};
use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Reverse DNS lookups are blocking network calls with no built-in timeout.
/// On a cycle with many brand-new (pid, ip, port) tuples — plausible with
/// several concurrent AI-tool processes — resolving them one at a time can
/// stall the whole poll loop for minutes if some IPs have no PTR record and
/// the resolver is slow to give up. Resolving the batch concurrently and
/// giving up after this deadline bounds the damage to one slow cycle instead.
const DNS_BATCH_TIMEOUT: Duration = Duration::from_secs(2);

/// How long to leave an IP alone after a failed/timed-out lookup before
/// trying it again. Without this, a lookup that simply didn't finish within
/// `DNS_BATCH_TIMEOUT` (e.g. it landed in a large batch and lost the race)
/// would be indistinguishable from a genuine "no PTR record" IP and get
/// permanently stuck showing the raw address for the rest of the collector's
/// uptime. Retrying periodically fixes the transient case while still
/// sparing truly unresolvable IPs (Cloudflare et al.) from being re-queried
/// on every single new connection.
const DNS_FAIL_RETRY_COOLDOWN: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone)]
pub struct NetConn {
    pub pid: u32,
    pub raddr: IpAddr,
    pub rport: u16,
    pub status: String,
}

pub struct NetTracker {
    dns_cache: HashMap<IpAddr, String>,
    dns_failed_at: HashMap<IpAddr, Instant>,
    seen: HashSet<(u32, IpAddr, u16)>,
}

impl NetTracker {
    pub fn new() -> Self {
        Self { dns_cache: HashMap::new(), dns_failed_at: HashMap::new(), seen: HashSet::new() }
    }

    /// Resolves every not-yet-cached, not-in-cooldown IP concurrently (one
    /// thread each), waiting at most `DNS_BATCH_TIMEOUT` total. Anything
    /// still unresolved when the deadline hits is recorded as failed-just-now
    /// (not cached as a permanent ""), so it's eligible for another attempt
    /// after `DNS_FAIL_RETRY_COOLDOWN` instead of being stuck forever; its
    /// thread is left to finish in the background and is simply ignored.
    fn resolve_batch(&mut self, ips: Vec<IpAddr>) {
        let now = Instant::now();
        let dns_cache = &self.dns_cache;
        let dns_failed_at = &self.dns_failed_at;
        let to_resolve: Vec<IpAddr> = ips
            .into_iter()
            .collect::<HashSet<_>>()
            .into_iter()
            .filter(|ip| {
                if dns_cache.contains_key(ip) {
                    return false;
                }
                match dns_failed_at.get(ip) {
                    Some(&last_fail) => now.duration_since(last_fail) >= DNS_FAIL_RETRY_COOLDOWN,
                    None => true,
                }
            })
            .collect();
        if to_resolve.is_empty() {
            return;
        }

        let (tx, rx) = mpsc::channel();
        for ip in &to_resolve {
            let tx = tx.clone();
            let ip = *ip;
            std::thread::spawn(move || {
                let host = dns_lookup::lookup_addr(&ip).unwrap_or_default();
                let _ = tx.send((ip, host));
            });
        }
        drop(tx);

        let deadline = Instant::now() + DNS_BATCH_TIMEOUT;
        let mut resolved: HashMap<IpAddr, String> = HashMap::new();
        while resolved.len() < to_resolve.len() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match rx.recv_timeout(remaining) {
                Ok((ip, host)) => {
                    resolved.insert(ip, host);
                }
                Err(_) => break,
            }
        }

        for ip in to_resolve {
            match resolved.remove(&ip).filter(|h| !h.is_empty()) {
                Some(host) => {
                    self.dns_cache.insert(ip, host);
                    self.dns_failed_at.remove(&ip);
                }
                None => {
                    self.dns_failed_at.insert(ip, now);
                }
            }
        }
    }

    /// Returns only *new* (pid, raddr, rport) connections among the given
    /// AI-tool pids, deduping exactly like Python's `seen_conns` set.
    pub fn poll_new_connections(&mut self, ai_pids: &[u32]) -> Vec<(NetConn, String)> {
        let watched: HashSet<u32> = ai_pids.iter().copied().collect();
        let af_flags = AddressFamilyFlags::IPV4 | AddressFamilyFlags::IPV6;
        let proto_flags = ProtocolFlags::TCP;
        let sockets = get_sockets_info(af_flags, proto_flags).unwrap_or_default();

        let mut new_conns: Vec<NetConn> = Vec::new();
        for socket in &sockets {
            let ProtocolSocketInfo::Tcp(tcp) = &socket.protocol_socket_info else { continue };
            if tcp.remote_port == 0 {
                continue;
            }
            for pid in &socket.associated_pids {
                if !watched.contains(pid) {
                    continue;
                }
                let key = (*pid, tcp.remote_addr, tcp.remote_port);
                if self.seen.contains(&key) {
                    continue;
                }
                self.seen.insert(key);
                new_conns.push(NetConn {
                    pid: *pid,
                    raddr: tcp.remote_addr,
                    rport: tcp.remote_port,
                    status: format!("{:?}", tcp.state),
                });
            }
        }

        if new_conns.is_empty() {
            return Vec::new();
        }

        let unique_ips: Vec<IpAddr> = new_conns.iter().map(|c| c.raddr).collect();
        self.resolve_batch(unique_ips);

        new_conns
            .into_iter()
            .map(|c| {
                let host = self.dns_cache.get(&c.raddr).cloned().unwrap_or_default();
                (c, host)
            })
            .collect()
    }

    /// Matches Python's `if len(seen_conns) > 50000: seen_conns.clear()`.
    pub fn clear_if_large(&mut self) {
        if self.seen.len() > 50_000 {
            self.seen.clear();
        }
    }
}

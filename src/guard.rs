//! Who is asking, and how often they may get a password wrong.
//!
//! - Behind a reverse proxy every request comes from the proxy. Proxies OwnNAS trusts (this
//!   machine by default, more with `--trusted-proxy`) say who the client is in
//!   `X-Forwarded-For` and whether it came over HTTPS in `X-Forwarded-Proto`; anyone else's
//!   headers are ignored.
//! - Wrong passwords slow down the address and the account they came from, more with each
//!   one, instead of locking anyone out: a stranger guessing cannot keep the owner from
//!   signing in, and guessing becomes too slow to get anywhere.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use axum::http::HeaderMap;

/// An address with a prefix length, such as `10.0.0.0/8` or `::1/128`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Net {
    addr: IpAddr,
    prefix: u8,
}

impl Net {
    /// `192.168.1.10`, `10.0.0.0/8`, `fd00::/8`.
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        let (addr, prefix) = match text.split_once('/') {
            Some((addr, prefix)) => (addr, Some(prefix)),
            None => (text, None),
        };
        let addr: IpAddr = addr.trim().parse().map_err(|_| format!("{text} is not an address or a network such as 10.0.0.0/8"))?;
        let addr = canonical(addr);
        let most = if addr.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix {
            Some(prefix) => prefix.trim().parse::<u8>().ok().filter(|p| *p <= most).ok_or_else(|| format!("{text} has a wrong prefix length"))?,
            None => most,
        };
        Ok(Self { addr, prefix })
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.addr, canonical(ip)) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                let mask = if self.prefix == 0 { 0 } else { u32::MAX << (32 - u32::from(self.prefix)) };
                u32::from(net) & mask == u32::from(ip) & mask
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                let mask = if self.prefix == 0 { 0 } else { u128::MAX << (128 - u32::from(self.prefix)) };
                u128::from(net) & mask == u128::from(ip) & mask
            }
            _ => false,
        }
    }
}

/// `::ffff:1.2.3.4` is 1.2.3.4.
fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        v4 => v4,
    }
}

/// A reverse proxy on this machine.
pub fn default_trusted() -> Vec<Net> {
    vec![Net { addr: IpAddr::V4([127, 0, 0, 0].into()), prefix: 8 }, Net { addr: IpAddr::V6(std::net::Ipv6Addr::LOCALHOST), prefix: 128 }]
}

/// `--trusted-proxy 10.0.0.5,172.17.0.0/16`: these, and this machine.
pub fn parse_trusted(list: &[String]) -> Result<Vec<Net>, String> {
    let mut nets = default_trusted();
    for item in list.iter().flat_map(|item| item.split(',')).map(str::trim).filter(|item| !item.is_empty()) {
        nets.push(Net::parse(item)?);
    }
    Ok(nets)
}

fn trusted(nets: &[Net], ip: IpAddr) -> bool {
    nets.iter().any(|net| net.contains(ip))
}

/// The address of whoever is asking: the connection's, or, from a trusted proxy, the last
/// address in `X-Forwarded-For` that is not a trusted proxy itself (the ones before it could
/// be made up by the client).
pub fn client_ip(peer: Option<SocketAddr>, headers: &HeaderMap, nets: &[Net]) -> Option<IpAddr> {
    let peer = canonical(peer?.ip());
    if !trusted(nets, peer) {
        return Some(peer);
    }
    let mut client = peer;
    let forwarded: Vec<IpAddr> = headers
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .filter_map(parse_forwarded)
        .collect();
    for ip in forwarded.into_iter().rev() {
        client = ip;
        if !trusted(nets, ip) {
            break;
        }
    }
    Some(client)
}

/// `1.2.3.4`, `1.2.3.4:5678`, `2001:db8::1`, `[2001:db8::1]:443`.
fn parse_forwarded(text: &str) -> Option<IpAddr> {
    let text = text.trim();
    if let Ok(ip) = text.parse::<IpAddr>() {
        return Some(canonical(ip));
    }
    text.parse::<SocketAddr>().ok().map(|addr| canonical(addr.ip()))
}

/// Whether the request reached a trusted proxy over HTTPS.
pub fn forwarded_https(peer: Option<SocketAddr>, headers: &HeaderMap, nets: &[Net]) -> bool {
    peer.is_some_and(|peer| trusted(nets, canonical(peer.ip())))
        && headers
            .get("x-forwarded-proto")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(',').next())
            .is_some_and(|proto| proto.trim().eq_ignore_ascii_case("https"))
}

/// Wrong guesses by one address, one account, or one signed-in user's second password.
pub struct Throttle {
    /// Guesses allowed before any wait.
    free: u32,
    /// The longest wait.
    cap: Duration,
    seen: HashMap<String, (u32, Instant)>,
}

/// Remembered guesses past this many are forgotten, the oldest first.
const MOST_REMEMBERED: usize = 20_000;
/// A key with no wrong guess for this long starts afresh.
const FORGET_AFTER: Duration = Duration::from_secs(60 * 60);

impl Throttle {
    pub fn new(free: u32, cap: Duration) -> Self {
        Self { free, cap, seen: HashMap::new() }
    }

    /// How long `key` must still wait before its next guess (`None`: it may guess now).
    pub fn wait(&mut self, key: &str, now: Instant) -> Option<Duration> {
        let &(failures, last) = self.seen.get(key)?;
        if now.duration_since(last) >= FORGET_AFTER {
            self.seen.remove(key);
            return None;
        }
        let delay = self.delay(failures);
        let ready = last + delay;
        (ready > now).then(|| ready - now)
    }

    /// 1 s after the free guesses, then twice as long after each one, up to the cap.
    fn delay(&self, failures: u32) -> Duration {
        if failures < self.free {
            return Duration::ZERO;
        }
        let doublings = (failures - self.free).min(20);
        Duration::from_secs(1u64 << doublings).min(self.cap)
    }

    pub fn failed(&mut self, key: &str, now: Instant) {
        if self.seen.len() >= MOST_REMEMBERED && !self.seen.contains_key(key) {
            self.seen.retain(|_, (_, last)| now.duration_since(*last) < FORGET_AFTER);
            if self.seen.len() >= MOST_REMEMBERED {
                if let Some(oldest) = self.seen.iter().min_by_key(|(_, (_, last))| *last).map(|(key, _)| key.clone()) {
                    self.seen.remove(&oldest);
                }
            }
        }
        let entry = self.seen.entry(key.to_string()).or_insert((0, now));
        if now.duration_since(entry.1) >= FORGET_AFTER {
            entry.0 = 0;
        }
        entry.0 = entry.0.saturating_add(1);
        entry.1 = now;
    }

    pub fn succeeded(&mut self, key: &str) {
        self.seen.remove(key);
    }
}

/// "a few seconds", "2 minutes": for the message that says how long to wait.
pub fn wait_text(wait: Duration) -> String {
    let secs = wait.as_secs().max(1);
    if secs < 60 {
        format!("{secs} second{}", if secs == 1 { "" } else { "s" })
    } else {
        let minutes = secs.div_ceil(60);
        format!("{minutes} minute{}", if minutes == 1 { "" } else { "s" })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.append(*name, value.parse().unwrap());
        }
        map
    }

    #[test]
    fn networks() {
        let net = Net::parse("10.0.0.0/8").unwrap();
        assert!(net.contains("10.20.30.40".parse().unwrap()));
        assert!(!net.contains("11.0.0.1".parse().unwrap()));
        assert!(net.contains("::ffff:10.1.1.1".parse().unwrap()), "an IPv4 address written as IPv6");
        assert!(Net::parse("fd00::/8").unwrap().contains("fd12::1".parse().unwrap()));
        assert!(Net::parse("192.168.1.10").unwrap().contains("192.168.1.10".parse().unwrap()));
        assert!(Net::parse("10.0.0.0/40").is_err());
        assert!(Net::parse("example.com").is_err());
        assert_eq!(parse_trusted(&["172.17.0.0/16, 10.0.0.5".into()]).unwrap().len(), 4);
    }

    #[test]
    fn the_client_behind_a_proxy() {
        let nets = default_trusted();
        let proxy: SocketAddr = "127.0.0.1:50000".parse().unwrap();
        let outside: SocketAddr = "203.0.113.9:40000".parse().unwrap();
        // Straight from the internet: its own headers do not count.
        assert_eq!(client_ip(Some(outside), &headers(&[("x-forwarded-for", "10.0.0.1")]), &nets), Some(outside.ip()));
        assert!(!forwarded_https(Some(outside), &headers(&[("x-forwarded-proto", "https")]), &nets));
        // Through the proxy: the address it saw, not one the client wrote before it.
        let via = headers(&[("x-forwarded-for", "10.0.0.1, 198.51.100.7"), ("x-forwarded-proto", "https")]);
        assert_eq!(client_ip(Some(proxy), &via, &nets), Some("198.51.100.7".parse().unwrap()));
        assert!(forwarded_https(Some(proxy), &via, &nets));
        // Two proxies, both trusted.
        let more = parse_trusted(&["172.17.0.0/16".into()]).unwrap();
        let chain = headers(&[("x-forwarded-for", "198.51.100.7, 172.17.0.2")]);
        assert_eq!(client_ip(Some(proxy), &chain, &more), Some("198.51.100.7".parse().unwrap()));
        // No header: the proxy itself (this machine).
        assert_eq!(client_ip(Some(proxy), &HeaderMap::new(), &nets), Some(proxy.ip()));
        assert_eq!(client_ip(Some(proxy), &headers(&[("x-forwarded-for", "[2001:db8::1]:443")]), &nets), Some("2001:db8::1".parse().unwrap()));
    }

    #[test]
    fn wrong_guesses_slow_down_without_locking_out() {
        let mut throttle = Throttle::new(5, Duration::from_secs(60));
        let start = Instant::now();
        for _ in 0..5 {
            assert_eq!(throttle.wait("a", start), None);
            throttle.failed("a", start);
        }
        assert_eq!(throttle.wait("a", start), Some(Duration::from_secs(1)));
        assert_eq!(throttle.wait("a", start + Duration::from_secs(1)), None);
        for _ in 0..10 {
            throttle.failed("a", start);
        }
        assert_eq!(throttle.wait("a", start), Some(Duration::from_secs(60)), "never longer than the cap");
        assert_eq!(throttle.wait("b", start), None, "others are not held up");
        throttle.succeeded("a");
        assert_eq!(throttle.wait("a", start), None);
        throttle.failed("c", start);
        assert_eq!(throttle.wait("c", start + FORGET_AFTER), None, "old guesses are forgotten");
        assert_eq!(wait_text(Duration::from_secs(90)), "2 minutes");
    }
}

//! `aerolink guard`: bans abusive clients in the kernel firewall (docs/09-deployment.md OPS-013).
//!
//! nginx sends every line of its security log as a syslog datagram to `/run/aerolink-guard.sock`
//! (`aerolink-guard.socket`), so nothing tails or polls log files: the process sleeps in `recv` until
//! nginx reports something. Strikes are counted per client; after [`MAX_STRIKES`] within [`WINDOW`]
//! the client goes into an nftables set with a timeout. The kernel drops its packets to 80/443 and
//! expires the ban on its own, so there are no timers here either.
//!
//! The ban unit is one IPv4 address or one IPv6 /64 (a single host usually owns a whole /64 and can
//! rotate addresses inside it). Repeat offenders get doubled ban times, up to [`MAX_BAN`].

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::io::Write as _;
use std::net::{IpAddr, Ipv6Addr};
use std::process::{Command, Stdio};
use std::time::Duration;

use tokio::time::Instant;

use crate::config::Config;

pub const MAX_STRIKES: usize = 5;
pub const WINDOW: Duration = Duration::from_secs(10 * 60);
pub const BASE_BAN: Duration = Duration::from_secs(60 * 60);
pub const MAX_BAN: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// A client that behaves for this long after its last ban starts again from [`BASE_BAN`].
const REPEAT_MEMORY: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// Bound on tracked clients; a wide scan from many addresses cannot grow memory past this.
const MAX_TRACKED: usize = 65_536;
const TABLE: &str = "inet aerolink_guard";

// --- addresses ---

/// One IPv4 address or one IPv6 /64 (address masked).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Net {
    addr: IpAddr,
    prefix: u8,
}

impl Net {
    /// The ban unit containing `ip`. IPv4-mapped IPv6 addresses count as IPv4.
    pub fn of(ip: IpAddr) -> Net {
        match ip.to_canonical() {
            IpAddr::V4(v4) => Net { addr: IpAddr::V4(v4), prefix: 32 },
            IpAddr::V6(v6) => Net::new(IpAddr::V6(v6), 64),
        }
    }

    fn new(addr: IpAddr, prefix: u8) -> Net {
        let addr = match addr {
            IpAddr::V4(a) => IpAddr::V4((u32::from(a) & mask32(prefix)).into()),
            IpAddr::V6(a) => IpAddr::V6(Ipv6Addr::from(u128::from(a) & mask128(prefix))),
        };
        Net { addr, prefix }
    }

    /// `1.2.3.4`, `10.0.0.0/8`, `2001:db8::1`, `2001:db8::/48`.
    pub fn parse(s: &str) -> Option<Net> {
        let (addr, prefix) = match s.split_once('/') {
            Some((a, p)) => (a.parse::<IpAddr>().ok()?, Some(p.parse::<u8>().ok()?)),
            None => (s.parse::<IpAddr>().ok()?, None),
        };
        let addr = addr.to_canonical();
        let max = if addr.is_ipv4() { 32 } else { 128 };
        let prefix = prefix.unwrap_or(max);
        (prefix <= max).then(|| Net::new(addr, prefix))
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.addr, ip.to_canonical()) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => u32::from(ip) & mask32(self.prefix) == u32::from(net),
            (IpAddr::V6(net), IpAddr::V6(ip)) => u128::from(ip) & mask128(self.prefix) == u128::from(net),
            _ => false,
        }
    }

    fn set(&self) -> &'static str {
        if self.addr.is_ipv4() { "ban4" } else { "ban6" }
    }
}

impl fmt::Display for Net {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.addr {
            IpAddr::V4(a) if self.prefix == 32 => write!(f, "{a}"),
            a => write!(f, "{a}/{}", self.prefix),
        }
    }
}

fn mask32(prefix: u8) -> u32 {
    if prefix == 0 { 0 } else { u32::MAX << (32 - u32::from(prefix)) }
}

fn mask128(prefix: u8) -> u128 {
    if prefix == 0 { 0 } else { u128::MAX << (128 - u32::from(prefix)) }
}

// --- log lines ---

/// One line in nginx format `al_guard` / `al_guard_nohost` (`{"ip":…,"reason":…,"status":…,"uri":…}`;
/// the URI is cut to 160 characters there, because nginx drops syslog messages over ~2 KiB).
#[derive(Debug, PartialEq)]
pub struct Entry<'a> {
    pub ip: &'a str,
    pub reason: &'a str,
    pub status: u16,
    pub uri: &'a str,
}

/// Picks fields out of the line without a full JSON parse; a cut-off `uri` still yields its prefix.
pub fn parse(line: &str) -> Option<Entry<'_>> {
    let json = &line[line.find('{')?..];
    Some(Entry {
        ip: field(json, "ip")?,
        reason: field(json, "reason")?,
        status: field(json, "status")?.parse().ok()?,
        uri: field(json, "uri").unwrap_or(""),
    })
}

/// `"name":"value"` (escaped by nginx, so no bare quote inside; a truncated value runs to the end) or
/// `"name":123`.
fn field<'a>(json: &'a str, name: &str) -> Option<&'a str> {
    let key = format!("\"{name}\":");
    let rest = &json[json.find(&key)? + key.len()..];
    if let Some(s) = rest.strip_prefix('"') {
        Some(&s[..s.find('"').unwrap_or(s.len())])
    } else {
        Some(&rest[..rest.find([',', '}']).unwrap_or(rest.len())])
    }
}

/// What counts against a client: anything nginx refused (probe, scanner, method, foreign host), rate
/// limits, and failed authentication where guessing is possible. Other 401s are normal (the console asks
/// `/api/v1/me` on every visit to find out whether it is logged in); 404s are only logged.
pub fn is_strike(e: &Entry<'_>) -> bool {
    !e.reason.is_empty()
        || matches!(e.status, 429 | 444)
        || (e.status == 401 && (e.uri.starts_with("/api/v1/login") || e.uri.starts_with("/ws/robot")))
}

// --- strike counting ---

#[derive(Debug, PartialEq)]
pub struct Ban {
    pub net: Net,
    pub duration: Duration,
    /// 1 for a first ban, 2 for the next one, …
    pub count: u32,
}

#[derive(Default)]
struct Offender {
    strikes: VecDeque<Instant>,
    bans: u32,
    banned_until: Option<Instant>,
}

impl Offender {
    fn forgettable(&self, now: Instant) -> bool {
        let striking = self.strikes.back().is_some_and(|&t| now.duration_since(t) < WINDOW);
        let remembered = self.banned_until.is_some_and(|t| now < t + REPEAT_MEMORY);
        !striking && !remembered
    }
}

pub struct Guard {
    allow: Vec<Net>,
    offenders: HashMap<Net, Offender>,
}

impl Guard {
    pub fn new(mut allow: Vec<Net>) -> Guard {
        allow.extend([Net::new([127, 0, 0, 0].into(), 8), Net::new(Ipv6Addr::LOCALHOST.into(), 128)]);
        Guard { allow, offenders: HashMap::new() }
    }

    /// Feeds one security-log line; returns the ban to apply, if this line was the last straw.
    pub fn on_line(&mut self, line: &str, now: Instant) -> Option<Ban> {
        let entry = parse(line)?;
        if !is_strike(&entry) {
            return None;
        }
        let ip: IpAddr = entry.ip.parse().ok()?;
        if self.allow.iter().any(|n| n.contains(ip)) {
            return None;
        }
        let net = Net::of(ip);
        if !self.offenders.contains_key(&net) && self.offenders.len() >= MAX_TRACKED {
            self.offenders.retain(|_, o| !o.forgettable(now));
            if self.offenders.len() >= MAX_TRACKED {
                return None;
            }
        }
        let o = self.offenders.entry(net).or_default();
        if o.banned_until.is_some_and(|t| now < t) {
            return None; // lines still in flight from a client that is already banned
        }
        if o.banned_until.is_some_and(|t| now >= t + REPEAT_MEMORY) {
            o.bans = 0;
        }
        while o.strikes.front().is_some_and(|&t| now.duration_since(t) >= WINDOW) {
            o.strikes.pop_front();
        }
        o.strikes.push_back(now);
        if o.strikes.len() < MAX_STRIKES {
            return None;
        }
        o.strikes.clear();
        let duration = BASE_BAN.saturating_mul(1 << o.bans.min(16)).min(MAX_BAN);
        o.bans += 1;
        o.banned_until = Some(now + duration);
        Some(Ban { net, duration, count: o.bans })
    }
}

// --- nftables ---

pub struct Firewall {
    nft: String,
}

impl Firewall {
    pub fn new(nft: &str) -> Firewall {
        Firewall { nft: nft.to_owned() }
    }

    /// Creates the table, sets and chain if missing; existing bans are kept (`add` is idempotent, only
    /// the chain's rules are rebuilt).
    pub fn setup(&self) -> Result<(), String> {
        self.run(&format!(
            "add table {TABLE}\n\
             add set {TABLE} ban4 {{ type ipv4_addr; flags timeout; }}\n\
             add set {TABLE} ban6 {{ type ipv6_addr; flags interval, timeout; }}\n\
             add chain {TABLE} input {{ type filter hook input priority filter - 10; policy accept; }}\n\
             flush chain {TABLE} input\n\
             add rule {TABLE} input tcp dport {{ 80, 443 }} ip saddr @ban4 drop\n\
             add rule {TABLE} input tcp dport {{ 80, 443 }} ip6 saddr @ban6 drop\n"
        ))
    }

    /// Adds (or renews) a ban. If the table vanished (`systemctl restart nftables` flushes the ruleset),
    /// it is recreated and the ban retried once.
    pub fn ban(&self, ban: &Ban) -> Result<(), String> {
        let (net, set) = (ban.net, ban.net.set());
        let script = format!(
            "destroy element {TABLE} {set} {{ {net} }}\nadd element {TABLE} {set} {{ {net} timeout {}s }}\n",
            ban.duration.as_secs()
        );
        self.run(&script).or_else(|_| {
            self.setup()?;
            self.run(&script)
        })
    }

    pub fn unban(&self, net: Net) -> Result<(), String> {
        self.run(&format!("delete element {TABLE} {} {{ {net} }}\n", net.set()))
    }

    pub fn list(&self) -> Result<String, String> {
        let out = Command::new(&self.nft)
            .args(["list", "table", "inet", "aerolink_guard"])
            .output()
            .map_err(|e| format!("{}: {e}", self.nft))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_owned())
        }
    }

    /// `nft -f -`: one transaction per script. Bans are rare, so a short blocking spawn is fine.
    fn run(&self, script: &str) -> Result<(), String> {
        let mut child = Command::new(&self.nft)
            .args(["-f", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("{}: {e}", self.nft))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(script.as_bytes()).map_err(|e| format!("{}: {e}", self.nft))?;
        }
        let out = child.wait_with_output().map_err(|e| format!("{}: {e}", self.nft))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(format!("{}: {}", self.nft, String::from_utf8_lossy(&out.stderr).trim()))
        }
    }
}

// --- service ---

pub fn allow_list(cfg: &Config) -> Result<Vec<Net>, String> {
    cfg.guard_allow
        .iter()
        .map(|s| Net::parse(s).ok_or_else(|| format!("AEROLINK_GUARD_ALLOW: bad address or network: {s}")))
        .collect()
}

pub async fn run(cfg: Config) -> Result<(), String> {
    let mut guard = Guard::new(allow_list(&cfg)?);
    let socket = bind(&cfg.guard_listen).map_err(|e| {
        tracing::error!("guard listen {}: {e}", cfg.guard_listen);
        e.to_string()
    })?;
    let firewall = Firewall::new(&cfg.guard_nft);
    match firewall.setup() {
        Ok(()) => tracing::info!("aerolink guard {} started", env!("CARGO_PKG_VERSION")),
        // Keep running: the next ban retries the setup.
        Err(e) => tracing::error!("nftables setup: {e}"),
    }

    let shutdown = crate::server::shutdown_signal();
    tokio::pin!(shutdown);
    // nginx caps syslog messages well below this.
    let mut buf = vec![0u8; 8192];
    loop {
        let n = tokio::select! {
            r = socket.recv(&mut buf) => r.map_err(|e| {
                tracing::error!("guard recv: {e}");
                e.to_string()
            })?,
            _ = &mut shutdown => return Ok(()),
        };
        let line = String::from_utf8_lossy(&buf[..n]);
        let Some(ban) = guard.on_line(&line, Instant::now()) else { continue };
        let what = parse(&line).map(|e| format!("{} {} {}", e.status, e.reason, e.uri)).unwrap_or_default();
        let what: String = what.chars().take(200).collect();
        tracing::warn!("ban {} for {} h (#{}), last: {what}", ban.net, ban.duration.as_secs() / 3600, ban.count);
        if let Err(e) = firewall.ban(&ban) {
            tracing::error!("ban {}: {e}", ban.net);
        }
    }
}

fn bind(listen: &str) -> std::io::Result<tokio::net::UnixDatagram> {
    let socket = if listen == "systemd" {
        use std::os::fd::FromRawFd;
        let fd = crate::server::systemd_fd("AEROLINK_GUARD_LISTEN")?;
        // SAFETY: systemd passed exactly this one fd (aerolink-guard.socket) and nothing else owns it.
        unsafe { std::os::unix::net::UnixDatagram::from_raw_fd(fd) }
    } else if let Some(path) = listen.strip_prefix("unix:") {
        let _ = std::fs::remove_file(path);
        std::os::unix::net::UnixDatagram::bind(path)?
    } else {
        return Err(std::io::Error::other("expected `systemd` or `unix:/path`"));
    };
    socket.set_nonblocking(true)?;
    tokio::net::UnixDatagram::from_std(socket)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(ip: &str, reason: &str, status: u16, uri: &str) -> String {
        format!(
            "<190>Oct 29 12:00:00 aerolink: {{\"ip\":\"{ip}\",\"reason\":\"{reason}\",\"status\":{status},\"uri\":\"{uri}\"}}"
        )
    }

    #[test]
    fn parses_lines_even_truncated() {
        let l = line("192.0.2.1", "probe", 444, "/.env");
        assert_eq!(parse(&l), Some(Entry { ip: "192.0.2.1", reason: "probe", status: 444, uri: "/.env" }));
        let cut = &l[..l.find("/.env").unwrap() + 2];
        assert_eq!(parse(cut).unwrap().uri, "/.");
        assert_eq!(parse("garbage"), None);
        assert_eq!(parse("{\"ip\":\"1.2.3.4\"}"), None);
    }

    #[test]
    fn decides_what_is_a_strike() {
        let strike = |reason, status, uri| is_strike(&parse(&line("192.0.2.1", reason, status, uri)).unwrap());
        assert!(strike("agent", 444, "/"));
        assert!(strike("", 429, "/assets/a.js"));
        assert!(strike("", 401, "/api/v1/login"));
        assert!(strike("", 401, "/ws/robot"));
        assert!(!strike("", 401, "/api/v1/me"));
        assert!(!strike("", 401, "/ws/operator"));
        assert!(!strike("", 404, "/nope"));
        assert!(!strike("", 502, "/api/v1/me"));
    }

    #[test]
    fn bans_after_five_strikes_in_the_window() {
        let mut g = Guard::new(vec![]);
        let t = Instant::now();
        let probe = line("192.0.2.1", "probe", 444, "/.env");
        for i in 0..4 {
            assert_eq!(g.on_line(&probe, t + Duration::from_secs(i)), None);
        }
        let ban = g.on_line(&probe, t + Duration::from_secs(4)).unwrap();
        assert_eq!(ban, Ban { net: Net::parse("192.0.2.1").unwrap(), duration: BASE_BAN, count: 1 });
        // Already banned: in-flight lines change nothing.
        assert_eq!(g.on_line(&probe, t + Duration::from_secs(5)), None);
        // Strikes spread wider than the window never add up.
        let mut g = Guard::new(vec![]);
        for i in 0..20 {
            assert_eq!(g.on_line(&probe, t + WINDOW / 4 * i), None);
        }
    }

    #[test]
    fn repeat_offenders_get_longer_bans() {
        let mut g = Guard::new(vec![]);
        let probe = line("192.0.2.1", "probe", 444, "/.env");
        let mut t = Instant::now();
        let mut durations = vec![];
        for _ in 0..10 {
            let ban = (0..5).find_map(|_| g.on_line(&probe, t)).unwrap();
            durations.push(ban.duration.as_secs() / 3600);
            t += ban.duration;
        }
        assert_eq!(durations, [1, 2, 4, 8, 16, 32, 64, 128, 168, 168]);
        // A week of good behaviour after the last ban: back to one hour.
        t += REPEAT_MEMORY;
        assert_eq!((0..5).find_map(|_| g.on_line(&probe, t)).unwrap().duration, BASE_BAN);
    }

    #[test]
    fn ipv6_is_banned_per_64_and_allow_list_wins() {
        let mut g = Guard::new(vec![Net::parse("198.51.100.0/24").unwrap()]);
        let t = Instant::now();
        let mut ban = None;
        for i in 0..5 {
            ban = g.on_line(&line(&format!("2001:db8:1:2::{i}"), "probe", 444, "/.git/config"), t);
        }
        assert_eq!(ban.unwrap().net.to_string(), "2001:db8:1:2::/64");
        for _ in 0..10 {
            assert_eq!(g.on_line(&line("198.51.100.7", "probe", 444, "/"), t), None);
            assert_eq!(g.on_line(&line("127.0.0.1", "", 429, "/"), t), None);
            assert_eq!(g.on_line(&line("::1", "", 429, "/"), t), None);
        }
        assert_eq!(Net::of("::ffff:192.0.2.9".parse().unwrap()).to_string(), "192.0.2.9");
        assert_eq!(Net::parse("10.1.2.3/8").unwrap().to_string(), "10.0.0.0/8");
        assert_eq!(Net::parse("10.0.0.0/33"), None);
    }

    #[tokio::test]
    async fn bans_through_nft() {
        let dir = std::env::temp_dir().join(format!("aerolink-guard-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("nft.log");
        let nft = dir.join("nft");
        std::fs::write(&nft, format!("#!/bin/sh\ncat >> {}\n", log.display())).unwrap();
        std::fs::set_permissions(&nft, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let sock = dir.join("guard.sock");

        let cfg = Config::from_lookup(|k| match k {
            "AEROLINK_GUARD_LISTEN" => Some(format!("unix:{}", sock.display())),
            "AEROLINK_GUARD_NFT" => Some(nft.display().to_string()),
            _ => None,
        });
        let task = tokio::spawn(run(cfg));
        while !sock.exists() {
            tokio::task::yield_now().await;
        }
        let client = tokio::net::UnixDatagram::unbound().unwrap();
        for _ in 0..5 {
            client.send_to(line("203.0.113.5", "agent", 444, "/").as_bytes(), &sock).await.unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        let text = loop {
            let text = std::fs::read_to_string(&log).unwrap_or_default();
            if text.contains("add element") || Instant::now() > deadline {
                break text;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        task.abort();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(text.contains("add chain inet aerolink_guard input"), "{text}");
        assert!(text.contains("add element inet aerolink_guard ban4 { 203.0.113.5 timeout 3600s }"), "{text}");
    }
}

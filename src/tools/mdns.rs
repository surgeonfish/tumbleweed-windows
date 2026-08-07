//! Minimal mDNS (`.local`) name resolution built on std UDP sockets.
//!
//! Resolves a link-local hostname such as `tumbleweed.local` by sending mDNS
//! queries to `224.0.0.251:5353` and collecting the A / AAAA answers. No
//! external crates are required — it uses only [`std::net`].

use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::time::Duration;

/// mDNS IPv4 link-local multicast group.
const MDNS_GROUP: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
/// mDNS port.
const MDNS_PORT: u16 = 5353;
/// How long to wait for responses during a single lookup.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(3);

/// Resolve `name` (e.g. `"tumbleweed.local"`) to its IP addresses.
///
/// Blocks for up to [`DEFAULT_TIMEOUT`]. Call it on a background thread if you
/// don't want to stall the UI thread.
#[allow(dead_code)] // client-side discovery; this app currently runs as the server
pub fn resolve(name: &str) -> io::Result<Vec<IpAddr>> {
    resolve_with_timeout(name, DEFAULT_TIMEOUT)
}

/// Resolve `name`, waiting up to `timeout` for mDNS responses.
#[allow(dead_code)] // client-side discovery; this app currently runs as the server
pub fn resolve_with_timeout(name: &str, timeout: Duration) -> io::Result<Vec<IpAddr>> {
    let name = name.trim_end_matches('.');

    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    socket.set_multicast_ttl_v4(255)?;
    // Best-effort: joining lets us also hear multicast replies (some responders
    // ignore the unicast-response hint in the query), but we mainly rely on the
    // QU (unicast-response) bit, so a join failure shouldn't abort the lookup.
    let _ = socket.join_multicast_v4(&MDNS_GROUP, &Ipv4Addr::UNSPECIFIED);

    // Ask for both address record types to maximise responder compatibility.
    for qtype in [1u16 /* A */, 28u16 /* AAAA */] {
        let query = build_query(name, qtype);
        socket.send_to(&query, SocketAddr::new(IpAddr::V4(MDNS_GROUP), MDNS_PORT))?;
    }

    let deadline = std::time::Instant::now() + timeout;
    let mut addrs = Vec::new();
    let mut buf = [0u8; 4096];

    loop {
        let now = std::time::Instant::now();
        if now >= deadline {
            break;
        }
        socket.set_read_timeout(Some(deadline - now))?;
        let (len, _src) = match socket.recv_from(&mut buf) {
            Ok(v) => v,
            Err(e)
                if e.kind() == io::ErrorKind::WouldBlock
                    || e.kind() == io::ErrorKind::TimedOut =>
            {
                break
            }
            Err(e) => return Err(e),
        };
        parse_response(&buf[..len], name, &mut addrs);
    }

    Ok(addrs)
}

/// Continuously resolve `name` every `interval`, calling `on_report` with the
/// current result **every cycle** — so it doubles as a liveness heartbeat. The
/// first call happens right after the first lookup, even when nothing is found.
///
/// Blocks forever — run it on its own thread, e.g.:
///
/// ```text
/// std::thread::spawn(|| {
///     tools::mdns::listen("tumbleweed.local", Duration::from_secs(5), |addrs| {
///         match addrs {
///             Some(a) => println!("tumbleweed.local -> {a:?}"),
///             None => println!("tumbleweed.local: not found"),
///         }
///     });
/// });
/// ```
#[allow(dead_code)] // client-side discovery; this app currently runs as the server
pub fn listen<F>(name: &str, interval: Duration, mut on_report: F)
where
    F: FnMut(Option<Vec<IpAddr>>),
{
    loop {
        let current = match resolve(name) {
            Ok(addrs) if !addrs.is_empty() => Some(addrs),
            _ => None,
        };
        // Report every poll (not just on change), so there's always output
        // proving the listener is alive.
        on_report(current.clone());
        std::thread::sleep(interval);
    }
}

/// Encode a domain name into DNS label format:
/// `tumbleweed.local` -> `\x0atumbleweed\x05local\x00`.
fn encode_name(name: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(name.len() + 2);
    for label in name.split('.') {
        out.push(label.len() as u8);
        out.extend_from_slice(label.as_bytes());
    }
    out.push(0);
    out
}

/// Build a DNS query packet for `name` with the given record `qtype`.
fn build_query(name: &str, qtype: u16) -> Vec<u8> {
    let mut pkt = Vec::with_capacity(12 + name.len() + 6);
    pkt.extend_from_slice(&0u16.to_be_bytes()); // ID
    pkt.extend_from_slice(&0x0000u16.to_be_bytes()); // flags: standard query
    pkt.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
    pkt.extend_from_slice(&0u16.to_be_bytes()); // ANCOUNT
    pkt.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT
    pkt.extend_from_slice(&0u16.to_be_bytes()); // ARCOUNT
    pkt.extend_from_slice(&encode_name(name));
    pkt.extend_from_slice(&qtype.to_be_bytes());
    // QCLASS = IN (1) | QU (0x8000): ask responders to send a unicast reply
    // back to our source port instead of to the multicast group.
    pkt.extend_from_slice(&0x8001u16.to_be_bytes());
    pkt
}

/// Parse one DNS/mDNS response, appending A/AAAA records for `want` to `out`.
fn parse_response(pkt: &[u8], want: &str, out: &mut Vec<IpAddr>) {
    if pkt.len() < 12 {
        return;
    }
    let qdcount = u16::from_be_bytes([pkt[4], pkt[5]]) as usize;
    let ancount = u16::from_be_bytes([pkt[6], pkt[7]]) as usize;

    let mut pos = 12usize;

    // Skip the question section.
    for _ in 0..qdcount {
        let Some(consumed) = skip_name(pkt, pos) else { return };
        pos += consumed;
        if pos + 4 > pkt.len() {
            return;
        }
        pos += 4; // QTYPE + QCLASS
    }

    // Walk the answer section.
    for _ in 0..ancount {
        let Some((name, consumed)) = read_name(pkt, pos) else { return };
        pos += consumed;
        if pos + 10 > pkt.len() {
            return;
        }
        let rtype = u16::from_be_bytes([pkt[pos], pkt[pos + 1]]);
        let rclass = u16::from_be_bytes([pkt[pos + 2], pkt[pos + 3]]);
        // TTL lives in pkt[pos+4..pos+8]; not needed here.
        let rdlen = u16::from_be_bytes([pkt[pos + 8], pkt[pos + 9]]) as usize;
        pos += 10;
        if pos + rdlen > pkt.len() {
            return;
        }
        let rdata = &pkt[pos..pos + rdlen];
        pos += rdlen;

        // Ignore non-IN classes (clear the mDNS cache-flush bit first).
        if rclass & 0x7FFF != 1 {
            continue;
        }
        if !name.eq_ignore_ascii_case(want) {
            continue;
        }
        match rtype {
            1 if rdlen == 4 => out.push(IpAddr::V4(Ipv4Addr::new(
                rdata[0], rdata[1], rdata[2], rdata[3],
            ))),
            28 if rdlen == 16 => {
                let mut octets = [0u8; 16];
                octets.copy_from_slice(&rdata[..16]);
                out.push(IpAddr::V6(Ipv6Addr::from(octets)));
            }
            _ => {}
        }
    }
}

/// Skip a (possibly compressed) DNS name, returning bytes consumed at `start`.
fn skip_name(pkt: &[u8], start: usize) -> Option<usize> {
    let mut pos = start;
    loop {
        if pos >= pkt.len() {
            return None;
        }
        let len = pkt[pos] as usize;
        if len & 0xC0 == 0xC0 {
            return Some(pos + 2 - start); // compression pointer ends the local walk
        }
        if len == 0 {
            return Some(pos + 1 - start); // terminating root label
        }
        pos += 1 + len;
    }
}

/// Read a (possibly compressed) DNS name starting at `start`. Returns the
/// dotted name and the number of bytes consumed at `start` (a compression
/// pointer ends the local walk even though the name continues elsewhere).
fn read_name(pkt: &[u8], start: usize) -> Option<(String, usize)> {
    let mut labels: Vec<String> = Vec::new();
    let mut pos = start;
    let mut consumed: Option<usize> = None;
    let mut visited = std::collections::HashSet::new();

    loop {
        if pos >= pkt.len() {
            return None;
        }
        if !visited.insert(pos) {
            return None; // compression pointer loop — malformed packet
        }
        let len = pkt[pos] as usize;
        if len & 0xC0 == 0xC0 {
            if pos + 1 >= pkt.len() {
                return None;
            }
            let ptr = ((len & 0x3F) << 8) | pkt[pos + 1] as usize;
            if consumed.is_none() {
                consumed = Some(pos + 2 - start);
            }
            pos = ptr;
        } else if len == 0 {
            if consumed.is_none() {
                consumed = Some(pos + 1 - start);
            }
            break;
        } else {
            if pos + 1 + len > pkt.len() {
                return None;
            }
            labels.push(String::from_utf8_lossy(&pkt[pos + 1..pos + 1 + len]).to_string());
            pos += 1 + len;
        }
    }

    Some((labels.join("."), consumed.unwrap_or(0)))
}

// ---------------------------------------------------------------------------
// mDNS advertiser — the app advertises itself as a `.local` hostname so other
// devices on the LAN can reach it (e.g. its HTTP file server).
// ---------------------------------------------------------------------------

/// One resource record for building mDNS responses.
struct Answer {
    name: String,
    rtype: u16,
    class: u16,
    ttl: u32,
    rdata: Vec<u8>,
}

/// Build a DNS response packet. `question` (if any) is echoed verbatim.
fn build_response(id: u16, flags: u16, question: Option<&[u8]>, answers: &[Answer]) -> Vec<u8> {
    let mut pkt = Vec::new();
    pkt.extend_from_slice(&id.to_be_bytes());
    pkt.extend_from_slice(&flags.to_be_bytes());
    let qd = if question.is_some() { 1u16 } else { 0u16 };
    pkt.extend_from_slice(&qd.to_be_bytes());
    pkt.extend_from_slice(&(answers.len() as u16).to_be_bytes());
    pkt.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT
    pkt.extend_from_slice(&0u16.to_be_bytes()); // ARCOUNT
    if let Some(q) = question {
        pkt.extend_from_slice(q);
    }
    for a in answers {
        pkt.extend_from_slice(&encode_name(&a.name));
        pkt.extend_from_slice(&a.rtype.to_be_bytes());
        pkt.extend_from_slice(&a.class.to_be_bytes());
        pkt.extend_from_slice(&a.ttl.to_be_bytes());
        pkt.extend_from_slice(&(a.rdata.len() as u16).to_be_bytes());
        pkt.extend_from_slice(&a.rdata);
    }
    pkt
}

/// DNS-SD service type shared by every tumbleweed device. This is the fixed
/// name clients query to discover all devices, regardless of each device's
/// unique instance/hostname.
fn service_type(_host: &str) -> String {
    "_tumbleweed._tcp.local".to_string()
}

/// The service instance name (derived from the hostname).
fn service_instance(host: &str) -> String {
    format!("{}._tcp.local", host.trim_end_matches(".local"))
}

/// Build the PTR + SRV + TXT + A answers that advertise this device.
fn service_answers(name: &str, service_port: u16, addrs: &[Ipv4Addr]) -> Vec<Answer> {
    let instance = service_instance(name);
    let service = service_type(name);
    let mut answers = Vec::new();

    // PTR: _tumbleweed._tcp.local -> tumbleweed._tcp.local
    answers.push(Answer {
        name: service,
        rtype: 12, // PTR
        class: 1,
        ttl: 120,
        rdata: encode_name(&instance),
    });
    // SRV: tumbleweed._tcp.local -> priority/weight/port + tumbleweed.local
    let mut srv = Vec::new();
    srv.extend_from_slice(&0u16.to_be_bytes()); // priority
    srv.extend_from_slice(&0u16.to_be_bytes()); // weight
    srv.extend_from_slice(&service_port.to_be_bytes());
    srv.extend_from_slice(&encode_name(name));
    answers.push(Answer {
        name: instance.clone(),
        rtype: 33, // SRV
        class: 1,
        ttl: 120,
        rdata: srv,
    });
    // TXT: Android NsdManager refuses to resolve a service without a TXT
    // record, so always advertise one (txtvers is the conventional first key).
    answers.push(Answer {
        name: instance.clone(),
        rtype: 16, // TXT
        class: 1,
        ttl: 120,
        rdata: b"\x08txtvers=1".to_vec(),
    });
    // A: tumbleweed.local -> ip (unique, cache-flush)
    for ip in addrs {
        answers.push(Answer {
            name: name.to_string(),
            rtype: 1,
            class: 0x8001, // IN | cache-flush
            ttl: 120,
            rdata: ip.octets().to_vec(),
        });
    }
    answers
}

/// Unsolicited announcement: service PTR + SRV + host A record(s).
fn build_announcement(name: &str, service_port: u16, addrs: &[Ipv4Addr]) -> Vec<u8> {
    let answers = service_answers(name, service_port, addrs);
    build_response(0, 0x8400, None, &answers)
}

/// If `pkt` is a query we should answer, build a response. Returns the packet
/// and where to send it (unicast to `src` when the QU bit is set, else
/// multicast). Answers host A queries and DNS-SD PTR/SRV queries.
fn response_for_query(
    pkt: &[u8],
    name: &str,
    service_port: u16,
    addrs: &[Ipv4Addr],
    src: SocketAddr,
) -> Option<(Vec<u8>, SocketAddr)> {
    if pkt.len() < 12 {
        return None;
    }
    let flags = u16::from_be_bytes([pkt[2], pkt[3]]);
    if flags & 0x8000 != 0 {
        return None; // it's a response, not a query
    }
    let (qname, consumed) = read_name(pkt, 12)?;
    if consumed == 0 {
        return None;
    }
    let pos = 12 + consumed;
    if pos + 4 > pkt.len() {
        return None;
    }
    let qtype = u16::from_be_bytes([pkt[pos], pkt[pos + 1]]);
    let qclass = u16::from_be_bytes([pkt[pos + 2], pkt[pos + 3]]);
    let q_end = pos + 4;

    let instance = service_instance(name);
    let service = service_type(name);
    let host = name.to_string();

    let mut answers = Vec::new();
    if qname.eq_ignore_ascii_case(&host) && (qtype == 1 || qtype == 255) {
        // Host A / ANY query.
        for ip in addrs {
            answers.push(Answer {
                name: host.clone(),
                rtype: 1,
                class: 0x8001,
                ttl: 120,
                rdata: ip.octets().to_vec(),
            });
        }
    } else if qname.eq_ignore_ascii_case(&service) && (qtype == 12 || qtype == 255) {
        // Service PTR / ANY query -> bundle everything so the client learns
        // the instance, port, and address from one response.
        answers = service_answers(name, service_port, addrs);
    } else if qname.eq_ignore_ascii_case(&instance) && (qtype == 33 || qtype == 255) {
        // Service instance SRV / ANY query.
        let mut srv = Vec::new();
        srv.extend_from_slice(&0u16.to_be_bytes());
        srv.extend_from_slice(&0u16.to_be_bytes());
        srv.extend_from_slice(&service_port.to_be_bytes());
        srv.extend_from_slice(&encode_name(name));
        answers.push(Answer {
            name: instance.clone(),
            rtype: 33,
            class: 1,
            ttl: 120,
            rdata: srv,
        });
        // Android NsdManager needs a TXT record to resolve the service.
        answers.push(Answer {
            name: instance.clone(),
            rtype: 16,
            class: 1,
            ttl: 120,
            rdata: b"\x08txtvers=1".to_vec(),
        });
    }

    if answers.is_empty() {
        return None;
    }

    let id = u16::from_be_bytes([pkt[0], pkt[1]]);
    let question = &pkt[12..q_end];
    let resp = build_response(id, 0x8400, Some(question), &answers);
    let dest = if qclass & 0x8000 != 0 {
        src // QU bit: reply unicast to the query source
    } else {
        SocketAddr::new(IpAddr::V4(MDNS_GROUP), MDNS_PORT)
    };
    Some((resp, dest))
}

/// Best-effort list of this machine's non-loopback IPv4 addresses (via the
/// UDP-connect trick — works offline since connect only selects a route).
fn local_ipv4_addrs() -> Vec<Ipv4Addr> {
    let mut addrs = Vec::new();
    for target in ["8.8.8.8:80", "1.1.1.1:80", "192.168.1.1:80"] {
        let Ok(s) = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)) else {
            continue;
        };
        if s.connect(target).is_err() {
            continue;
        }
        let Ok(addr) = s.local_addr() else {
            continue;
        };
        if let IpAddr::V4(v4) = addr.ip() {
            if !v4.is_unspecified() && !v4.is_loopback() && !addrs.contains(&v4) {
                addrs.push(v4);
            }
        }
    }
    addrs
}

/// The machine's primary LAN IPv4 (first usable non-loopback address).
pub(crate) fn lan_ipv4() -> Option<Ipv4Addr> {
    local_ipv4_addrs().into_iter().next()
}

/// Write a line to a log file (`%TEMP%\tumbleweed-mdns.log`) so GUI-subsystem
/// builds (which have no console) still surface mDNS status and errors.
pub(crate) fn log_msg(msg: &str) {
    use std::io::Write;
    let path = std::env::temp_dir().join("tumbleweed-mdns.log");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(f, "{msg}");
    }
}

/// Bind the mDNS responder socket on port 5353 with `SO_REUSEADDR` so it can
/// share the port with any OS mDNS responder, and join the multicast group.
/// `iface` is the LAN IPv4 used as the multicast egress interface.
fn bind_mdns_socket(iface: Ipv4Addr) -> io::Result<UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    socket.set_multicast_ttl_v4(255)?;
    socket.set_multicast_loop_v4(true)?;
    socket.set_multicast_if_v4(&iface)?;
    socket.bind(&SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), MDNS_PORT).into())?;
    // Join the group on the specific LAN interface (not the OS default), so a
    // machine with multiple adapters reliably receives LAN multicast.
    socket.join_multicast_v4(&MDNS_GROUP, &iface)?;
    Ok(socket.into())
}

/// Bind a socket for sending multicast announcements from an *ephemeral* port
/// with the multicast egress pinned to `iface`. Android's NsdManager discovers
/// unsolicited announcements sent this way, but ignores ones sourced from port
/// 5353 (and multicast sent out a virtual adapter never reaches the LAN).
fn bind_announcer(iface: Ipv4Addr) -> io::Result<UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_multicast_ttl_v4(255)?;
    socket.set_multicast_loop_v4(true)?;
    socket.set_multicast_if_v4(&iface)?;
    socket.bind(&SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0).into())?;
    Ok(socket.into())
}

/// How long a discovered peer is considered alive without a new packet.
const PEER_TTL: Duration = Duration::from_secs(15);

/// Peers discovered on the LAN, maintained by the advertiser thread from the
/// packets it already receives on its single mDNS socket (see
/// [`record_peer_packet`]). Keyed by IP with a last-seen timestamp.
static PEERS: std::sync::OnceLock<
    std::sync::Mutex<Vec<(DiscoveredDevice, std::time::Instant)>>,
> = std::sync::OnceLock::new();

fn peers() -> &'static std::sync::Mutex<Vec<(DiscoveredDevice, std::time::Instant)>> {
    PEERS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

/// Record any peer devices advertised in `pkt` (PTR instance + A records) into
/// the shared registry, pruning entries that have gone stale.
fn record_peer_packet(pkt: &[u8]) {
    let (instances, ips) = parse_discovery_response(pkt);
    if ips.is_empty() {
        return;
    }
    let own_ips = local_ipv4_addrs();
    let now = std::time::Instant::now();
    if let Ok(mut guard) = peers().lock() {
        for (i, ip) in ips.iter().enumerate() {
            if let IpAddr::V4(v4) = ip {
                if own_ips.contains(v4) {
                    continue;
                }
            }
            let name = instances
                .get(i)
                .or_else(|| instances.first())
                .cloned()
                .unwrap_or_else(|| "tumbleweed".to_string());
            let name = display_name(&name);
            match guard.iter_mut().find(|e| e.0.ip == *ip) {
                Some(e) => {
                    e.0.name = name;
                    e.1 = now;
                }
                None => guard.push((
                    DiscoveredDevice {
                        name,
                        ip: *ip,
                        kind: String::new(),
                        version: String::new(),
                    },
                    now,
                )),
            }
        }
        guard.retain(|e| e.1.elapsed() < PEER_TTL);
    }
}

/// Advertise `name` (e.g. `"tumbleweed.local"`) over mDNS and answer queries,
/// so other machines on the LAN can reach this app. Also advertises the DNS-SD
/// service (`_tumbleweed._tcp.local`) so other tumbleweed apps can discover it.
///
/// `service_port` is the TCP port of the HTTP file server (used in the SRV
/// record). Sends an initial announcement, re-announces periodically, and
/// responds to incoming queries. Blocking — run on its own thread.
///
/// Announcements and query responses are sent from the standard mDNS port
/// (5353) when it can be bound; if that port is taken, an ephemeral socket is
/// used for announcement-only mode so devices still learn of us.
pub fn advertise(name: &str, service_port: u16) -> io::Result<()> {
    let name = name.trim_end_matches('.');
    let addrs = local_ipv4_addrs();
    if addrs.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no usable IPv4 address to advertise",
        ));
    }
    let iface = addrs[0];

    // Prefer the standard mDNS port: announce and answer queries from it. If
    // it's already taken (e.g. by the OS mDNS responder), fall back to an
    // ephemeral socket that can at least send announcements.
    let (socket, standard) = match bind_mdns_socket(iface) {
        Ok(s) => (s, true),
        Err(e) => {
            log_msg(&format!(
                "[mdns] could not bind 5353 ({e}); using ephemeral announcement-only mode"
            ));
            (bind_announcer(iface)?, false)
        }
    };

    let announce = build_announcement(name, service_port, &addrs);
    socket.send_to(&announce, SocketAddr::new(IpAddr::V4(MDNS_GROUP), MDNS_PORT))?;
    log_msg(&format!(
        "[mdns] advertising {name} at {addrs:?} (service {}) standard_5353={standard}",
        service_type(name)
    ));

    let mut buf = [0u8; 4096];
    let mut last_announce = std::time::Instant::now();
    let probe = build_query(&service_type(name), 12);
    let group = SocketAddr::new(IpAddr::V4(MDNS_GROUP), MDNS_PORT);
    loop {
        if standard {
            let _ = socket.set_read_timeout(Some(Duration::from_secs(1)));
            match socket.recv_from(&mut buf) {
                Ok((len, src)) => {
                    // This socket sees every peer's multicast announcements and
                    // replies — record them so the UI's device list stays fresh
                    // without a second (starved) 5353 socket.
                    record_peer_packet(&buf[..len]);
                    if let Some((resp, dest)) =
                        response_for_query(&buf[..len], name, service_port, &addrs, src)
                    {
                        let _ = socket.send_to(&resp, dest);
                    }
                }
                Err(e)
                    if e.kind() == io::ErrorKind::WouldBlock
                        || e.kind() == io::ErrorKind::TimedOut => {}
                // Never let a responder hiccup kill the advertiser.
                Err(e) => log_msg(&format!("[mdns] responder recv error (continuing): {e}")),
            }
        } else {
            std::thread::sleep(Duration::from_millis(500));
        }
        // Re-announce often enough that clients reliably learn of us, and probe
        // for other responders so we discover query-only peers too.
        if last_announce.elapsed() >= Duration::from_secs(3) {
            let _ = socket.send_to(&announce, group);
            let _ = socket.send_to(&probe, group);
            last_announce = std::time::Instant::now();
        }
    }
}

// ---------------------------------------------------------------------------
// mDNS client discovery — find other tumbleweed devices on the LAN.
// ---------------------------------------------------------------------------

/// A device discovered on the LAN via mDNS.
#[derive(Clone, Debug, PartialEq)]
pub struct DiscoveredDevice {
    pub name: String,
    pub ip: IpAddr,
    /// Device type reported over HTTP (`/info`): "pc", "phone" or "" while
    /// unknown/pending.
    pub kind: String,
    /// App version reported over HTTP (`/info`), "" while unknown/pending.
    pub version: String,
}

/// A unique, per-machine `.local` hostname, e.g. `tumbleweed-desktop-abc123.local`.
///
/// Derived from the Windows `COMPUTERNAME` so each device advertises its own
/// instance/hostname instead of colliding on a shared name.
pub fn device_hostname() -> String {
    let raw = std::env::var("COMPUTERNAME").unwrap_or_default();
    let label = sanitize_label(&raw).unwrap_or_else(|| "host".to_string());
    format!("tumbleweed-{label}.local")
}

/// This machine's plain host name (from `COMPUTERNAME`), e.g. `desktop-abc123`.
pub fn device_host_name() -> String {
    let raw = std::env::var("COMPUTERNAME").unwrap_or_default();
    sanitize_label(&raw).unwrap_or_else(|| "host".to_string())
}

/// This machine's non-loopback IPv4 addresses as display strings, for the
/// "This device" entry in the Devices page.
pub fn local_ip_addrs() -> Vec<String> {
    local_ipv4_addrs().iter().map(|a| a.to_string()).collect()
}

/// Sanitize a hostname-ish string into a valid single DNS label.
fn sanitize_label(s: &str) -> Option<String> {
    let mut out: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    out = out.trim_matches('-').to_string();
    out.truncate(63);
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Strip the `tumbleweed-` prefix and the `._tumbleweed._tcp.local` /
/// `._tcp.local` / `.local` suffix for a clean display name
/// (e.g. `tumbleweed-oneplus-8t._tumbleweed._tcp.local` -> `oneplus-8t`).
fn display_name(instance: &str) -> String {
    let trimmed = instance
        .trim_end_matches("._tumbleweed._tcp.local")
        .trim_end_matches("._tcp.local")
        .trim_end_matches(".local")
        .trim_end_matches('.');
    trimmed
        .strip_prefix("tumbleweed-")
        .unwrap_or(trimmed)
        .to_string()
}

/// Discover tumbleweed devices on the LAN by PTR-querying the tumbleweed
/// service type and collecting each responder's advertised host IP. Blocks
/// for up to `timeout` — call it on a background thread.
pub fn discover_devices(timeout: Duration) -> io::Result<Vec<DiscoveredDevice>> {
    // Send a PTR query to prompt responders, then return the snapshot the
    // advertiser thread maintains from its single 5353 socket (which sees all
    // multicast — a second 5353 socket in the same process would be starved by
    // Windows). The snapshot is updated continuously by `record_peer_packet`.
    let _ = timeout;
    let service = service_type("tumbleweed.local");
    let query = build_query(&service, 12);
    if let Ok(s) = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)) {
        let _ = s.send_to(&query, SocketAddr::new(IpAddr::V4(MDNS_GROUP), MDNS_PORT));
    }
    // Give responders a moment to answer (the advertiser thread records them).
    std::thread::sleep(Duration::from_millis(250));
    let mut snapshot: Vec<DiscoveredDevice> = peers()
        .lock()
        .map(|g| g.iter().map(|e| e.0.clone()).collect())
        .unwrap_or_default();
    // Ask each newly seen peer what kind of device it is over HTTP (`/info`),
    // caching the answer in the shared registry so we only fetch each peer
    // once (not every poll).
    for d in snapshot.iter_mut() {
        if !d.kind.is_empty() {
            continue;
        }
        if let Some(info) = super::client::fetch_info(d.ip, super::server::HTTP_PORT) {
            d.kind = info.kind;
            d.version = info.version;
            if let Ok(mut guard) = peers().lock() {
                if let Some(e) = guard.iter_mut().find(|e| e.0.ip == d.ip) {
                    e.0.kind = d.kind.clone();
                    e.0.version = d.version.clone();
                }
            }
        }
    }
    log_snapshot_change(&snapshot);
    Ok(snapshot)
}

/// Log `[mdns] devices: ...` only when the discovered set changes, so the log
/// file shows peers appearing/leaving without a line every poll.
fn log_snapshot_change(snapshot: &[DiscoveredDevice]) {
    static LAST: std::sync::OnceLock<std::sync::Mutex<Option<Vec<DiscoveredDevice>>>> =
        std::sync::OnceLock::new();
    let current = snapshot.to_vec();
    let mut last = LAST
        .get_or_init(|| std::sync::Mutex::new(None))
        .lock()
        .unwrap();
    if last.as_ref() != Some(&current) {
        log_msg(&format!("[mdns] devices: {snapshot:?}"));
        *last = Some(current);
    }
}

/// Parse a discovery response, collecting PTR instance names and A-record IPs.
fn parse_discovery_response(pkt: &[u8]) -> (Vec<String>, Vec<IpAddr>) {
    let empty = || (Vec::new(), Vec::new());
    if pkt.len() < 12 {
        return empty();
    }
    let qdcount = u16::from_be_bytes([pkt[4], pkt[5]]) as usize;
    let ancount = u16::from_be_bytes([pkt[6], pkt[7]]) as usize;
    let mut pos = 12usize;

    // Skip the question section.
    for _ in 0..qdcount {
        let Some(consumed) = skip_name(pkt, pos) else {
            return empty();
        };
        pos += consumed;
        if pos + 4 > pkt.len() {
            return empty();
        }
        pos += 4;
    }

    let mut instances = Vec::new();
    let mut ips = Vec::new();

    for _ in 0..ancount {
        let Some((_owner, consumed)) = read_name(pkt, pos) else {
            break;
        };
        pos += consumed;
        if pos + 10 > pkt.len() {
            break;
        }
        let rtype = u16::from_be_bytes([pkt[pos], pkt[pos + 1]]);
        let rdlen = u16::from_be_bytes([pkt[pos + 8], pkt[pos + 9]]) as usize;
        pos += 10;
        if pos + rdlen > pkt.len() {
            break;
        }
        let rdata = &pkt[pos..pos + rdlen];
        match rtype {
            12 => {
                // PTR rdata is the instance name (may use compression pointers).
                if let Some((instance, _)) = read_name(pkt, pos) {
                    instances.push(instance);
                }
            }
            1 if rdlen == 4 => {
                ips.push(IpAddr::V4(Ipv4Addr::new(
                    rdata[0], rdata[1], rdata[2], rdata[3],
                )));
            }
            _ => {}
        }
        pos += rdlen;
    }

    (instances, ips)
}

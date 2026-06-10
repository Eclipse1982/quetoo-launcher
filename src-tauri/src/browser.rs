//! Quetoo server-browser protocol module.
//!
//! Pure, testable parsers + thin async I/O layer (tokio UdpSocket).
//! No I/O in unit-tested functions — all network calls are in `fetch_master_list`
//! and `probe_server`.

use crate::error::{LauncherError, Result};
use serde::Serialize;
use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddrV4};

// ── Public constants ──────────────────────────────────────────────────────────

pub const QUETOO_PROTOCOL: u32 = 2027;
pub const MASTER_HOST: &str = "master.quetoo.org";
pub const MASTER_PORT: u16 = 1996;
pub const DEFAULT_SERVER_PORT: u16 = 1998;

/// OOB packet prefix: four 0xFF bytes.
const OOB: [u8; 4] = [0xFF, 0xFF, 0xFF, 0xFF];

/// Probe / master timeout in milliseconds.
const TIMEOUT_MS: u64 = 1500;

// ── Data shapes ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerInfo {
    pub name: String,
    pub score: i32,
    pub ping: i32,
    pub bot: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    /// "ip:port"
    pub addr: String,
    pub hostname: String,
    pub map: String,
    pub gameplay: String,
    /// Human players (non-bot player lines).
    pub clients: u32,
    pub bots: u32,
    pub max_clients: u32,
    /// RTT in ms, clamped 1–999.
    pub ping: u32,
    /// sv_protocol from the infostring, 0 if absent.
    pub protocol: u32,
    pub favorite: bool,
    pub players: Vec<PlayerInfo>,
}

/// Intermediate result from `parse_status_response` before addr/favorite are known.
#[derive(Debug, Clone, PartialEq)]
pub struct StatusInfo {
    pub hostname: String,
    pub map: String,
    pub gameplay: String,
    pub clients: u32,
    pub bots: u32,
    pub max_clients: u32,
    pub protocol: u32,
    pub players: Vec<PlayerInfo>,
}

// ── Pure builders ─────────────────────────────────────────────────────────────

/// Build the UDP packet sent to the master server.
/// Bytes: `\xFF\xFF\xFF\xFF` + `getservers <QUETOO_PROTOCOL>`
pub fn master_request() -> Vec<u8> {
    let mut pkt = OOB.to_vec();
    pkt.extend_from_slice(format!("getservers {QUETOO_PROTOCOL}").as_bytes());
    pkt
}

/// Build the UDP status-request packet sent to an individual server.
/// Bytes: `\xFF\xFF\xFF\xFF` + `status`
pub fn status_request() -> Vec<u8> {
    let mut pkt = OOB.to_vec();
    pkt.extend_from_slice(b"status");
    pkt
}

// ── Pure parsers ──────────────────────────────────────────────────────────────

/// Parse a master-server response into a list of server addresses.
///
/// Expected layout:
/// - Bytes 0–3:  `\xFF\xFF\xFF\xFF`
/// - Bytes 4–11: `servers ` (8 ASCII bytes, total header = 12 bytes)
/// - Bytes 12+:  repeated 6-byte entries: 4-byte IPv4 + 2-byte big-endian port.
///   An entry with port == 0 terminates the list.  A truncated trailing entry
///   (fewer than 6 bytes) is silently ignored.
///
/// Returns `Err(Network)` if the 12-byte header does not match.
pub fn parse_master_response(buf: &[u8]) -> Result<Vec<SocketAddrV4>> {
    // Validate the 12-byte header: FF FF FF FF s e r v e r s SP
    let expected_header: &[u8] = b"\xFF\xFF\xFF\xFFservers ";
    if buf.len() < 12 || &buf[..12] != expected_header {
        return Err(LauncherError::Network(
            "master response: invalid header".into(),
        ));
    }

    let payload = &buf[12..];
    let mut addrs = Vec::new();
    let mut i = 0;
    while i + 6 <= payload.len() {
        let ip = Ipv4Addr::new(payload[i], payload[i + 1], payload[i + 2], payload[i + 3]);
        let port = u16::from_be_bytes([payload[i + 4], payload[i + 5]]);
        if port == 0 {
            break;
        }
        addrs.push(SocketAddrV4::new(ip, port));
        i += 6;
    }
    // Any remaining bytes (< 6) are a truncated trailing entry — silently ignore.
    Ok(addrs)
}

/// Parse a `\key\value\...` infostring (tolerant of leading/trailing backslash
/// and odd token counts).
pub fn parse_info_string(s: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    // Strip a leading backslash so the first token is a key, not an empty prefix.
    let s = s.strip_prefix('\\').unwrap_or(s);
    let mut parts = s.split('\\');
    loop {
        match (parts.next(), parts.next()) {
            (Some(k), Some(v)) => {
                // Empty keys and values are tolerated; we insert them verbatim
                // so callers can detect missing keys via `get` returning None
                // vs an empty-string value.
                map.insert(k.to_string(), v.to_string());
            }
            // Trailing key with no value (odd token count): skip it.
            (Some(_), None) => break,
            _ => break,
        }
    }
    map
}

/// Parse a status-response buffer into a `StatusInfo`.
///
/// Expected layout:
/// - Bytes 0–3:   `\xFF\xFF\xFF\xFF`
/// - Bytes 4–10:  `status\n`
/// - Remaining:   LINE 0 = infostring `\key\value\...`; subsequent lines = player lines.
///
/// Player line format: `\score\N\ping\N\name\NAME` (optional `\ai\1` suffix for bots).
/// Returns `Err(Network)` for truncated / unrecognisable buffers.
pub fn parse_status_response(buf: &[u8]) -> Result<StatusInfo> {
    // Validate OOB prefix.
    if buf.len() < 4 || &buf[..4] != &OOB {
        return Err(LauncherError::Network(
            "status response: missing OOB prefix".into(),
        ));
    }

    // The remainder after the 4-byte prefix must be valid UTF-8.
    let text = std::str::from_utf8(&buf[4..])
        .map_err(|_| LauncherError::Network("status response: non-UTF-8 body".into()))?;

    // Must start with "status\n".
    let rest = text
        .strip_prefix("status\n")
        .ok_or_else(|| LauncherError::Network("status response: missing 'status\\n' header".into()))?;

    let mut lines = rest.lines();

    // Line 0: infostring.
    let infostring = lines
        .next()
        .ok_or_else(|| LauncherError::Network("status response: missing infostring".into()))?;
    let info = parse_info_string(infostring);

    let hostname = info.get("sv_hostname").cloned().unwrap_or_default();
    let map = info.get("sv_map").cloned().unwrap_or_default();
    let gameplay = info.get("g_gameplay").cloned().unwrap_or_default();
    let max_clients = info
        .get("sv_max_clients")
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(0);
    let protocol = info
        .get("sv_protocol")
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(0);

    // Remaining lines: per-player entries.
    let mut players = Vec::new();
    for line in lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(player) = parse_player_line(line) {
            players.push(player);
        }
    }

    let bots = players.iter().filter(|p| p.bot).count() as u32;
    let clients = players.iter().filter(|p| !p.bot).count() as u32;

    Ok(StatusInfo {
        hostname,
        map,
        gameplay,
        clients,
        bots,
        max_clients,
        protocol,
        players,
    })
}

/// Parse a single player line: `\score\N\ping\N\name\NAME` (+ optional `\ai\1`).
/// Player names may contain spaces — split ONLY on backslash.
/// Returns `None` for lines that cannot be parsed as a player entry.
fn parse_player_line(line: &str) -> Option<PlayerInfo> {
    // Strip leading backslash so the first token is a key.
    let line = line.strip_prefix('\\').unwrap_or(line);
    let mut parts = line.split('\\');

    // We need at least score, score-val, ping, ping-val, name, name-val.
    let mut map: BTreeMap<&str, &str> = BTreeMap::new();
    loop {
        match (parts.next(), parts.next()) {
            (Some(k), Some(v)) => {
                map.insert(k, v);
            }
            _ => break,
        }
    }

    let score = map.get("score").and_then(|v| v.parse::<i32>().ok())?;
    let ping = map.get("ping").and_then(|v| v.parse::<i32>().ok())?;
    let name = map.get("name").copied().unwrap_or("").to_string();
    let bot = map.get("ai").map(|v| *v == "1").unwrap_or(false);

    Some(PlayerInfo {
        name,
        score,
        ping,
        bot,
    })
}

// ── Async I/O ─────────────────────────────────────────────────────────────────

/// Resolve `master.quetoo.org:1996`, send the list request, receive the
/// response (up to 64 KB), parse and return the address list.
/// Times out after `TIMEOUT_MS` ms.  DNS failure, timeout, and bad header
/// all return `Err`.
pub async fn fetch_master_list() -> Result<Vec<SocketAddrV4>> {
    use tokio::net::UdpSocket;
    use tokio::time::{timeout, Duration};

    // DNS-resolve master, take the first IPv4 address.
    let master_addr = {
        let addrs: Vec<_> = tokio::net::lookup_host(format!("{}:{}", MASTER_HOST, MASTER_PORT))
            .await
            .map_err(|e| LauncherError::Network(format!("DNS lookup failed: {e}")))?
            .filter(|a| a.is_ipv4())
            .collect();
        addrs
            .into_iter()
            .next()
            .ok_or_else(|| LauncherError::Network("no IPv4 address for master host".into()))?
    };

    let sock = UdpSocket::bind("0.0.0.0:0")
        .await
        .map_err(|e| LauncherError::Network(format!("bind failed: {e}")))?;

    let req = master_request();
    sock.send_to(&req, master_addr)
        .await
        .map_err(|e| LauncherError::Network(format!("send to master failed: {e}")))?;

    let mut buf = vec![0u8; 65536];
    let n = timeout(Duration::from_millis(TIMEOUT_MS), sock.recv(&mut buf))
        .await
        .map_err(|_| LauncherError::Network("master server timed out".into()))?
        .map_err(|e| LauncherError::Network(format!("recv from master failed: {e}")))?;

    parse_master_response(&buf[..n])
}

/// Send a status request to `addr`, measure RTT, parse the response.
///
/// On timeout:
/// - If `favorite` is true: return a stub `ServerInfo` with hostname `"(no response)"`,
///   ping 999, everything else zeroed/empty, and `favorite: true`.
/// - Otherwise: return `None`.
pub async fn probe_server(addr: SocketAddrV4, favorite: bool) -> Option<ServerInfo> {
    use std::time::Instant;
    use tokio::net::UdpSocket;
    use tokio::time::{timeout, Duration};

    let addr_str = addr.to_string();

    let sock = match UdpSocket::bind("0.0.0.0:0").await {
        Ok(s) => s,
        Err(_) => return dead_server_or_none(addr_str, favorite),
    };

    if sock.connect(addr).await.is_err() {
        return dead_server_or_none(addr_str, favorite);
    }

    let req = status_request();
    if sock.send(&req).await.is_err() {
        return dead_server_or_none(addr_str, favorite);
    }

    let start = Instant::now();
    let mut buf = vec![0u8; 65536];

    let n = match timeout(Duration::from_millis(TIMEOUT_MS), sock.recv(&mut buf)).await {
        Ok(Ok(n)) => n,
        _ => return dead_server_or_none(addr_str, favorite),
    };

    let rtt_ms = start.elapsed().as_millis() as u32;
    let ping = rtt_ms.clamp(1, 999);

    let status = match parse_status_response(&buf[..n]) {
        Ok(s) => s,
        Err(_) => return dead_server_or_none(addr_str, favorite),
    };

    Some(ServerInfo {
        addr: addr_str,
        hostname: status.hostname,
        map: status.map,
        gameplay: status.gameplay,
        clients: status.clients,
        bots: status.bots,
        max_clients: status.max_clients,
        ping,
        protocol: status.protocol,
        favorite,
        players: status.players,
    })
}

/// Return a dead-server stub (favorite) or None (non-favorite).
fn dead_server_or_none(addr: String, favorite: bool) -> Option<ServerInfo> {
    if favorite {
        Some(ServerInfo {
            addr,
            hostname: "(no response)".into(),
            map: String::new(),
            gameplay: String::new(),
            clients: 0,
            bots: 0,
            max_clients: 0,
            ping: 999,
            protocol: 0,
            favorite: true,
            players: Vec::new(),
        })
    } else {
        None
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── master_request ────────────────────────────────────────────────────────

    #[test]
    fn master_request_exact_bytes() {
        let pkt = master_request();
        assert_eq!(&pkt[..4], &[0xFF, 0xFF, 0xFF, 0xFF]);
        assert_eq!(&pkt[4..], b"getservers 2027");
        assert_eq!(pkt.len(), 4 + 15);
    }

    // ── status_request ────────────────────────────────────────────────────────

    #[test]
    fn status_request_exact_bytes() {
        let pkt = status_request();
        assert_eq!(&pkt[..4], &[0xFF, 0xFF, 0xFF, 0xFF]);
        assert_eq!(&pkt[4..], b"status");
        assert_eq!(pkt.len(), 10);
    }

    // ── parse_master_response ─────────────────────────────────────────────────

    /// Normal response with 2 entries.
    #[test]
    fn parse_master_response_two_entries() {
        let mut buf: Vec<u8> = b"\xFF\xFF\xFF\xFFservers ".to_vec();
        // Entry 1: 1.2.3.4:1998
        buf.extend_from_slice(&[1, 2, 3, 4]);
        buf.extend_from_slice(&1998u16.to_be_bytes());
        // Entry 2: 5.6.7.8:27960
        buf.extend_from_slice(&[5, 6, 7, 8]);
        buf.extend_from_slice(&27960u16.to_be_bytes());

        let addrs = parse_master_response(&buf).unwrap();
        assert_eq!(addrs.len(), 2);
        assert_eq!(addrs[0], SocketAddrV4::new(Ipv4Addr::new(1, 2, 3, 4), 1998));
        assert_eq!(addrs[1], SocketAddrV4::new(Ipv4Addr::new(5, 6, 7, 8), 27960));
    }

    /// Zero-port terminator stops parsing; junk bytes after it are ignored.
    #[test]
    fn parse_master_response_zero_port_terminator_with_junk() {
        let mut buf: Vec<u8> = b"\xFF\xFF\xFF\xFFservers ".to_vec();
        // Entry 1: valid
        buf.extend_from_slice(&[10, 0, 0, 1]);
        buf.extend_from_slice(&1998u16.to_be_bytes());
        // Zero-port terminator
        buf.extend_from_slice(&[0, 0, 0, 0]);
        buf.extend_from_slice(&0u16.to_be_bytes());
        // Junk after terminator
        buf.extend_from_slice(&[99, 99, 99, 99, 99, 99]);

        let addrs = parse_master_response(&buf).unwrap();
        assert_eq!(addrs.len(), 1);
        assert_eq!(addrs[0], SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 1), 1998));
    }

    /// A truncated trailing entry (fewer than 6 bytes) is silently ignored.
    #[test]
    fn parse_master_response_truncated_trailing_entry_ignored() {
        let mut buf: Vec<u8> = b"\xFF\xFF\xFF\xFFservers ".to_vec();
        // Full entry
        buf.extend_from_slice(&[10, 0, 0, 1]);
        buf.extend_from_slice(&1998u16.to_be_bytes());
        // Truncated entry: only 3 bytes (not a full 6)
        buf.extend_from_slice(&[10, 0, 0, 2, 0x07]);

        let addrs = parse_master_response(&buf).unwrap();
        assert_eq!(addrs.len(), 1);
    }

    /// Empty payload (just the header) returns an empty list.
    #[test]
    fn parse_master_response_empty_list() {
        let buf: Vec<u8> = b"\xFF\xFF\xFF\xFFservers ".to_vec();
        let addrs = parse_master_response(&buf).unwrap();
        assert!(addrs.is_empty());
    }

    /// Wrong header → Err(Network).
    #[test]
    fn parse_master_response_wrong_header_errors() {
        let buf: Vec<u8> = b"\xFF\xFF\xFF\xFFwrongheader_here_xx".to_vec();
        assert!(matches!(
            parse_master_response(&buf),
            Err(LauncherError::Network(_))
        ));
    }

    /// Buffer shorter than 12 bytes → Err(Network).
    #[test]
    fn parse_master_response_too_short_errors() {
        let buf: Vec<u8> = b"\xFF\xFF\xFF\xFF".to_vec();
        assert!(matches!(
            parse_master_response(&buf),
            Err(LauncherError::Network(_))
        ));
    }

    // ── parse_info_string ─────────────────────────────────────────────────────

    /// Leading backslash is stripped.
    #[test]
    fn parse_info_string_leading_backslash() {
        let m = parse_info_string("\\key1\\val1\\key2\\val2");
        assert_eq!(m.get("key1").map(String::as_str), Some("val1"));
        assert_eq!(m.get("key2").map(String::as_str), Some("val2"));
        assert_eq!(m.len(), 2);
    }

    /// No leading backslash still works.
    #[test]
    fn parse_info_string_no_leading_backslash() {
        let m = parse_info_string("key1\\val1\\key2\\val2");
        assert_eq!(m.get("key1").map(String::as_str), Some("val1"));
        assert_eq!(m.get("key2").map(String::as_str), Some("val2"));
    }

    /// Trailing backslash (odd token count after strip) is tolerated.
    #[test]
    fn parse_info_string_trailing_backslash() {
        // "key1\val1\dangling\" — after stripping leading \, tokens are:
        // key1, val1, dangling, (empty from trailing \) — even count, parses fine.
        // Actually: "\key1\val1\dangling\" after strip → "key1\val1\dangling\"
        // split by \ → ["key1", "val1", "dangling", ""] — pairs: (key1,val1), (dangling,"")
        let m = parse_info_string("\\key1\\val1\\dangling\\");
        assert_eq!(m.get("key1").map(String::as_str), Some("val1"));
        assert_eq!(m.get("dangling").map(String::as_str), Some(""));
    }

    /// Empty values are preserved.
    #[test]
    fn parse_info_string_empty_values() {
        let m = parse_info_string("\\sv_hostname\\\\sv_map\\dm1");
        assert_eq!(m.get("sv_hostname").map(String::as_str), Some(""));
        assert_eq!(m.get("sv_map").map(String::as_str), Some("dm1"));
    }

    /// Odd token count (unpaired trailing key) is tolerated — key is dropped.
    #[test]
    fn parse_info_string_odd_token_count() {
        // "\\key1\\val1\\orphan" → after strip: "key1\\val1\\orphan"
        // tokens: key1, val1, orphan — orphan is dropped (no value).
        let m = parse_info_string("\\key1\\val1\\orphan");
        assert_eq!(m.get("key1").map(String::as_str), Some("val1"));
        assert!(!m.contains_key("orphan"));
    }

    // ── parse_status_response ─────────────────────────────────────────────────

    fn make_status_buf(body: &str) -> Vec<u8> {
        let mut buf = b"\xFF\xFF\xFF\xFF".to_vec();
        buf.extend_from_slice(body.as_bytes());
        buf
    }

    /// Full response with 3 players: one bot, one with a name containing spaces.
    #[test]
    fn parse_status_response_full_with_players() {
        let body = concat!(
            "status\n",
            "\\sv_hostname\\Test Server\\sv_map\\dm1\\g_gameplay\\deathmatch",
            "\\sv_max_clients\\16\\sv_protocol\\2027\n",
            "\\score\\5\\ping\\42\\name\\Alice\n",
            "\\score\\3\\ping\\99\\name\\Big James\n",      // name with space
            "\\score\\0\\ping\\1\\name\\Bot1\\ai\\1\n",    // bot
        );
        let buf = make_status_buf(body);
        let info = parse_status_response(&buf).unwrap();

        assert_eq!(info.hostname, "Test Server");
        assert_eq!(info.map, "dm1");
        assert_eq!(info.gameplay, "deathmatch");
        assert_eq!(info.max_clients, 16);
        assert_eq!(info.protocol, 2027);
        assert_eq!(info.clients, 2);
        assert_eq!(info.bots, 1);
        assert_eq!(info.players.len(), 3);

        let alice = &info.players[0];
        assert_eq!(alice.name, "Alice");
        assert_eq!(alice.score, 5);
        assert_eq!(alice.ping, 42);
        assert!(!alice.bot);

        let james = &info.players[1];
        assert_eq!(james.name, "Big James");
        assert!(!james.bot);

        let bot = &info.players[2];
        assert!(bot.bot);
        assert_eq!(bot.name, "Bot1");
    }

    /// No players section — clients/bots both 0.
    #[test]
    fn parse_status_response_no_players() {
        let body =
            "status\n\\sv_hostname\\Empty\\sv_map\\dm2\\g_gameplay\\tdm\\sv_max_clients\\8\\sv_protocol\\2027\n";
        let buf = make_status_buf(body);
        let info = parse_status_response(&buf).unwrap();

        assert_eq!(info.clients, 0);
        assert_eq!(info.bots, 0);
        assert!(info.players.is_empty());
        assert_eq!(info.hostname, "Empty");
    }

    /// Missing `g_gameplay` key defaults to "".
    #[test]
    fn parse_status_response_missing_g_gameplay_defaults_empty() {
        let body = "status\n\\sv_hostname\\Test\\sv_map\\dm1\\sv_max_clients\\8\\sv_protocol\\2027\n";
        let buf = make_status_buf(body);
        let info = parse_status_response(&buf).unwrap();
        assert_eq!(info.gameplay, "");
    }

    /// `sv_protocol` absent → 0.
    #[test]
    fn parse_status_response_missing_protocol_defaults_zero() {
        let body = "status\n\\sv_hostname\\Test\\sv_map\\dm1\\g_gameplay\\dm\\sv_max_clients\\8\n";
        let buf = make_status_buf(body);
        let info = parse_status_response(&buf).unwrap();
        assert_eq!(info.protocol, 0);
    }

    /// Garbage buffer → Err(Network).
    #[test]
    fn parse_status_response_garbage_errors() {
        let buf = b"this is not a valid quetoo packet at all".to_vec();
        assert!(matches!(
            parse_status_response(&buf),
            Err(LauncherError::Network(_))
        ));
    }

    /// Missing `status\n` after OOB → Err(Network).
    #[test]
    fn parse_status_response_missing_status_header_errors() {
        let buf = make_status_buf("nope\n\\sv_hostname\\x\n");
        assert!(matches!(
            parse_status_response(&buf),
            Err(LauncherError::Network(_))
        ));
    }

    // ── dead_server_or_none ───────────────────────────────────────────────────

    #[test]
    fn dead_server_favorite_returns_stub() {
        let stub = dead_server_or_none("1.2.3.4:1998".into(), true).unwrap();
        assert_eq!(stub.hostname, "(no response)");
        assert_eq!(stub.ping, 999);
        assert!(stub.favorite);
        assert_eq!(stub.addr, "1.2.3.4:1998");
        assert_eq!(stub.clients, 0);
    }

    #[test]
    fn dead_server_non_favorite_returns_none() {
        assert!(dead_server_or_none("1.2.3.4:1998".into(), false).is_none());
    }

    // ── parse_status_response: empty player name (spec §5) ───────────────────

    /// A player line with an empty name ("\\score\\5\\ping\\20\\name\\") must parse
    /// successfully with name == "" and be counted as a human client.
    #[test]
    fn parse_status_response_player_empty_name_is_counted() {
        let body = concat!(
            "status\n",
            "\\sv_hostname\\Test\\sv_map\\dm1\\g_gameplay\\dm\\sv_max_clients\\8\\sv_protocol\\2027\n",
            "\\score\\5\\ping\\20\\name\\\n",
        );
        let buf = make_status_buf(body);
        let info = parse_status_response(&buf).unwrap();
        assert_eq!(info.players.len(), 1);
        assert_eq!(info.players[0].name, "");
        assert_eq!(info.players[0].score, 5);
        assert_eq!(info.players[0].ping, 20);
        assert!(!info.players[0].bot);
        assert_eq!(info.clients, 1);
        assert_eq!(info.bots, 0);
    }
}

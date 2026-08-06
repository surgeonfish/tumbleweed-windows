//! Minimal HTTP client (std only) used to push files to other tumbleweed
//! devices discovered on the LAN.
//!
//! The remote end is our own [`super::server`] module, so the request mirrors
//! what its `handle_put` expects: `PUT /<url-encoded-name>` with the raw file
//! bytes as the body and a `Content-Length` header.

use std::fs::File;
use std::io::{self, BufRead, Read, Write};
use std::net::{IpAddr, TcpStream};
use std::path::Path;
use std::time::Duration;

use super::server::url_encode;

/// Upper bound for the response head (status line + headers) we drain.
const MAX_RESPONSE_HEAD: usize = 32 * 1024;
/// How long to wait for the remote to confirm the upload. The remote shows a
/// confirmation dialog before replying, so this has to cover human reaction
/// time (the server itself waits 600s before rejecting).
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(620);

/// Upload `path` to `ip:port` over HTTP `PUT`.
///
/// Blocks until the remote accepts (or rejects) the transfer. On success the
/// remote saves the file and replies `201`; otherwise an error is returned.
/// Call it on a background thread so the UI thread never blocks. The file is
/// streamed from disk, so huge files use constant memory.
pub(crate) fn send_file(ip: IpAddr, port: u16, path: &Path) -> io::Result<()> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing file name"))?;
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let target = format!("/{}", url_encode(&name));

    let mut stream = TcpStream::connect((ip, port))?;
    stream.set_read_timeout(Some(RESPONSE_TIMEOUT))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;

    let head = format!(
        "PUT {target} HTTP/1.1\r\nHost: {ip}\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(head.as_bytes())?;
    io::copy(&mut file, &mut stream)?;
    stream.flush()?;

    // Read the status line, then drain the header block so the socket settles.
    let mut reader = io::BufReader::new(&mut stream);
    let mut status_line = String::new();
    reader.read_line(&mut status_line)?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);

    let mut consumed = status_line.len();
    loop {
        if consumed > MAX_RESPONSE_HEAD {
            break;
        }
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        consumed += line.len();
        if line.trim().is_empty() {
            break;
        }
    }

    if (200..300).contains(&status) {
        println!("[client] uploaded {name} -> {ip}:{port}");
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::Other,
            format!("server responded {status}"),
        ))
    }
}

/// Device metadata returned by a peer's `GET /info` endpoint.
#[derive(Clone, Debug, PartialEq)]
pub struct PeerInfo {
    pub name: String,
    pub kind: String,
    pub port: u16,
}

/// Query a peer's `GET /info` endpoint to learn its name, type and port.
/// Returns `None` when the peer doesn't expose the endpoint (an older version)
/// or is unreachable. Short timeouts keep this cheap to call per device.
pub(crate) fn fetch_info(ip: IpAddr, port: u16) -> Option<PeerInfo> {
    let Ok(mut stream) =
        TcpStream::connect_timeout(&std::net::SocketAddr::new(ip, port), Duration::from_secs(1))
    else {
        return None;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
    let req = format!("GET /info HTTP/1.1\r\nHost: {ip}\r\nConnection: close\r\n\r\n");
    if stream.write_all(req.as_bytes()).is_err() {
        return None;
    }
    let _ = stream.flush();

    let mut reader = io::BufReader::new(&mut stream);
    let mut status_line = String::new();
    if reader.read_line(&mut status_line).unwrap_or(0) == 0 {
        return None;
    }
    if !status_line.contains(" 200 ") {
        return None;
    }
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        if line.trim().is_empty() {
            break;
        }
        if let Some((key, value)) = line.split_once(':') {
            if key.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().unwrap_or(0);
            }
        }
    }
    if content_length == 0 || content_length > 4096 {
        return None;
    }
    let mut body = vec![0u8; content_length];
    let mut read = 0usize;
    while read < content_length {
        let n = reader.read(&mut body[read..]).unwrap_or(0);
        if n == 0 {
            break;
        }
        read += n;
    }
    parse_info(&String::from_utf8_lossy(&body[..read]))
}

/// Extract a JSON string field like `"name": "..."` (our own fixed format).
fn json_str_field(body: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let idx = body.find(&needle)?;
    let rest = &body[idx + needle.len()..];
    let rest = &rest[rest.find(':')? + 1..];
    let rest = rest.trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Extract a JSON numeric field like `"port": 8000`.
fn json_num_field(body: &str, key: &str) -> Option<u16> {
    let needle = format!("\"{key}\"");
    let idx = body.find(&needle)?;
    let rest = &body[idx + needle.len()..];
    let rest = rest[rest.find(':')? + 1..].trim_start();
    let end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
    rest[..end].parse().ok()
}

/// Parse a peer's `/info` JSON body.
fn parse_info(body: &str) -> Option<PeerInfo> {
    Some(PeerInfo {
        name: json_str_field(body, "name")?,
        kind: json_str_field(body, "type")?,
        port: json_num_field(body, "port").unwrap_or(0),
    })
}

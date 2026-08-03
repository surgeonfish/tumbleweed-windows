//! Minimal HTTP client (std only) used to push files to other tumbleweed
//! devices discovered on the LAN.
//!
//! The remote end is our own [`super::server`] module, so the request mirrors
//! what its `handle_put` expects: `PUT /<url-encoded-name>` with the raw file
//! bytes as the body and a `Content-Length` header.

use std::io::{self, BufRead, Write};
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
/// Call it on a background thread so the UI thread never blocks.
pub(crate) fn send_file(ip: IpAddr, port: u16, path: &Path) -> io::Result<()> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing file name"))?;
    let data = std::fs::read(path)?;
    let target = format!("/{}", url_encode(&name));

    let mut stream = TcpStream::connect((ip, port))?;
    stream.set_read_timeout(Some(RESPONSE_TIMEOUT))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;

    let head = format!(
        "PUT {target} HTTP/1.1\r\nHost: {ip}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        data.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(&data)?;
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

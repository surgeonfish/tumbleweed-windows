//! Minimal HTTP file server (std only).
//!
//! Serves the folder currently shown in the Explore page over HTTP and accepts
//! file uploads via `PUT`. Run [`serve`] on its own background thread.
//!
//! Endpoints:
//! - `GET  /path`  → file download (directory listing is an opt-in feature)
//! - `HEAD /path`  → headers only
//! - `PUT  /path`  → write an uploaded file (creates parent folders)

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use super::upload_gate;
use super::upload_gate::UploadDecision;

/// The HTTP port the file server listens on. Every tumbleweed device uses the
/// same port, so a client can reach a discovered device at `<ip>:HTTP_PORT`.
pub const HTTP_PORT: u16 = 8000;

/// The folder the server exposes, updated as the user browses in Explore.
static SERVER_ROOT: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Update the served folder (called when the Explore folder changes).
pub(crate) fn set_root(path: PathBuf) {
    if let Ok(mut root) = SERVER_ROOT.lock() {
        *root = Some(path);
    }
}

/// The folder currently served.
fn current_root() -> PathBuf {
    SERVER_ROOT
        .lock()
        .ok()
        .and_then(|r| r.clone())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from("C:\\")))
}

/// Whether `GET` on a directory returns an HTML listing. Off by default;
/// enable it later via [`set_list_directories`].
static LIST_DIRECTORIES: AtomicBool = AtomicBool::new(false);

/// Turn the directory-listing feature on/off. Currently disabled by default.
#[allow(dead_code)] // opt-in feature, not wired to the UI yet
pub(crate) fn set_list_directories(enabled: bool) {
    LIST_DIRECTORIES.store(enabled, Ordering::Relaxed);
}

fn list_directories_enabled() -> bool {
    LIST_DIRECTORIES.load(Ordering::Relaxed)
}

/// JSON body for the `GET /info` endpoint: this device's advertised name,
/// type ("pc"), HTTP server port and app version, so peers can tell what we
/// are over HTTP.
fn device_info_json() -> String {
    format!(
        "{{\"name\":\"{}\",\"type\":\"pc\",\"port\":{},\"version\":\"{}\"}}",
        super::mdns::device_hostname(),
        HTTP_PORT,
        env!("CARGO_PKG_VERSION")
    )
}

/// Run the HTTP server forever on `port`. Blocking — spawn it on a thread.
pub fn serve(port: u16) -> io::Result<()> {
    let listener = TcpListener::bind(("0.0.0.0", port))?;
    println!("[server] HTTP file server listening on http://0.0.0.0:{port}");
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                std::thread::spawn(move || {
                    let _ = handle_connection(stream);
                });
            }
            Err(_) => continue,
        }
    }
    Ok(())
}

/// Upper bound for the request head (request line + headers).
const MAX_HEAD: usize = 32 * 1024;

fn handle_connection(mut stream: TcpStream) -> io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);

    let mut request_line = String::new();
    if reader.read_line(&mut request_line)? == 0 {
        return Ok(());
    }
    let request_line = request_line.trim_end().to_string();

    // Read headers until the blank line; track Content-Length for PUT.
    let mut head_bytes = 0usize;
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        head_bytes += line.len();
        if head_bytes > MAX_HEAD {
            return send_error(&mut stream, 413, "Request Too Large");
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break;
        }
        if let Some((key, value)) = trimmed.split_once(':') {
            if key.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().unwrap_or(0);
            }
        }
    }

    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_uppercase();
    let target = parts.next().unwrap_or("/");

    let root = current_root();
    match method.as_str() {
        // Device-info handshake: peers fetch this to learn our type ("pc")
        // and server port over HTTP, instead of guessing via a TCP probe.
        "GET" | "HEAD" if target.split('?').next() == Some("/info") => {
            let head_only = method == "HEAD";
            send_bytes(
                &mut stream,
                200,
                "application/json",
                device_info_json().as_bytes(),
                head_only,
            )?
        }
        "GET" => handle_get(&mut stream, &root, target, false)?,
        "HEAD" => handle_get(&mut stream, &root, target, true)?,
        "PUT" => handle_put(&mut stream, target, &mut reader, content_length)?,
        _ => send_error(&mut stream, 405, "Method Not Allowed")?,
    }
    Ok(())
}

fn handle_get(stream: &mut TcpStream, root: &Path, target: &str, head_only: bool) -> io::Result<()> {
    let Some(fs_path) = safe_path(root, target) else {
        return send_error(stream, 400, "Bad Request");
    };
    if fs_path.is_dir() {
        if list_directories_enabled() {
            let html = directory_listing(&fs_path, target);
            send_bytes(stream, 200, "text/html; charset=utf-8", html.as_bytes(), head_only)
        } else {
            send_error(stream, 403, "Forbidden")
        }
    } else if fs_path.is_file() {
        let ctype = content_type(&fs_path);
        send_file(stream, 200, ctype, &fs_path, head_only)
    } else {
        send_error(stream, 404, "Not Found")
    }
}

fn handle_put(
    stream: &mut TcpStream,
    target: &str,
    reader: &mut BufReader<TcpStream>,
    content_length: usize,
) -> io::Result<()> {
    // Stream the body into a temp file instead of buffering the whole thing in
    // memory, so huge files use constant memory. Moved to the destination once
    // the user confirms.
    let tmp = unique_temp_path();
    {
        let mut tmp_file = File::create(&tmp)?;
        let mut remaining = content_length as u64;
        let mut buf = [0u8; 64 * 1024];
        while remaining > 0 {
            let to_read = remaining.min(buf.len() as u64) as usize;
            let n = reader.read(&mut buf[..to_read])?;
            if n == 0 {
                break;
            }
            tmp_file.write_all(&buf[..n])?;
            remaining -= n as u64;
        }
    } // tmp_file closed before rename

    // The file name is the last URL path segment, percent-decoded.
    let raw_name = target.rsplit('/').next().unwrap_or(target).to_string();
    let Some(name) = url_decode(&raw_name) else {
        let _ = std::fs::remove_file(&tmp);
        return send_error(stream, 400, "Bad Request");
    };
    if name.is_empty() || name == "." || name == ".." {
        let _ = std::fs::remove_file(&tmp);
        return send_error(stream, 400, "Bad Request");
    }

    // Ask the UI thread to confirm the upload and pick a destination folder.
    let Some((id, rx)) = upload_gate::submit_upload(name.clone(), content_length as u64) else {
        let _ = std::fs::remove_file(&tmp);
        return send_error(stream, 503, "Not ready");
    };

    let decision = rx.recv_timeout(Duration::from_secs(600)).ok();
    upload_gate::remove_upload(id);

    match decision {
        Some(UploadDecision::Save(dir)) => {
            let dest = dir.join(&name);
            if let Some(parent) = dest.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            // Move the temp file into place; fall back to copy+delete when the
            // temp dir and destination are on different volumes.
            let moved = match std::fs::rename(&tmp, &dest) {
                Ok(()) => true,
                Err(_) => {
                    let copied = std::fs::copy(&tmp, &dest).is_ok();
                    let _ = std::fs::remove_file(&tmp);
                    copied
                }
            };
            if moved {
                println!("[server] saved upload {name} -> {}", dest.display());
                send_bytes(stream, 201, "text/plain", b"Saved\n", false)
            } else {
                println!("[server] save error for {name}");
                send_error(stream, 500, "Save failed")
            }
        }
        _ => {
            let _ = std::fs::remove_file(&tmp);
            send_error(stream, 403, "Rejected")
        }
    }
}

/// A per-request unique temp path (concurrent PUTs must not collide).
fn unique_temp_path() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "tumbleweed-upload-{}-{}.tmp",
        std::process::id(),
        n
    ))
}

/// Map a URL path to a filesystem path under `root`, rejecting traversal.
fn safe_path(root: &Path, target: &str) -> Option<PathBuf> {
    let target = target.split('?').next().unwrap_or(target);
    let decoded = url_decode(target)?;
    let mut path = root.to_path_buf();
    for seg in decoded.split('/') {
        if seg.is_empty() || seg == "." {
            continue;
        }
        if seg == ".." {
            return None; // forbid path traversal
        }
        path.push(seg);
    }
    Some(path)
}

fn directory_listing(dir: &Path, url_path: &str) -> String {
    let title = dir.display().to_string();
    let mut html = String::from("<!DOCTYPE html><html><head><meta charset='utf-8'><title>");
    html.push_str(&html_escape(&title));
    html.push_str("</title></head><body><h1>");
    html.push_str(&html_escape(&title));
    html.push_str("</h1><ul>");

    // Parent link (unless we're at the root).
    let parent = url_path.trim_end_matches('/');
    if !parent.is_empty() {
        let up = parent.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
        let up_url = if up.is_empty() { "/" } else { up };
        html.push_str(&format!("<li><a href=\"{}\">..</a></li>", url_encode(up_url)));
    }

    if let Ok(rd) = std::fs::read_dir(dir) {
        let mut names: Vec<String> = rd
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        names.sort();
        for name in names {
            let is_dir = dir.join(&name).is_dir();
            let display = if is_dir {
                format!("{name}/")
            } else {
                name.clone()
            };
            let href = format!("{}/{}", parent, url_encode(&name));
            html.push_str(&format!(
                "<li><a href=\"{}\">{}</a></li>",
                href,
                html_escape(&display)
            ));
        }
    }

    html.push_str("</ul><p><i>tumbleweed HTTP file server</i></p></body></html>");
    html
}

fn content_type(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("txt" | "md" | "log" | "csv" | "json") => "text/plain; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("svg") => "image/svg+xml",
        Some("pdf") => "application/pdf",
        Some("zip") => "application/zip",
        _ => "application/octet-stream",
    }
}

pub(crate) fn url_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'/') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn url_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                if i + 2 >= bytes.len() {
                    return None;
                }
                let hi = hex_val(bytes[i + 1])?;
                let lo = hex_val(bytes[i + 2])?;
                out.push((hi << 4) | lo);
                i += 3;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Request Too Large",
        500 => "Internal Server Error",
        _ => "OK",
    }
}

fn send_bytes(
    stream: &mut TcpStream,
    status: u16,
    ctype: &str,
    body: &[u8],
    head_only: bool,
) -> io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status} {}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        reason(status),
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    if !head_only {
        stream.write_all(body)?;
    }
    stream.flush()
}

/// Streams a file to the client (constant memory even for huge files).
fn send_file(
    stream: &mut TcpStream,
    status: u16,
    ctype: &str,
    path: &Path,
    head_only: bool,
) -> io::Result<()> {
    let len = std::fs::metadata(path)?.len();
    let head = format!(
        "HTTP/1.1 {status} {}\r\nContent-Type: {ctype}\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n",
        reason(status)
    );
    stream.write_all(head.as_bytes())?;
    if !head_only {
        let mut file = File::open(path)?;
        io::copy(&mut file, stream)?;
    }
    stream.flush()
}

fn send_error(stream: &mut TcpStream, status: u16, message: &str) -> io::Result<()> {
    let body = format!("<!DOCTYPE html><html><body><h1>{status} {message}</h1></body></html>");
    send_bytes(stream, status, "text/html; charset=utf-8", body.as_bytes(), false)
}

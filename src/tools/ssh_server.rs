//! Embedded SSH server (russh) for Tumbleweed.
//!
//! Runs on port 2222 and serves two purposes:
//! - Receive SFTP uploads from paired phones (public-key auth). Files land in
//!   the folder the Explorer page is currently showing.
//! - Pairing bootstrap: while the pairing QR's one-time token is current, a
//!   phone can connect with `username == token`, authenticate with its own key,
//!   and run `tumbleweed add-key` to register that key so future SFTP transfers
//!   are authorized without the token.
//!
//! The host key is the app's Ed25519 identity (`%LOCALAPPDATA%\Tumbleweed\
//! tumbleweed_ed25519`), and authorized phone keys live in `authorized_keys`
//! in the same folder. Both are shared with `ssh_pair`.

use std::collections::HashMap;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use russh_sftp::protocol::{
    Attrs, File, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode, Version,
};

use async_trait::async_trait;
use russh::server::{Auth, Config, Msg, Server as ServerTrait, Session};
use russh::{Channel, ChannelId, CryptoVec};
use russh_keys::key::PublicKey;
use russh_keys::PublicKeyBase64;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::Mutex;

/// The SSH port every Tumbleweed device listens on (mirrors `server::HTTP_PORT`).
pub const SSH_PORT: u16 = 2222;

/// Bridges `log` records (russh's internal diagnostics) into the app's log file.
pub struct FileLogger;

impl log::Log for FileLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.target().starts_with("russh")
    }
    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            crate::tools::mdns::log_msg(&format!(
                "[log/{}] {}",
                record.level(),
                record.args()
            ));
        }
    }
    fn flush(&self) {}
}

// ---- shared folder (the Explorer page's current directory) ----

static SHARE_ROOT: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

/// Update the folder SCP uploads land in (called when the Explorer folder changes).
pub(crate) fn set_share_root(path: PathBuf) {
    if let Ok(mut root) = SHARE_ROOT.lock() {
        *root = Some(path);
    }
}

fn share_root() -> PathBuf {
    SHARE_ROOT
        .lock()
        .ok()
        .and_then(|r| r.clone())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from("C:\\")))
}

// ---- authorized phone keys ----

/// Path of the `authorized_keys` file inside the app's folder.
fn authorized_keys_path() -> PathBuf {
    crate::tools::ssh_pair::app_folder().join("authorized_keys")
}

/// The current pairing token (the QR's one-time bootstrap secret), if any.
pub(crate) fn pairing_token() -> Option<String> {
    crate::tools::ssh_pair::pairing_token()
}

/// Load the currently authorized phone public keys from `authorized_keys`.
fn authorized_keys() -> Vec<PublicKey> {
    let path = authorized_keys_path();
    let Ok(text) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            // Keep the `ssh-ed25519 AAAA...` prefix, drop any trailing comment.
            let head: Vec<&str> = line.split_whitespace().take(2).collect();
            russh_keys::parse_public_key_base64(&head.join(" ")).ok()
        })
        .collect()
}

/// Append `key` (an OpenSSH public-key line) to `authorized_keys`.
pub(crate) fn add_authorized_key(line: &str) {
    let line = line.trim();
    if line.is_empty() {
        return;
    }
    let path = authorized_keys_path();
    if let Err(e) = fs::create_dir_all(path.parent().unwrap_or(Path::new("."))) {
        crate::tools::mdns::log_msg(&format!("[ssh] mkdir authorized_keys dir: {e}"));
        return;
    }
    // Avoid duplicate lines.
    let existing = fs::read_to_string(&path).unwrap_or_default();
    if existing.lines().any(|l| l.trim() == line) {
        return;
    }
    use std::io::Write;
    if let Err(e) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| {
            writeln!(f, "{line}")?;
            f.flush()
        })
    {
        crate::tools::mdns::log_msg(&format!("[ssh] append authorized key: {e}"));
    }
}

// ---- the server ----

/// Start the SSH server on a background thread. Non-blocking.
pub(crate) fn start() {
    std::thread::spawn(|| {
        let runtime = match tokio::runtime::Runtime::new() {
            Ok(r) => r,
            Err(e) => {
                crate::tools::mdns::log_msg(&format!("[ssh] tokio runtime: {e}"));
                return;
            }
        };
        if let Err(e) = runtime.block_on(run_server()) {
            crate::tools::mdns::log_msg(&format!("[ssh] server error: {e}"));
        }
    });
}

async fn run_server() -> anyhow::Result<()> {
    // Ensure the one-time pairing token exists up-front (the QR also embeds it).
    let _ = crate::tools::ssh_pair::pairing_token();

    // The app's Ed25519 identity doubles as the SSH host key.
    let key_text = fs::read_to_string(crate::tools::ssh_pair::private_key_path())
        .map_err(|e| anyhow::anyhow!("read host key: {e}"))?;
    let host_key = russh_keys::decode_secret_key(&key_text, None)
        .map_err(|e| anyhow::anyhow!("decode host key: {e}"))?;

    let config = Arc::new(Config {
        inactivity_timeout: Some(Duration::from_secs(3600)),
        auth_rejection_time: Duration::from_secs(3),
        auth_rejection_time_initial: Some(Duration::from_secs(0)),
        keys: vec![host_key],
        ..Default::default()
    });

    let mut server = Server {
        clients: Arc::new(Mutex::new(HashMap::new())),
    };
    crate::tools::mdns::log_msg(&format!("[ssh] listening on 0.0.0.0:{SSH_PORT}"));
    server.run_on_address(config, ("0.0.0.0", SSH_PORT)).await?;
    Ok(())
}

#[derive(Clone)]
struct Server {
    clients: Arc<Mutex<HashMap<ChannelId, Channel<Msg>>>>,
}

impl ServerTrait for Server {
    type Handler = SshHandler;

    fn new_client(&mut self, _: Option<SocketAddr>) -> Self::Handler {
        SshHandler {
            clients: Arc::clone(&self.clients),
            pairing_key: None,
        }
    }

    fn handle_session_error(&mut self, error: <Self::Handler as russh::server::Handler>::Error) {
        crate::tools::mdns::log_msg(&format!("[ssh] session error: {error:#}"));
    }
}

struct SshHandler {
    clients: Arc<Mutex<HashMap<ChannelId, Channel<Msg>>>>,
    /// Set when a client authenticates via the one-time pairing token; the key
    /// offered is the phone's public key, registered by `tumbleweed add-key`.
    pairing_key: Option<PublicKey>,
}

impl SshHandler {
    fn is_authorized(&self, user: &str, key: &PublicKey) -> bool {
        // Pairing bootstrap: while the token is current, a client using it as
        // the username may register any key it offers.
        if pairing_token().as_deref() == Some(user) {
            return true;
        }
        authorized_keys().iter().any(|k| k == key)
    }
}

#[async_trait]
impl russh::server::Handler for SshHandler {
    type Error = anyhow::Error;

    async fn auth_publickey_offered(
        &mut self,
        user: &str,
        key: &PublicKey,
    ) -> Result<Auth, Self::Error> {
        let ok = self.is_authorized(user, key);
        crate::tools::mdns::log_msg(&format!(
            "[ssh] offered user={user:?} key={} authorized={ok}",
            key.public_key_base64()
        ));
        if ok {
            Ok(Auth::Accept)
        } else {
            Ok(Auth::Reject {
                proceed_with_methods: None,
            })
        }
    }

    async fn auth_publickey(
        &mut self,
        user: &str,
        key: &PublicKey,
    ) -> Result<Auth, Self::Error> {
        let ok = self.is_authorized(user, key);
        crate::tools::mdns::log_msg(&format!(
            "[ssh] auth user={user:?} key={} authorized={ok}",
            key.public_key_base64()
        ));
        if !ok {
            return Ok(Auth::Reject {
                proceed_with_methods: None,
            });
        }
        if pairing_token().as_deref() == Some(user) {
            self.pairing_key = Some(key.clone());
        }
        Ok(Auth::Accept)
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        _session: &mut Session,
    ) -> Result<bool, Self::Error> {
        self.clients.lock().await.insert(channel.id(), channel);
        Ok(true)
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let command = String::from_utf8_lossy(data).to_string();
        crate::tools::mdns::log_msg(&format!("[ssh] exec: {command}"));

        let trimmed = command.trim();
        if trimmed == "tumbleweed add-key" {
            // Acknowledge the exec request, then register the key offered during
            // token-based pairing.
            let _ = session.channel_success(channel);
            match self.pairing_key.take() {
                Some(k) => {
                    add_authorized_key(&k.public_key_base64());
                    let _ = session.data(channel, CryptoVec::from(b"registered\n".to_vec()));
                }
                None => {
                    let _ = session.data(
                        channel,
                        CryptoVec::from(b"no pairing session\n".to_vec()),
                    );
                }
            }
            let _ = session.eof(channel);
            // Exec clients (JSch, OpenSSH) wait for the exit status + channel
            // close before considering the command finished.
            let _ = session.exit_status_request(channel, 0);
            let _ = session.close(channel);
            return Ok(());
        }

        if trimmed.starts_with("scp ") {
            // Acknowledge the exec request; the client then speaks the scp
            // protocol, starting by waiting for our initial '\0'. Run the
            // receiver on a separate task so the request reply + initial ack
            // flush immediately.
            let _ = session.channel_success(channel);
            let channel_obj = self.clients.lock().await.remove(&channel);
            if let Some(ch) = channel_obj {
                let root = share_root();
                tokio::spawn(async move {
                    let mut stream = ch.into_stream();
                    if let Err(e) = scp_receive(&mut stream, &root).await {
                        crate::tools::mdns::log_msg(&format!("[ssh] scp receive error: {e}"));
                    }
                });
            }
            return Ok(());
        }

        // Unknown command: reject.
        let _ = session.channel_failure(channel);
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        channel_id: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if name == "sftp" {
            let channel = self.clients.lock().await.remove(&channel_id);
            if let Some(ch) = channel {
                let _ = session.channel_success(channel_id);
                let sftp = SftpSession::new(share_root());
                crate::tools::mdns::log_msg("[ssh] sftp subsystem started");
                russh_sftp::server::run(ch.into_stream(), sftp).await;
                crate::tools::mdns::log_msg("[ssh] sftp subsystem ended");
            } else {
                let _ = session.channel_failure(channel_id);
            }
        } else {
            let _ = session.channel_failure(channel_id);
        }
        Ok(())
    }
}

// ---- SFTP receive (the `sftp` subsystem) ----

/// Resolve an SFTP path inside `root`, rejecting anything that escapes it.
fn resolve_path(root: &Path, path: &str) -> Option<PathBuf> {
    let mut out = root.to_path_buf();
    for comp in path.split(['/', '\\']) {
        match comp {
            "" | "." => continue,
            ".." => return None,
            c if c.contains(':') => return None,
            c => out = out.join(c),
        }
    }
    Some(out)
}

/// Handles one SFTP client (a paired phone), writing uploads into `root`.
struct SftpSession {
    root: PathBuf,
    files: HashMap<String, tokio::fs::File>,
}

impl SftpSession {
    fn new(root: PathBuf) -> Self {
        Self {
            root,
            files: HashMap::new(),
        }
    }
}

// russh-sftp 2.x uses native async fn in traits (no async_trait attribute).
impl russh_sftp::server::Handler for SftpSession {
    type Error = StatusCode;

    fn unimplemented(&self) -> StatusCode {
        StatusCode::OpUnsupported
    }

    async fn init(
        &mut self,
        _version: u32,
        _extensions: HashMap<String, String>,
    ) -> Result<Version, StatusCode> {
        Ok(Version::new())
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, StatusCode> {
        // Return a normalized path under root so later open()/mkdir() calls can
        // re-resolve it consistently (JSch uses realpath output for the handle).
        let rel = path.trim_start_matches(['/', '\\']);
        Ok(Name {
            id,
            files: vec![File {
                filename: format!("/{rel}"),
                longname: String::new(),
                attrs: FileAttributes::default(),
            }],
        })
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        _pflags: OpenFlags,
        _attrs: FileAttributes,
    ) -> Result<Handle, StatusCode> {
        let Some(path) = resolve_path(&self.root, &filename) else {
            return Err(StatusCode::Failure);
        };
        if let Some(parent) = path.parent() {
            if fs::create_dir_all(parent).is_err() {
                return Err(StatusCode::Failure);
            }
        }
        let file = tokio::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .await
            .map_err(|_| StatusCode::Failure)?;
        let handle = filename.clone();
        self.files.insert(handle.clone(), file);
        Ok(Handle { id, handle })
    }

    async fn write(
        &mut self,
        _id: u32,
        handle: String,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<Status, StatusCode> {
        use tokio::io::{AsyncSeekExt, AsyncWriteExt};
        let Some(file) = self.files.get_mut(&handle) else {
            return Err(StatusCode::Failure);
        };
        file.seek(std::io::SeekFrom::Start(offset))
            .await
            .map_err(|_| StatusCode::Failure)?;
        file.write_all(&data)
            .await
            .map_err(|_| StatusCode::Failure)?;
        Ok(Status {
            id: 0,
            status_code: StatusCode::Ok,
            error_message: "Ok".to_string(),
            language_tag: "en-US".to_string(),
        })
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, StatusCode> {
        self.files.remove(&handle);
        Ok(Status {
            id,
            status_code: StatusCode::Ok,
            error_message: "Ok".to_string(),
            language_tag: "en-US".to_string(),
        })
    }

    async fn mkdir(
        &mut self,
        id: u32,
        path: String,
        _attrs: FileAttributes,
    ) -> Result<Status, StatusCode> {
        let Some(path) = resolve_path(&self.root, &path) else {
            return Err(StatusCode::Failure);
        };
        tokio::fs::create_dir_all(&path)
            .await
            .map_err(|_| StatusCode::Failure)?;
        Ok(Status {
            id,
            status_code: StatusCode::Ok,
            error_message: "Ok".to_string(),
            language_tag: "en-US".to_string(),
        })
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, StatusCode> {
        let Some(path) = resolve_path(&self.root, &path) else {
            return Err(StatusCode::NoSuchFile);
        };
        let Ok(meta) = fs::metadata(&path) else {
            return Err(StatusCode::NoSuchFile);
        };
        Ok(Attrs {
            id,
            attrs: FileAttributes {
                size: Some(meta.len()),
                permissions: Some(0o100644),
                ..Default::default()
            },
        })
    }
}

// ---- SCP receive (the `scp -t` protocol) ----

/// Reject path traversal: only allow a single plain file/dir name.
fn sanitize_component(name: &str) -> Option<String> {
    let name = name.trim();
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name == ".."
        || name == "."
    {
        return None;
    }
    Some(name.to_string())
}

/// Receives one SCP upload (`scp -t <dir>`) over `stream`, writing files into
/// `root`. Implements the OpenSSH sink protocol: ack each header/data block.
async fn scp_receive<S>(stream: &mut S, root: &Path) -> anyhow::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    crate::tools::mdns::log_msg("[ssh] scp_receive started");
    // Initial readiness ack; the sender waits for it before sending headers.
    stream.write_all(&[0]).await?;
    stream.flush().await?;
    crate::tools::mdns::log_msg("[ssh] scp initial ack sent");

    // Current output directory, tracked through 'D'/'E' commands.
    let mut current: PathBuf = root.to_path_buf();
    let mut byte = [0u8; 1];
    let mut line: Vec<u8> = Vec::new();

    loop {
        line.clear();
        // Read one control line (terminated by '\n').
        loop {
            let n = stream.read(&mut byte).await?;
            if n == 0 {
                return Ok(()); // sender closed cleanly
            }
            if byte[0] == b'\n' {
                break;
            }
            if line.len() > 8192 {
                return Err(anyhow::anyhow!("scp: control line too long"));
            }
            line.push(byte[0]);
        }
        if line.is_empty() {
            continue;
        }
        let c = line[0];
        let rest = String::from_utf8_lossy(&line[1..]).to_string();
        match c {
            b'C' => {
                // "C<mode> <size> <name>"
                let mut it = rest.splitn(3, ' ');
                let _mode = it.next().unwrap_or("");
                let size: u64 = it.next().unwrap_or("0").trim().parse().unwrap_or(0);
                let Some(name) = sanitize_component(it.next().unwrap_or("")) else {
                    return Err(anyhow::anyhow!("scp: bad file name: {rest}"));
                };
                // Ack the header, then receive the payload.
                stream.write_all(&[0]).await?;
                stream.flush().await?;
                let dest = current.join(&name);
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent)?;
                }
                let mut file = tokio::fs::File::create(&dest).await?;
                let mut remaining = size;
                let mut buf = vec![0u8; 64 * 1024];
                while remaining > 0 {
                    let want = remaining.min(buf.len() as u64) as usize;
                    let n = stream.read(&mut buf[..want]).await?;
                    if n == 0 {
                        return Err(anyhow::anyhow!("scp: EOF mid-file"));
                    }
                    file.write_all(&buf[..n]).await?;
                    remaining -= n as u64;
                }
                file.flush().await?;
                // Trailing '\0' terminator from the sender.
                stream.read_exact(&mut byte).await?;
                // Final ack.
                stream.write_all(&[0]).await?;
                stream.flush().await?;
            }
            b'D' => {
                // "D<mode> <size> <name>" — enter a directory.
                let name = rest.splitn(3, ' ').nth(2).unwrap_or("");
                let Some(name) = sanitize_component(name) else {
                    return Err(anyhow::anyhow!("scp: bad dir name: {rest}"));
                };
                current = current.join(&name);
                fs::create_dir_all(&current)?;
                stream.write_all(&[0]).await?;
                stream.flush().await?;
            }
            b'E' => {
                // End of directory — step back out.
                if current.parent().is_some() {
                    current = current
                        .parent()
                        .map(Path::to_path_buf)
                        .unwrap_or_else(|| root.to_path_buf());
                }
                stream.write_all(&[0]).await?;
                stream.flush().await?;
            }
            b'T' => {
                // Timestamp header — ignore but ack.
                stream.write_all(&[0]).await?;
                stream.flush().await?;
            }
            1 | 2 => {
                return Err(anyhow::anyhow!("scp: sender error: {rest}"));
            }
            _ => {
                return Err(anyhow::anyhow!("scp: unexpected control byte 0x{c:02x}"));
            }
        }
    }
}

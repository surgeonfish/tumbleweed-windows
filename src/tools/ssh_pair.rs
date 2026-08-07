//! SSH pairing for the Settings page: generate an Ed25519 key pair (via the
//! OpenSSH `ssh-keygen` that ships with Windows 10+), expose the public key,
//! and render a QR code that encodes the public key + LAN IP + app version so
//! the Android app can scan it and pair with this PC.

use std::io;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;

/// The app's data folder, where the SSH identity is stored.
pub(crate) fn app_folder() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("APPDATA"))
        .map(PathBuf::from)
        .unwrap_or_default()
        .join("Tumbleweed")
}

/// Where the app keeps its SSH identity (inside the app's folder).
fn ssh_dir() -> PathBuf {
    app_folder()
}

pub(crate) fn private_key_path() -> PathBuf {
    ssh_dir().join("tumbleweed_ed25519")
}

fn public_key_path() -> PathBuf {
    ssh_dir().join("tumbleweed_ed25519.pub")
}

/// The one-time pairing token: a random secret the QR encodes so a phone can
/// register its own public key on this PC over SSH (bootstrap). Kept in the
/// app's folder; refreshed whenever the key pair is regenerated.
pub(crate) fn pairing_token() -> Option<String> {
    let dir = ssh_dir();
    let path = dir.join("pairing_token");
    if let Ok(t) = std::fs::read_to_string(&path) {
        let t = t.trim();
        if !t.is_empty() {
            return Some(t.to_string());
        }
    }
    // Fresh 32-byte token. Not security-critical (one-time LAN bootstrap), but
    // mix a monotonic counter, process id, and the clock so it is not guessable
    // from a single timestamp.
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut state = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
        ^ (std::process::id() as u64) << 32
        ^ COUNTER.fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed);
    let mut bytes = [0u8; 32];
    for b in bytes.iter_mut() {
        // xorshift64
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *b = (state & 0xff) as u8;
    }
    let token = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
    if std::fs::create_dir_all(&dir).is_ok() {
        let _ = std::fs::write(&path, &token);
    }
    Some(token)
}

/// Locate `ssh-keygen` (OpenSSH ships one with Windows 10+).
fn ssh_keygen_exe() -> Option<PathBuf> {
    let candidates = [
        PathBuf::from(r"C:\Windows\System32\OpenSSH\ssh-keygen.exe"),
        PathBuf::from(r"C:\Program Files\OpenSSH\ssh-keygen.exe"),
    ];
    candidates.into_iter().find(|p| p.exists())
}

fn run_ssh_keygen(args: &[&str]) -> io::Result<()> {
    let mut cmd = match ssh_keygen_exe() {
        Some(exe) => std::process::Command::new(exe),
        None => std::process::Command::new("ssh-keygen"),
    };
    let status = cmd
        .args(args)
        // CREATE_NO_WINDOW: ssh-keygen is a console-subsystem executable; without
        // this flag a console window flashes up briefly every time the key pair
        // is generated or regenerated.
        .creation_flags(0x0800_0000)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::Other,
            "ssh-keygen exited unsuccessfully",
        ))
    }
}

/// Generate the Ed25519 key pair (stored in the app's folder) if it doesn't
/// already exist. Returns the path to the private key.
pub(crate) fn generate_keypair() -> io::Result<PathBuf> {
    let dir = ssh_dir();
    std::fs::create_dir_all(&dir)?;
    let key = private_key_path();
    if !key.exists() {
        let path = key.to_str().unwrap_or("tumbleweed_ed25519");
        run_ssh_keygen(&[
            "-t",
            "ed25519",
            "-f",
            path,
            "-N",
            "",
            "-C",
            "tumbleweed@local",
        ])?;
    }
    Ok(key)
}

/// The OpenSSH public key line, e.g. `ssh-ed25519 AAAA... tumbleweed@local`.
pub(crate) fn public_key() -> Option<String> {
    std::fs::read_to_string(public_key_path())
        .ok()
        .map(|s| s.trim().to_string())
}

/// Whether a key pair already exists in the app's folder. The Settings page uses
/// this to load an existing identity on startup instead of forcing a regenerate.
pub(crate) fn has_keypair() -> bool {
    private_key_path().exists() && public_key_path().exists()
}

/// Force a brand-new key pair, replacing any existing one in the app's folder.
/// The Settings page's "Generate key pair" button calls this so clicking it
/// always produces a fresh identity (and therefore a fresh QR).
pub(crate) fn regenerate_keypair() -> io::Result<PathBuf> {
    let _ = std::fs::remove_file(private_key_path());
    let _ = std::fs::remove_file(public_key_path());
    // A regenerated identity needs a fresh pairing token too.
    let _ = std::fs::remove_file(ssh_dir().join("pairing_token"));
    generate_keypair()
}

/// Result of building the pairing QR, safe to show in the UI.
#[derive(Clone, PartialEq)]
pub(crate) struct PairingInfo {
    pub error: Option<String>,
    pub public_key: Option<String>,
    pub name: Option<String>,
    pub device_type: Option<String>,
    pub ip: Option<String>,
    pub version: Option<String>,
    /// QR module matrix (row-major, `size` x `size`), held in memory so the QR
    /// always reflects the current key pair without any file on disk.
    pub matrix: Option<Vec<bool>>,
    pub size: usize,
}

/// Ensure a key pair exists and build the QR matrix. Never fails hard — errors
/// are reported through [`PairingInfo::error`] for the UI to show.
pub(crate) fn build_pairing_info() -> PairingInfo {
    let err = |m: &str| PairingInfo {
        error: Some(m.to_string()),
        public_key: None,
        name: None,
        device_type: None,
        ip: None,
        version: None,
        matrix: None,
        size: 0,
    };
    if let Err(e) = generate_keypair() {
        return err(&format!("Could not generate SSH key pair: {e}"));
    }
    let Some(pk) = public_key() else {
        return err("Generated the key pair but could not read the public key.");
    };
    let Some(ip) = crate::tools::mdns::lan_ipv4() else {
        return err("Could not determine this device's LAN IP address.");
    };
    let host = crate::tools::mdns::device_hostname();
    let name = host
        .strip_prefix("tumbleweed-")
        .unwrap_or(&host)
        .trim_end_matches(".local")
        .to_string();
    let device_type = "pc";
    let version = env!("CARGO_PKG_VERSION").to_string();
    let token = pairing_token().unwrap_or_default();
    let payload = format!(
        "{{\"pk\":\"{pk}\",\"ip\":\"{ip}\",\"v\":\"{version}\",\"name\":\"{name}\",\"type\":\"{device_type}\",\"token\":\"{token}\"}}"
    );
    let (matrix, size) = match qr_matrix(&payload) {
        Ok(v) => v,
        Err(e) => return err(&format!("Could not render QR code: {e}")),
    };
    PairingInfo {
        error: None,
        public_key: Some(pk),
        name: Some(name),
        device_type: Some(device_type.to_string()),
        ip: Some(ip.to_string()),
        version: Some(version),
        matrix: Some(matrix),
        size,
    }
}

/// Encode `payload` into a QR module matrix (row-major, `size` x `size`).
fn qr_matrix(payload: &str) -> io::Result<(Vec<bool>, usize)> {
    let code = qrcode::QrCode::new(payload.as_bytes())
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
    let size = code.width() as usize;
    let matrix: Vec<bool> = code
        .to_colors()
        .iter()
        .map(|c| *c == qrcode::Color::Dark)
        .collect();
    Ok((matrix, size))
}

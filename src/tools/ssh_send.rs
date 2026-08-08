//! russh SFTP client used to push files to a paired phone.
//!
//! Replaces the old HTTP `PUT` client. A transfer works like this:
//!
//! 1. Connect to the phone's SSH port (2222) with TCP_NODELAY.
//! 2. Verify the phone's *host* key against the PC's `authorized_keys`. A
//!    paired phone's host key is the same RSA key it registered during
//!    pairing, so this confirms we're talking to the phone we paired with
//!    (no man-in-the-middle).
//! 3. Authenticate with this PC's Ed25519 identity (already in the phone's
//!    `authorized_keys`, written when the phone scanned our pairing QR).
//! 4. Open the file over SFTP (`CREATE | TRUNCATE | WRITE`) — the phone shows
//!    its confirmation dialog before the write is allowed — then stream the
//!    bytes, reporting progress to the Transfer page.
//!
//! Call [`send_file`] on a background thread (it blocks). A dedicated tokio
//! runtime is created per call so the caller doesn't need one.

use std::io::Read;
use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use russh::client;
use russh_keys::key::PublicKey;
use russh_keys::PublicKeyBase64;
use tokio::io::AsyncWriteExt;

/// A client handler whose only job is to verify the phone's host key.
#[derive(Clone, Default)]
struct SshClient;

#[async_trait]
impl client::Handler for SshClient {
    type Error = anyhow::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKey,
    ) -> Result<bool, Self::Error> {
        let known = super::ssh_server::is_known_phone_key(server_public_key);
        super::mdns::log_msg(&format!(
            "[ssh-send] phone host key {} known={known}",
            server_public_key.public_key_base64()
        ));
        Ok(known)
    }
}

/// Upload `path` to `ip:port` over SSH/SFTP.
///
/// Blocks until the phone accepts (or rejects) the transfer. On success the
/// phone saves the file (after a user confirmation) and `Ok(())` is returned;
/// otherwise an error describing the failure is returned. The file is streamed
/// from disk, so huge files use constant memory.
pub(crate) fn send_file(ip: IpAddr, port: u16, path: &Path) -> anyhow::Result<()> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .ok_or_else(|| anyhow::anyhow!("missing file name"))?;
    let mut file = std::fs::File::open(path)
        .map_err(|e| anyhow::anyhow!("open {}: {e}", path.display()))?;
    let len = file.metadata()?.len();

    super::transfer_progress::start(&name, true, len);
    let result = (|| -> anyhow::Result<()> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let name = name.clone();
        runtime.block_on(async move {
            push_file(&name, ip, port, len, &mut file).await
        })
    })();
    super::transfer_progress::finish(&name);
    result
}

/// The actual async SFTP push, run on a per-call tokio runtime.
async fn push_file(
    name: &str,
    ip: IpAddr,
    port: u16,
    len: u64,
    src: &mut std::fs::File,
) -> anyhow::Result<()> {
    let session = open_ssh_session(ip, port).await?;

    // Tell the phone how big this file is before opening the SFTP handle, so
    // it can show a determinate progress bar (SFTP never carries a total
    // size). The phone only acks the announce once the size is recorded, so it
    // is guaranteed to be in place before the OPEN arrives.
    announce_size(&session, name, len).await?;

    // Open an SFTP subsystem channel.
    let channel = session
        .channel_open_session()
        .await
        .map_err(|e| anyhow::anyhow!("open session channel: {e}"))?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|e| anyhow::anyhow!("request sftp subsystem: {e}"))?;
    let sftp = russh_sftp::client::SftpSession::new_with_config(
        channel.into_stream(),
        russh_sftp::client::Config {
            // The phone shows a confirmation dialog before accepting the
            // file open, and a human may take a while to answer. Match the
            // server's confirmation timeout (600 s) instead of the 10 s
            // default, or the open aborts while the dialog is still up.
            request_timeout_secs: 600,
            ..Default::default()
        },
    )
    .await
    .map_err(|e| anyhow::anyhow!("sftp init: {e}"))?;

    // Open the file for writing (CREATE|TRUNCATE|WRITE). On the phone this is
    // the point where the user confirms the incoming transfer. (We don't send
    // the size in the OPEN attrs: MINA on Android rejects an OPEN that carries
    // a "size" attribute, so the phone shows an indeterminate bar instead.)
    let mut remote = sftp
        .create(name)
        .await
        .map_err(|e| anyhow::anyhow!("open {name} on {ip}: {e}"))?;

    // Stream the body, reporting byte counts every ~1%.
    let mut done = 0u64;
    let mut last = 0.0f32;
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = src.read(&mut buf)?;
        if n == 0 {
            break;
        }
        remote.write_all(&buf[..n]).await?;
        done += n as u64;
        if len > 0 {
            let frac = (done as f32) / (len as f32);
            if frac - last >= 0.01 {
                super::transfer_progress::update(name, done, len);
                last = frac;
            }
        }
    }
    remote.flush().await?;
    // Shutdown flushes remaining writes and closes the remote handle.
    remote.shutdown().await?;

    super::mdns::log_msg(&format!("[ssh-send] uploaded {name} -> {ip}:{port}"));
    Ok(())
}

/// Announce a file's size to the peer over a `tumbleweed transfer` exec
/// channel. The peer records `name -> size` and replies `channel_success` only
/// after storing it, so waiting for that ack guarantees the size is known
/// before the SFTP open. If the peer is an older build or a plain SFTP client
/// that rejects the command, we carry on anyway — the transfer still works, it
/// just shows an indeterminate bar.
async fn announce_size(
    session: &client::Handle<SshClient>,
    name: &str,
    size: u64,
) -> anyhow::Result<()> {
    let mut ch = session
        .channel_open_session()
        .await
        .map_err(|e| anyhow::anyhow!("open announce channel: {e}"))?;
    ch.exec(true, "tumbleweed transfer")
        .await
        .map_err(|e| anyhow::anyhow!("exec announce: {e}"))?;
    let payload = format!("{size}\n{name}\n").into_bytes();
    ch.data(&payload[..])
        .await
        .map_err(|e| anyhow::anyhow!("send announce: {e}"))?;
    ch.eof()
        .await
        .map_err(|e| anyhow::anyhow!("announce eof: {e}"))?;
    // Wait for the peer's acknowledgement (sent only after it stored the
    // size). Timeout so an unresponsive peer can't hang the transfer.
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        async {
            loop {
                match ch.wait().await {
                    Some(russh::ChannelMsg::Success) => break,
                    Some(russh::ChannelMsg::Failure) => break,
                    Some(russh::ChannelMsg::Eof) | Some(_) => continue,
                    None => break,
                }
            }
        },
    )
    .await;
    let _ = ch.close().await;
    Ok(())
}

/// Connect to `ip:port`, verify the phone's host key and authenticate with this
/// PC's Ed25519 identity. Returns an authenticated session.
async fn open_ssh_session(ip: IpAddr, port: u16) -> anyhow::Result<client::Handle<SshClient>> {
    // Prefer AES-CTR (hardware AES-NI) and allow large packets, mirroring the
    // server config so both directions negotiate the fast path.
    let config = Arc::new(client::Config {
        maximum_packet_size: 262144,
        preferred: russh::Preferred {
            cipher: std::borrow::Cow::Borrowed(&[
                russh::cipher::AES_128_CTR,
                russh::cipher::AES_256_CTR,
            ]),
            ..Default::default()
        },
        ..Default::default()
    });

    // Connect with TCP_NODELAY so SFTP control packets are never held back by
    // Nagle while the data stream is in flight.
    let stream = tokio::net::TcpStream::connect((ip, port))
        .await
        .map_err(|e| anyhow::anyhow!("connect {ip}:{port}: {e}"))?;
    stream
        .set_nodelay(true)
        .map_err(|e| anyhow::anyhow!("set_nodelay: {e}"))?;

    let mut session = client::connect_stream(config, stream, SshClient)
        .await
        .map_err(|e| anyhow::anyhow!("ssh handshake with {ip}:{port}: {e}"))?;

    // Authenticate with this PC's Ed25519 identity.
    let key_text = std::fs::read_to_string(super::ssh_pair::private_key_path())
        .map_err(|e| anyhow::anyhow!("read client key: {e}"))?;
    let keypair = russh_keys::decode_secret_key(&key_text, None)
        .map_err(|e| anyhow::anyhow!("decode client key: {e}"))?;
    let authed = session
        .authenticate_publickey("tumbleweed", Arc::new(keypair))
        .await?;
    if !authed {
        anyhow::bail!("{ip} rejected our key — is the phone paired?");
    }
    Ok(session)
}

# Tumbleweed

A LAN file-transfer app for Windows, built in **Rust + WinUI 3** with the
[`windows-reactor`](https://github.com/microsoft/windows-rs) widget library.

Each Tumbleweed instance is both an **SSH/SFTP server** and an **mDNS
advertiser**, so machines on the same network can discover each other, pair
with a QR code, and push files to one another — no cloud, no accounts, no
configuration.

![Tumbleweed screenshot](assets/screenshot.png)

## Features

- **File explorer** — browse folders with a breadcrumb bar, a **drive
  selector**, and a manual **refresh button**. Remembers the last folder you
  opened (defaults to your Downloads folder on first launch).
- **Fuzzy search** — type in the title-bar search box to fuzzy-match files in
  the current folder; choosing a result selects it in the explorer list.
- **SSH pairing** — generate an Ed25519 key pair on the Settings page; the app
  renders it as an accent-colored QR code (with key-pair status and device
  meta). Scan it with the Android app to pair this PC.
- **Send to device** — every file row has a device flyout (device-type icon +
  name) listing the discovered peers; pick one and hit the upload button to
  push the file over SFTP. Success/failure is reported with a transient
  InfoBar.
- **mDNS advertising & discovery** — advertises this machine as
  `tumbleweed-<hostname>.local` with the service type `_tumbleweed._tcp.local`
  (device type + app version in the TXT record) and discovers other Tumbleweed
  devices on the LAN every few seconds. Your own advertisement is filtered
  out.
- **Devices tab** — shows this device (host name, IPs, version) plus every
  paired and newly-discovered device in separate sections, with a live
  Online/Offline status for paired devices.
- **Incoming upload confirmation** — the moment a transfer starts you're
  asked to confirm and pick the destination folder (queued if several arrive
  at once). If the app is in the background, the taskbar button flashes.
- **Transfer history** — a Transfer page with **All / Downloads / Uploads**
  sections. Active transfers show a live progress bar with transferred/total
  byte counts; completed rows are removable and in-flight transfers are
  cancellable. History persists across app restarts.
- **Settings** — SSH key pair management + pairing QR, an **mDNS discovery
  toggle**, and a theme selector (System / Light / Dark); the choices are
  remembered on restart.

## How it works

```
┌───────────────┐   mDNS  _tumbleweed._tcp.local   ┌───────────────┐
│  Tumbleweed A │ ◄──────────────────────────────► │  Tumbleweed B │
│  (this app)   │    SSH/SFTP  (file push, :2222)  │  (peer)       │
└───────────────┘                                  └───────────────┘
```

- On startup the app advertises a unique per-machine hostname over **mDNS**
  and starts an **SSH server on port 2222** (russh).
- Peers are discovered via mDNS PTR/A queries; each advertises its device type
  and app version in the TXT record, so no extra HTTP probe is needed.
- **Pairing:** the Settings page generates an Ed25519 key pair. The QR encodes
  the public key, LAN IP, app version, and a one-time token. A phone connects
  with `username == token`, authenticates with its own key, and runs
  `tumbleweed add-key` to register itself for future transfers.
- To send a file, the app opens an SFTP session to the picked device's SSH
  port, verifies the peer's host key against its registered public key, and
  pushes the file; the peer shows a confirmation dialog and saves it wherever
  you choose.

## Project structure

```
src/
├── main.rs              # App entry: shared state, title bar, navigation
├── controls/            # Shared widgets (section, simple_card)
├── pages/
│   ├── devices.rs       # Devices tab (this device, paired, new)
│   ├── explorer.rs      # File explorer + fuzzy search + upload
│   ├── settings.rs      # Settings page (pairing QR, theme, mDNS)
│   └── transfer.rs      # Transfer history + live progress
└── tools/
    ├── attention.rs     # Taskbar flashing for background alerts
    ├── mdns.rs          # mDNS advertiser + device discovery
    ├── picker.rs        # WinUI folder picker (incoming uploads)
    ├── qr_surface.rs    # QR rendering onto a canvas
    ├── settings_store.rs# INI-style settings persistence
    ├── ssh_pair.rs      # SSH key pair + pairing QR payload
    ├── ssh_send.rs      # SFTP client used to push files to peers
    ├── ssh_server.rs    # Embedded SSH server (SFTP receive, pairing)
    ├── transfer_progress.rs # In-progress transfer tracking/cancel
    └── upload_gate.rs   # Bridges upload confirmations to the UI thread
```

The pages all share a single render context (`RenderCx`); state lives in
`app` and is passed down as parameters.

## Requirements

- Windows 10/11
- Rust toolchain (stable). The project uses Windows-specific dependencies
  pulled from the `windows-rs` git branch, so it builds on Windows only.

## Build & run

```powershell
cargo build --release
cargo run
```

> **Note:** if you get a `resources.pri` lock error (`os error 1224`) while
> rebuilding, a previous instance is still running. Stop it first:
>
> ```powershell
> Get-Process tumbleweed -ErrorAction SilentlyContinue | Stop-Process -Force
> ```

The executable is framework-dependent and expects the Windows App SDK runtime
to be installed (handled by the `windows-reactor-setup` build dependency).

## Configuration

- **SSH port:** `tools::ssh_server::SSH_PORT` (default `2222`).
- **Persistence:** settings (last-opened folder, theme, mDNS toggle) are
  stored in `%LOCALAPPDATA%\tumbleweed\settings.ini`; transfer history in
  `%LOCALAPPDATA%\tumbleweed\history.txt`; the SSH identity and
  `authorized_keys` live in the app's folder (`%LOCALAPPDATA%\Tumbleweed`).

## Security notes

- Transfer and pairing traffic is **SSH**, but it is intended for **trusted
  LANs only** — do not expose port 2222 to the public internet.
- The SSH server only accepts clients whose public key is in its
  `authorized_keys`; the QR's one-time token bootstraps that the first time.
- Incoming files are only written after you confirm and pick a destination,
  so the server never writes blindly.

## CI / releases

The GitHub Actions workflow (`.github/workflows/rust.yml`) builds a release
and uploads a `tumbleweed.zip` artifact to the GitHub Release whenever you
push a tag matching `v*`.

## License

See the repository's license file (if any). This project is for learning and
LAN experimentation.

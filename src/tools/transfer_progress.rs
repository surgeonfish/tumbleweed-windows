//! Tracks in-progress file transfers (uploads and downloads) so the UI can
//! show a progress bar and transferred/total byte counts on the matching row.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use windows_reactor::AsyncSetState;

/// One in-progress transfer.
#[derive(Clone, PartialEq)]
pub(crate) struct TransferProgress {
    pub(crate) name: String,
    /// `true` for an upload (this app -> peer), `false` for a download
    /// (peer -> this app).
    pub(crate) is_upload: bool,
    /// Bytes transferred so far.
    pub(crate) done: u64,
    /// Total bytes for the transfer.
    pub(crate) total: u64,
}

/// name -> (is_upload, done_bytes, total_bytes)
static CURRENT: OnceLock<Mutex<HashMap<String, (bool, u64, u64)>>> = OnceLock::new();
static SETTER: Mutex<Option<AsyncSetState<Vec<TransferProgress>>>> = Mutex::new(None);

fn current() -> &'static Mutex<HashMap<String, (bool, u64, u64)>> {
    CURRENT.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Called once from the UI thread to install the state setter that receives
/// progress snapshots.
pub(crate) fn install_progress_setter(setter: AsyncSetState<Vec<TransferProgress>>) {
    *SETTER.lock().unwrap() = Some(setter);
    push();
}

/// Register `name` as starting a transfer of `total` bytes.
pub(crate) fn start(name: &str, is_upload: bool, total: u64) {
    current()
        .lock()
        .unwrap()
        .insert(name.to_string(), (is_upload, 0, total));
    push();
}

/// Update `name`'s byte counts, preserving its direction.
pub(crate) fn update(name: &str, done: u64, total: u64) {
    let mut map = current().lock().unwrap();
    if let Some((is_upload, _, _)) = map.get(name).cloned() {
        map.insert(name.to_string(), (is_upload, done, total));
    }
    drop(map);
    push();
}

/// Remove `name` (transfer finished or failed).
pub(crate) fn finish(name: &str) {
    current().lock().unwrap().remove(name);
    push();
}

/// Push the current snapshot to the UI thread (no-op until the setter is
/// installed).
fn push() {
    let snapshot: Vec<TransferProgress> = current()
        .lock()
        .unwrap()
        .iter()
        .map(|(name, (is_upload, done, total))| TransferProgress {
            name: name.clone(),
            is_upload: *is_upload,
            done: *done,
            total: *total,
        })
        .collect();
    if let Some(setter) = SETTER.lock().unwrap().clone() {
        setter.call(snapshot);
    }
}


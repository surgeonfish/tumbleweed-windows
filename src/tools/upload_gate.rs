//! Bridge between the HTTP server thread and the WinUI UI thread for
//! confirming incoming uploads.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Mutex, OnceLock};
use windows_reactor::AsyncSetState;

/// An upload waiting for the user's confirmation in the UI.
#[derive(Clone, PartialEq)]
pub(crate) struct IncomingUpload {
    pub(crate) id: u64,
    pub(crate) name: String,
    pub(crate) size: u64,
}

/// The user's decision about an incoming upload.
pub(crate) enum UploadDecision {
    /// Save the file into this folder.
    Save(PathBuf),
    /// Decline the upload.
    Reject,
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static REPLIES: OnceLock<Mutex<HashMap<u64, mpsc::Sender<UploadDecision>>>> = OnceLock::new();
static SETTER: Mutex<Option<AsyncSetState<Option<IncomingUpload>>>> = Mutex::new(None);

fn replies() -> &'static Mutex<HashMap<u64, mpsc::Sender<UploadDecision>>> {
    REPLIES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Called once from the UI thread to install the state setter the server uses
/// to surface pending uploads.
pub(crate) fn install_upload_setter(setter: AsyncSetState<Option<IncomingUpload>>) {
    *SETTER.lock().unwrap() = Some(setter);
}

/// Register a reply channel and hand the upload to the UI thread. Returns the
/// upload id and the receiver the server waits on, or `None` if the UI isn't
/// ready yet.
pub(crate) fn submit_upload(
    name: String,
    size: u64,
) -> Option<(u64, mpsc::Receiver<UploadDecision>)> {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = mpsc::channel();
    replies().lock().unwrap().insert(id, tx);

    let setter = SETTER.lock().unwrap().clone();
    if setter.is_none() {
        replies().lock().unwrap().remove(&id);
        return None;
    }
    setter.unwrap().call(Some(IncomingUpload { id, name, size }));
    Some((id, rx))
}

/// The UI thread sends its decision back to the waiting server connection.
pub(crate) fn reply(id: u64, decision: UploadDecision) {
    if let Some(tx) = replies().lock().unwrap().get(&id).cloned() {
        let _ = tx.send(decision);
    }
}

/// Forget an upload (called by the server once it has a decision).
pub(crate) fn remove_upload(id: u64) {
    replies().lock().unwrap().remove(&id);
}

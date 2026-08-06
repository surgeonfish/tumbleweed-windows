//! Bridge between the HTTP server thread and the WinUI UI thread for
//! confirming incoming uploads. Incoming uploads are queued so that several
//! concurrent transfers (e.g. the phone uploading to this PC while another
//! device does too) are each confirmed in turn instead of one clobbering the
//! other.

use std::collections::{HashMap, VecDeque};
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
static SETTER: Mutex<Option<AsyncSetState<Vec<IncomingUpload>>>> = Mutex::new(None);
static PENDING: Mutex<VecDeque<IncomingUpload>> = Mutex::new(VecDeque::new());

fn replies() -> &'static Mutex<HashMap<u64, mpsc::Sender<UploadDecision>>> {
    REPLIES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Called once from the UI thread to install the state setter the server uses
/// to surface pending uploads.
pub(crate) fn install_upload_setter(setter: AsyncSetState<Vec<IncomingUpload>>) {
    *SETTER.lock().unwrap() = Some(setter);
    push_queue();
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
    PENDING.lock().unwrap().push_back(IncomingUpload { id, name, size });
    push_queue();
    Some((id, rx))
}

/// The UI thread sends its decision back to the waiting server connection.
pub(crate) fn reply(id: u64, decision: UploadDecision) {
    if let Some(tx) = replies().lock().unwrap().get(&id).cloned() {
        let _ = tx.send(decision);
    }
}

/// The UI decided the front-of-queue upload: drop it and show the next one.
pub(crate) fn advance() {
    PENDING.lock().unwrap().pop_front();
    push_queue();
}

/// Forget an upload (called by the server once it has a decision).
pub(crate) fn remove_upload(id: u64) {
    replies().lock().unwrap().remove(&id);
}

/// The server-side transfer failed before a decision (e.g. the client
/// disconnected mid-stream): drop the upload so the UI doesn't keep waiting on
/// a dialog for a connection that's already gone.
pub(crate) fn fail_upload(id: u64) {
    replies().lock().unwrap().remove(&id);
    PENDING.lock().unwrap().retain(|u| u.id != id);
    push_queue();
}

/// Push the current pending queue to the UI thread (no-op until the setter is
/// installed).
fn push_queue() {
    let setter = SETTER.lock().unwrap().clone();
    if let Some(setter) = setter {
        let queue: Vec<IncomingUpload> =
            PENDING.lock().unwrap().iter().cloned().collect();
        setter.call(queue);
    }
}

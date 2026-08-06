use std::path::{Component, Path, PathBuf};
use windows_reactor::*;
use crate::tools::mdns::DiscoveredDevice;

// ---------- Filesystem helpers for the Explorer page ----------

#[derive(Clone)]
pub(crate) struct FsEntry {
    name: String,
    path: PathBuf,
    is_dir: bool,
}

/// Path to the app's settings file (`%LOCALAPPDATA%\tumbleweed\settings.ini`).
fn settings_file() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".to_string());
    Path::new(&base).join("tumbleweed").join("settings.ini")
}

/// Remember the last folder the user opened so a future launch resumes there.
pub(crate) fn save_last_folder(path: &Path) {
    let file = settings_file();
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&file, format!("last_folder={}\n", path.display()));
}

/// The folder restored from settings, if it still exists.
fn load_last_folder() -> Option<PathBuf> {
    let text = std::fs::read_to_string(settings_file()).ok()?;
    text.lines()
        .find_map(|l| l.strip_prefix("last_folder=").map(PathBuf::from))
        .filter(|p| p.is_dir())
}

/// Folder to show on first launch (the app has never opened a folder): the
/// user's Downloads folder, falling back to the profile dir, then `C:\`.
pub(crate) fn default_folder() -> PathBuf {
    if let Some(saved) = load_last_folder() {
        return saved;
    }
    if let Ok(profile) = std::env::var("USERPROFILE") {
        let downloads = Path::new(&profile).join("Downloads");
        if downloads.is_dir() {
            return downloads;
        }
        return PathBuf::from(profile);
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("C:\\"))
}

/// List the immediate children of `path`; folders first, then alphabetically.
fn list_entries(path: &Path) -> Vec<FsEntry> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(path) {
        for item in rd.flatten() {
            let p = item.path();
            let name = item.file_name().to_string_lossy().to_string();
            let is_dir = p.is_dir();
            out.push(FsEntry { name, path: p, is_dir });
        }
    }
    out.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    out
}

/// `C:\Users\Anna\Downloads` -> `["C:", "Users", "Anna", "Downloads"]`.
fn path_to_crumbs(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|c| match c {
            Component::Prefix(p) => Some(p.as_os_str().to_string_lossy().to_string()),
            Component::Normal(n) => Some(n.to_string_lossy().to_string()),
            _ => None,
        })
        .collect()
}

/// Rebuild a path from the first `upto + 1` crumbs (breadcrumb click).
fn path_from_crumbs(crumbs: &[String], upto: usize) -> PathBuf {
    let mut p = PathBuf::new();
    for (i, part) in crumbs.iter().take(upto + 1).enumerate() {
        if i == 0 {
            p = PathBuf::from(format!("{}\\", part)); // drive root, e.g. "C:\"
        } else {
            p.push(part);
        }
    }
    p
}

/// Folder-derived data shared with the app's single render context.
#[derive(Clone)]
pub(crate) struct ExplorerData {
    pub(crate) entries: Vec<FsEntry>,
    pub(crate) crumbs: Vec<String>,
}

/// Compute the listing + breadcrumb items for `path`.
pub(crate) fn view_data(path: &Path) -> ExplorerData {
    ExplorerData {
        entries: list_entries(path),
        crumbs: path_to_crumbs(path),
    }
}

/// Case-insensitive subsequence match: every char of `query` appears in `name`
/// in order (classic fuzzy search). Empty query matches everything.
pub(crate) fn fuzzy_match(query: &str, name: &str) -> bool {
    let q: Vec<char> = query.to_lowercase().chars().collect();
    if q.is_empty() {
        return true;
    }
    let mut qi = 0;
    for c in name.to_lowercase().chars() {
        if c == q[qi] {
            qi += 1;
            if qi == q.len() {
                return true;
            }
        }
    }
    false
}

/// Names of entries in `data` that fuzzy-match `query` (for suggestions).
pub(crate) fn search_suggestions(data: &ExplorerData, query: &str) -> Vec<String> {
    let q = query.trim();
    if q.is_empty() {
        return Vec::new();
    }
    data.entries
        .iter()
        .filter(|e| fuzzy_match(q, &e.name))
        .map(|e| e.name.clone())
        .take(10)
        .collect()
}

/// Index (into `data.entries`) of the best fuzzy match for `query`: prefers an
/// entry whose name starts with the query, otherwise the first fuzzy match.
/// Returns `None` when nothing matches.
pub(crate) fn search_best_index(data: &ExplorerData, query: &str) -> Option<usize> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return None;
    }
    let mut best: Option<usize> = None;
    let mut best_is_prefix = false;
    for (i, e) in data.entries.iter().enumerate() {
        if !fuzzy_match(&q, &e.name) {
            continue;
        }
        let is_prefix = e.name.to_lowercase().starts_with(&q);
        let take = match best {
            None => true,
            Some(_) => is_prefix && !best_is_prefix,
        };
        if take {
            best = Some(i);
            best_is_prefix = is_prefix;
        }
    }
    best
}

/// Result of a file-send attempt, surfaced as a transient InfoBar in the
/// Explorer page (auto-dismissed after a few seconds).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum UploadOutcome {
    /// The file `name` was delivered to the picked device.
    Success(String),
    /// The send failed with `message`.
    Error(String),
}

/// The Explorer page. It renders inside the app's shared [`RenderCx`] (the same
/// one every page uses); the folder state — which defaults to the user's
/// Downloads folder on first launch — is owned by `app` and passed in here.
pub(crate) fn explorer_page(
    _cx: &mut RenderCx,
    set_current_path: SetState<PathBuf>,
    data: &ExplorerData,
    hovered_index: Option<usize>,
    set_hovered_index: SetState<Option<usize>>,
    selected_device: Option<DiscoveredDevice>,
    set_upload_result: AsyncSetState<Option<UploadOutcome>>,
    upload_outcome: &Option<UploadOutcome>,
    search_target: Option<usize>,
) -> Element {
    // Clicking a breadcrumb navigates back to that ancestor folder.
    let crumb_set = set_current_path.clone();
    let crumb_items = data.crumbs.clone();
    let breadcrumb = BreadcrumbBar::new(crumb_items.clone()).on_item_clicked(move |idx: i32| {
        if idx >= 0 {
            let target = path_from_crumbs(&crumb_items, idx as usize);
            crumb_set.call(target);
        }
    });

    // Selecting a folder in the list navigates into it. Each row has a
    // reveal-on-hover upload button.
    let list: Element = list_view(data.entries.clone(), move |e, idx| {
        let label = if e.is_dir {
            format!("📁  {}", e.name)
        } else {
            format!("📄  {}", e.name)
        };

        // Upload button, shown only while this row is hovered.
        let is_hovered = hovered_index == Some(idx);
        let upload = button("")
            .icon(Symbol::Upload)
            .subtle()
            .enabled(is_hovered)
            .opacity(if is_hovered { 1.0 } else { 0.0 })
            .on_click({
                let device = selected_device.clone();
                let path = e.path.clone();
                let name = e.name.clone();
                let set_upload_result = set_upload_result.clone();
                move || {
                    // Only send when a device is picked in the footer dropdown.
                    let Some(device) = device.as_ref() else {
                        println!("[explorer] no device selected; upload skipped");
                        set_upload_result.call(Some(UploadOutcome::Error(format!(
                            "No device selected; could not send"
                            ))));
                        return;
                    };
                    let ip = device.ip;
                    let path = path.clone();
                    let name = name.clone();
                    let set_upload_result = set_upload_result.clone();
                    // Do the network transfer off the UI thread; report back via
                    // the async state so Transfer history updates on success.
                    std::thread::spawn(move || {
                        match crate::tools::client::send_file(
                            ip,
                            crate::tools::server::HTTP_PORT,
                            &path,
                        ) {
                            Ok(()) => {
                                set_upload_result.call(Some(UploadOutcome::Success(name)))
                            }
                            Err(e) => {
                                eprintln!("[client] upload to {ip} failed: {e}");
                                set_upload_result.call(Some(UploadOutcome::Error(format!(
                                    "Could not send {name} to {ip}: {e}"
                                ))));
                            }
                        }
                    });
                }
            });

        // The whole entry is hoverable: a full-width Grid (label left, upload
        // button right) with a transparent background so the empty row area
        // also hit-tests. The hover handlers live on the entry, not on the
        // inner content.
        grid((
            TextBlock::new(label)
                .padding(Thickness::uniform(8.0))
                .grid_column(0),
            upload
                .horizontal_alignment(HorizontalAlignment::Right)
                .grid_column(1),
        ))
        .columns([GridLength::Auto, GridLength::STAR])
        .background(Color { a: 0, r: 0, g: 0, b: 0 })
        .on_pointer_entered({
            let set_hovered_index = set_hovered_index.clone();
            move |_info: PointerEventInfo| set_hovered_index.call(Some(idx))
        })
        .on_pointer_exited({
            let set_hovered_index = set_hovered_index.clone();
            move || set_hovered_index.call(None)
        })
    })
    .with_key_selector(|e| e.path.to_string_lossy().to_string())
    .selected_index(search_target.map(|i| i as i32).unwrap_or(-1))
    .on_selection_changed({
        let set_current = set_current_path.clone();
        let entries = data.entries.clone();
        move |idx: i32| {
            if idx >= 0 {
                if let Some(entry) = entries.get(idx as usize) {
                    if entry.is_dir {
                        set_current.call(entry.path.clone());
                    }
                }
            }
        }
    })
    .into();

    // Transient upload result bar at the bottom of the page (auto-dismissed).
    let outcome_bar: Element = match upload_outcome {
        Some(UploadOutcome::Success(name)) => InfoBar::new(format!("Sent {name}"))
            .message("File delivered to the picked device.")
            .success()
            .is_closable(false)
            .into(),
        Some(UploadOutcome::Error(msg)) => InfoBar::new("Upload failed")
            .message(msg.clone())
            .error()
            .is_closable(false)
            .into(),
        None => Element::Empty,
    };

    grid((
        title("Explorer")
            .margin(Thickness {
                left: 0.0,
                top: 0.0,
                right: 0.0,
                bottom: 12.0,
            })
            .grid_row(0),
        breadcrumb.grid_row(1),
        list.grid_row(2),
        outcome_bar.grid_row(3),
    ))
    .rows([
        GridLength::Auto,
        GridLength::Auto,
        GridLength::STAR,
        GridLength::Auto,
    ])
    .margin(Thickness {
        left: 36.0,
        right: 36.0,
        top: 24.0,
        bottom: 0.0,
    })
    .into()
}

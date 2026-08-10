use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};
use windows_reactor::*;
use crate::tools::mdns::DiscoveredDevice;

// ---------- Filesystem helpers for the Explorer page ----------

#[derive(Clone)]
pub(crate) struct FsEntry {
    name: String,
    path: PathBuf,
    is_dir: bool,
}

/// Remember the last folder the user opened so a future launch resumes there.
pub(crate) fn save_last_folder(path: &Path) {
    crate::tools::settings_store::set("last_folder", &path.display().to_string());
}

/// The folder restored from settings, if it still exists.
fn load_last_folder() -> Option<PathBuf> {
    crate::tools::settings_store::get("last_folder")
        .map(PathBuf::from)
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

/// All mounted drive roots on this machine (e.g. `["C:\\", "D:\\"]`), found
/// by probing every drive letter.
fn drive_roots() -> Vec<String> {
    (b'A'..=b'Z')
        .map(|c| format!("{}:\\", c as char))
        .filter(|p| Path::new(p).is_dir())
        .collect()
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
    explorer_refresh: u32,
    set_explorer_refresh: SetState<u32>,
    data: &ExplorerData,
    devices: &[DiscoveredDevice],
    selected_index: Option<usize>,
    set_selected_index: SetState<Option<usize>>,
    last_tap: HookRef<(Option<usize>, Instant)>,
    set_upload_result: AsyncSetState<Option<UploadOutcome>>,
    upload_outcome: &Option<UploadOutcome>,
    search_target: Option<usize>,
) -> Element {
    // Own the device list: the list-view row builder is a `'static` closure,
    // so it can't borrow the `&[DiscoveredDevice]` parameter. Cloning here lets
    // each row's device-picker flyout capture the devices by value.
    let devices = devices.to_vec();

    // Clicking a breadcrumb navigates back to that ancestor folder.
    let crumb_set = set_current_path.clone();
    let crumb_items = data.crumbs.clone();
    let breadcrumb = BreadcrumbBar::new(crumb_items.clone()).on_item_clicked(move |idx: i32| {
        if idx >= 0 {
            let target = path_from_crumbs(&crumb_items, idx as usize);
            crumb_set.call(target);
        }
    });

    // Drive selector: lists this machine's mounted volumes so the user can
    // jump straight to another drive.
    let current_drive = data.crumbs.first().cloned().unwrap_or_default();
    let drive_items: Vec<MenuItemDef> = drive_roots()
        .iter()
        .map(|root| menu_item(root.trim_end_matches('\\')))
        .collect();
    let drive_picker: Element = drop_down_button(current_drive)
        .menu_flyout(drive_items)
        .on_item_clicked({
            let set_current_path = set_current_path.clone();
            move |label: String| {
                set_current_path.call(PathBuf::from(format!("{label}\\")));
            }
        })
        .into();

    let tool_bar = hstack((
        button("")
            // Refresh glyph (Segoe Fluent Icons).
            .icon(Icon::font_family("\u{E8F7}", "Segoe Fluent Icons"))
            .on_click({
                let set_explorer_refresh = set_explorer_refresh.clone();
                move || set_explorer_refresh.call(explorer_refresh + 1)
            }),
        // Match the refresh button's height.
        drive_picker.height(34.0),
        border(breadcrumb)
            .padding(Thickness::xy(8.0, 2.0))
            .corner_radius(4.0)
            .background(ThemeRef::ControlFill)
            .border_brush(ThemeRef::ControlStroke)
            .border_thickness(Thickness::uniform(1.0)),
    ))
    .spacing(8.0);

    // Selecting a folder in the list navigates into it. Each row has a
    // reveal-on-hover upload button.
    let list: Element = list_view(data.entries.clone(), move |e, idx| {
        let label = if e.is_dir {
            format!("📁  {}", e.name)
        } else {
            format!("📄  {}", e.name)
        };

        // Upload button that opens a CommandBarFlyout listing every discovered
        // device as an AppBarButton (device-type icon + device name), so the
        // sender picks the destination right from the row. Revealed only while
        // this row is selected.
        let is_selected = selected_index == Some(idx);
        let device_commands: Vec<CommandBarCommandDef> = devices
            .iter()
            .map(|d| {
                app_bar_button_icon(
                    d.name.clone(),
                    Icon::font_family(
                        crate::pages::devices::kind_icon(&d.kind),
                        "Segoe Fluent Icons",
                    ),
                )
            })
            .collect();
        let upload = button("")
            .icon(Icon::font_family(
                "\u{E72D}",
                "Segoe Fluent Icons",
            ))
            .subtle()
            .enabled(is_selected)
            .opacity(if is_selected { 1.0 } else { 0.0 })
            .command_bar_flyout(device_commands)
            .on_command_bar_flyout_click({
                let path = e.path.clone();
                let name = e.name.clone();
                let set_upload_result = set_upload_result.clone();
                let devices = devices.to_vec();
                move |label: String| {
                    // The clicked AppBarButton's label is the device name.
                    let Some(device) = devices.iter().find(|d| d.name == label) else {
                        set_upload_result.call(Some(UploadOutcome::Error(format!(
                            "Device not found: {label}"
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
                        match crate::tools::ssh_send::send_file(
                            ip,
                            crate::tools::ssh_server::SSH_PORT,
                            &path,
                        ) {
                            Ok(()) => {
                                set_upload_result.call(Some(UploadOutcome::Success(name)))
                            }
                            Err(e) => {
                                eprintln!("[ssh-send] upload to {ip} failed: {e}");
                                set_upload_result.call(Some(UploadOutcome::Error(format!(
                                    "Could not send {name} to {ip}: {e}"
                                ))));
                            }
                        }
                    });
                }
            });

        // The whole entry is a full-width Grid (label left, upload button
        // right) with a transparent background so the empty row area also
        // hit-tests.
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
        .on_pointer_pressed({
            // A quick second press on the same row opens a folder; a single
            // press only selects it (highlights + reveals the upload button).
            // We count pointer presses rather than `Tapped` because WinUI
            // swallows the second Tapped of a double-click into DoubleTapped,
            // which would make double-click detection need a third click.
            let set_current = set_current_path.clone();
            let last_tap = last_tap.clone();
            let path = e.path.clone();
            let is_dir = e.is_dir;
            move |info: PointerEventInfo| {
                // Only count left-button presses (ignore right/middle clicks).
                if !info.is_left_button_pressed {
                    return;
                }
                let now = Instant::now();
                let mut last = last_tap.borrow_mut();
                let is_double = last.0 == Some(idx)
                    && now.duration_since(last.1) <= Duration::from_millis(500);
                *last = (Some(idx), now);
                drop(last);
                if is_double && is_dir {
                    set_current.call(path.clone());
                }
            }
        })
    })
    .with_key_selector(|e| e.path.to_string_lossy().to_string())
    .selected_index(search_target.map(|i| i as i32).unwrap_or(-1))
    .on_selection_changed({
        let set_selected_index = set_selected_index.clone();
        move |idx: i32| {
            if idx >= 0 {
                // Remember the selection so the row's upload button is revealed.
                set_selected_index.call(Some(idx as usize));
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
        tool_bar.grid_row(1),
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

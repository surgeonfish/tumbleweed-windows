use std::path::{Path, PathBuf};
use windows_reactor::*;

use crate::tools::transfer_progress::TransferProgress;

/// Direction of a transfer history entry.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransferDirection {
    Uploaded,
    Downloaded,
}

impl TransferDirection {
    /// Leading glyph (Segoe Fluent Icons) shown on the history row.
    fn icon(&self) -> &'static str {
        match self {
            Self::Uploaded => "\u{E898}",   // Upload
            Self::Downloaded => "\u{E896}", // Download
        }
    }
    /// Short stable tag used for list keys.
    fn tag(&self) -> &'static str {
        match self {
            Self::Uploaded => "up",
            Self::Downloaded => "down",
        }
    }
}

/// One entry in the transfer history.
#[derive(Clone, PartialEq)]
pub(crate) struct TransferRecord {
    pub(crate) name: String,
    pub(crate) direction: TransferDirection,
}

/// Actions that mutate the transfer history (shared state in `app`).
#[derive(Clone)]
pub(crate) enum TransferAction {
    /// Mark `name` as uploaded — fired by the Explorer page's upload button.
    MarkUploaded(String),
    /// Mark `name` as downloaded — fired when an incoming upload is accepted.
    MarkDownloaded(String),
}

/// Path to the transfer history file (`%LOCALAPPDATA%\tumbleweed\history.txt`).
fn history_file() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".to_string());
    Path::new(&base).join("tumbleweed").join("history.txt")
}

/// Persist the transfer history (one `<name>\t<up|down>` line per record), so
/// it survives app restarts.
pub(crate) fn save_history(history: &[TransferRecord]) {
    let file = history_file();
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut text = String::new();
    for r in history {
        let tag = match r.direction {
            TransferDirection::Uploaded => "up",
            TransferDirection::Downloaded => "down",
        };
        text.push_str(&format!("{}\t{}\n", r.name, tag));
    }
    let _ = std::fs::write(&file, text);
}

/// Load the transfer history saved by a previous launch.
pub(crate) fn load_history() -> Vec<TransferRecord> {
    let Ok(text) = std::fs::read_to_string(history_file()) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| {
            let (name, tag) = line.split_once('\t')?;
            let direction = match tag {
                "up" => TransferDirection::Uploaded,
                "down" => TransferDirection::Downloaded,
                _ => return None,
            };
            Some(TransferRecord {
                name: name.to_string(),
                direction,
            })
        })
        .collect()
}

/// A row shown in a transfer list: a completed record or an in-progress
/// transfer with its (done, total) byte counts.
struct Row {
    name: String,
    direction: TransferDirection,
    progress: Option<(u64, u64)>,
}

/// Format a byte count as a compact human string (e.g. "3.2 MB").
fn fmt_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

/// Combine in-progress transfers (with progress) and completed history into
/// the rows for one list, optionally filtered by direction.
fn make_rows(
    history: &[TransferRecord],
    transfers: &[TransferProgress],
    direction: Option<TransferDirection>,
) -> Vec<Row> {
    let matches = |d: TransferDirection| direction.map_or(true, |f| f == d);

    // Names currently being transferred (per direction) so we don't also list
    // their history record and end up with a duplicate "done" row.
    let in_progress: Vec<(String, TransferDirection)> = transfers
        .iter()
        .map(|t| {
            let dir = if t.is_upload {
                TransferDirection::Uploaded
            } else {
                TransferDirection::Downloaded
            };
            (t.name.clone(), dir)
        })
        .collect();

    let mut rows = Vec::new();
    for t in transfers {
        let dir = if t.is_upload {
            TransferDirection::Uploaded
        } else {
            TransferDirection::Downloaded
        };
        if matches(dir) {
            rows.push(Row {
                name: t.name.clone(),
                direction: dir,
                progress: Some((t.done, t.total)),
            });
        }
    }
    for r in history {
        let already_listed = in_progress
            .iter()
            .any(|(n, d)| n == &r.name && *d == r.direction);
        if matches(r.direction) && !already_listed {
            rows.push(Row {
                name: r.name.clone(),
                direction: r.direction,
                progress: None,
            });
        }
    }
    rows
}

/// Build a list view of `rows`, each prefixed with its direction icon and, for
/// in-progress transfers, a progress bar with transferred/total byte counts.
fn history_list(rows: Vec<Row>) -> Element {
    list_view(rows, |r, _| {
        // Right-hand column: progress bar + "done / total" caption, or empty.
        let right = match r.progress {
            Some((done, total)) => {
                // When the total is unknown (0) show an indeterminate bar so
                // the row still visibly animates during the transfer.
                let bar: Element = if total > 0 {
                    ProgressBar::new((done as f64) / (total as f64) * 100.0)
                        .height(4.0)
                        .width(100.0)
                        .vertical_alignment(VerticalAlignment::Center)
                        .into()
                } else {
                    ProgressBar::indeterminate()
                        .height(4.0)
                        .width(100.0)
                        .vertical_alignment(VerticalAlignment::Center)
                        .into()
                };
                let caption = if total > 0 {
                    format!("{} / {}", fmt_bytes(done), fmt_bytes(total))
                } else {
                    fmt_bytes(done)
                };
                // A Grid (not an hstack) so the ProgressBar gets a finite
                // STAR width — in a horizontal StackPanel it is measured with
                // infinite width and collapses to nothing.
                grid((
                    TextBlock::new(caption)
                        .font_size(11.0)
                        .foreground(Color { a: 255, r: 130, g: 130, b: 130 })
                        .vertical_alignment(VerticalAlignment::Center)
                        .grid_column(0),
                    bar.grid_column(1),
                ))
                .columns([GridLength::Auto, GridLength::STAR])
                .column_spacing(8.0)
                .margin(Thickness {
                    left: 12.0,
                    top: 0.0,
                    right: 8.0,
                    bottom: 0.0,
                })
                .vertical_alignment(VerticalAlignment::Center)
                .horizontal_alignment(HorizontalAlignment::Right)
                .into()
            }
            None => Element::Empty,
        };

        grid((
            hstack((
                TextBlock::new(r.direction.icon())
                    .font_family("Segoe Fluent Icons")
                    .font_size(12.0)
                    .vertical_alignment(VerticalAlignment::Center)
                    .padding(Thickness {
                        left: 0.0,
                        top: 8.0,
                        right: 0.0,
                        bottom: 8.0,
                    }),
                TextBlock::new(r.name.clone())
                    .vertical_alignment(VerticalAlignment::Center)
                    .padding(Thickness {
                        left: 4.0,
                        top: 8.0,
                        right: 8.0,
                        bottom: 8.0,
                    }),
            ))
            .horizontal_alignment(HorizontalAlignment::Stretch)
            .grid_column(0),
            right.grid_column(1),
        ))
        .columns([GridLength::Auto, GridLength::STAR])
        .horizontal_alignment(HorizontalAlignment::Stretch)
    })
    .with_key_selector(|r| {
        format!(
            "{}-{}#{}",
            if r.progress.is_some() { "in" } else { "done" },
            r.name,
            r.direction.tag()
        )
    })
    .into()
}

/// The Transfer page: All / Downloads / Uploads shown as a Top-mode
/// NavigationView over the shared transfer history. It renders inside the
/// app's shared [`RenderCx`]; the history and selected tab are owned by `app`.
pub(crate) fn transfer_page(
    _cx: &mut RenderCx,
    history: &[TransferRecord],
    tab: String,
    set_tab: SetState<String>,
    transfers: &[TransferProgress],
) -> Element {
    // The list for the active sub-tab (in-progress items first).
    let content = match tab.as_str() {
        "downloads" => history_list(make_rows(
            history,
            transfers,
            Some(TransferDirection::Downloaded),
        )),
        "uploads" => history_list(make_rows(
            history,
            transfers,
            Some(TransferDirection::Uploaded),
        )),
        _ => history_list(make_rows(history, transfers, None)),
    };

    grid((
        title("Transfer")
            .margin(Thickness {
                left: 0.0,
                top: 0.0,
                right: 0.0,
                bottom: 0.0,
            })
            .grid_row(0),
        NavigationView::new(
            [
                NavViewItem::new("All").tag("all"),
                NavViewItem::new("Downloads").tag("downloads"),
                NavViewItem::new("Uploads").tag("uploads"),
            ],
            content,
        )
        .selected_tag(tab)
        .on_selection_changed(set_tab)
        .pane_display_mode(NavigationViewPaneDisplayMode::Top)
        .back_button_visible(false)
        .settings_visible(false)
        .grid_row(1),
    ))
    .rows([GridLength::Auto, GridLength::STAR])
    .margin(Thickness {
        left: 36.0,
        right: 36.0,
        top: 24.0,
        bottom: 0.0,
    })
    .into()
}

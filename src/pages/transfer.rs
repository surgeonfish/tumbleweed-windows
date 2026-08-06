use windows_reactor::*;

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

/// Build a list view of `records`, each row prefixed with its direction icon.
fn history_list(records: Vec<TransferRecord>) -> Element {
    list_view(records, |r, _| {
        hstack((
            TextBlock::new(r.direction.icon())
                .font_family("Segoe Fluent Icons")
                .font_size(16.0)
                .vertical_alignment(VerticalAlignment::Center)
                .padding(Thickness {
                    left: 8.0,
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
    })
    .with_key_selector(|r| format!("{}#{}", r.name, r.direction.tag()))
    .into()
}

/// The Transfer page: 3 tabs over the shared transfer history. It renders
/// inside the app's shared [`RenderCx`]; the history is owned by `app`.
pub(crate) fn transfer_page(_cx: &mut RenderCx, history: &[TransferRecord]) -> Element {
    let downloads: Vec<_> = history
        .iter()
        .filter(|r| r.direction == TransferDirection::Downloaded)
        .cloned()
        .collect();
    let uploads: Vec<_> = history
        .iter()
        .filter(|r| r.direction == TransferDirection::Uploaded)
        .cloned()
        .collect();

    TabView::new([
        TabItem::new("All", history_list(history.to_vec())),
        TabItem::new("Downloads", history_list(downloads)),
        TabItem::new("Uploads", history_list(uploads)),
    ])
    .selected_index(0)
    .is_add_tab_button_visible(false)
    .margin(Thickness::uniform(12.0))
    .into()
}

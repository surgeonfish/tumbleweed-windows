// Windows GUI subsystem: no console window appears when the app is launched
// from Explorer (Rust defaults to a console-subsystem binary otherwise).
#![windows_subsystem = "windows"]

use std::time::Duration;
use windows_reactor::*;

mod pages;
mod tools;
use pages::devices::devices_page;
use pages::explorer::{default_folder, explorer_page, save_last_folder, view_data, UploadOutcome};
use pages::settings::settings_page;
use pages::transfer::{transfer_page, TransferAction, TransferDirection, TransferRecord};
use tools::mdns::DiscoveredDevice;
use tools::transfer_progress::TransferProgress;
use tools::upload_gate::{IncomingUpload, UploadDecision};

/// One line for a discovered device in the footer dropdown: just its display
/// name.
fn device_text(d: &DiscoveredDevice) -> String {
    d.name.clone()
}

fn app(cx: &mut RenderCx) -> Element {
    let (page, set_page) = cx.use_state("explorer".to_string());
    let (search_text, set_search_text) = cx.use_state(String::new());
    // Index (into the current Explorer listing) of the fuzzy-searched entry to
    // select in the list; `None` leaves the selection untouched.
    let (search_target, set_search_target) = cx.use_state(None::<usize>);

    // Explorer page state lives here, in the app's single render context, so
    // every page shares the same `cx`. Defaults to the Downloads folder.
    let default = cx.use_memo((), || default_folder());
    let (current_path, set_current_path) = cx.use_state(default.clone());

    // Persist the folder and keep the HTTP file server pointing at the same
    // folder the user is currently exploring.
    cx.use_effect((current_path.clone(),), {
        let current_path = current_path.clone();
        let set_search_target = set_search_target.clone();
        move || {
            save_last_folder(&current_path);
            tools::server::set_root(current_path.clone());
            // A folder change invalidates any previously searched row index.
            set_search_target.call(None);
        }
    });

    // Listing + breadcrumbs recompute only when the folder changes.
    let data = cx.use_memo((current_path.clone(),), || view_data(&current_path));

    // Hovered row in the explorer list — reveals the per-row upload button.
    let (hovered_index, set_hovered_index) = cx.use_state(None::<usize>);

    // Transfer history, shared across pages. The Explorer upload button appends
    // uploads; downloads will be recorded once a download action exists.
    let (transfer_history, dispatch_transfer) = cx.use_reducer_fn(
        |history: Vec<TransferRecord>, action: TransferAction| match action {
            TransferAction::MarkUploaded(name) => {
                let mut h = history;
                h.push(TransferRecord {
                    name,
                    direction: TransferDirection::Uploaded,
                });
                h
            }
            TransferAction::MarkDownloaded(name) => {
                let mut h = history;
                h.push(TransferRecord {
                    name,
                    direction: TransferDirection::Downloaded,
                });
                h
            }
        },
        Vec::new(),
    );

    // Incoming upload confirmation bridge (HTTP server thread -> UI thread).
    // Multiple concurrent transfers are queued; the UI confirms them one at a
    // time from the front of the queue.
    let (incoming, _set_incoming) = cx.use_async_state(Vec::<IncomingUpload>::new());
    cx.use_effect((), {
        let set_incoming = _set_incoming.clone();
        move || tools::upload_gate::install_upload_setter(set_incoming)
    });

    // When an incoming upload arrives, flash the taskbar if we're in the
    // background so the confirmation dialog gets noticed.
    cx.use_effect((incoming.clone(),), {
        let incoming = incoming.clone();
        move || {
            if !incoming.is_empty() {
                tools::attention::flash_if_background();
            }
        }
    });

    // In-progress transfers (uploads/downloads), shown as progress bars on the
    // Transfer page rows. Populated by the client/server background threads.
    let (transfers, set_transfers) = cx.use_async_state(Vec::<TransferProgress>::new());
    cx.use_effect((), {
        let set_transfers = set_transfers.clone();
        move || tools::transfer_progress::install_progress_setter(set_transfers)
    });

    // Discovered LAN devices, refreshed every 3 seconds in the background.
    let (devices, set_devices) = cx.use_async_state(Vec::<DiscoveredDevice>::new());
    cx.use_effect((), {
        let set_devices = set_devices.clone();
        move || {
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_secs(3));
                let found =
                    tools::mdns::discover_devices(Duration::from_secs(1)).unwrap_or_default();
                set_devices.call(found);
            });
        }
    });

    // Device picked in the footer dropdown; Explorer uploads target this device.
    let (selected_device, set_selected_device) = cx.use_state(None::<DiscoveredDevice>);

    // If the picked device drops off the discovery list (went offline), forget
    // the selection so we never try to send to a stale address.
    cx.use_effect((devices.clone(),), {
        let devices = devices.clone();
        let selected_device = selected_device.clone();
        let set_selected_device = set_selected_device.clone();
        move || {
            let Some(picked) = selected_device.as_ref() else {
                return;
            };
            if !devices.iter().any(|d| d.ip == picked.ip) {
                set_selected_device.call(None);
            }
        }
    });

    // "File sent to the picked device" result, marshalled from the worker thread
    // back to the UI thread. Success is recorded in Transfer history; both
    // outcomes show an InfoBar in Explorer that auto-dismisses after 5 seconds.
    let (upload_result, set_upload_result) = cx.use_async_state(None::<UploadOutcome>);
    cx.use_effect((upload_result.clone(),), {
        let dispatch_transfer = dispatch_transfer.clone();
        let set_upload_result = set_upload_result.clone();
        let result = upload_result.clone();
        move || {
            if let Some(outcome) = &result {
                if let UploadOutcome::Success(name) = outcome {
                    dispatch_transfer.call(TransferAction::MarkUploaded(name.clone()));
                }
                // Auto-dismiss the InfoBar after 5 seconds.
                let clear = set_upload_result.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_secs(5));
                    clear.call(None);
                });
            }
        }
    });

    // App theme (light / dark / follow system), selectable from Settings.
    let (theme, set_theme) = cx.use_state(RequestedTheme::Default);
    cx.use_effect((theme,), move || set_requested_theme(theme));

    // Selected sub-tab on the Transfer page (All / Downloads / Uploads).
    let (transfer_tab, set_transfer_tab) = cx.use_state("all".to_string());

    // Fuzzy-search the current Explorer folder's entries for suggestions.
    let suggestions: Vec<String> = pages::explorer::search_suggestions(&data, &search_text);

    let search_box: Element = auto_suggest_box(&*search_text)
        .placeholder_text("Search files...")
        .items(suggestions)
        .on_text_changed(set_search_text)
        .on_query_submitted({
            let data = data.clone();
            let set_search_target = set_search_target.clone();
            move |query: String| {
                // Submit selects the best fuzzy match in the Explorer list.
                if let Some(i) = pages::explorer::search_best_index(&data, &query) {
                    set_search_target.call(Some(i));
                }
            }
        })
        .on_suggestion_chosen({
            let data = data.clone();
            let set_search_target = set_search_target.clone();
            move |chosen: String| {
                // Choosing a suggestion selects that entry in the Explorer list.
                if let Some(i) = pages::explorer::search_best_index(&data, &chosen) {
                    set_search_target.call(Some(i));
                }
            }
        })
        .into();
    let search_box = search_box.width(652.0);

    let menu_items = [
        NavViewItem::new("Explorer").tag("explorer").icon(Symbol::Folder),
        NavViewItem::new("Transfer").tag("transfer").icon(Symbol::Send),
        NavViewItem::new("Devices").tag("devices").icon(Symbol::Scan),
    ];

    let body: Element = match page.as_str() {
        "explorer" => explorer_page(
            cx,
            set_current_path.clone(),
            &data,
            hovered_index,
            set_hovered_index.clone(),
            selected_device.clone(),
            set_upload_result.clone(),
            &upload_result,
            search_target,
        ),
        "transfer" => transfer_page(
            cx,
            &transfer_history,
            transfer_tab,
            set_transfer_tab.clone(),
            &transfers,
        ),
        "devices" => devices_page(
            cx,
            &devices,
            &tools::mdns::device_hostname(),
            &tools::mdns::local_ip_addrs(),
            env!("CARGO_PKG_VERSION"),
        ),
        _ => settings_page(cx, theme, set_theme.clone()),
    };

    // Confirm incoming uploads (front of the queue): show a dialog; if
    // accepted, pick a folder. After the decision, advance to the next one.
    let dialog = match incoming.first() {
        Some(upload) => {
            let dispatch_transfer = dispatch_transfer.clone();
            let upload_id = upload.id;
            let upload_name = upload.name.clone();
            ContentDialog::new("Incoming upload")
                .content(format!("{}  ·  {} bytes", upload.name, upload.size))
                .primary_button_text("Save…")
                .close_button_text("Cancel")
                .is_open(true)
                .on_closed(move |r: ContentDialogResult| {
                    if r == ContentDialogResult::Primary {
                        // Accepted — record the incoming transfer as downloaded.
                        dispatch_transfer
                            .call(TransferAction::MarkDownloaded(upload_name.clone()));
                        tools::picker::pick_folder(move |dir| {
                            let decision = match dir {
                                Some(path) => UploadDecision::Save(path),
                                None => UploadDecision::Reject,
                            };
                            tools::upload_gate::reply(upload_id, decision);
                            tools::upload_gate::advance();
                        });
                    } else {
                        tools::upload_gate::reply(upload_id, UploadDecision::Reject);
                        tools::upload_gate::advance();
                    }
                })
        }
        None => ContentDialog::new("").is_open(false),
    };

    // Title-bar footer: dropdown listing discovered devices + count badge.
    // Picking a device records it so Explorer uploads target that device.
    // While no devices are found, show an indeterminate ring (searching);
    // once one or more are discovered, show the numeric InfoBadge instead.
    let device_label = selected_device
        .as_ref()
        .map(device_text)
        .unwrap_or_else(|| "Devices".to_string());

    let count_badge: Element = if devices.is_empty() {
        ProgressRing::indeterminate()
            .width(18.0)
            .height(18.0)
            .into()
    } else {
        InfoBadge::numeric(devices.len() as i32).into()
    };

    let footer: Element = hstack((
        drop_down_button(device_label)
            .menu_flyout(
                devices
                    .iter()
                    .map(|d| menu_item(device_text(d)))
                    .collect(),
            )
            .on_item_clicked({
                let devices = devices.clone();
                let set_selected_device = set_selected_device.clone();
                move |text: String| {
                    if let Some(d) = devices.iter().find(|d| device_text(d) == text) {
                        set_selected_device.call(Some(d.clone()));
                    }
                }
            }),
        count_badge,
    ))
    .spacing(8.0)
    .into();

    grid((
        TitleBar::new("Tumbleweed")
            .tall(true)
            .content(search_box)
            .footer(footer)
            .grid_row(0),
        NavigationView::new(menu_items, body)
            .selected_tag(page.clone())
            .on_selection_changed(set_page)
            .pane_display_mode(NavigationViewPaneDisplayMode::Left)
            .back_button_visible(false)
            .pane_toggle_button_visible(true)
            .settings_visible(true)
            .grid_row(1),
        dialog.grid_row(1),
    ))
    .rows([GridLength::Auto, GridLength::STAR])
    .into()
}

fn main() -> Result<()> {
    bootstrap()?;

    // The app is the server: advertise "tumbleweed.local" over mDNS and serve
    // the Explorer folder over HTTP, both on background threads.
    std::thread::spawn(|| {
        let host = tools::mdns::device_hostname();
        if let Err(e) = tools::mdns::advertise(&host, tools::server::HTTP_PORT) {
            // No console in GUI-subsystem builds, so log to a file instead.
            tools::mdns::log_msg(&format!("[mdns] advertise error: {e}"));
        }
    });
    std::thread::spawn(|| {
        if let Err(e) = tools::server::serve(tools::server::HTTP_PORT) {
            println!("[server] serve error: {e}");
        }
    });

    App::new()
        .backdrop(Backdrop::Mica)
        .render(app)
}
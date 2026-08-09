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
use pages::transfer::{
    load_history, save_history, transfer_page, TransferAction, TransferDirection, TransferRecord,
};
use tools::mdns::DiscoveredDevice;
use tools::settings_store::{load_theme, save_theme};
use tools::transfer_progress::TransferProgress;
use tools::upload_gate::{IncomingUpload, UploadDecision};

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

    // Persist the folder and keep the SSH (SFTP) server pointing at the same
    // folder the user is currently exploring.
    cx.use_effect((current_path.clone(),), {
        let current_path = current_path.clone();
        let set_search_target = set_search_target.clone();
        move || {
            save_last_folder(&current_path);
            tools::ssh_server::set_share_root(current_path.clone());
            // A folder change invalidates any previously searched row index.
            set_search_target.call(None);
        }
    });

    // Listing + breadcrumbs recompute only when the folder changes.
    let data = cx.use_memo((current_path.clone(),), || view_data(&current_path));

    // Explorer's selected row (reveals its upload button) and the last-tap
    // time used for double-click folder navigation. These live at app level
    // because every page shares one positional hook cursor — a hook created
    // inside a page function would collide with another page's hooks (e.g.
    // Settings) and break switching to it.
    let (selected_index, set_selected_index) = cx.use_state(None::<usize>);
    let last_tap = cx.use_ref((None::<usize>, std::time::Instant::now()));

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
        // Load the history persisted by a previous launch.
        load_history(),
    );

    // Persist the transfer history whenever it changes.
    cx.use_effect((transfer_history.clone(),), {
        let transfer_history = transfer_history.clone();
        move || save_history(&transfer_history)
    });

    // Incoming upload confirmation bridge (SSH server thread -> UI thread).
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

    // Online/offline status of paired devices, checked over SSH (`tumbleweed
    // ping`) every 15 s in the background, so the Devices page stays fresh even
    // when a phone comes/goes offline.
    let (paired_online, set_paired_online) =
        cx.use_async_state((0u64, std::collections::HashMap::<String, bool>::new()));
    cx.use_effect((), {
        let set_paired_online = set_paired_online.clone();
        move || {
            std::thread::spawn(move || {
                let mut generation = 0u64;
                let mut last: std::collections::HashMap<String, bool> =
                    std::collections::HashMap::new();
                loop {
                    let ips = tools::ssh_server::paired_device_ips();
                    let map: std::collections::HashMap<String, bool> = ips
                        .iter()
                        .map(|ip| (ip.clone(), tools::ssh_send::is_online(ip)))
                        .collect();
                    if map != last {
                        generation += 1;
                        last = map.clone();
                        set_paired_online.call((generation, map));
                    }
                    std::thread::sleep(Duration::from_secs(15));
                }
            });
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
    // Loaded from and saved back to the settings file.
    let (theme, set_theme) = cx.use_state(load_theme());
    cx.use_effect((theme,), {
        move || {
            set_requested_theme(theme);
            save_theme(theme);
        }
    });

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
            &devices,
            selected_index,
            set_selected_index.clone(),
            last_tap.clone(),
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
            &tools::mdns::device_host_name(),
            &tools::mdns::local_ip_addrs(),
            env!("CARGO_PKG_VERSION"),
            &paired_online.1,
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
                .content(if upload.size > 0 {
                    format!("{}  ·  {} bytes", upload.name, upload.size)
                } else {
                    // SFTP opens don't carry the file size up front.
                    format!("{}  ·  (size unknown)", upload.name)
                })
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

    grid((
        TitleBar::new("Tumbleweed")
            .tall(true)
            .content(search_box)
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
    // Bridge the `log` crate (used by russh) into the app's diagnostics file.
    static LOGGER: tools::ssh_server::FileLogger = tools::ssh_server::FileLogger;
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(log::LevelFilter::Debug);

    bootstrap()?;

    // Honour the persisted mDNS toggle from the Settings page (default on).
    tools::mdns::set_mdns_enabled(tools::settings_store::load_mdns_enabled());

    // The app is the server: advertise "tumbleweed.local" over mDNS (SSH port
    // 2222, type "pc", this app's version in the TXT record) on a background
    // thread. SSH handles both directions — phones push files over SFTP here,
    // and the Explorer page pushes files to phones via `tools::ssh_send`.
    std::thread::spawn(|| {
        let host = tools::mdns::device_hostname();
        if let Err(e) = tools::mdns::advertise(
            &host,
            tools::ssh_server::SSH_PORT,
            "pc",
            env!("CARGO_PKG_VERSION"),
        ) {
            // No console in GUI-subsystem builds, so log to a file instead.
            tools::mdns::log_msg(&format!("[mdns] advertise error: {e}"));
        }
    });
    // Embedded SSH server for SFTP uploads from paired phones.
    tools::ssh_server::start();

    App::new()
        .backdrop(Backdrop::Mica)
        .render(app)
}
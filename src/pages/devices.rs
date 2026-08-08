use windows_reactor::*;

use crate::tools::mdns::DiscoveredDevice;

/// This app is a Windows app, so the "This device" entry is always a PC.
const THIS_DEVICE_KIND: &str = "pc";

/// Segoe Fluent Icons glyph for a device type.
fn kind_icon(kind: &str) -> &'static str {
    match kind {
        "phone" => "\u{E8EA}", // Smartphone
        _ => "\u{E977}",       // DesktopPC
    }
}

/// A card describing one device in three columns: icon | name + IP | version.
/// `online` (Some) appends an Online/Offline indicator to the trailing column,
/// used for paired devices to show their SSH reachability.
fn device_card(
    name: &str,
    kind: &str,
    version: &str,
    ips: &[String],
    online: Option<bool>,
) -> Element {
    let mut trailing: Vec<Element> = vec![
        TextBlock::new("\u{E890}") // Tag
            .font_family("Segoe Fluent Icons")
            .font_size(14.0)
            .vertical_alignment(VerticalAlignment::Center)
            .into(),
        TextBlock::new(format!("v{version}"))
            .font_size(12.0)
            .vertical_alignment(VerticalAlignment::Center)
            .into(),
    ];
    if let Some(on) = online {
        let (label, color) = if on {
            ("Online", Color { a: 255, r: 76, g: 175, b: 80 })
        } else {
            ("Offline", Color { a: 255, r: 150, g: 150, b: 150 })
        };
        trailing.push(
            TextBlock::new(format!("\u{25CF} {label}"))
                .font_size(12.0)
                .foreground(color)
                .vertical_alignment(VerticalAlignment::Center)
                .into(),
        );
    }
    border(
        grid((
            TextBlock::new(kind_icon(kind))
                .font_family("Segoe Fluent Icons")
                .font_size(28.0)
                .vertical_alignment(VerticalAlignment::Center)
                .grid_column(0),
            grid((
                body(name).grid_row(0),
                caption(ips.join(", ")).grid_row(1),
            ))
            .rows([GridLength::Auto, GridLength::Auto])
            .vertical_alignment(VerticalAlignment::Center)
            .grid_column(1),
            hstack(trailing)
                .spacing(4.0)
                .vertical_alignment(VerticalAlignment::Center)
                .grid_column(2),
        ))
        .columns([GridLength::Auto, GridLength::STAR, GridLength::Auto])
        .column_spacing(12.0)
        .padding(Thickness::uniform(16.0)),
    )
    .corner_radius(4.0)
    // Card fill: ControlFillColorDefaultBrush; stroke: CardStrokeColorDefaultBrush.
    .background(ThemeRef::ControlFill)
    .border_brush(ThemeRef::CardStroke)
    .border_thickness(Thickness::uniform(1.0))
    .into()
}

/// A card for a discovered peer, with the kind/version from its mDNS TXT
/// record (a blank version is shown as "unknown"). `online` is the paired
/// device's SSH reachability (None for this device / new devices).
fn peer_card(d: &DiscoveredDevice, online: Option<bool>) -> Element {
    let version = if d.version.is_empty() {
        "unknown"
    } else {
        &d.version
    };
    device_card(&d.name, &d.kind, version, &[d.ip.to_string()], online)
}

/// The Devices page: this device, then the devices already paired with this
/// PC, then the new devices discovered on the LAN. Renders inside the app's
/// shared [`RenderCx`]; the discovered list is owned by `app` and passed in.
pub(crate) fn devices_page(
    cx: &mut RenderCx,
    devices: &[DiscoveredDevice],
    this_name: &str,
    this_ips: &[String],
    this_version: &str,
) -> Element {
    // This device card — always shown as a PC (it's a Windows app).
    let this_card = device_card(this_name, THIS_DEVICE_KIND, this_version, this_ips, None);

    // Online status for paired devices, checked over SSH (`tumbleweed ping`)
    // in the background and keyed by IP.
    let paired_ips = crate::tools::ssh_server::paired_device_ips();
    let (online, set_online) =
        cx.use_async_state((0u64, std::collections::HashMap::<String, bool>::new()));
    let generation = online.0;
    cx.use_effect((paired_ips.clone(),), {
        let set_online = set_online.clone();
        let paired_ips = paired_ips.clone();
        move || {
            if paired_ips.is_empty() {
                return;
            }
            std::thread::spawn(move || {
                let map: std::collections::HashMap<String, bool> = paired_ips
                    .iter()
                    .map(|ip| (ip.clone(), crate::tools::ssh_send::is_online(ip)))
                    .collect();
                set_online.call((generation + 1, map));
            });
        }
    });

    // Split the discovered peers into already-paired (registered a key during
    // pairing) and new ones. A device that already paired is not "new".
    let (paired, new): (Vec<&DiscoveredDevice>, Vec<&DiscoveredDevice>) = devices
        .iter()
        .partition(|d| paired_ips.iter().any(|ip| ip == &d.ip.to_string()));

    // Section headers live above each group; all cards go into one vstack.
    let mut children: Vec<Element> = Vec::new();
    children.push(body_strong("This device").into());
    children.push(this_card);

    if !paired.is_empty() {
        children.push(body_strong("Paired devices").into());
        for d in paired {
            let on = online.1.get(&d.ip.to_string()).copied();
            children.push(peer_card(d, on));
        }
    }

    children.push(body_strong("New devices").into());
    if new.is_empty() {
        children.push(
            TextBlock::new("No new devices found on the LAN.")
                .font_size(13.0)
                .foreground(Color { a: 255, r: 150, g: 150, b: 150 })
                .padding(Thickness::uniform(8.0))
                .into(),
        );
    } else {
        for d in new {
            children.push(peer_card(d, None));
        }
    }

    let stack: Element = vstack(children).spacing(8.0).into();
    let scroll: Element = scroll_view(stack).into();

    vstack((
            title("Devices").margin(Thickness {
                left: 0.0,
                top: 0.0,
                right: 0.0,
                bottom: 12.0,
            }),
            scroll
        ))
        .margin(Thickness {
            left: 36.0,
            right: 36.0,
            top: 24.0,
            bottom: 0.0,
        })
        .into()
}

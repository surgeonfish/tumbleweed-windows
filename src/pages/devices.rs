use windows_reactor::*;

use crate::tools::mdns::DiscoveredDevice;

/// This app is a Windows app, so the "This device" entry is always a PC.
const THIS_DEVICE_KIND: &str = "pc";

/// Segoe Fluent Icons glyph for a device type.
pub(crate) fn kind_icon(kind: &str) -> &'static str {
    match kind {
        "phone" => "\u{E8EA}", // Smartphone
        _ => "\u{E977}",       // DesktopPC
    }
}

/// A card describing one device in three columns: icon | name + IP | version.
/// `online` (Some) appends an Online/Offline indicator to the trailing column,
/// used for paired devices to show their SSH reachability. The shell is the
/// shared `simple_card`; only the icon size (28px) and trailing differ.
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
    crate::controls::simple_card(
        kind_icon(kind),
        20.0,
        name,
        ips.join(", "),
        hstack(trailing).spacing(4.0),
    )
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
    _cx: &mut RenderCx,
    devices: &[DiscoveredDevice],
    this_name: &str,
    this_ips: &[String],
    this_version: &str,
    online: &std::collections::HashMap<String, bool>,
) -> Element {
    // This device card — always shown as a PC (it's a Windows app).
    let this_card = device_card(this_name, THIS_DEVICE_KIND, this_version, this_ips, None);

    // Split the discovered peers into already-paired (registered a key during
    // pairing) and new ones. A device that already paired is not "new". The
    // paired devices' online status comes from `app` (refreshed periodically).
    let paired_ips = crate::tools::ssh_server::paired_device_ips();
    let (paired, new): (Vec<&DiscoveredDevice>, Vec<&DiscoveredDevice>) = devices
        .iter()
        .partition(|d| paired_ips.iter().any(|ip| ip == &d.ip.to_string()));

    // Each group is a titled section; the shared `section` widget renders the
    // heading and its cards together.
    let mut sections: Vec<Element> = Vec::new();
    sections.push(crate::controls::section("This device", vec![this_card]));

    if !paired.is_empty() {
        let cards: Vec<Element> = paired
            .into_iter()
            .map(|d| peer_card(d, online.get(&d.ip.to_string()).copied()))
            .collect();
        sections.push(crate::controls::section("Paired devices", cards));
    }

    let new_cards: Vec<Element> = if new.is_empty() {
        vec![
            TextBlock::new("No new devices found on the LAN.")
                .font_size(13.0)
                .foreground(Color { a: 255, r: 150, g: 150, b: 150 })
                .padding(Thickness::uniform(8.0))
                .into(),
        ]
    } else {
        new.into_iter().map(|d| peer_card(d, None)).collect()
    };
    sections.push(crate::controls::section("New devices", new_cards));

    let stack: Element = vstack(sections).spacing(8.0).into();
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

use windows_reactor::*;

use crate::tools::mdns::DiscoveredDevice;

/// Human-readable label for a device type reported over HTTP `/info`.
fn kind_label(kind: &str) -> &'static str {
    match kind {
        "pc" => "PC",
        "phone" => "Phone",
        _ => "Unknown",
    }
}

/// Segoe Fluent Icons glyph for a device type.
fn kind_icon(kind: &str) -> &'static str {
    match kind {
        "phone" => "\u{E8EA}", // Smartphone
        _ => "\u{E77B}",       // DesktopPC
    }
}

/// One labeled `label : value` line used inside a device card.
fn info_row(label: &str, value: &str) -> Element {
    grid((
        TextBlock::new(label)
            .foreground(Color { a: 255, r: 130, g: 130, b: 130 })
            .grid_column(0),
        TextBlock::new(value).grid_column(1),
    ))
    .columns([GridLength::Auto, GridLength::STAR])
    .into()
}

/// A card describing one device: icon, name, subtitle, then IP / type /
/// version rows.
fn device_card(name: &str, kind: &str, version: &str, ips: &[String], subtitle: &str) -> Element {
    grid((
        TextBlock::new(kind_icon(kind))
            .font_family("Segoe Fluent Icons")
            .font_size(28.0)
            .vertical_alignment(VerticalAlignment::Top)
            .grid_column(0),
        grid((
            TextBlock::new(name).font_size(16.0).grid_row(0),
            TextBlock::new(subtitle)
                .font_size(12.0)
                .foreground(Color { a: 255, r: 130, g: 130, b: 130 })
                .grid_row(1),
            info_row("IP", &ips.join(", ")).grid_row(2),
            info_row("Type", kind_label(kind)).grid_row(3),
            info_row("Version", version).grid_row(4),
        ))
        .rows([
            GridLength::Auto,
            GridLength::Auto,
            GridLength::Auto,
            GridLength::Auto,
            GridLength::Auto,
        ])
        .grid_column(1),
    ))
    .columns([GridLength::Auto, GridLength::STAR])
    .padding(Thickness::uniform(16.0))
    .background(Color { a: 0, r: 0, g: 0, b: 0 })
    .into()
}

/// The Devices page: this device plus every other tumbleweed device found on
/// the LAN. Renders inside the app's shared [`RenderCx`]; the discovered list
/// is owned by `app` and passed in.
pub(crate) fn devices_page(
    _cx: &mut RenderCx,
    devices: &[DiscoveredDevice],
    this_name: &str,
    this_ips: &[String],
    this_version: &str,
) -> Element {
    // "This device" card first, then the discovered peers.
    let mut cards: Vec<Element> = Vec::with_capacity(devices.len() + 1);
    cards.push(device_card(
        this_name,
        "pc",
        this_version,
        this_ips,
        "This device",
    ));
    for d in devices {
        let version = if d.version.is_empty() {
            "unknown"
        } else {
            &d.version
        };
        cards.push(device_card(
            &d.name,
            &d.kind,
            version,
            &[d.ip.to_string()],
            "Discovered on LAN",
        ));
    }

    let stack: Element = vstack(cards).spacing(8.0).into();
    scroll_view(stack)
        .margin(Thickness::uniform(12.0))
        .into()
}

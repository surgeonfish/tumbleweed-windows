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
fn device_card(name: &str, kind: &str, version: &str, ips: &[String]) -> Element {
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
            hstack((
                TextBlock::new("\u{E890}") // Tag
                    .font_family("Segoe Fluent Icons")
                    .font_size(14.0)
                    .vertical_alignment(VerticalAlignment::Center),
                TextBlock::new(format!("v{version}"))
                    .font_size(12.0)
                    .vertical_alignment(VerticalAlignment::Center),
            ))
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

/// The Devices page: this device in one section, other tumbleweed devices in
/// another. Renders inside the app's shared [`RenderCx`]; the discovered list
/// is owned by `app` and passed in.
pub(crate) fn devices_page(
    _cx: &mut RenderCx,
    devices: &[DiscoveredDevice],
    this_name: &str,
    this_ips: &[String],
    this_version: &str,
) -> Element {
    // This device card — always shown as a PC (it's a Windows app).
    let this_card = device_card(this_name, THIS_DEVICE_KIND, this_version, this_ips);

    // Other device cards (or an empty hint).
    let mut other_cards: Vec<Element> = Vec::new();
    if devices.is_empty() {
        other_cards.push(
            TextBlock::new("No other devices found on the LAN.")
                .font_size(13.0)
                .foreground(Color { a: 255, r: 150, g: 150, b: 150 })
                .padding(Thickness::uniform(8.0))
                .into(),
        );
    } else {
        for d in devices {
            let version = if d.version.is_empty() {
                "unknown"
            } else {
                &d.version
            };
            other_cards.push(device_card(&d.name, &d.kind, version, &[d.ip.to_string()]));
        }
    }

    // Section headers live above each group; all cards go into one vstack.
    let mut children: Vec<Element> = Vec::new();
    children.push(body_strong("This device").into());
    children.push(this_card);
    children.push(body_strong("Other devices").into());
    children.extend(other_cards);

    let stack: Element = vstack(children).spacing(8.0).into();
    scroll_view(stack)
        .margin(Thickness::uniform(12.0))
        .into()
}

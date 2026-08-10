use windows_reactor::*;

/// The Settings page. It renders inside the app's shared [`RenderCx`] (the same
/// one every page uses); any shared state is owned by `app` and passed in. This
/// page deliberately creates no hooks of its own: every page shares one
/// positional hook cursor, so a page function creating hooks (as this one used
/// to) collided with the other pages and made navigation away from Settings
/// flaky.
pub(crate) fn settings_page(
    _cx: &mut RenderCx,
    theme: RequestedTheme,
    set_theme: SetState<RequestedTheme>,
    accent_gen: u32,
    pairing: &(u64, Option<crate::tools::ssh_pair::PairingInfo>),
    set_pairing: AsyncSetState<(u64, Option<crate::tools::ssh_pair::PairingInfo>)>,
    mdns_on: bool,
    set_mdns_on: SetState<bool>,
) -> Element {
    // The pairing generation counter is bumped on every regeneration so the QR
    // image element gets a fresh key and re-renders.
    let generation = pairing.0;

    let ssh_card = crate::controls::simple_card(
        "\u{E72E}", // shield / lock
        20.0,
        "SSH key pair",
        "Generate an SSH key pair so your phone can pair with this PC.",
        button("Generate").on_click({
            let set_pairing = set_pairing.clone();
            let next_gen = generation + 1;
            move || {
                // Clone so the outer (Fn) callback stays callable and the clone
                // can be moved into the worker thread (it is Send).
                let set_pairing = set_pairing.clone();
                std::thread::spawn(move || {
                    // Always mint a fresh identity, then rebuild the pairing QR.
                    let _ = crate::tools::ssh_pair::regenerate_keypair();
                    let info = crate::tools::ssh_pair::build_pairing_info();
                    set_pairing.call((next_gen, Some(info)));
                });
            }
        }),
    );

    let qr_child: Element = match &pairing.1 {
        None => text_block::caption(
            "Tap \"Generate key pair\" first, then expand this to see the QR code.",
        )
        .into(),
        Some(info) if info.error.is_some() => {
            text_block::caption(info.error.clone().unwrap_or_default()).into()
        }
        Some(info) => {
            let qr_el: Element = match &info.matrix {
                Some(m) => {
                    let m = m.clone();
                    let size = info.size;
                    let size_dips = crate::tools::qr_surface::qr_size(size);
                    // `canvas` is demand-driven: it only paints on mount,
                    // resize, or a key change, so an idle QR does no GPU/CPU
                    // work (an `animated_canvas` repaints every frame and was
                    // hammering the accent-color WinRT query). It paints on
                    // mount, so it never comes up blank after switching tabs.
                    // Keyed by generation (regeneration) and the color scheme
                    // (theme change) so either one remounts and repaints with
                    // the current accent/ControlFill colors.
                    canvas(move |ctx| crate::tools::qr_surface::draw_qr(ctx, &m, size))
                        .with_key(format!(
                            "qr-{generation}-{accent_gen}-{}",
                            matches!(
                                current_color_scheme(),
                                windows_reactor::ColorScheme::Dark
                            )
                        ))
                        .width(size_dips)
                        .height(size_dips)
                        .into()
                }
                None => text_block::caption("Rendering QR code…").into(),
            };
            let meta = format!(
                "{}, {}, {}, v{}",
                info.name.as_deref().unwrap_or(""),
                info.device_type.as_deref().unwrap_or(""),
                info.ip.as_deref().unwrap_or(""),
                info.version.as_deref().unwrap_or(""),
            );
            grid((
                // QR code is in column 0, the key pair info is in column 1,
                // and the meta info is in column 2.
                qr_el.grid_column(0),
                hstack((
                    TextBlock::new("\u{E8D7}")
                        .font_family("Segoe Fluent Icons")
                        .font_size(16.0)
                        .vertical_alignment(VerticalAlignment::Center)
                        .grid_column(0),
                    vstack((
                        body_strong("Key Pair"),
                        caption("Generated")
                    ))
                    .spacing(4.0)
                    .vertical_alignment(VerticalAlignment::Center),
                ))
                .spacing(8.0)
                .horizontal_alignment(HorizontalAlignment::Right)
                .grid_column(1),
                hstack((
                    TextBlock::new("\u{E928}")
                        .font_family("Segoe Fluent Icons")
                        .font_size(16.0)
                        .vertical_alignment(VerticalAlignment::Center)
                        .grid_column(0),
                    vstack((
                        body_strong("Meta"),
                        caption(meta)
                    ))
                    .spacing(4.0)
                    .vertical_alignment(VerticalAlignment::Center),
                ))
                .spacing(8.0)
                .horizontal_alignment(HorizontalAlignment::Right)
                .grid_column(2),
            ))
            .columns([GridLength::Auto, GridLength::STAR, GridLength::Auto])
            .column_spacing(40.0)
            .into()
        }
    };

    // ---- Discovery section ----
    // Live toggle for mDNS advertising + discovery, persisted in settings.
    let mdns_card = crate::controls::simple_card(
        "\u{E701}", // Wifi
        20.0,
        "mDNS discovery",
        "Advertise this PC and find nearby Tumbleweed devices on your network.",
        ToggleSwitch::new(mdns_on).on_toggled({
            let set_mdns_on = set_mdns_on.clone();
            move |on: bool| {
                crate::tools::mdns::set_mdns_enabled(on);
                crate::tools::settings_store::save_mdns_enabled(on);
                set_mdns_on.call(on);
            }
        }),
    );

    // ---- Appearance section ----
    // Theme selector: a drop-down button whose menu offers Follow system /
    // Light / Dark; persisted in settings.
    let theme_label = match theme {
        RequestedTheme::Light => "Light",
        RequestedTheme::Dark => "Dark",
        _ => "System",
    };
    let theme_card = crate::controls::simple_card(
        "\u{E790}",
        20.0,
        "Theme",
        "Choose the color theme for this app.",
        drop_down_button(theme_label)
            .menu_flyout(vec![
                menu_item("System"),
                menu_item("Light"),
                menu_item("Dark"),
            ])
            .on_item_clicked({
                let set_theme = set_theme.clone();
                move |label: String| {
                    let next = match label.as_str() {
                        "Light" => RequestedTheme::Light,
                        "Dark" => RequestedTheme::Dark,
                        _ => RequestedTheme::Default,
                    };
                    set_theme.call(next);
                }
            }),
    );

    // The cards live inside the scroll view; the page title sits above them.
    let card_stack: Element = vstack((
        crate::controls::section("SSH pairing", vec![ssh_card]),
        crate::controls::section("Discovery", vec![mdns_card]),
        crate::controls::section("Appearance", vec![theme_card]),
    ))
    .spacing(8.0)
    .into();
    let scroll: Element = scroll_view(card_stack).into();

    vstack((
        title("Settings").margin(Thickness {
            left: 0.0,
            top: 0.0,
            right: 0.0,
            bottom: 12.0,
        }),
        qr_child,
        scroll,
    ))
    .margin(Thickness {
        left: 36.0,
        right: 36.0,
        top: 24.0,
        bottom: 0.0,
    })
    .into()
}

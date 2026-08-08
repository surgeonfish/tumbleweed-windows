use windows_reactor::*;

/// A reusable settings card: icon on the left (column 0), title over caption
/// (column 1), and an optional trailing control such as a `ComboBox` on the
/// right (column 2). Future settings can build their rows with this layout.
pub(crate) fn simple_card(
    icon: &str,
    title: impl Into<String>,
    caption: impl Into<String>,
    trailing: impl Into<Element>,
) -> Element {
     border(
        grid((
            TextBlock::new(icon)
                .font_family("Segoe Fluent Icons")
                .font_size(20.0)
                .vertical_alignment(VerticalAlignment::Center)
                .grid_column(0),
            grid((
                text_block::body(title).grid_row(0),
                text_block::caption(caption).grid_row(1),
            ))
            .rows([GridLength::Auto, GridLength::Auto])
            .vertical_alignment(VerticalAlignment::Center)
            .grid_column(1),
            trailing
                .into()
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

/// The Settings page. It renders inside the app's shared [`RenderCx`] (the same
/// one every page uses); any shared state is owned by `app` and passed in.
pub(crate) fn settings_page(
    cx: &mut RenderCx,
    theme: RequestedTheme,
    set_theme: SetState<RequestedTheme>,
) -> Element {
    // ---- SSH pairing section ----
    // State: (generation counter, pairing result). The counter is bumped on
    // every generation so the QR image element gets a fresh key and re-renders.
    // use_async_state: its setter is Send, so it can be called from the
    // background thread that does the keygen + QR work.
    let (pairing, set_pairing) =
        cx.use_async_state((0u64, None::<crate::tools::ssh_pair::PairingInfo>));
    let generation = pairing.0;

    // On page load, if a key pair already exists in the app's folder, load it
    // and show its QR without requiring the user to regenerate.
    cx.use_effect((), {
        let set_pairing = set_pairing.clone();
        move || {
            if crate::tools::ssh_pair::has_keypair() {
                std::thread::spawn(move || {
                    let info = crate::tools::ssh_pair::build_pairing_info();
                    set_pairing.call((1u64, Some(info)));
                });
            }
        }
    });

    let ssh_title = TextBlock::new("SSH pairing")
        .font_size(20.0)
        .margin(Thickness {
            left: 0.0,
            top: 16.0,
            right: 0.0,
            bottom: 4.0,
        });

    let ssh_card = simple_card(
        "\u{E72E}", // shield / lock
        "SSH key pair",
        "Generate an SSH key pair so your phone can pair with this PC.",
        button("Generate key pair").on_click({
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
                    // `animated_canvas` repaints every frame on the UI thread, so
                    // the QR picks up a system accent-color change immediately
                    // (the draw closure reads the accent each frame) without the
                    // need for a cross-thread event subscription. It still paints
                    // on mount, so the QR never comes up blank after switching
                    // tabs. Keyed by generation so a regenerated key pair gets a
                    // fresh canvas.
                    animated_canvas(move |ctx| crate::tools::qr_surface::draw_qr(ctx, &m, size))
                        .with_key(format!("qr-{generation}"))
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
                border(qr_el)
                    .corner_radius(16.0)
                    .background(ThemeRef::ControlFill)
                    .border_brush(ThemeRef::CardStroke)
                    .border_thickness(Thickness::uniform(1.0))
                    .grid_column(0),
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
    let discovery_title = TextBlock::new("Discovery")
        .font_size(20.0)
        .margin(Thickness {
            left: 0.0,
            top: 16.0,
            right: 0.0,
            bottom: 4.0,
        });
    // Live toggle for mDNS advertising + discovery, persisted in settings.
    let (mdns_on, set_mdns_on) = cx.use_state(crate::tools::mdns::mdns_enabled());
    let mdns_card = simple_card(
        "\u{E701}", // Wifi
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
    let appearance_title = TextBlock::new("Appearance")
        .font_size(20.0)
        .margin(Thickness {
            left: 0.0,
            top: 16.0,
            right: 0.0,
            bottom: 4.0,
        });
    // Theme ComboBox: Follow system / Light / Dark.
    let theme_index = match theme {
        RequestedTheme::Light => 1,
        RequestedTheme::Dark => 2,
        _ => 0,
    };
    let theme_card = simple_card(
        "\u{E790}",
        "Appearance",
        "Choose the color theme for this app.",
        ComboBox::new(["Follow system", "Light", "Dark"])
            .selected_index(theme_index)
            .on_selection_changed({
                let set_theme = set_theme.clone();
                move |idx: i32| {
                    let next = match idx {
                        1 => RequestedTheme::Light,
                        2 => RequestedTheme::Dark,
                        _ => RequestedTheme::Default,
                    };
                    set_theme.call(next);
                }
            }),
    );

    // The cards live inside the scroll view; the page title sits above them.
    let card_stack: Element = vstack((
        ssh_title,
        ssh_card,
        discovery_title,
        mdns_card,
        appearance_title,
        theme_card,
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

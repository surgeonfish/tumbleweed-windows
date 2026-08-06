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
                .font_size(26.0)
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
    _cx: &mut RenderCx,
    theme: RequestedTheme,
    set_theme: SetState<RequestedTheme>,
) -> Element {
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

    let stack: Element = vstack((theme_card,)).spacing(8.0).into();
    scroll_view(stack)
        .margin(Thickness::uniform(12.0))
        .into()
}

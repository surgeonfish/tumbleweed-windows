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
    grid((
        TextBlock::new(icon)
            .font_family("Segoe Fluent Icons")
            .font_size(26.0)
            .vertical_alignment(VerticalAlignment::Center)
            .grid_column(0),
        grid((
            TextBlock::new(title).grid_row(0),
            TextBlock::new(caption)
                .font_size(12.0)
                .foreground(Color { a: 255, r: 130, g: 130, b: 130 })
                .grid_row(1),
        ))
        .rows([GridLength::Auto, GridLength::Auto])
        .vertical_alignment(VerticalAlignment::Center)
        .margin(Thickness {
            left: 12.0,
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
        })
        .grid_column(1),
        trailing
            .into()
            .vertical_alignment(VerticalAlignment::Center)
            .grid_column(2),
    ))
    .columns([GridLength::Auto, GridLength::STAR, GridLength::Auto])
    .padding(Thickness::uniform(16.0))
    .background(Color { a: 0, r: 0, g: 0, b: 0 })
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

    list_view(vec![theme_card], |card, _idx| card.clone())
        .with_key_selector(|_| "appearance".to_string())
        .build()
        .margin(Thickness::uniform(12.0))
}

use windows_reactor::*;

/// A reusable card: icon on the left (column 0), title over caption (column 1),
/// and an optional trailing control such as a `ComboBox` or `Button` on the
/// right (column 2). Shared by the Settings and Devices pages.
/// `icon_size` lets callers size their icon independently (e.g. device cards
/// use 28.0, settings cards use 20.0).
pub(crate) fn simple_card(
    icon: &str,
    icon_size: f64,
    title: impl Into<String>,
    caption: impl Into<String>,
    trailing: impl Into<Element>,
) -> Element {
    border(
        grid((
            TextBlock::new(icon)
                .font_family("Segoe Fluent Icons")
                .font_size(icon_size)
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

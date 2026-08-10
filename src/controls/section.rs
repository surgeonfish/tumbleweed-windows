use windows_reactor::*;

/// A titled section: a bold section heading with a vertical stack of cards
/// beneath it. Shared by the Settings and Devices pages (and any future page
/// that groups content into headings with cards).
pub(crate) fn section(title: impl Into<String>, cards: Vec<Element>) -> Element {
    vstack((
        body_strong(title).margin(Thickness {
            left: 0.0,
            top: 16.0,
            right: 0.0,
            bottom: 4.0,
        }),
        vstack(cards).spacing(8.0),
    ))
    .spacing(8.0)
    .into()
}

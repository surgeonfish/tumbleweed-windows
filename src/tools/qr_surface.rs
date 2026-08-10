//! Render the SSH-pairing QR code into a reactor `canvas` (SwapChainPanel)
//! widget via the `windows-canvas` drawing session. Everything stays in memory —
//! no temporary file is ever written. The module matrix comes from `ssh_pair`
//! and is repainted whenever the key pair changes.
//!
//! Why a canvas and not an `Image` with a `SurfaceImageSource`? A
//! `SurfaceImageSource` displayed through `Image` needs the XAML compositor's
//! own D3D device; this pinned windows-rs rev can't hand that out
//! (`ICompositorInterop::GetD3DDevice` is not exposed), and using a separately
//! created device crashes XAML with a stowed exception the first time the
//! surface is composed. A `SwapChainPanel`-backed canvas, by contrast, is
//! designed to accept a caller-created device and is the reactor's most
//! exercised GPU path.

use windows::UI::ViewManagement::{UIColorType, UISettings};
use windows_canvas::{ColorF, Rect};
use windows_reactor::{ColorScheme, DrawContext, Result, current_color_scheme};

/// QR quiet zone, in modules.
const QUIET: f32 = 4.0;
/// DIPs per module — 2 keeps the QR compact but still crisp and scannable.
const SCALE: f32 = 2.0;
/// Module size of the placeholder QR shown before a key pair exists. Chosen
/// near the middle of typical QR versions so the placeholder reads at roughly
/// the same on-screen scale as the real code.
const PLACEHOLDER_SIZE: usize = 53;

/// The app's accent color (Windows Settings -> Personalization) as a
/// Direct2D color, so the QR modules match the app's accent. Falls back to
/// black if the accent can't be queried.
fn accent_color() -> ColorF {
    match UISettings::new().and_then(|u| u.GetColorValue(UIColorType::Accent)) {
        Ok(c) => ColorF {
            r: c.R as f32 / 255.0,
            g: c.G as f32 / 255.0,
            b: c.B as f32 / 255.0,
            a: 1.0,
        },
        Err(_) => ColorF {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        },
    }
}

/// The WinUI `ControlFillColorDefaultBrush` color for the current theme, as a
/// Direct2D color. This is the solid color behind the QR modules (the same
/// fill the container border gets from `ThemeRef::ControlFill`), so the QR
/// always renders on a ControlFill card.
fn control_fill_color() -> ColorF {
    match current_color_scheme() {
        ColorScheme::Dark => ColorF::from_rgb8(43, 43, 43),
        ColorScheme::Light => ColorF::from_rgb8(243, 243, 243),
    }
}

/// Draw `matrix` (row-major, `size` x `size`) into `ctx`: a ControlFill
/// background with accent-colored modules. Call from a `canvas`/`canvas`
/// draw callback.
pub(crate) fn draw_qr(ctx: &DrawContext, matrix: &[bool], size: usize) -> Result<()> {
    ctx.clear(control_fill_color());
    let brush = ctx.create_solid_brush(accent_color())?;
    for y in 0..size {
        for x in 0..size {
            if matrix[y * size + x] {
                let rect = Rect::new(
                    (QUIET + x as f32) * SCALE,
                    (QUIET + y as f32) * SCALE,
                    (QUIET + x as f32 + 1.0) * SCALE,
                    (QUIET + y as f32 + 1.0) * SCALE,
                );
                ctx.fill_rect(&rect, &brush);
            }
        }
    }
    Ok(())
}

/// Total on-screen size of the QR in DIPs, for sizing the canvas widget.
pub(crate) fn qr_size(size: usize) -> f64 {
    (size as f64 + 2.0 * QUIET as f64) * SCALE as f64
}

/// Draw an "empty QR" placeholder: a ControlFill background with only the
/// three position-detection (finder) patterns, in a faded accent. Used before
/// a key pair exists so the QR slot still reads as a QR code rather than
/// showing bare text.
pub(crate) fn draw_placeholder_qr(ctx: &DrawContext) -> Result<()> {
    let size = PLACEHOLDER_SIZE;
    ctx.clear(control_fill_color());
    // Faded accent: the placeholder is a hint, not a scannable code.
    let mut c = accent_color();
    c.a = 0.35;
    let brush = ctx.create_solid_brush(c)?;
    let finder = |fx: usize, fy: usize| {
        // Outer 7x7 ring (one-module border).
        for i in 0..7 {
            for j in 0..7 {
                if i == 0 || i == 6 || j == 0 || j == 6 {
                    ctx.fill_rect(
                        &Rect::new(
                            (QUIET + (fx + i) as f32) * SCALE,
                            (QUIET + (fy + j) as f32) * SCALE,
                            (QUIET + (fx + i) as f32 + 1.0) * SCALE,
                            (QUIET + (fy + j) as f32 + 1.0) * SCALE,
                        ),
                        &brush,
                    );
                }
            }
        }
        // Center 3x3 solid.
        for i in 2..5 {
            for j in 2..5 {
                ctx.fill_rect(
                    &Rect::new(
                        (QUIET + (fx + i) as f32) * SCALE,
                        (QUIET + (fy + j) as f32) * SCALE,
                        (QUIET + (fx + i) as f32 + 1.0) * SCALE,
                        (QUIET + (fy + j) as f32 + 1.0) * SCALE,
                    ),
                    &brush,
                );
            }
        }
    };
    finder(0, 0);
    finder(size - 7, 0);
    finder(0, size - 7);
    Ok(())
}

/// On-screen size of the placeholder QR in DIPs, for sizing the canvas widget.
pub(crate) fn placeholder_qr_size() -> f64 {
    qr_size(PLACEHOLDER_SIZE)
}


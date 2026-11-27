use gpui::{Font, FontFallbacks, Pixels, Window, font, px};

pub(crate) const LINE_HEIGHT_SCALE: f32 = 18.0 / 14.0;
pub(crate) const TEXT_PADDING_X: Pixels = px(12.0);
pub(crate) const TEXT_PADDING_Y: Pixels = px(1.0);
const MIN_TEXT_PADDING_Y: Pixels = px(1.0);
pub(crate) const CUSTOM_TITLE_BAR_HEIGHT: Pixels = px(32.0);
pub(crate) const STATUS_BAR_HEIGHT: Pixels = px(42.0);

pub(crate) fn measure_cell_width(
    window: &mut Window,
    font_family: &str,
    font_fallbacks: Option<&FontFallbacks>,
    font_size: Pixels,
) -> Pixels {
    let mono = build_terminal_font(font_family, font_fallbacks);
    let font_id = window.text_system().resolve_font(&mono);
    window
        .text_system()
        .advance(font_id, font_size, 'M')
        .map(|advance| advance.width)
        .unwrap_or(px(8.0))
}

pub(crate) fn build_terminal_font(font_family: &str, font_fallbacks: Option<&FontFallbacks>) -> Font {
    let mut mono = font(font_family.to_string());
    mono.fallbacks = font_fallbacks.cloned();
    mono
}

pub(crate) fn line_height_for(font_size: Pixels) -> Pixels {
    (font_size * LINE_HEIGHT_SCALE).max(font_size + px(2.0))
}

pub(crate) fn terminal_content_padding_y(
    surface_height: Pixels,
    line_height: Pixels,
    rows: usize,
) -> Pixels {
    let grid_painted_height = line_height * rows as f32;
    let vertical_slack = (surface_height - grid_painted_height).max(MIN_TEXT_PADDING_Y * 2.0);
    (vertical_slack / 2.0).max(MIN_TEXT_PADDING_Y)
}

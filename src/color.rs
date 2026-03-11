use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color as AnsiColor, NamedColor};

pub(crate) fn ansi_bg_to_hsla(color: AnsiColor, colors: &Colors) -> Option<gpui::Hsla> {
    match color {
        AnsiColor::Named(NamedColor::Background) => None,
        other => Some(ansi_to_hsla(other, colors, Flags::empty(), false)),
    }
}

pub(crate) fn ansi_to_hsla(
    color: AnsiColor,
    colors: &Colors,
    flags: Flags,
    is_foreground: bool,
) -> gpui::Hsla {
    let resolved = ansi_to_rgb(color, colors, flags, is_foreground);
    gpui::rgb(((resolved.0 as u32) << 16) | ((resolved.1 as u32) << 8) | resolved.2 as u32).into()
}

fn ansi_to_rgb(
    color: AnsiColor,
    colors: &Colors,
    flags: Flags,
    is_foreground: bool,
) -> (u8, u8, u8) {
    match color {
        AnsiColor::Spec(rgb) => {
            let mut value = (rgb.r, rgb.g, rgb.b);
            if is_foreground && flags.contains(Flags::DIM) && !flags.contains(Flags::BOLD) {
                value = dim_rgb(value);
            }
            value
        }
        AnsiColor::Named(named) => {
            named_to_rgb(named_color_variant(named, flags, is_foreground), colors)
        }
        AnsiColor::Indexed(index) => indexed_to_rgb(index, colors),
    }
}

fn named_color_variant(named: NamedColor, flags: Flags, is_foreground: bool) -> NamedColor {
    if !is_foreground {
        return named;
    }

    match (
        flags.contains(Flags::BOLD),
        flags.contains(Flags::DIM),
        named,
    ) {
        (true, false, NamedColor::Foreground) => NamedColor::BrightForeground,
        (true, false, value) => value.to_bright(),
        (false, true, value) => value.to_dim(),
        _ => named,
    }
}

fn named_to_rgb(named: NamedColor, colors: &Colors) -> (u8, u8, u8) {
    if let Some(rgb) = colors[named] {
        return (rgb.r, rgb.g, rgb.b);
    }

    match named {
        NamedColor::Black => (0x1d, 0x1f, 0x21),
        NamedColor::Red => (0xcc, 0x66, 0x66),
        NamedColor::Green => (0xb5, 0xbd, 0x68),
        NamedColor::Yellow => (0xf0, 0xc6, 0x74),
        NamedColor::Blue => (0x81, 0xa2, 0xbe),
        NamedColor::Magenta => (0xb2, 0x94, 0xbb),
        NamedColor::Cyan => (0x8a, 0xbe, 0xb7),
        NamedColor::White => (0xc5, 0xc8, 0xc6),
        NamedColor::BrightBlack => (0x66, 0x66, 0x66),
        NamedColor::BrightRed => (0xd5, 0x4e, 0x53),
        NamedColor::BrightGreen => (0xb9, 0xca, 0x4a),
        NamedColor::BrightYellow => (0xe7, 0xc5, 0x47),
        NamedColor::BrightBlue => (0x7a, 0xa6, 0xda),
        NamedColor::BrightMagenta => (0xc3, 0x97, 0xd8),
        NamedColor::BrightCyan => (0x70, 0xc0, 0xba),
        NamedColor::BrightWhite => (0xea, 0xea, 0xea),
        NamedColor::Foreground => (0xd7, 0xda, 0xe0),
        NamedColor::Background => (0x0f, 0x11, 0x15),
        NamedColor::Cursor => (0x3b, 0x82, 0xf6),
        NamedColor::DimBlack => dim_rgb((0x1d, 0x1f, 0x21)),
        NamedColor::DimRed => dim_rgb((0xcc, 0x66, 0x66)),
        NamedColor::DimGreen => dim_rgb((0xb5, 0xbd, 0x68)),
        NamedColor::DimYellow => dim_rgb((0xf0, 0xc6, 0x74)),
        NamedColor::DimBlue => dim_rgb((0x81, 0xa2, 0xbe)),
        NamedColor::DimMagenta => dim_rgb((0xb2, 0x94, 0xbb)),
        NamedColor::DimCyan => dim_rgb((0x8a, 0xbe, 0xb7)),
        NamedColor::DimWhite => dim_rgb((0xc5, 0xc8, 0xc6)),
        NamedColor::BrightForeground => (0xff, 0xff, 0xff),
        NamedColor::DimForeground => dim_rgb((0xd7, 0xda, 0xe0)),
    }
}

pub(crate) fn indexed_to_rgb(index: u8, colors: &Colors) -> (u8, u8, u8) {
    if let Some(rgb) = colors[index as usize] {
        return (rgb.r, rgb.g, rgb.b);
    }

    match index {
        0 => named_to_rgb(NamedColor::Black, &Default::default()),
        1 => named_to_rgb(NamedColor::Red, &Default::default()),
        2 => named_to_rgb(NamedColor::Green, &Default::default()),
        3 => named_to_rgb(NamedColor::Yellow, &Default::default()),
        4 => named_to_rgb(NamedColor::Blue, &Default::default()),
        5 => named_to_rgb(NamedColor::Magenta, &Default::default()),
        6 => named_to_rgb(NamedColor::Cyan, &Default::default()),
        7 => named_to_rgb(NamedColor::White, &Default::default()),
        8 => named_to_rgb(NamedColor::BrightBlack, &Default::default()),
        9 => named_to_rgb(NamedColor::BrightRed, &Default::default()),
        10 => named_to_rgb(NamedColor::BrightGreen, &Default::default()),
        11 => named_to_rgb(NamedColor::BrightYellow, &Default::default()),
        12 => named_to_rgb(NamedColor::BrightBlue, &Default::default()),
        13 => named_to_rgb(NamedColor::BrightMagenta, &Default::default()),
        14 => named_to_rgb(NamedColor::BrightCyan, &Default::default()),
        15 => named_to_rgb(NamedColor::BrightWhite, &Default::default()),
        16..=231 => {
            let index = index - 16;
            let r = index / 36;
            let g = (index % 36) / 6;
            let b = index % 6;
            (cube_value(r), cube_value(g), cube_value(b))
        }
        232..=255 => {
            let gray = 8 + (index - 232) * 10;
            (gray, gray, gray)
        }
    }
}

fn cube_value(step: u8) -> u8 {
    match step {
        0 => 0,
        n => 55 + n * 40,
    }
}

fn dim_rgb((r, g, b): (u8, u8, u8)) -> (u8, u8, u8) {
    (
        ((r as f32) * 0.66) as u8,
        ((g as f32) * 0.66) as u8,
        ((b as f32) * 0.66) as u8,
    )
}

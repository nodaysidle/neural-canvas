//! Chrome colours for the dark and light themes (DESIGN.md): neutrals carry the chrome, one accent
//! marks focus, the active tool and primary actions; amber and red only mark state.

use nc_render::CanvasTheme;

/// Linear RGBA from an sRGB hex colour.
pub fn hex(rgb: u32) -> [f32; 4] {
    let channel = |shift: u32| {
        let c = ((rgb >> shift) & 0xff) as f32 / 255.0;
        if c <= 0.040_45 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    [channel(16), channel(8), channel(0), 1.0]
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub canvas: CanvasTheme,
    /// Panels and bars.
    pub panel: [f32; 4],
    /// A control under the pointer, and the selected row.
    pub raised: [f32; 4],
    pub pressed: [f32; 4],
    pub border: [f32; 4],
    pub text: [f32; 4],
    pub muted: [f32; 4],
    pub accent: [f32; 4],
    /// Text on an accent fill.
    pub on_accent: [f32; 4],
    pub warning: [f32; 4],
    pub error: [f32; 4],
    /// Dims the canvas behind a dialog.
    pub scrim: [f32; 4],
}

impl Palette {
    pub fn dark() -> Self {
        Self {
            canvas: CanvasTheme::DARK,
            panel: hex(0x17181d),
            raised: hex(0x24262d),
            pressed: hex(0x2e3139),
            border: hex(0x30333b),
            text: hex(0xe7e9ee),
            muted: hex(0x9ba1ad),
            accent: hex(0x5e9dff),
            on_accent: hex(0x0a1220),
            warning: hex(0xf0a33a),
            error: hex(0xf07171),
            scrim: [0.0, 0.0, 0.0, 0.55],
        }
    }

    pub fn light() -> Self {
        Self {
            canvas: CanvasTheme::LIGHT,
            panel: hex(0xffffff),
            raised: hex(0xeff1f4),
            pressed: hex(0xe3e6eb),
            border: hex(0xd3d7de),
            text: hex(0x16181e),
            muted: hex(0x575d6b),
            accent: hex(0x1f5fd1),
            on_accent: hex(0xffffff),
            warning: hex(0x9a5a00),
            error: hex(0xbb2a2a),
            scrim: [0.0, 0.0, 0.0, 0.3],
        }
    }

    pub fn of(dark: bool) -> Self {
        if dark { Self::dark() } else { Self::light() }
    }
}

/// WCAG 2.1 contrast ratio of two opaque linear colours.
pub fn contrast(a: [f32; 4], b: [f32; 4]) -> f32 {
    let l = |c: [f32; 4]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    let (hi, lo) = (l(a).max(l(b)), l(a).min(l(b)));
    (hi + 0.05) / (lo + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_decodes_to_linear() {
        assert_eq!(hex(0xffffff), [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(hex(0x000000)[0], 0.0);
        // sRGB 0x80 is 0.2158605 linear.
        assert!(
            (hex(0x808080)[0] - 0.215_860_5).abs() < 1e-5,
            "{}",
            hex(0x808080)[0]
        );
    }

    /// UX principle 9: chrome text ≥ 4.5:1 (WCAG AA), focus rings and control edges ≥ 3:1 (WCAG 1.4.11).
    #[test]
    fn chrome_meets_wcag_aa_in_both_themes() {
        for (name, p) in [("dark", Palette::dark()), ("light", Palette::light())] {
            for (what, fg, bg) in [
                ("text on panel", p.text, p.panel),
                ("text on raised", p.text, p.raised),
                ("text on pressed", p.text, p.pressed),
                ("muted on panel", p.muted, p.panel),
                ("muted on raised", p.muted, p.raised),
                ("on-accent on accent", p.on_accent, p.accent),
                ("warning on panel", p.warning, p.panel),
                ("error on panel", p.error, p.panel),
            ] {
                let c = contrast(fg, bg);
                assert!(c >= 4.5, "{name}: {what} is {c:.2}:1");
            }
            let canvas = {
                let [r, g, b] = p.canvas.background;
                [r, g, b, 1.0]
            };
            for (what, fg, bg) in [
                ("accent on panel", p.accent, p.panel),
                ("accent on canvas", p.accent, canvas),
                ("muted text on canvas", p.muted, canvas),
            ] {
                let c = contrast(fg, bg);
                assert!(c >= 3.0, "{name}: {what} is {c:.2}:1");
            }
        }
    }
}

//! Light and dark canvas. Ink is stored as drawn on the dark canvas; the light theme shows each colour
//! with its lightness inverted and its hue kept, so the same sketch reads on both backgrounds (white
//! ink becomes near-black, a pale blue a deep blue). The canvas shader applies the same formula.

/// Linear → sRGB, per channel.
fn encode(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// sRGB → linear, per channel.
fn decode(c: f32) -> f32 {
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// How a linear RGBA ink colour shows on the light canvas: in sRGB, luma `Y` becomes `1 − Y` while
/// the chroma (`c − Y`) is kept, i.e. `c' = c + 1 − 2Y`.
pub fn ink_on_light(c: [f32; 4]) -> [f32; 4] {
    let s = [encode(c[0]), encode(c[1]), encode(c[2])];
    let y = 0.299 * s[0] + 0.587 * s[1] + 0.114 * s[2];
    let shift = 1.0 - 2.0 * y;
    [
        decode((s[0] + shift).clamp(0.0, 1.0)),
        decode((s[1] + shift).clamp(0.0, 1.0)),
        decode((s[2] + shift).clamp(0.0, 1.0)),
        c[3],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greys_invert_and_hue_survives() {
        let white = ink_on_light([1.0, 1.0, 1.0, 1.0]);
        assert!(
            white[..3].iter().all(|c| *c < 1e-3),
            "white ink turns black: {white:?}"
        );
        let mid = ink_on_light([decode(0.5); 4]);
        assert!((encode(mid[0]) - 0.5).abs() < 1e-3, "mid grey stays");
        let red = ink_on_light([1.0, 0.0, 0.0, 1.0]);
        assert!(red[0] > red[1] && red[0] > red[2], "still red: {red:?}");
        assert_eq!(ink_on_light([0.2, 0.3, 0.4, 0.5])[3], 0.5, "alpha kept");
    }
}

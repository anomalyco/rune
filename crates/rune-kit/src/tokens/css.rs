//! Reading theme data authored for the web: computed CSS `box-shadow` lists.

use rune::{Color, Shadow};

/// Parses a computed `box-shadow` list (`#rrggbbaa 0px 1px 1.5px 0px [inset], ...`). Colours
/// go through `color` (so a theme can re-solve translucency); fully transparent layers drop.
pub fn box_shadows(src: &str, color: impl Fn(Color) -> Color) -> Vec<Shadow> {
    let mut out = Vec::new();
    for layer in split_layers(src) {
        let mut c = None;
        let mut nums = Vec::new();
        let mut inset = false;
        for tok in layer.split_whitespace() {
            if tok == "inset" {
                inset = true;
            } else if tok.starts_with('#') {
                c = rune::theme::hex(tok);
            } else if let Ok(v) = tok.trim_end_matches("px").parse::<f32>() {
                nums.push(v);
            }
        }
        let Some(c) = c else { continue };
        if c.a <= 0.0 {
            continue;
        }
        let n = |i: usize| nums.get(i).copied().unwrap_or(0.0);
        out.push(Shadow { x: n(0), y: n(1), blur: n(2), spread: n(3), color: color(c), inset });
    }
    out
}

fn split_layers(src: &str) -> Vec<&str> {
    let (mut depth, mut start, mut out) = (0, 0, Vec::new());
    for (i, ch) in src.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                out.push(src[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(src[start..].trim());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_layers_and_inset() {
        let s = box_shadows(
            "#0000001a 0px 1px 1.5px 0px, #ffffff29 0px 0px 0px 0.5px inset, #00000000 0px 0px 0px 0px",
            |c| c,
        );
        assert_eq!(s.len(), 2);
        assert_eq!((s[0].y, s[0].blur, s[0].inset), (1.0, 1.5, false));
        assert_eq!((s[1].spread, s[1].inset), (0.5, true));
    }
}

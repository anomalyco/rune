//! `enter` and `exit`: a node's presence transition, written as parts separated by spaces.
//!
//! Each part is a name and, after a colon, its amount: `"fade slide-up:24"`. `fade[:opacity]`
//! (0), `scale[:s]` (0.95), `slide-up`, `slide-down`, `slide-left` and `slide-right[:points]`
//! (8) and `blur[:sigma]` (1.5) say what the node looks like while absent;
//! `spring:<secs>[:<bounce>]` times that side of the transition (`El::transition_motion`).
//! A slide names the way the node moves: `enter = "slide-up"` rises into place and
//! `exit = "slide-up"` rises out. `none`, alone, shows or removes the node at once, also under
//! reduced motion, which drops every other `enter` and `exit`.

use rune::{Anim, Effect, El, Spring, Transition, Vec2};

/// One side of a presence transition: how the node looks absent, and the spring that times it,
/// or `None`: the node shows or goes at once.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Presence {
    Animated { effect: Effect, spring: Option<Spring> },
    None,
}

/// Parses an `enter` (`exit = false`) or `exit` string.
pub(crate) fn parse(s: &str, exit: bool) -> Result<Presence, String> {
    let prop = if exit { "exit" } else { "enter" };
    let mut effect = Effect::IDENTITY;
    let mut spring = None;
    let parts: Vec<&str> = s.split_ascii_whitespace().collect();
    match parts.as_slice() {
        [] => return Err(format!("{prop} must name at least one part, such as \"fade\"")),
        ["none"] => return Ok(Presence::None),
        many if many.contains(&"none") => return Err(format!("{prop}: none can't be combined with other parts, got {s:?}")),
        _ => {}
    }
    for part in parts {
        let mut args = part.split(':');
        let name = args.next().unwrap_or_default();
        let args: Vec<&str> = args.collect();
        let amount = |default: f32| -> Result<f32, String> {
            match args.as_slice() {
                [] => Ok(default),
                [a] => a.parse::<f32>().ok().filter(|n| n.is_finite()).ok_or_else(|| format!("{prop}: {name} takes a number, got {a:?}")),
                _ => Err(format!("{prop}: {name} takes one amount, got {part:?}")),
            }
        };
        // Which way a slide moves the node while it is absent: an entrance starts on the far
        // side of where it is heading, an exit ends there.
        let toward = |x: f32, y: f32, d: f32| {
            let d = if exit { d } else { -d };
            Vec2::new(x * d, y * d)
        };
        let part_effect = match name {
            "fade" => Effect::opacity(amount(0.0)?.clamp(0.0, 1.0)),
            "scale" => {
                let n = amount(0.95)?;
                if n <= 0.0 {
                    return Err(format!("{prop}: scale must be above 0, got {n}"));
                }
                Effect::scale(n)
            }
            "blur" => Effect::blur(amount(1.5)?.max(0.0)),
            "slide-up" => Effect { offset: toward(0.0, -1.0, amount(8.0)?), ..Effect::IDENTITY },
            "slide-down" => Effect { offset: toward(0.0, 1.0, amount(8.0)?), ..Effect::IDENTITY },
            "slide-left" => Effect { offset: toward(-1.0, 0.0, amount(8.0)?), ..Effect::IDENTITY },
            "slide-right" => Effect { offset: toward(1.0, 0.0, amount(8.0)?), ..Effect::IDENTITY },
            "spring" => {
                spring = Some(parse_spring(prop, part, &args)?);
                continue;
            }
            other => return Err(format!("{prop}: unknown part {other:?} (fade, scale, blur, slide-up, slide-down, slide-left, slide-right, spring, or none alone)")),
        };
        effect = effect.combined(part_effect);
    }
    Ok(Presence::Animated { effect, spring })
}

/// `spring:<secs>[:<bounce>]`: a positive duration under 10 000 seconds, a bounce in (-1, 1).
fn parse_spring(prop: &str, part: &str, args: &[&str]) -> Result<Spring, String> {
    let num = |s: &str| s.parse::<f32>().ok().filter(|n| n.is_finite());
    let (secs, bounce) = match args {
        [secs] => (num(secs), Some(0.0)),
        [secs, bounce] => (num(secs), num(bounce)),
        _ => (None, None),
    };
    match (secs, bounce) {
        (Some(d), Some(b)) if d > 0.0 && d < 1e4 && b > -1.0 && b < 1.0 => Ok(Spring::new(d, b)),
        _ => Err(format!("{prop}: {part:?} must be spring:<secs>[:<bounce>], with secs above 0 and bounce between -1 and 1")),
    }
}

/// Puts `enter` and `exit` on `e`. A side that isn't given fades on its default spring. Under
/// reduced motion only `none` holds: the rest is dropped for Rune's own reduced transitions.
pub(crate) fn apply(e: El, enter: Option<Presence>, exit: Option<Presence>, reduced: bool) -> El {
    let keep = |p: Option<Presence>| p.filter(|p| !reduced || *p == Presence::None);
    let (enter, exit) = (keep(enter), keep(exit));
    if enter.is_none() && exit.is_none() {
        return e;
    }
    let base = Transition::opacity();
    // How the node looks absent on a side, and the curve that times it when not its default.
    let side = |p: Option<Presence>, absent: Effect| match p {
        None => (absent, None),
        Some(Presence::None) => (Effect::IDENTITY, Some(Anim::INSTANT)),
        Some(Presence::Animated { effect, spring }) => (effect, spring.map(Anim::from)),
    };
    let (insertion, enter_anim) = side(enter, base.insertion);
    let (removal, exit_anim) = side(exit, base.removal);
    let e = e.transition(Transition { insertion, removal, ..base });
    if enter_anim.is_none() && exit_anim.is_none() {
        return e;
    }
    e.transition_motion(enter_anim.unwrap_or(base.springs.enter.into()), exit_anim.unwrap_or(base.springs.exit.into()))
}

#[cfg(test)]
#[path = "motion_tests.rs"]
mod tests;

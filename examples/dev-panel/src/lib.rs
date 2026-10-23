use rune::{AppSpec, Cx, El, Spring, Tx, col, el, text, tok};

// Proves the disabled macro is a literal expression: neither key nor range is resolved.
#[cfg(not(all(feature = "dev", debug_assertions)))]
const _: f32 = rune::tune!(unresolved_key(), 3.0_f32, unresolved_min(), unresolved_max(), unresolved_step());

#[derive(Default, Debug, PartialEq)]
pub enum State {
    #[default]
    Lobby,
    Joining,
    Room,
}

pub fn view(state: &State, _: &mut Cx<State>) -> El {
    let spring = rune::tune!("rooms.join.spring", Spring::new(0.35, 0.0));
    let width = rune::tune!("rooms.card.width", 260.0_f32, 120.0, 420.0, 1.0);
    let radius = rune::tune!("rooms.card.radius", 20.0_f32, 0.0, 64.0, 1.0);
    let fill = rune::tune!("rooms.card.fill", rune::oklch(0.65, 0.13, 250.0));
    let (label, x) = match state {
        State::Lobby => ("Lobby", 24.0),
        State::Joining => ("Joining", 100.0),
        State::Room => ("Room", 220.0),
    };
    let theme = tok::theme(true);
    col().w_full().h_full().fill(theme.bg)
        .child(el().key("card").absolute(x, 110.0).size(width, 120.0)
            .radius(radius).fill(fill).motion(spring).center()
            .child(text(label).key("state-label").font(tok::type_::TITLE).color(theme.text)))
}

pub fn join(state: &mut State, _: &mut Tx) { *state = State::Room; }
pub fn leave(state: &mut State, _: &mut Tx) { *state = State::Lobby; }

pub fn spec() -> AppSpec<State> {
    AppSpec {
        scenes: &[
            ("lobby", |_| {}),
            ("joining", |s| *s = State::Joining),
            ("room", |s| *s = State::Room),
        ],
        ..AppSpec::new("Rune dev panel", (680.0, 360.0), State::default, view)
    }
}

#[cfg(all(feature = "dev", debug_assertions))]
pub fn config() -> rune::dev::Config<State> {
    rune::dev::Config::default().transitions([
        rune::dev::Transition::new("Join", "lobby", join),
        rune::dev::Transition::new("Leave", "room", leave),
    ])
}

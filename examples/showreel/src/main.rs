//! Showreel: purpose-made 960×540 scenes for Rune's showcase video. Each scene animates itself from
//! the (virtual) clock, so a headless `--video` run records it frame-exact:
//!
//!     showreel --scene deploy --video deploy.mp4 --size 960x540 --scale 2 --duration 4

mod gfx;
mod kit;
mod scenes;

use rune::{AppSpec, Cx, El};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Scene {
    #[default]
    Word,
    Card,
    Deploy,
    Glass,
    Fire,
    Pricing,
    Ask,
    Shock,
    Keys,
    Tabs,
    Type,
}

#[derive(Default)]
pub struct State {
    pub scene: Scene,
    /// Seconds since the scene started (Rune's virtual clock when headless).
    pub t: f64,
}

fn view(s: &State, cx: &mut Cx<State>) -> El {
    match s.scene {
        Scene::Word => scenes::word::view(s, cx, false),
        Scene::Card => scenes::word::view(s, cx, true),
        Scene::Deploy => scenes::deploy::view(s, cx),
        Scene::Glass => scenes::glass::view(s, cx),
        Scene::Fire => scenes::fire::view(s, cx),
        Scene::Pricing => scenes::pricing::view(s, cx),
        Scene::Ask => scenes::ask::view(s, cx),
        Scene::Shock => scenes::shock::view(s, cx),
        Scene::Keys => scenes::keys::view(s, cx),
        Scene::Tabs => scenes::tabs::view(s, cx),
        Scene::Type => scenes::type_::view(s, cx),
    }
}

fn main() {
    let app = rune::App::new(AppSpec::<State> {
        scenes: &[
            ("word", |s| s.scene = Scene::Word),
            ("card", |s| s.scene = Scene::Card),
            ("deploy", |s| s.scene = Scene::Deploy),
            ("glass", |s| s.scene = Scene::Glass),
            ("fire", |s| s.scene = Scene::Fire),
            ("pricing", |s| s.scene = Scene::Pricing),
            ("ask", |s| s.scene = Scene::Ask),
            ("shock", |s| s.scene = Scene::Shock),
            ("keys", |s| s.scene = Scene::Keys),
            ("tabs", |s| s.scene = Scene::Tabs),
            ("type", |s| s.scene = Scene::Type),
        ],
        fonts: &[
            include_bytes!("../../../crates/rune/assets/Inter-Regular.ttf"),
            include_bytes!("../../../crates/rune/assets/Inter-Medium.ttf"),
            include_bytes!("../../../crates/rune/assets/Inter-SemiBold.ttf"),
            include_bytes!("../../../themes/opencode/assets/fonts/IBMPlexMono-Medium.ttf"),
        ],
        families: ("Inter", "IBM Plex Mono"),
        poll: Some(|s, t, _| {
            s.t = t;
            true
        }),
        ..AppSpec::new("Showreel", (960.0, 540.0), State::default, view)
    })
    .compositor(gfx::Gfx::default());
    app.run();
}

//! The "Controls/Haptics" playground story: interactive detent stepper (`Haptic::Selection`),
//! impact triggers (`Light`, `Medium`, `Heavy`, `Soft`, `Rigid`), and notification triggers
//! (`Success`, `Warning`, `Error`) using [`Tx::haptic`].

use rune::tok::{self, radius, type_};
use rune::{Color, Cx, El, Haptic, HapticOutcome, ImpactStrength, Story, Tx, col, row, text};

const STOPS: [&str; 6] = ["Peek", "Quarter", "Half", "Three-quarter", "Tall", "Full"];

pub(crate) struct HapticsDemo {
    stop: usize,
    log: Vec<&'static str>,
}

impl Default for HapticsDemo {
    fn default() -> Self {
        Self { stop: 2, log: Vec::new() }
    }
}

impl HapticsDemo {
    fn record(&mut self, label: &'static str, haptic: Haptic, tx: &mut Tx) {
        tx.haptic(haptic);
        self.log.push(label);
        if self.log.len() > 6 {
            self.log.remove(0);
        }
    }
}

fn trigger_button(
    key: &'static str,
    label: &'static str,
    haptic: Haptic,
    t: &tok::Theme,
) -> El {
    row().key(key).h(30.0).px(10.0).center().radius(radius::SM)
        .fill(t.control_track).hover(t.hover)
        .role("button").label(label).focusable(true)
        .on_press_tx(move |s: &mut HapticsDemo, tx: &mut Tx| s.record(label, haptic, tx))
        .child(text(label).font(type_::LABEL).color(t.text))
}

fn view(s: &HapticsDemo, cx: &mut Cx<HapticsDemo>) -> El {
    let t = tok::theme(cx.system_dark());
    let accent = tok::accent(0, cx.system_dark());

    let stops = row().key("detent-stops").gap(6.0).children(STOPS.iter().enumerate().map(|(i, name)| {
        let active = s.stop == i;
        row().key(format!("stop-{i}")).h(28.0).px(10.0).center().radius(radius::SM)
            .fill(if active { accent } else { t.control_track })
            .hover(if active { accent } else { t.hover })
            .role("radio").label(*name).checked(active).focusable(true)
            .on_press_tx(move |s: &mut HapticsDemo, tx: &mut Tx| {
                if s.stop != i {
                    s.stop = i;
                    s.record("Selection", Haptic::Selection, tx);
                }
            })
            .child(text(*name).font(type_::LABEL).color(if active { Color::WHITE } else { t.text }))
    }));

    let impacts = row().key("impact-buttons").gap(6.0)
        .child(trigger_button("impact-light", "Light", Haptic::Impact(ImpactStrength::Light), &t))
        .child(trigger_button("impact-medium", "Medium", Haptic::Impact(ImpactStrength::Medium), &t))
        .child(trigger_button("impact-heavy", "Heavy", Haptic::Impact(ImpactStrength::Heavy), &t))
        .child(trigger_button("impact-soft", "Soft", Haptic::Impact(ImpactStrength::Soft), &t))
        .child(trigger_button("impact-rigid", "Rigid", Haptic::Impact(ImpactStrength::Rigid), &t));

    let outcomes = row().key("notify-buttons").gap(6.0)
        .child(trigger_button("notify-success", "Success", Haptic::Notify(HapticOutcome::Success), &t))
        .child(trigger_button("notify-warning", "Warning", Haptic::Notify(HapticOutcome::Warning), &t))
        .child(trigger_button("notify-error", "Error", Haptic::Notify(HapticOutcome::Error), &t));

    let log_text = if s.log.is_empty() {
        "No haptics requested yet — pick a stop or trigger above".to_owned()
    } else {
        format!("Recent requests: {}", s.log.join(" -> "))
    };

    col().key("haptics-story").w(540.0).px(20.0).py(20.0).gap(14.0).radius(radius::MD)
        .fill(t.card).border(1.0, t.card_border)
        .child(text("System Haptics (Tx::haptic)").font(type_::SECTION).color(t.text))
        .child(text("macOS plays Selection (LevelChange), Light/Soft (Alignment) and Medium/Heavy/Rigid (Generic) on Force Touch trackpads; Notify is silent on macOS. iOS/Android hosts drain LayerHost::take_haptics().").font(type_::BODY).color(t.text_secondary))
        .child(text(format!("Sheet stop: {} (Haptic::Selection)", STOPS[s.stop])).font(type_::LABEL).color(t.text))
        .child(stops)
        .child(text("Impact (Haptic::Impact)").font(type_::LABEL).color(t.text))
        .child(impacts)
        .child(text("Notification (Haptic::Notify)").font(type_::LABEL).color(t.text))
        .child(outcomes)
        .child(text(log_text).key("haptics-log").font(type_::BODY).color(t.text_secondary))
}

pub(crate) fn story() -> Story<HapticsDemo> {
    Story::new("Controls/Haptics", HapticsDemo::default, view)
        .trigger('n', "Next stop (Selection)", |s, _| {
            s.stop = (s.stop + 1) % STOPS.len();
            s.log.push("Selection");
            if s.log.len() > 6 { s.log.remove(0); }
        })
        .variant("Default", |_| {})
        .variant("Full stop", |s| {
            s.stop = STOPS.len() - 1;
            s.log = vec!["Selection", "Light", "Medium", "Success"];
        })
}

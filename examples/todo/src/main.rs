//! A list with state: keyed rows you can add, tick off and remove. Rows animate in and out on
//! their own and the progress bar springs to its new width; the view only says where things are.
//! `cargo run -p todo -- --snap todo.png`.

#![forbid(unsafe_code)]

use rune::tok::{self, Theme, radius, type_};
use rune::{AppSpec, Curve, Cx, Easing, El, ScrollSpec, Spring, Transition, col, el, row, text};

struct Todo { id: u32, title: String, done: bool }

struct State { todos: Vec<Todo>, next: u32, timed: bool }

impl Default for State {
    fn default() -> Self {
        let titles = ["Write the view", "Run it headless", "Ship it"];
        let todos = titles.iter().zip(0..)
            .map(|(title, id)| Todo { id, title: title.to_string(), done: id == 0 });
        State { todos: todos.collect(), next: 3, timed: false }
    }
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    let t = tok::theme(true);
    let mut list = col().key("list").gap(4.0);
    for todo in &s.todos {
        let id = todo.id;
        let mut check = el().key("check").size(16.0, 16.0).radius(8.0)
            .fill(if todo.done { tok::accent(0, true) } else { t.control_off })
            .on_press(move |s: &mut State| {
                if let Some(t) = s.todos.iter_mut().find(|t| t.id == id) { t.done = !t.done }
            });
        if s.timed {
            check = check.press(t.text.alpha(0.15)).press_scale(0.92).press_motion(
                Curve::tween(0.10, Easing::EASE_OUT),
                Curve::tween(0.14, Easing::EASE_OUT),
            );
        }
        let title = text(todo.title.clone()).font(type_::BODY).grow(1.0)
            .color(if todo.done { t.text_secondary } else { t.text });
        let remove = text("Remove").font(type_::LABEL).color(t.text_secondary)
            .on_press(move |s: &mut State| s.todos.retain(|t| t.id != id));
        let mut item = row().key(format!("todo-{id}")).h(36.0).px(12.0).gap(12.0).center_y()
            .radius(radius::SM).fill(t.card)
            .child(check).child(title).child(remove);
        if s.timed {
            item = item.transition(Transition::opacity()).transition_motion(
                Curve::tween(0.20, Easing::EASE_OUT),
                Curve::tween(0.18, Easing::EASE_OUT),
            );
        }
        list = list.child(item);
    }
    let mut add = text("Add a todo").key("add").font(type_::LABEL).color(tok::accent(0, true))
        .on_press(|s: &mut State| {
            s.todos.push(Todo { id: s.next, title: format!("Todo {}", s.next + 1), done: false });
            s.next += 1;
        });
    if s.timed {
        add = add.press_scale(0.96).press_motion(
            Curve::tween(0.10, Easing::EASE_OUT),
            Curve::tween(0.14, Easing::EASE_OUT),
        );
    }
    let curves = text("Timed curves").key("curves").font(type_::LABEL)
        .color(if s.timed { tok::accent(0, true) } else { t.text_secondary })
        .on_press(|s: &mut State| s.timed = !s.timed);
    let footer = row().w_full().justify_between().center_y().child(add).child(curves);
    let done = s.todos.iter().filter(|t| t.done).count();
    col().w_full().h_full().padding(24.0).gap(12.0).fill(t.bg)
        .child(progress(done, s.todos.len(), s.timed, t))
        // Past what the window holds, the list scrolls, following new rows, and "Add" stays.
        .child(list.scroll(ScrollSpec::stick_bottom()).shrink().min_h(0.0))
        .child(footer)
}

fn progress(done: usize, total: usize, timed: bool, t: Theme) -> El {
    let width = 312.0 * done as f32 / total.max(1) as f32;
    let bar = el().key("bar").w(width).h(4.0).radius(2.0).fill(tok::accent(0, true));
    let bar = if timed {
        bar.frame_motion(Curve::tween(0.24, Easing::EASE_OUT))
    } else {
        bar.motion(Spring::new(0.4, 0.2))
    };
    el().key("track").w_full().h(4.0).radius(2.0).fill(t.control_off).child(bar)
}

fn main() {
    rune::run(AppSpec::<State> {
        scenes: &[("default", |_| {}), ("timed", |s| s.timed = true)],
        ..AppSpec::new("Todo", (360.0, 240.0), State::default, view)
    });
}

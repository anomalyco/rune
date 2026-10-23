//! A list with state: keyed rows you can add, tick off and remove. Rows animate in and out on
//! their own and the progress bar springs to its new width; the view only says where things are.
//! `cargo run -p todo -- --snap todo.png`.

use rune::tok::{self, Theme, radius, type_};
use rune::{AppSpec, Cx, El, Spring, col, el, row, text};

struct Todo { id: u32, title: String, done: bool }

struct State { todos: Vec<Todo>, next: u32 }

impl Default for State {
    fn default() -> Self {
        let titles = ["Write the view", "Run it headless", "Ship it"];
        let todos = titles.iter().zip(0..)
            .map(|(title, id)| Todo { id, title: title.to_string(), done: id == 0 });
        State { todos: todos.collect(), next: 3 }
    }
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    let t = tok::theme(true);
    let mut list = col().key("list").gap(4.0);
    for todo in &s.todos {
        let id = todo.id;
        let check = el().key("check").size(16.0, 16.0).radius(8.0)
            .fill(if todo.done { tok::accent(0, true) } else { t.control_off })
            .on_press(move |s: &mut State| {
                if let Some(t) = s.todos.iter_mut().find(|t| t.id == id) { t.done = !t.done }
            });
        let title = text(todo.title.clone()).font(type_::BODY).grow(1.0)
            .color(if todo.done { t.text_secondary } else { t.text });
        let remove = text("Remove").font(type_::LABEL).color(t.text_secondary)
            .on_press(move |s: &mut State| s.todos.retain(|t| t.id != id));
        list = list.child(
            row().key(format!("todo-{id}")).h(36.0).px(12.0).gap(12.0).center_y()
                .radius(radius::SM).fill(t.card)
                .child(check).child(title).child(remove),
        );
    }
    let add = text("Add a todo").key("add").font(type_::LABEL).color(tok::accent(0, true))
        .on_press(|s: &mut State| {
            s.todos.push(Todo { id: s.next, title: format!("Todo {}", s.next + 1), done: false });
            s.next += 1;
        });
    let done = s.todos.iter().filter(|t| t.done).count();
    col().w_full().h_full().padding(24.0).gap(12.0).fill(t.bg)
        .child(progress(done, s.todos.len(), t))
        .child(list)
        .child(add)
}

fn progress(done: usize, total: usize, t: Theme) -> El {
    let width = 312.0 * done as f32 / total.max(1) as f32;
    el().key("track").w_full().h(4.0).radius(2.0).fill(t.control_off).child(
        el().key("bar").w(width).h(4.0).radius(2.0).fill(tok::accent(0, true))
            .motion(Spring::new(0.4, 0.2)),
    )
}

fn main() {
    rune::run(AppSpec::new("Todo", (360.0, 240.0), State::default, view));
}

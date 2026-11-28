use super::*;
use crate::{GrowthSpec, Key, KeyEvent, Mods, ScrollSpec, TextEdit, TextStyle, Tracked, col, region, text, text_input};
use crate::memo::region_warnings;

#[test]
fn focused_tracked_text_field_does_not_write_or_rebuild_region_on_unrelated_commits() {
    struct App {
        edit: Tracked<TextEdit>,
        counter: Tracked<u32>,
    }

    fn view(s: &App, _: &mut Cx<App>) -> El {
        let style = TextStyle::new(14.0, 400.0);
        col().size(400.0, 300.0).children([
            region("field-pane", || {
                let mut pane = col().key("field-pane").w_full().child(
                    text_input(&s.edit, style)
                        .key("field")
                        .label("Draft")
                        .size(360.0, 32.0)
                        .on_edit(|s: &mut App| &mut *s.edit),
                );
                for i in 0..20 {
                    pane = pane.child(text(format!("row {i}")).font(style));
                }
                pane
            }),
            region("counter-pane", || {
                text(format!("count {}", *s.counter)).key("counter-pane").font(style)
            }),
        ])
    }

    let mut edit = TextEdit::new("draft");
    edit.focused = true;
    let mut rt = Runtime::new(
        App { edit: Tracked::new(edit), counter: Tracked::new(0) },
        view,
        (400.0, 300.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    assert!(rt.state.edit.focused);
    assert!(rt.kbd.focus.path().is_some_and(|p| p.ends_with("/field")));

    let edit_ver_before = Tracked::version(&rt.state.edit);
    let memo_builds_before = rt.commit.memo_builds();

    const COMMITS: usize = 120;
    for _ in 0..COMMITS {
        rt.update(|s, _| *s.counter += 1);
        rt.frame(1.0 / 60.0);
    }

    let edit_ver_moves = Tracked::version(&rt.state.edit) - edit_ver_before;
    let total_region_builds = rt.commit.memo_builds() - memo_builds_before;
    assert_eq!(edit_ver_moves, 0, "idle focused field must not write its Tracked<TextEdit>");
    assert_eq!(
        total_region_builds, COMMITS,
        "only the counter region should rebuild across {COMMITS} unrelated commits (field region rebuilt {} times)",
        total_region_builds.saturating_sub(COMMITS),
    );
    assert!(rt.state.edit.focused);
    assert!(rt.kbd.focus.path().is_some_and(|p| p.ends_with("/field")));
    assert!(
        region_warnings().iter().all(|w| !w.contains("field-pane")),
        "reused field region must stay consistent: {:?}",
        region_warnings(),
    );
}

#[test]
fn focused_and_unfocused_overrides_settle_without_bumping_tracked_edit() {
    struct App {
        single: Tracked<TextEdit>,
        secure_focused: Tracked<TextEdit>,
        secure_idle: Tracked<TextEdit>,
        line_home: Tracked<TextEdit>,
        counter: Tracked<u32>,
    }

    fn view(s: &App, _: &mut Cx<App>) -> El {
        let style = TextStyle::new(14.0, 400.0);
        col().size(400.0, 300.0).children([
            region("fields-overrides", || {
                col().key("fields-overrides").w_full().children([
                    text_input(&s.single, style)
                        .key("single")
                        .label("Single")
                        .size(200.0, 28.0)
                        .single_line()
                        .on_edit(|s: &mut App| &mut *s.single),
                    text_input(&s.secure_focused, style)
                        .key("sec-focused")
                        .label("Secure single")
                        .size(200.0, 28.0)
                        .single_line()
                        .secure()
                        .revealed(true)
                        .on_edit(|s: &mut App| &mut *s.secure_focused),
                    text_input(&s.secure_idle, style)
                        .key("sec-idle")
                        .label("Secure multi")
                        .size(200.0, 28.0)
                        .secure()
                        .revealed(false)
                        .on_edit(|s: &mut App| &mut *s.secure_idle),
                    text_input(&s.line_home, style)
                        .key("line-home")
                        .label("Code")
                        .size(200.0, 28.0)
                        .line_home_end(true)
                        .on_edit(|s: &mut App| &mut *s.line_home),
                ])
            }),
            region("counter-overrides", || {
                text(format!("tick {}", *s.counter)).key("counter-overrides").font(style)
            }),
        ])
    }

    let mut single = TextEdit::new("hello");
    single.focused = true;
    let mut rt = Runtime::new(
        App {
            single: Tracked::new(single),
            secure_focused: Tracked::new(TextEdit::new("pw1")),
            secure_idle: Tracked::new(TextEdit::new("pw2")),
            line_home: Tracked::new(TextEdit::new("code")),
            counter: Tracked::new(0),
        },
        view,
        (400.0, 300.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    // Initial commit pushed every view override into the app models.
    assert!(rt.state.single.focused && rt.state.single.is_single_line());
    assert!(rt.state.secure_focused.is_secure() && rt.state.secure_focused.is_revealed() && rt.state.secure_focused.is_single_line());
    assert!(rt.state.secure_idle.is_secure() && !rt.state.secure_idle.is_revealed() && !rt.state.secure_idle.is_single_line());
    assert!(rt.state.line_home.line_home_end());

    let versions = |rt: &Runtime<App>| (
        Tracked::version(&rt.state.single),
        Tracked::version(&rt.state.secure_focused),
        Tracked::version(&rt.state.secure_idle),
        Tracked::version(&rt.state.line_home),
    );
    let before_v = versions(&rt);
    let before_builds = rt.commit.memo_builds();

    for _ in 0..20 {
        rt.update(|s, _| *s.counter += 1);
        rt.frame(1.0 / 60.0);
    }

    assert_eq!(versions(&rt), before_v, "settled fields must not bump Tracked versions on unrelated commits");
    assert_eq!(rt.commit.memo_builds() - before_builds, 20, "only the counter region should rebuild");
    assert!(
        region_warnings().iter().all(|w| !w.contains("fields-overrides")),
        "reused override region must stay consistent: {:?}",
        region_warnings(),
    );
}

#[test]
fn real_edits_focus_changes_ime_and_dynamic_overrides_still_sync_and_bump_tracked_edit() {
    struct App {
        edit: Tracked<TextEdit>,
        revealed: Tracked<bool>,
        line_home_end: Tracked<bool>,
        secure: Tracked<bool>,
        single_line: Tracked<bool>,
        counter: Tracked<u32>,
    }

    fn view(s: &App, _: &mut Cx<App>) -> El {
        let style = TextStyle::new(14.0, 400.0);
        col().size(400.0, 200.0).children([
            region("dynamic-field", || {
                let mut field = text_input(&s.edit, style)
                    .key("field")
                    .label("Dynamic")
                    .size(300.0, 32.0)
                    .revealed(*s.revealed)
                    .line_home_end(*s.line_home_end)
                    .on_edit(|s: &mut App| &mut *s.edit);
                if *s.single_line {
                    field = field.single_line();
                }
                if *s.secure {
                    field = field.secure();
                }
                col().key("dynamic-field").w_full().child(field)
            }),
            region("dynamic-counter", || {
                text(format!("c {}", *s.counter)).key("dynamic-counter").font(style)
            }),
        ])
    }

    let mut rt = Runtime::new(
        App {
            edit: Tracked::new(TextEdit::new("hi")),
            revealed: Tracked::new(false),
            line_home_end: Tracked::new(false),
            secure: Tracked::new(false),
            single_line: Tracked::new(false),
            counter: Tracked::new(0),
        },
        view,
        (400.0, 200.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    assert!(!rt.state.edit.focused);

    // Focus via Tx::focus -> must sync focused=true and bump Tracked<TextEdit>.
    let v0 = Tracked::version(&rt.state.edit);
    rt.update(|_, tx| tx.focus("field", false));
    rt.frame(1.0 / 60.0);
    assert!(rt.state.edit.focused);
    assert!(Tracked::version(&rt.state.edit) > v0, "focus gain must bump Tracked<TextEdit>");

    // Unrelated commits stay quiet.
    let v1 = Tracked::version(&rt.state.edit);
    for _ in 0..5 {
        rt.update(|s, _| *s.counter += 1);
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(Tracked::version(&rt.state.edit), v1);

    // Typing bumps Tracked<TextEdit> and updates text.
    rt.key(KeyEvent { key: Key::Text("!".into()), mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.edit.text, "hi!");
    assert!(Tracked::version(&rt.state.edit) > v1);

    // Caret move bumps Tracked<TextEdit>.
    let v2 = Tracked::version(&rt.state.edit);
    rt.key(KeyEvent { key: Key::Left, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.edit.focus, 2);
    assert!(Tracked::version(&rt.state.edit) > v2);

    // Dynamic view overrides sync into the app model and settle afterward.
    rt.update(|s, _| {
        *s.single_line = true;
        *s.line_home_end = true;
        *s.revealed = true;
        *s.secure = true;
    });
    rt.frame(1.0 / 60.0);
    assert!(rt.state.edit.is_single_line());
    assert!(rt.state.edit.line_home_end());
    assert!(rt.state.edit.is_revealed());
    assert!(rt.state.edit.is_secure());

    let v3 = Tracked::version(&rt.state.edit);
    for _ in 0..5 {
        rt.update(|s, _| *s.counter += 1);
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(Tracked::version(&rt.state.edit), v3, "no further writes after dynamic overrides sync");

    // IME preedit while unfocused is still cancelled on blur.
    rt.update(|s, _| {
        *s.secure = false;
        s.edit.set_secure(false);
    });
    rt.frame(1.0 / 60.0);
    rt.ime(crate::Ime::Preedit("ka".into(), None, Vec::new()));
    rt.frame(1.0 / 60.0);
    assert!(rt.state.edit.is_composing());
    rt.update(|_, tx| tx.blur());
    rt.frame(1.0 / 60.0);
    assert!(!rt.state.edit.focused);
    assert!(!rt.state.edit.is_composing());
}

#[test]
fn offscreen_virtual_focused_tracked_field_does_not_bump_and_blurs_when_inert() {
    struct App {
        edits: Vec<Tracked<TextEdit>>,
        inert: Tracked<bool>,
        counter: Tracked<u32>,
    }

    fn view(s: &App, _: &mut Cx<App>) -> El {
        let style = TextStyle::new(14.0, 400.0);
        let mut vp = col().key("viewport").size(200.0, 120.0)
            .scroll(ScrollSpec { stick_bottom: false })
            .clip_soft(0.0)
            .child(
                col().key("rows").w_full().growth(GrowthSpec::default()).virtualize(40.0).children(
                    s.edits.iter().enumerate().map(|(i, edit)| {
                        text_input(edit, style)
                            .key(format!("row-{i}"))
                            .label("Row field")
                            .size(200.0, 40.0)
                            .on_edit(move |s: &mut App| &mut *s.edits[i])
                    }),
                ),
            );
        if *s.inert {
            vp = vp.inert();
        }
        col().size(200.0, 200.0).children([
            vp,
            region("virt-counter", || text(format!("c {}", *s.counter)).key("virt-counter").font(style)),
        ])
    }

    let mut edits: Vec<_> = (0..60).map(|_| Tracked::new(TextEdit::new(""))).collect();
    edits[0].focused = true;
    let mut rt = Runtime::new(
        App { edits, inert: Tracked::new(false), counter: Tracked::new(0) },
        view,
        (200.0, 200.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    let path = rt.kbd.focus.owned().expect("row 0 focused");

    // Scroll row 0 offscreen into virtual_inputs.
    rt.wheel(100.0, 60.0, -1500.0);
    rt.frame(1.0 / 60.0);
    assert!(rt.node(&path).is_none(), "row 0 should be offscreen");
    assert_eq!(rt.kbd.focus.owned().as_deref(), Some(path.as_str()));

    let v_before = Tracked::version(&rt.state.edits[0]);
    for _ in 0..10 {
        rt.update(|s, _| *s.counter += 1);
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(
        Tracked::version(&rt.state.edits[0]),
        v_before,
        "offscreen focused virtual input must not bump Tracked<TextEdit> on unrelated commits",
    );

    // Turning the viewport inert must still clear focused on the app's TextEdit.
    rt.update(|s, _| *s.inert = true);
    rt.frame(1.0 / 60.0);
    assert!(!rt.state.edits[0].focused, "turning an offscreen virtual input inert must clear edit.focused");
    assert!(rt.kbd.focus.path().is_none());
}

#[test]
fn premutated_edit_clone_still_syncs_flags_and_keeps_tracked_edit_clean() {
    struct App {
        secret: Tracked<TextEdit>,
        code: Tracked<TextEdit>,
        revealed: Tracked<bool>,
        line_home_end: Tracked<bool>,
        counter: Tracked<u32>,
    }

    fn view(s: &App, _: &mut Cx<App>) -> El {
        let style = TextStyle::new(14.0, 400.0);
        col().size(400.0, 240.0).children([
            region("clone-fields", || {
                // Mirror wrapper widgets that clone `TextEdit` and mutate flags on the clone
                // before calling `text_input`.
                let mut secret_copy = (*s.secret).clone();
                secret_copy.set_secure(true);
                secret_copy.set_revealed(*s.revealed);
                let mut code_copy = (*s.code).clone();
                code_copy.set_line_home_end(*s.line_home_end);
                col().key("clone-fields").w_full().children([
                    text_input(&secret_copy, style)
                        .key("secret")
                        .label("Secret")
                        .size(200.0, 28.0)
                        .single_line()
                        .revealed(*s.revealed)
                        .on_edit(|s: &mut App| &mut *s.secret),
                    text_input(&code_copy, style)
                        .key("code")
                        .label("Code")
                        .size(200.0, 60.0)
                        .line_home_end(*s.line_home_end)
                        .on_edit(|s: &mut App| &mut *s.code),
                ])
            }),
            region("clone-counter", || {
                text(format!("tick {}", *s.counter)).key("clone-counter").font(style)
            }),
        ])
    }

    let mut secret = TextEdit::new("pw");
    secret.focused = true;
    let mut rt = Runtime::new(
        App {
            secret: Tracked::new(secret),
            code: Tracked::new(TextEdit::new("a\nb")),
            revealed: Tracked::new(true),
            line_home_end: Tracked::new(true),
            counter: Tracked::new(0),
        },
        view,
        (400.0, 240.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    // First commit must sync flags into both app-owned models even though the clones already had them.
    assert!(rt.state.secret.is_secure() && rt.state.secret.is_revealed() && rt.state.secret.is_single_line());
    assert!(rt.state.code.line_home_end());

    // Subsequent unrelated commits must keep both Tracked<TextEdit>s clean.
    let v_secret = Tracked::version(&rt.state.secret);
    let v_code = Tracked::version(&rt.state.code);
    let builds = rt.commit.memo_builds();
    for _ in 0..10 {
        rt.update(|s, _| *s.counter += 1);
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(Tracked::version(&rt.state.secret), v_secret);
    assert_eq!(Tracked::version(&rt.state.code), v_code);
    assert_eq!(rt.commit.memo_builds() - builds, 10);

    // Dynamic flag flips must still propagate to the app-owned models, then settle again.
    rt.update(|s, _| {
        *s.revealed = false;
        *s.line_home_end = false;
    });
    rt.frame(1.0 / 60.0);
    assert!(rt.state.secret.is_secure() && !rt.state.secret.is_revealed());
    assert!(!rt.state.code.line_home_end());

    let v_secret2 = Tracked::version(&rt.state.secret);
    let v_code2 = Tracked::version(&rt.state.code);
    for _ in 0..5 {
        rt.update(|s, _| *s.counter += 1);
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(Tracked::version(&rt.state.secret), v_secret2);
    assert_eq!(Tracked::version(&rt.state.code), v_code2);
}

#[test]
fn preconfigured_overrides_keep_tracked_edit_and_region_clean_from_next_commit() {
    struct App {
        secret: Tracked<TextEdit>,
        code: Tracked<TextEdit>,
        counter: Tracked<u32>,
    }

    fn view(s: &App, _: &mut Cx<App>) -> El {
        let style = TextStyle::new(14.0, 400.0);
        col().size(400.0, 240.0).children([
            region("preconfig-fields", || {
                col().key("preconfig-fields").w_full().children([
                    text_input(&s.secret, style)
                        .key("secret")
                        .label("Secret")
                        .size(200.0, 28.0)
                        .single_line()
                        .secure()
                        .revealed(true)
                        .on_edit(|s: &mut App| &mut *s.secret),
                    text_input(&s.code, style)
                        .key("code")
                        .label("Code")
                        .size(200.0, 60.0)
                        .line_home_end(true)
                        .on_edit(|s: &mut App| &mut *s.code),
                ])
            }),
            region("preconfig-counter", || {
                text(format!("tick {}", *s.counter)).key("preconfig-counter").font(style)
            }),
        ])
    }

    let mut secret = TextEdit::new("pw").secure().revealed(true);
    secret.focused = true;
    let mut code = TextEdit::new("a\nb");
    code.set_line_home_end(true);
    let mut rt = Runtime::new(
        App {
            secret: Tracked::new(secret),
            code: Tracked::new(code),
            counter: Tracked::new(0),
        },
        view,
        (400.0, 240.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);

    let v_secret = Tracked::version(&rt.state.secret);
    let v_code = Tracked::version(&rt.state.code);
    let builds = rt.commit.memo_builds();
    for _ in 0..10 {
        rt.update(|s, _| *s.counter += 1);
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(Tracked::version(&rt.state.secret), v_secret);
    assert_eq!(Tracked::version(&rt.state.code), v_code);
    assert_eq!(rt.commit.memo_builds() - builds, 10);
}

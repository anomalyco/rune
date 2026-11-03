//! An open menu's ghost trigger holds the pressed fill: it lands the moment the press opens the
//! menu, with no hover or press overlay on top of it.
use rune::{Cx, El, Env, Runtime, col};
use rune_kit::*;

fn view(open: &bool, cx: &mut Cx<bool>) -> El {
    let k = Kit::neutral(cx.system_dark());
    let button = Button::new("Sort").variant(ButtonVariant::Ghost).expanded(*open).on_press(|o: &mut bool| *o ^= true);
    let icon = IconButton::new(k.glyph.copy)
        .variant(IconButtonVariant::GhostMuted)
        .expanded(*open)
        .on_press(|o: &mut bool| *o ^= true);
    col().w_full().h_full().child(button.build(&k).key("button")).child(icon.build(&k).key("icon"))
}

fn settle(rt: &mut Runtime<bool>) {
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
}

fn fills(rt: &Runtime<bool>) -> Vec<[f32; 4]> {
    rt.instances.iter().map(|i| i.fill).collect()
}

#[test]
fn an_open_trigger_draws_the_pressed_fill_alone() {
    let k = Kit::neutral(true);
    for key in ["button", "icon"] {
        let mut rt = Runtime::new(false, view, (200.0, 200.0), Env::new(true, 1.0));
        rt.frame(0.0);
        let info = |rt: &Runtime<bool>| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap();
        let [x, y, w, h] = info(&rt).rect;
        let (cx, cy) = (x + w / 2.0, y + h / 2.0);

        rt.pointer_move(cx, cy);
        settle(&mut rt);
        assert!(info(&rt).hover > 0.9, "{key}: a closed trigger shows hover");

        rt.pointer_down(cx, cy);
        settle(&mut rt);
        rt.pointer_up(cx, cy);
        rt.frame(1.0 / 60.0);
        assert!(rt.state, "{key}: the press opens it");
        assert_eq!(info(&rt).fill, k.c.pressed, "{key}: the pressed fill lands on release, not easing in from clear");
        settle(&mut rt);
        assert_eq!(info(&rt).fill, k.c.pressed, "{key}: an open trigger fills with pressed");
        let under_pointer = fills(&rt);

        rt.pointer_down(cx, cy);
        settle(&mut rt);
        assert_eq!(fills(&rt), under_pointer, "{key}: holding a press on an open trigger doesn't lighten it");

        rt.pointer_move(190.0, 190.0);
        settle(&mut rt);
        assert_eq!(fills(&rt), under_pointer, "{key}: an open trigger draws the same with and without the pointer");
    }
}

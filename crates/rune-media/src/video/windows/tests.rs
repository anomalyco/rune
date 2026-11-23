//! The UI thread's side of a clip, driven with a pipe no decoder thread reads: the tests play
//! the decoder's part by writing the inbox.

use super::inbox::Inbox;
use super::*;
use crate::native_surface::{ColorInfo, Format, Orient};
use crate::video::phases;
use std::sync::MutexGuard;

const FRAME: f64 = 1.0 / 30.0;

/// A 10 s, 30 fps clip, opened, with its decoder's pipe.
struct Rig {
    player: Player,
    pipe: Arc<Pipe>,
}

fn rig() -> Rig {
    let player = Player::file("missing.mp4");
    let pipe = Pipe::new(Inbox::default());
    let meta = Meta { duration: 10.0, size: Some((2, 2)), frame: FRAME, sound: false };
    let picture = mf::Picture { coded: (2, 2), crop: (0, 0, 2, 2), natural: (2, 2), format: Format::Nv12Video, color: ColorInfo::default(), orient: Orient::UP, frame: FRAME };
    player.shared.ctl.lock().unwrap().open(Ok(meta));
    let mut native = player.shared.native.lock().unwrap();
    native.pipe = Some(pipe.clone());
    native.facts = Some(Facts { meta, picture: Some(picture), sound: None, hardware: false, gpu: false });
    native.rate = 1.0;
    drop(native);
    Rig { player, pipe }
}

impl Rig {
    fn native(&self) -> MutexGuard<'_, Native> {
        self.player.shared.native.lock().unwrap()
    }

    /// The handle's edit, as `Player::play`, `pause` and `seek` record it.
    fn edit(&self, event: phases::Event) {
        phases::step(&mut self.player.shared.ctl.lock().unwrap(), event);
    }

    /// Starts from `phase` with the picture at `shown` on the surface.
    fn stand(&self, phase: Phase, shown: Option<f64>) {
        let mut native = self.native();
        native.phase = phase;
        native.shown = shown;
        native.playing = self.player.is_playing();
    }

    /// Folds the handle's intent and the decoder's news in, as a pull or an edit does.
    fn reconcile(&self) {
        let mut native = self.native();
        let mut c = self.player.shared.ctl.lock().unwrap();
        native.reconcile(&self.player.shared, &mut c, rune_core::time::now(), false);
    }

    /// A frame draws the clip, presenting at `target`.
    fn show(&self, target: Instant) {
        let mut native = self.native();
        let mut c = self.player.shared.ctl.lock().unwrap();
        native.show(&self.player.shared, &mut c, self.player.surface(), target, 0.0);
    }

    fn phase(&self) -> Phase {
        self.native().phase
    }

    /// The decoder reached the end of the current seek's pictures.
    fn decoder_ends(&self) {
        let generation = self.native().seeks.current().0;
        self.pipe.lock().ended = Some(generation);
    }
}

#[test]
fn a_paused_seek_the_decoder_ends_with_no_picture_stops_buffering_and_holds() {
    let rig = rig();
    rig.stand(Phase::Paused { at: 2.0 }, Some(2.0));
    rig.edit(phases::Event::Seek(9.99));
    rig.reconcile();
    assert_eq!(rig.phase(), Phase::Buffering { at: 9.99, play: false });
    rig.decoder_ends();
    rig.show(rune_core::time::now());
    assert_eq!(rig.phase(), Phase::Paused { at: 9.99 });
    assert_eq!(rig.player.state(), PlayerState::Ready);
    // Play from there has nothing left to show: the clip ends at once.
    rig.edit(phases::Event::Play);
    rig.reconcile();
    rig.show(rune_core::time::now());
    assert!(matches!(rig.phase(), Phase::Ended { .. }), "{:?}", rig.phase());
    assert_eq!(rig.player.state(), PlayerState::Ended);
}

#[test]
fn a_playing_seek_the_decoder_ends_with_no_picture_ends_the_clip() {
    let rig = rig();
    rig.edit(phases::Event::Play);
    rig.stand(Phase::Paused { at: 2.0 }, Some(2.0));
    rig.native().playing = false;
    rig.edit(phases::Event::Seek(9.99));
    rig.reconcile();
    assert_eq!(rig.phase(), Phase::Buffering { at: 9.99, play: true });
    // Still waiting while the decoder hasn't said it ended.
    rig.show(rune_core::time::now());
    assert_eq!(rig.phase(), Phase::Buffering { at: 9.99, play: true });
    rig.decoder_ends();
    rig.show(rune_core::time::now());
    assert_eq!(rig.phase(), Phase::Ended { at: 9.99 });
    assert_eq!(rig.player.state(), PlayerState::Ended);
    assert_eq!(rig.player.time(), 10.0);
}

#[test]
fn a_seek_on_a_parked_paused_clip_forgets_its_picture_so_it_opens_when_drawn() {
    let rig = rig();
    rig.stand(Phase::Idle { at: 3.0 }, Some(3.0));
    rig.native().pipe = None;
    rig.edit(phases::Event::Seek(7.5));
    rig.reconcile();
    assert_eq!(rig.phase(), Phase::Idle { at: 7.5 });
    // No picture on screen is what opens a paused clip in the frame that draws it.
    assert_eq!(rig.native().shown, None);
}

#[test]
fn a_clip_let_go_while_it_waits_for_a_seeks_picture_opens_again_at_the_seek() {
    let rig = rig();
    rig.stand(Phase::Buffering { at: 7.5, play: false }, None);
    rig.pipe.lock().news.push(News::Parked);
    rig.reconcile();
    assert_eq!(rig.phase(), Phase::Idle { at: 7.5 });
}

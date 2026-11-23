//! The UI thread's side of a clip, driven with a pipe no decoder thread reads: the tests play
//! the decoder's part by writing the inbox.

use super::inbox::Inbox;
use super::*;
use crate::native_surface::{ByteFrame, ColorInfo, Format, Orient, Source};
use machine::Anchor;
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

    fn generation(&self) -> u32 {
        self.native().seeks.current().0
    }

    /// The decoder queues the picture at `pts` for the window.
    fn decodes(&self, pts: f64) {
        self.pipe.lock().push(picture(self.generation(), pts));
    }

    /// The decoder presents the picture at `pts` on the clip's layer.
    fn presents(&self, pts: f64) {
        self.pipe.lock().push_presented(picture(self.generation(), pts), rune_core::time::now());
    }

    /// A playing clip showing the picture at 2 s, with its clock started at `start`.
    fn plays_from_two(&self, start: Instant) {
        self.edit(phases::Event::Play);
        self.stand(Phase::Buffering { at: 2.0, play: true }, None);
        self.decodes(2.0);
        self.show(start);
        assert_eq!(self.phase(), Phase::Playing { clock: Anchor { at: start, pos: 2.0, rate: 1.0 } });
        assert_eq!(self.on_surface(), Some(20));
    }

    /// The window's frame while the clip's place is still: it asks for the layer at the clip's
    /// size, and the decoder has presented there.
    fn window_layers(&self) -> LayerStatus {
        self.pipe.lock().layer_shown = Some((SIZE, self.generation()));
        layer(&self.player.shared, Some(SIZE))
    }

    /// The window's frame once the layer can't show the clip (the clock stopped, its place
    /// moved): it asks for no layer and draws the clip itself.
    fn window_draws(&self) {
        layer(&self.player.shared, Some(SIZE));
        layer(&self.player.shared, None);
        unlayer(&self.player.shared, self.player.surface());
    }

    /// The picture on the surface, as `picture` marks it: its time in tenths of a second.
    fn on_surface(&self) -> Option<u8> {
        match &self.player.surface().latest()?.source {
            Source::Bytes(bytes) => bytes.planes[0].first().copied(),
            _ => None,
        }
    }

    /// What the decoder is asked for: a seek, sound and looping. Read out before it's compared,
    /// so a failed assertion never unwinds with the inbox locked.
    fn asked(&self) -> (Option<(f64, u32)>, Option<f32>, bool) {
        let inbox = self.pipe.lock();
        (inbox.seek, inbox.sound, inbox.looping)
    }

    /// The decoder reached the end of the current seek's pictures.
    fn decoder_ends(&self) {
        let generation = self.native().seeks.current().0;
        self.pipe.lock().ended = Some(generation);
    }
}

const SIZE: (u32, u32) = (2, 2);

/// A decoded picture whose luma says its time, in tenths of a second.
fn picture(generation: u32, pts: f64) -> Decoded {
    let luma = (pts * 10.0).round() as u8;
    let bytes = ByteFrame { width: 2, height: 2, format: Format::Nv12Video, color: ColorInfo::default(), planes: [vec![luma; 4], vec![128; 2]] };
    Decoded { generation, pts, picture: Pic::Cpu(bytes) }
}

#[test]
fn a_pause_on_the_layer_holds_the_picture_and_the_time_the_layer_shows() {
    let rig = rig();
    rig.plays_from_two(rune_core::time::now());
    // The clip goes on its layer and plays there for three seconds: no frame pulls a picture.
    assert!(rig.window_layers().showing);
    rig.presents(4.9);
    rig.presents(5.0);
    rig.edit(phases::Event::Pause);
    rig.reconcile();
    assert_eq!(rig.phase(), Phase::Paused { at: 5.0 });
    assert_eq!(rig.player.time(), 5.0);
    assert_eq!(rig.native().clock.get(), None);
    // The window draws the clip again: the surface gets the picture the layer showed.
    rig.window_draws();
    assert_eq!(rig.on_surface(), Some(50));
    assert_eq!(rig.native().screen, Screen::Surface);
    assert_eq!(rig.native().shown, Some(5.0));
    // Play goes on from there.
    rig.edit(phases::Event::Play);
    rig.reconcile();
    let Phase::Playing { clock } = rig.phase() else { panic!("{:?}", rig.phase()) };
    assert!((5.0..5.1).contains(&clock.pos), "{clock:?}");
}

#[test]
fn a_clip_that_ends_on_its_layer_shows_its_last_picture() {
    let rig = rig();
    let start = rune_core::time::now();
    rig.plays_from_two(start);
    assert!(rig.window_layers().showing);
    rig.presents(9.9);
    rig.decoder_ends();
    // The frame that hears of the end, once the clock passed it.
    rig.show(start + Duration::from_secs(9));
    assert_eq!(rig.phase(), Phase::Ended { at: 9.9 });
    assert_eq!(rig.player.state(), PlayerState::Ended);
    rig.window_draws();
    assert_eq!(rig.on_surface(), Some(99));
}

#[test]
fn a_playing_clip_the_window_takes_back_from_its_layer_goes_on_from_the_layers_picture() {
    let rig = rig();
    let start = rune_core::time::now();
    rig.plays_from_two(start);
    assert!(rig.window_layers().showing);
    rig.presents(5.0);
    // The clip's place moves: the window asks for no layer.
    layer(&rig.player.shared, None);
    assert_eq!(rig.native().screen, Screen::Behind);
    assert_eq!(rig.native().shown, Some(5.0));
    assert_eq!(rig.player.time(), 5.0);
    rig.decodes(5.1);
    rig.show(start + Duration::from_millis(3050));
    assert_eq!(rig.on_surface(), Some(50));
    assert_eq!(rig.native().screen, Screen::Surface);
}

#[test]
fn a_pause_after_the_window_let_go_holds_a_picture_the_layer_showed_since() {
    let rig = rig();
    rig.plays_from_two(rune_core::time::now());
    assert!(rig.window_layers().showing);
    rig.presents(5.0);
    // The window lets go, and the decoder, already past its checks, presents one more.
    layer(&rig.player.shared, None);
    rig.presents(5.1);
    rig.edit(phases::Event::Pause);
    rig.reconcile();
    assert_eq!(rig.phase(), Phase::Paused { at: 5.1 });
    assert_eq!(rig.player.time(), 5.1);
    rig.show(rune_core::time::now());
    assert_eq!(rig.on_surface(), Some(51));
}

#[test]
fn a_paused_clip_whose_held_picture_left_the_queue_decodes_it_again() {
    let rig = rig();
    rig.plays_from_two(rune_core::time::now());
    assert!(rig.window_layers().showing);
    rig.presents(5.0);
    rig.edit(phases::Event::Pause);
    rig.reconcile();
    rig.pipe.lock().frames.clear();
    rig.window_draws();
    assert_eq!(rig.phase(), Phase::Buffering { at: 5.0, play: false });
    assert_eq!(rig.asked().0, Some((5.0, rig.generation())));
    // Its picture lands as any seek's does.
    rig.pipe.lock().seek = None;
    rig.decodes(5.0);
    rig.show(rune_core::time::now());
    assert_eq!(rig.phase(), Phase::Paused { at: 5.0 });
    assert_eq!(rig.on_surface(), Some(50));
}

#[test]
fn a_clip_let_go_while_its_surface_is_behind_opens_again_when_drawn() {
    let rig = rig();
    rig.plays_from_two(rune_core::time::now());
    // Paused with its picture on the surface: let go, it keeps that picture and needs no decoder.
    rig.edit(phases::Event::Pause);
    rig.reconcile();
    assert!(!rig.native().wants_decoder(false, Kind::Video));
    // Paused on the layer, and never drawn again before the decoder lets go.
    rig.edit(phases::Event::Play);
    rig.reconcile();
    assert!(rig.window_layers().showing);
    rig.presents(5.0);
    rig.edit(phases::Event::Pause);
    rig.reconcile();
    layer(&rig.player.shared, None);
    rig.pipe.lock().news.push(News::Parked);
    rig.reconcile();
    assert_eq!(rig.phase(), Phase::Idle { at: 5.0 });
    assert!(rig.native().wants_decoder(false, Kind::Video));
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

#[test]
fn a_clip_that_hasnt_opened_yet_keeps_the_sound_its_decoder_was_asked_for() {
    let rig = rig();
    // The decoder started with sound on, and hasn't said yet what the clip holds.
    rig.native().facts = None;
    rig.pipe.lock().sound = Some(1.0);
    rig.player.shared.ctl.lock().unwrap().muted = false;
    rig.stand(Phase::Opening { at: 0.0, play: false }, None);
    rig.reconcile();
    assert_eq!(rig.asked().1, Some(1.0));
    // Looping still reaches the decoder before it opens.
    rig.player.shared.ctl.lock().unwrap().looping = true;
    rig.reconcile();
    assert_eq!(rig.asked(), (None, Some(1.0), true));
}

#[test]
fn an_opened_clip_follows_the_handles_mute() {
    let rig = rig();
    let sound = mf::Sound { channels: 2, rate: 48_000 };
    rig.native().facts.as_mut().unwrap().sound = Some(sound);
    rig.player.shared.ctl.lock().unwrap().muted = false;
    rig.stand(Phase::Paused { at: 0.0 }, Some(0.0));
    rig.reconcile();
    assert_eq!(rig.asked().1, Some(1.0));
    rig.player.shared.ctl.lock().unwrap().muted = true;
    rig.reconcile();
    assert_eq!(rig.asked().1, None);
}
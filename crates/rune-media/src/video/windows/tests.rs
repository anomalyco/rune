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
    let meta = Meta { duration: 10.0, size: Some((2, 2)), frame: Some(FRAME), sound: false };
    let picture = mf::Picture { coded: (2, 2), crop: (0, 0, 2, 2), natural: (2, 2), format: Format::Nv12Video, color: ColorInfo::default(), orient: Orient::UP, frame_duration: Some(FRAME) };
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
        self.stand(Phase::Buffering { at: 2.0, play: true, seeking: false }, None);
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
    fn asked(&self) -> (Option<SeekAsk>, Option<f32>, bool) {
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
    assert_eq!(rig.phase(), Phase::Buffering { at: 5.0, play: false, seeking: true });
    assert_eq!(rig.asked().0, Some(SeekAsk { to: 5.0, generation: rig.generation(), exact: true }));
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
    assert_eq!(rig.phase(), Phase::Idle { at: 5.0, seek: None });
    assert!(rig.native().wants_decoder(false, Kind::Video));
}

#[test]
fn a_paused_seek_the_decoder_ends_with_no_picture_stops_buffering_and_holds() {
    let rig = rig();
    rig.stand(Phase::Paused { at: 2.0 }, Some(2.0));
    rig.edit(phases::Event::Seek { to: 9.99, exact: true });
    rig.reconcile();
    assert_eq!(rig.phase(), Phase::Buffering { at: 9.99, play: false, seeking: true });
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
    rig.edit(phases::Event::Seek { to: 9.99, exact: true });
    rig.reconcile();
    assert_eq!(rig.phase(), Phase::Buffering { at: 9.99, play: true, seeking: true });
    // Still waiting while the decoder hasn't said it ended.
    rig.show(rune_core::time::now());
    assert_eq!(rig.phase(), Phase::Buffering { at: 9.99, play: true, seeking: true });
    rig.decoder_ends();
    rig.show(rune_core::time::now());
    assert_eq!(rig.phase(), Phase::Ended { at: 9.99 });
    assert_eq!(rig.player.state(), PlayerState::Ended);
    assert_eq!(rig.player.time(), 10.0);
}

#[test]
fn a_seek_on_a_parked_paused_clip_forgets_its_picture_so_it_opens_when_drawn() {
    let rig = rig();
    rig.stand(Phase::Idle { at: 3.0, seek: None }, Some(3.0));
    rig.native().pipe = None;
    rig.edit(phases::Event::Seek { to: 7.5, exact: true });
    rig.reconcile();
    assert_eq!(rig.phase(), Phase::Idle { at: 7.5, seek: Some(machine::SeekMode::Exact) });
    // No picture on screen is what opens a paused clip in the frame that draws it.
    assert_eq!(rig.native().shown, None);
}

#[test]
fn a_clip_let_go_while_it_waits_for_a_seeks_picture_opens_again_at_the_seek() {
    let rig = rig();
    rig.stand(Phase::Buffering { at: 7.5, play: false, seeking: true }, None);
    rig.pipe.lock().news.push(News::Parked);
    rig.reconcile();
    assert_eq!(rig.phase(), Phase::Idle { at: 7.5, seek: None });
}

#[test]
fn a_clip_that_hasnt_opened_yet_keeps_the_sound_its_decoder_was_asked_for() {
    let rig = rig();
    // The decoder started with sound on, and hasn't said yet what the clip holds.
    rig.native().facts = None;
    rig.pipe.lock().sound = Some(1.0);
    rig.player.shared.ctl.lock().unwrap().muted = false;
    rig.stand(Phase::Opening { at: 0.0, play: false, seeking: false }, None);
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

#[test]
fn unmuting_mid_clip_turns_sound_on_without_seeking_or_interrupting_playback() {
    let rig = rig();
    let sound = mf::Sound { channels: 2, rate: 48_000 };
    rig.native().facts.as_mut().unwrap().sound = Some(sound);
    let start = rune_core::time::now();
    rig.plays_from_two(start);
    // Playing muted at 2 s: no sound ask yet.
    rig.reconcile();
    assert_eq!(rig.asked(), (None, None, false));
    // Unmute mid-clip: sound turns on, playback stays Playing with no seek or buffering gap.
    rig.player.shared.ctl.lock().unwrap().muted = false;
    rig.reconcile();
    assert!(matches!(rig.phase(), Phase::Playing { .. }), "{:?}", rig.phase());
    assert_eq!(rig.asked(), (None, Some(1.0), false));
}

#[test]
fn trailing_audio_past_last_picture_holds_without_stalling_idles_next_and_ends_after_drain() {
    let rig = rig();
    rig.native().facts.as_mut().unwrap().sound = Some(mf::Sound { channels: 2, rate: 48_000 });
    rig.player.shared.ctl.lock().unwrap().muted = false;
    let start = rune_core::time::now();
    rig.plays_from_two(start);
    // Last video picture at 9.967 s; video hits EOS while trailing audio continues to 12.5 s.
    rig.decodes(9.967);
    let t_last = start + Duration::from_millis(7_970);
    rig.show(t_last);
    assert_eq!(rig.native().shown, Some(9.967));
    rig.pipe.lock().video_ended = Some(rig.generation());

    // 1.5 s past the last video frame (pos = 11.5 s): must stay Playing (no Stall) with clock running.
    let t_mid = start + Duration::from_millis(9_500);
    rig.show(t_mid);
    assert!(matches!(rig.phase(), Phase::Playing { .. }), "trailing audio stalled: {:?}", rig.phase());
    assert!(rig.native().clock.get().is_some(), "clock stopped during trailing audio");
    // Frame queue is empty and video ended: next() idles instead of spinning every vsync.
    {
        let native = rig.native();
        let c = rig.player.shared.ctl.lock().unwrap();
        assert_eq!(native.next(&c, t_mid), (false, None));
    }

    // Audio stream reaches EOS at 12.5 s while clock is at 11.5 s: still holds and idles until drain.
    {
        let mut inbox = rig.pipe.lock();
        inbox.ended = Some(rig.generation());
        inbox.audio_end = Some(12.5);
    }
    rig.show(t_mid);
    assert!(matches!(rig.phase(), Phase::Playing { .. }), "ended before audio drained: {:?}", rig.phase());
    {
        let native = rig.native();
        let c = rig.player.shared.ctl.lock().unwrap();
        assert_eq!(native.next(&c, t_mid), (false, None));
    }

    // Once clock passes audio_end (12.5 s), playback ends cleanly.
    rig.show(start + Duration::from_millis(10_550));
    assert_eq!(rig.phase(), Phase::Ended { at: 9.967 });
    assert_eq!(rig.player.state(), PlayerState::Ended);
}

#[test]
fn muting_during_trailing_audio_drain_ends_immediately() {
    let rig = rig();
    rig.native().facts.as_mut().unwrap().sound = Some(mf::Sound { channels: 2, rate: 48_000 });
    rig.player.shared.ctl.lock().unwrap().muted = false;
    let start = rune_core::time::now();
    rig.plays_from_two(start);
    rig.decodes(9.967);
    rig.show(start + Duration::from_millis(7_970));
    {
        let mut inbox = rig.pipe.lock();
        inbox.video_ended = Some(rig.generation());
        inbox.ended = Some(rig.generation());
        inbox.audio_end = Some(12.5);
    }
    // Unmuted at 11.5 s: still playing while trailing audio drains.
    let t_mid = start + Duration::from_millis(9_500);
    rig.show(t_mid);
    assert!(matches!(rig.phase(), Phase::Playing { .. }));
    // Mute mid-drain: ends immediately instead of waiting silently until 12.5 s.
    rig.player.shared.ctl.lock().unwrap().muted = true;
    rig.reconcile();
    rig.show(t_mid);
    assert_eq!(rig.phase(), Phase::Ended { at: 9.967 });
    assert_eq!(rig.player.state(), PlayerState::Ended);
}

#[test]
fn a_clip_on_its_layer_waits_for_trailing_audio_before_ending() {
    let rig = rig();
    rig.native().facts.as_mut().unwrap().sound = Some(mf::Sound { channels: 2, rate: 48_000 });
    rig.player.shared.ctl.lock().unwrap().muted = false;
    let start = rune_core::time::now();
    rig.plays_from_two(start);
    assert!(rig.window_layers().showing);
    rig.presents(9.9);
    {
        let mut inbox = rig.pipe.lock();
        inbox.video_ended = Some(rig.generation());
        inbox.ended = Some(rig.generation());
        inbox.audio_end = Some(12.5);
    }
    // Past duration (10 s) at 11.5 s, still waits for trailing audio to drain to 12.5 s.
    let t_mid = start + Duration::from_millis(9_500);
    rig.show(t_mid);
    assert!(matches!(rig.phase(), Phase::Playing { .. }));
    {
        let native = rig.native();
        let c = rig.player.shared.ctl.lock().unwrap();
        assert_eq!(native.next(&c, t_mid), (false, None));
    }
    // Past 12.5 s: ends cleanly.
    rig.show(start + Duration::from_millis(10_550));
    assert_eq!(rig.phase(), Phase::Ended { at: 9.9 });
}

#[test]
fn reconcile_folds_nudged_audio_clock_into_playing_phase() {
    let rig = rig();
    let start = rune_core::time::now();
    rig.plays_from_two(start);
    let Phase::Playing { clock } = rig.phase() else { panic!("{:?}", rig.phase()) };
    let nudged = Anchor { pos: clock.pos + 0.006, ..clock };
    rig.native().clock.nudge(clock, 0.006);
    rig.reconcile();
    assert_eq!(rig.phase(), Phase::Playing { clock: nudged });
    assert_eq!(rig.native().clock.get(), Some(nudged));
}

#[test]
fn layer_showing_reports_playing_layer_without_mutating_state_and_false_after_pause_seek_or_end() {
    let r = rig();
    let start = rune_core::time::now();
    r.plays_from_two(start);
    assert!(r.window_layers().showing);
    assert_eq!(r.native().screen, Screen::Layer);
    assert_eq!(r.pipe.lock().layer, Some(SIZE));

    assert!(layer_showing(&r.player.shared, SIZE));
    assert!(!layer_showing(&r.player.shared, (SIZE.0 + 2, SIZE.1)), "different size is not showing");
    assert_eq!(r.native().screen, Screen::Layer, "read-only check leaves screen untouched");
    assert_eq!(r.pipe.lock().layer, Some(SIZE), "read-only check leaves decoder ask untouched");

    // Refusing the layer makes layer_showing false without altering screen.
    r.pipe.lock().layer_refused = true;
    assert!(!layer_showing(&r.player.shared, SIZE));
    assert_eq!(r.native().screen, Screen::Layer);
    r.pipe.lock().layer_refused = false;
    assert!(layer_showing(&r.player.shared, SIZE));

    // Pausing leaves Screen::Layer intact until unlayer runs, yet layer_showing turns false.
    r.edit(phases::Event::Pause);
    r.reconcile();
    assert_eq!(r.native().screen, Screen::Layer);
    assert!(!layer_showing(&r.player.shared, SIZE));

    // Seeking while layered also makes layer_showing false.
    r.edit(phases::Event::Play);
    r.stand(Phase::Playing { clock: Anchor { at: start, pos: 2.0, rate: 1.0 } }, Some(2.0));
    assert!(r.window_layers().showing);
    assert!(layer_showing(&r.player.shared, SIZE));
    r.edit(phases::Event::Seek { to: 5.0, exact: true });
    r.reconcile();
    assert!(!layer_showing(&r.player.shared, SIZE));

    // Ending while layered also makes layer_showing false.
    let rig_end = rig();
    rig_end.plays_from_two(start);
    assert!(rig_end.window_layers().showing);
    assert!(layer_showing(&rig_end.player.shared, SIZE));
    rig_end.pipe.lock().ended = Some(rig_end.generation());
    rig_end.show(start + Duration::from_secs(10));
    assert!(matches!(rig_end.phase(), Phase::Ended { .. }));
    assert!(!layer_showing(&rig_end.player.shared, SIZE));
}

#[test]
fn seek_fast_asks_for_keyframe_reports_keyframe_time_and_tracks_is_seeking() {
    let rig = rig();
    rig.stand(Phase::Paused { at: 1.0 }, Some(1.0));
    assert!(!rig.player.is_seeking());

    // Fast seek while paused: is_seeking() turns true immediately (even before reconcile).
    rig.player.seek_fast(5.7);
    assert!(rig.player.is_seeking());
    assert_eq!(rig.player.state(), PlayerState::Ready);
    rig.reconcile();
    assert!(rig.player.is_seeking());
    assert_eq!(rig.phase(), Phase::Buffering { at: 5.7, play: false, seeking: true });
    assert_eq!(rig.asked().0, Some(SeekAsk { to: 5.7, generation: rig.generation(), exact: false }));

    // A stale picture from an earlier generation does not satisfy the seek.
    let stale_gen = rig.generation().wrapping_sub(1);
    rig.pipe.lock().push(picture(stale_gen, 1.1));
    rig.show(rune_core::time::now());
    assert!(rig.player.is_seeking(), "stale generation picture must not clear is_seeking");

    // The keyframe for the current seek generation lands at 4.0 s: time reports 4.0 and seeking clears.
    rig.decodes(4.0);
    rig.show(rune_core::time::now());
    assert!(!rig.player.is_seeking());
    assert_eq!(rig.phase(), Phase::Paused { at: 4.0 });
    assert_eq!(rig.player.time(), 4.0);
    assert_eq!(rig.on_surface(), Some(40));

    // Exact seek on scrub release replaces with exact=true and lands at 5.7 s.
    rig.player.seek(5.7);
    rig.reconcile();
    assert!(rig.player.is_seeking());
    assert_eq!(rig.asked().0, Some(SeekAsk { to: 5.7, generation: rig.generation(), exact: true }));
    rig.decodes(5.7);
    rig.show(rune_core::time::now());
    assert!(!rig.player.is_seeking());
    assert_eq!(rig.player.time(), 5.7);
    assert_eq!(rig.on_surface(), Some(57));
}

#[test]
fn seek_fast_while_playing_resumes_from_keyframe_and_stalls_do_not_report_seeking() {
    let rig = rig();
    let start = rune_core::time::now();
    rig.plays_from_two(start);
    assert!(!rig.player.is_seeking());

    // A playback stall enters Buffering with seeking == false.
    rig.show(start + Duration::from_secs(2));
    assert!(matches!(rig.phase(), Phase::Buffering { play: true, seeking: false, .. }));
    assert!(!rig.player.is_seeking(), "playback stalls must not report is_seeking");

    // Recover playback, then issue seek_fast while playing.
    rig.decodes(2.1);
    rig.show(start + Duration::from_secs(2));
    assert!(matches!(rig.phase(), Phase::Playing { .. }));

    rig.player.seek_fast(7.4);
    rig.reconcile();
    assert!(rig.player.is_seeking());
    assert_eq!(rig.player.state(), PlayerState::Playing);
    assert_eq!(rig.asked().0, Some(SeekAsk { to: 7.4, generation: rig.generation(), exact: false }));

    let landed_at = start + Duration::from_secs(3);
    rig.decodes(6.0);
    rig.show(landed_at);
    assert!(!rig.player.is_seeking());
    assert_eq!(rig.player.time(), 6.0);
    assert_eq!(rig.phase(), Phase::Playing { clock: Anchor { at: landed_at, pos: 6.0, rate: 1.0 } });
}

#[test]
fn frame_duration_and_presented_time_track_surface_layer_seeking_and_loops() {
    let rig = rig();
    let start = rune_core::time::now();
    assert_eq!(rig.player.frame_duration(), Some(FRAME));
    assert_eq!(rig.player.presented_time(), None);

    // Frame lands on surface at 2.0 s.
    rig.plays_from_two(start);
    assert_eq!(rig.player.presented_time(), Some(2.0));

    // On DirectComposition layer: presented_time reads the newest layer frame directly.
    assert!(rig.window_layers().showing);
    rig.presents(4.9);
    assert_eq!(rig.player.presented_time(), Some(4.9));

    // Looping past duration (10.0 s): presented_time wraps to clip time.
    rig.presents(12.5);
    assert!((rig.player.presented_time().unwrap() - 2.5).abs() < 1e-9, "{:?}", rig.player.presented_time());

    // Pause and seek: while seeking, presented_time preserves the picture still on screen while
    // time() reflects the seek target.
    rig.player.pause();
    rig.reconcile();
    rig.window_draws();
    rig.decodes(2.5);
    rig.show(start);
    assert!((rig.player.presented_time().unwrap() - 2.5).abs() < 1e-9);

    rig.player.seek(6.4);
    rig.reconcile();
    assert!(rig.player.is_seeking());
    assert!((rig.player.time() - 6.4).abs() < 1e-9);
    assert!((rig.player.presented_time().unwrap() - 2.5).abs() < 1e-9, "old picture stays on screen until seek lands");

    rig.decodes(6.3);
    rig.show(start);
    assert!(!rig.player.is_seeking());
    assert!((rig.player.presented_time().unwrap() - 6.3).abs() < 1e-9);

    // Failure clears presented_time.
    rig.player.shared.ctl.lock().unwrap().fail("lost".into());
    rig.reconcile();
    assert_eq!(rig.player.presented_time(), None);
}

#[test]
fn step_pauses_seeks_to_neighbour_frame_and_clamps_without_seeking_on_windows() {
    let rig = rig();
    let start = rune_core::time::now();
    rig.stand(Phase::Paused { at: 0.0 }, Some(0.0));
    rig.player.shared.ctl.lock().unwrap().presented = Some(0.0);

    // Already on frame 0: step(-1) stays put without seeking.
    rig.player.step(-1);
    assert!(!rig.player.is_seeking());
    rig.reconcile();
    assert_eq!(rig.asked().0, None);
    assert!(!rig.player.is_seeking());

    // Step +1 from frame 0 -> exact seek to frame 1 start (1/30 s).
    rig.player.step(1);
    assert!(rig.player.is_seeking());
    rig.reconcile();
    let ask = rig.asked().0.expect("exact seek issued for step(+1)");
    assert!(ask.exact);
    assert!((ask.to - FRAME).abs() < 1e-9, "got {}, want {FRAME}", ask.to);
    assert_eq!(rig.phase(), Phase::Buffering { at: ask.to, play: false, seeking: true });

    rig.decodes(FRAME);
    rig.show(start);
    assert!(!rig.player.is_seeking());
    assert!((rig.player.presented_time().unwrap() - FRAME).abs() < 1e-9);
    assert_eq!(rig.player.state(), PlayerState::Ready);

    // Step +1 while playing on layer: stops playback, steps from layer's presented frame (5.0 s).
    rig.plays_from_two(start);
    assert!(rig.window_layers().showing);
    rig.presents(5.0);
    rig.player.step(1);
    assert!(!rig.player.is_playing());
    rig.reconcile();
    assert!(rig.player.is_seeking());
    let ask = rig.asked().0.expect("exact seek from layer frame");
    assert!(ask.exact);
    assert!((ask.to - (5.0 + FRAME)).abs() < 1e-6, "got {}, want {}", ask.to, 5.0 + FRAME);

    // Leave the layer back to surface rendering (as the window does once paused/seeking), then
    // land the last frame and confirm step(+1) clamps without issuing a seek.
    rig.window_draws();
    let last_pts = 299.0 * FRAME;
    rig.decodes(last_pts);
    rig.show(start);
    assert!(!rig.player.is_seeking());
    rig.pipe.lock().seek = None;
    rig.player.step(1);
    assert!(!rig.player.is_seeking());
    rig.reconcile();
    assert_eq!(rig.asked().0, None);

    // Step -1 from Ended returns to Ready and steps to frame 298.
    rig.edit(phases::Event::Play);
    rig.stand(Phase::Ended { at: last_pts }, Some(last_pts));
    rig.player.shared.ctl.lock().unwrap().run = phases::Run::Ended;
    rig.player.step(-1);
    assert_eq!(rig.player.state(), PlayerState::Ready);
    rig.reconcile();
    let ask = rig.asked().0.expect("step(-1) from Ended");
    assert!(ask.exact);
    assert!((ask.to - 298.0 * FRAME).abs() < 1e-6, "got {}, want {}", ask.to, 298.0 * FRAME);
}
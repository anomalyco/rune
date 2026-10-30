- **Linux video and audio playback (`linux-gstreamer`).** Opt-in feature: `Player` /
  `el().video` on Linux via GStreamer `playbin` (appsink BGRA + system audio on one pipeline
  clock). Needs GStreamer and the usual plugin sets at runtime (`gstreamer`,
  `gst-plugins-base`, `gst-plugins-good`, plus `gst-libav` / `gst-plugins-bad` for many codecs).
  Posters and headless `--snap` still use `ffmpeg`/`ffprobe` on PATH. Off (the default), Linux
  reports unsupported like other non-macOS platforms. Enable with
  `rune = { ..., features = ["linux-gstreamer"] }`.

- **`Player::set_volume` / `volume`.** Linear gain `0.0`…`1.0` (default `1.0`), independent of
  mute, applied on macOS AVPlayer and Linux GStreamer playbin.

- **`Player` track selection (Linux, `linux-gstreamer`).** `tracks` / `current_track` /
  `set_track` list and switch audio, video, and subtitle streams on GStreamer `playbin`.
  Subtitles accept `None` for off. macOS returns empty lists for now.

- **`Player::decode_frame_at`.** Blocking BGRA decode (≤240 px long edge) for scrub previews;
  call from a worker. Does not hold the live-player lock across decode.

- **`video-player` / `video-player-extra` examples.** Scrubber and Linux-friendly open/unmute on
  `video-player`; `video-player-extra` is a full-chrome lab (tracks, volume, speed, scrub
  preview, chrome auto-hide). Both enable `linux-gstreamer`.

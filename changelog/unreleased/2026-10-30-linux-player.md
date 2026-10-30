- **Linux video and audio playback (`linux-gstreamer` / `linux-libmpv`).** Opt-in, mutually
  exclusive features for live `Player` / `el().video` on Linux. `linux-gstreamer` uses GStreamer
  `playbin` (appsink BGRA + system audio). `linux-libmpv` uses libmpv (`vo=libmpv` software BGRA
  render + mpv audio). Both need the matching runtime stack, and posters / headless `--snap` still
  use `ffmpeg`/`ffprobe` on PATH. Off (the default), Linux reports unsupported like other
  non-macOS platforms. Enable one of:
  `features = ["linux-gstreamer"]` or `features = ["linux-libmpv"]`.

- **`Player::set_volume` / `volume`.** Linear gain `0.0`…`1.0` (default `1.0`), independent of
  mute, applied on macOS AVPlayer and the Linux backends.

- **`Player` track selection (Linux backends).** `tracks` / `current_track` / `set_track` list and
  switch audio, video, and subtitle streams. Subtitles accept `None` for off. macOS returns empty
  lists for now.

- **`Player::decode_frame_at`.** Blocking BGRA decode (≤240 px long edge) for scrub previews;
  call from a worker. Does not hold the live-player lock across decode.

- **`video-player` / `video-player-extra` examples.** Scrubber and Linux-friendly open/unmute on
  `video-player`; `video-player-extra` is a full-chrome lab. Both enable `linux-gstreamer`.

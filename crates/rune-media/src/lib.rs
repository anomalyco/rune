//! Media playback, zero-copy native video surfaces and audio recording for Rune. Apps normally
//! reach these types through `rune`, which re-exports them.

pub mod audio;
pub mod native_surface;
pub mod recorder;
pub mod video;

pub use audio::AudioBackend;
pub use audio::AudioEvent;
pub use audio::AudioOutput;
pub use audio::AudioOutputError;
pub use audio::AudioOutputSpec;
pub use audio::AudioOutputState;
pub use audio::AudioPosition;
pub use audio::AudioPushError;
pub use audio::SampleFormat;
pub use native_surface::NativeSurface;
pub use native_surface::SurfaceFit;
pub use recorder::FakeMicrophone;
pub use recorder::Level;
pub use recorder::MicPermission;
pub use recorder::RecordFormat;
pub use recorder::Recorder;
pub use recorder::RecorderState;
pub use recorder::Recording;
pub use video::MediaRequest;
pub use video::MediaSource;
pub use video::Player;
pub use video::PlayerState;
pub use video::Thumbnail;
pub use video::ThumbnailPrecision;
pub use video::ThumbnailRequest;
pub use video::Thumbnails;
pub use video::Video;

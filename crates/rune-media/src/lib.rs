//! Media playback, zero-copy native video surfaces and audio recording for Rune. Apps normally
//! reach these types through `rune`, which re-exports them.

pub mod native_surface;
pub mod recorder;
pub mod video;

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
pub use video::Video;

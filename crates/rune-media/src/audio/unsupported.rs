//! Placeholder real-device backend on platforms without an OS audio output driver yet.

use super::{AudioOutputError, SinkDevice, ValidatedSpec};

pub(super) fn open(_spec: &ValidatedSpec) -> Result<Box<dyn SinkDevice>, AudioOutputError> {
    Err(AudioOutputError::Unsupported)
}

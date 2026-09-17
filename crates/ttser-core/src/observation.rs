use crate::speech::Transcript;
use std::path::Path;

/// Optional diagnostics must enqueue work without delaying audio capture or paste.
pub trait RecordingObserver {
    fn started(&self) -> Option<Box<dyn RecordingObservation>>;
}

/// Owned by one recording, so delayed diagnostics cannot attach to another take.
pub trait RecordingObservation: Send {
    fn finished(
        self: Box<Self>,
        history_directory: Option<&Path>,
        transcript: Option<&Transcript>,
        outcome: &str,
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Loading,
    Idle,
    Recording,
    Processing,
    Reviewing,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    Record,
    Transcribe,
    Busy,
    None,
}

#[derive(Default)]
pub struct Trigger {
    held: bool,
}

impl Trigger {
    pub fn press(&mut self, state: State) -> Action {
        if self.held {
            return Action::None;
        }
        self.held = true;
        state.start()
    }

    pub fn release(&mut self, state: State) -> Action {
        self.held = false;
        state.stop()
    }
}

impl State {
    pub fn start(self) -> Action {
        match self {
            Self::Idle => Action::Record,
            Self::Recording => Action::None,
            Self::Loading | Self::Processing | Self::Reviewing => Action::Busy,
        }
    }

    pub fn stop(self) -> Action {
        if self == Self::Recording {
            Action::Transcribe
        } else {
            Action::None
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Loading => "loading",
            Self::Idle => "idle",
            Self::Recording => "recording",
            Self::Processing => "processing",
            Self::Reviewing => "reviewing",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeats_and_unmatched_releases_do_not_create_work() {
        assert_eq!(State::Idle.start(), Action::Record);
        assert_eq!(State::Recording.start(), Action::None);
        assert_eq!(State::Recording.stop(), Action::Transcribe);
        for state in [
            State::Idle,
            State::Loading,
            State::Processing,
            State::Reviewing,
        ] {
            assert_eq!(state.stop(), Action::None);
        }
    }

    #[test]
    fn processing_rejects_new_recordings() {
        assert_eq!(State::Processing.start(), Action::Busy);
        assert_eq!(State::Reviewing.start(), Action::Busy);
        assert_eq!(State::Loading.start(), Action::Busy);
    }

    #[test]
    fn rejected_press_cannot_start_a_recording_when_processing_finishes() {
        let mut trigger = Trigger::default();
        assert_eq!(trigger.press(State::Processing), Action::Busy);
        assert_eq!(trigger.press(State::Idle), Action::None);
        assert_eq!(trigger.release(State::Idle), Action::None);
        assert_eq!(trigger.press(State::Idle), Action::Record);
        assert_eq!(trigger.press(State::Recording), Action::None);
        assert_eq!(trigger.release(State::Recording), Action::Transcribe);
    }
}

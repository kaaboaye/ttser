use crate::{
    TextOutput, TranscriptReview,
    audio::{Cue, Recording, Sounds},
    config::Config,
    history,
    speech::{Engine, Transcript},
    state::{Action, State, Trigger},
};
use anyhow::{Context, Result};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug)]
pub enum Command {
    Start,
    Stop,
    Status,
    Shutdown,
}

#[derive(Clone, Debug)]
pub struct Status {
    pub state: State,
    pub error: Option<String>,
}

pub struct Request {
    command: Command,
    reply: SyncSender<Status>,
}

/// Platform frontends share this small command API; transports stay outside core.
#[derive(Clone)]
pub struct Controller(SyncSender<Request>);

impl Controller {
    pub fn channel() -> (Self, Receiver<Request>) {
        let (tx, rx) = mpsc::sync_channel(16);
        (Self(tx), rx)
    }

    pub fn request(&self, command: Command) -> Result<Status> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.0
            .try_send(Request { command, reply: tx })
            .context("Dictation controller is unavailable or busy")?;
        rx.recv_timeout(Duration::from_secs(2))
            .context("Dictation controller did not respond")
    }
}

enum WorkerEvent {
    Ready,
    Reviewing,
    Finished(Result<()>),
    Failed(String),
}

/// Runs the application on the calling thread. Keeps the output backend alive
/// between dictations so it can continue serving resources it owns.
pub fn run(
    config: Config,
    mut output: impl TextOutput + TranscriptReview,
    commands: Receiver<Request>,
    shutdown: Arc<AtomicBool>,
) -> Result<()> {
    config.validate()?;
    let sounds = Sounds::new()?;
    let input_device = config.input_device.clone();
    let max_seconds = config.max_seconds;
    let (jobs, job_rx) = mpsc::sync_channel::<Vec<f32>>(1);
    let (events, event_rx) = mpsc::channel();
    let cancelled = shutdown.clone();
    let worker = thread::spawn(move || {
        let mut engine = match Engine::new(&config) {
            Ok(engine) => engine,
            Err(error) => {
                let _ = events.send(WorkerEvent::Failed(format!("{error:#}")));
                return;
            }
        };
        let _ = events.send(WorkerEvent::Ready);
        while let Ok(samples) = job_rx.recv() {
            if cancelled.load(Ordering::Relaxed) {
                break;
            }
            let started = Instant::now();
            let mut history = match history::Entry::start(&config, &samples) {
                Ok(entry) => entry,
                Err(error) => {
                    eprintln!("Could not save dictation audio: {error:#}");
                    None
                }
            };
            let transcription = engine.transcribe(&samples);
            if let (Some(entry), Ok(transcript)) = (&history, &transcription)
                && let Err(error) = entry.finish(
                    Some(transcript),
                    "transcribed",
                    None,
                    started.elapsed().as_millis() as u64,
                )
            {
                eprintln!("Could not save transcript before review: {error:#}");
            }
            let (outcome, result) = match &transcription {
                Err(error) => ("transcription_failed", Err(anyhow::anyhow!("{error:#}"))),
                Ok(transcript) => review_and_insert(
                    &mut output,
                    transcript,
                    &cancelled,
                    || {
                        let _ = events.send(WorkerEvent::Reviewing);
                    },
                    |text| {
                        if let Some(entry) = history.as_mut() {
                            entry.correct(
                                transcript,
                                text,
                                started.elapsed().as_millis() as u64,
                            )?;
                        }
                        Ok(())
                    },
                ),
            };
            if let Some(entry) = history {
                let error = result.as_ref().err().map(|error| format!("{error:#}"));
                if let Err(error) = entry.finish(
                    transcription.as_ref().ok(),
                    outcome,
                    error.as_deref(),
                    started.elapsed().as_millis() as u64,
                ) {
                    eprintln!("Could not save dictation result: {error:#}");
                }
            }
            eprintln!(
                "Dictation finished in {:.2}s",
                started.elapsed().as_secs_f32()
            );
            let _ = events.send(WorkerEvent::Finished(result));
        }
    });
    let mut status = Status {
        state: State::Loading,
        error: None,
    };
    let mut recording: Option<Recording> = None;
    let mut trigger = Trigger::default();
    let mut fatal = None;
    while !shutdown.load(Ordering::Relaxed) {
        while let Ok(event) = event_rx.try_recv() {
            match event {
                WorkerEvent::Ready => {
                    status.state = State::Idle;
                    eprintln!("Ready for dictation");
                }
                WorkerEvent::Reviewing => status.state = State::Reviewing,
                WorkerEvent::Finished(result) => {
                    status.state = State::Idle;
                    if let Err(error) = result {
                        set_error(&mut status, &sounds, error);
                    }
                }
                WorkerEvent::Failed(error) => {
                    fatal = Some(error);
                    shutdown.store(true, Ordering::Relaxed);
                }
            }
        }
        if let Some(error) = recording.as_ref().and_then(Recording::error) {
            recording.take();
            status.state = State::Idle;
            set_error(&mut status, &sounds, anyhow::anyhow!(error));
        }
        let request = match commands.recv_timeout(Duration::from_millis(20)) {
            Ok(request) => request,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        match request.command {
            Command::Start => match trigger.press(status.state) {
                Action::Record => match Recording::start(input_device.as_deref(), max_seconds) {
                    Ok(capture) => {
                        recording = Some(capture);
                        status = Status {
                            state: State::Recording,
                            error: None,
                        };
                        sounds.play(Cue::Start);
                    }
                    Err(error) => set_error(&mut status, &sounds, error),
                },
                Action::Busy => sounds.play(Cue::Busy),
                _ => {}
            },
            Command::Stop => {
                if trigger.release(status.state) == Action::Transcribe {
                    let audio = recording
                        .take()
                        .expect("recording state owns the microphone")
                        .finish();
                    sounds.play(Cue::Stop);
                    match audio.and_then(|samples| {
                        jobs.send(samples).context("Transcription worker stopped")
                    }) {
                        Ok(()) => status.state = State::Processing,
                        Err(error) => {
                            status.state = State::Idle;
                            set_error(&mut status, &sounds, error);
                        }
                    }
                }
            }
            Command::Status => {}
            Command::Shutdown => shutdown.store(true, Ordering::Relaxed),
        }
        let _ = request.reply.send(status.clone());
    }
    shutdown.store(true, Ordering::Relaxed);
    drop(recording);
    drop(jobs);
    worker
        .join()
        .map_err(|_| anyhow::anyhow!("Transcription worker panicked"))?;
    if let Some(error) = fatal {
        anyhow::bail!("{error}");
    }
    Ok(())
}

fn review_and_insert(
    output: &mut (impl TextOutput + TranscriptReview),
    transcript: &Transcript,
    cancelled: &AtomicBool,
    reviewing: impl FnOnce(),
    save_feedback: impl FnOnce(&str) -> Result<()>,
) -> (&'static str, Result<()>) {
    if cancelled.load(Ordering::Relaxed) {
        return ("cancelled", Ok(()));
    }
    if transcript.text.is_empty() {
        return ("empty", Ok(()));
    }
    reviewing();
    let text = match output.review(&transcript.text, cancelled) {
        Ok(Some(text)) if !cancelled.load(Ordering::Relaxed) => text,
        Ok(_) => return ("cancelled", Ok(())),
        Err(error) => return ("review_failed", Err(error)),
    };
    // Approval is useful training data even when the destination rejects the paste.
    if text != transcript.text
        && let Err(error) = save_feedback(&text)
    {
        eprintln!("Could not save transcription correction: {error:#}");
    }
    if cancelled.load(Ordering::Relaxed) {
        return ("cancelled", Ok(()));
    }
    if text.is_empty() {
        return ("empty", Ok(()));
    }
    match output.insert(&text) {
        Ok(()) => ("inserted", Ok(())),
        Err(error) => ("insertion_failed", Err(error)),
    }
}

fn set_error(status: &mut Status, sounds: &Sounds, error: anyhow::Error) {
    eprintln!("Dictation error: {error:#}");
    status.error = Some(format!("{error:#}"));
    sounds.play(Cue::Error);
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Output {
        reviewed: Result<Option<String>>,
        inserted: Vec<String>,
        fail_insert: bool,
        shutdown_in_review: bool,
    }

    impl TranscriptReview for Output {
        fn review(&mut self, _: &str, cancelled: &AtomicBool) -> Result<Option<String>> {
            if self.shutdown_in_review {
                cancelled.store(true, Ordering::Relaxed);
            }
            std::mem::replace(&mut self.reviewed, Ok(None))
        }
    }

    impl TextOutput for Output {
        fn insert(&mut self, text: &str) -> Result<()> {
            self.inserted.push(text.into());
            anyhow::ensure!(!self.fail_insert, "insertion failed");
            Ok(())
        }
    }

    #[test]
    fn approval_saves_only_actual_changes_even_when_insertion_fails() {
        for (approved, fail_insert, expected_status) in [
            (Some("original"), false, "inserted"),
            (Some("corrected\nŻółw 🐢"), false, "inserted"),
            (Some("corrected"), true, "insertion_failed"),
            (Some(""), false, "empty"),
            (None, false, "cancelled"),
        ] {
            let mut output = Output {
                reviewed: Ok(approved.map(str::to_owned)),
                inserted: vec![],
                fail_insert,
                shutdown_in_review: false,
            };
            let transcript = Transcript {
                text: "original".into(),
                ..Transcript::default()
            };
            let temp = tempfile::tempdir().unwrap();
            let config = Config {
                history_dir: Some(temp.path().join("history")),
                model: Some("model.bin".into()),
                ..Config::default()
            };
            let mut entry = history::Entry::start(&config, &[0.1]).unwrap().unwrap();
            let mut saved = Vec::new();
            let (status, result) = review_and_insert(
                &mut output,
                &transcript,
                &AtomicBool::new(false),
                || {},
                |text| {
                    saved.push(text.to_owned());
                    entry.correct(&transcript, text, 10)
                },
            );
            assert_eq!(status, expected_status);
            assert_eq!(result.is_err(), fail_insert);
            assert_eq!(
                saved,
                approved
                    .filter(|text| *text != "original")
                    .into_iter()
                    .collect::<Vec<_>>()
            );
            assert_eq!(
                output.inserted,
                approved
                    .filter(|text| !text.is_empty())
                    .into_iter()
                    .collect::<Vec<_>>()
            );
            assert_eq!(transcript.text, "original");
            entry.finish(Some(&transcript), status, None, 20).unwrap();
            let directory = std::fs::read_dir(config.history_dir.unwrap())
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path();
            let record: serde_yaml_ng::Value = serde_yaml_ng::from_str(
                &std::fs::read_to_string(directory.join("record.yaml")).unwrap(),
            )
            .unwrap();
            assert_eq!(record["transcript"]["text"], "original");
            assert_eq!(
                record
                    .get("corrected_text")
                    .and_then(|value| value.as_str()),
                approved.filter(|text| *text != "original")
            );
        }
    }

    #[test]
    fn cancellation_shutdown_and_editor_errors_never_paste_or_save_feedback() {
        for (reviewed, shutdown, expected) in [
            (Ok(None), false, "cancelled"),
            (Ok(Some("edited".into())), true, "cancelled"),
            (
                Err(anyhow::anyhow!("editor unavailable")),
                false,
                "review_failed",
            ),
        ] {
            let mut output = Output {
                reviewed,
                inserted: vec![],
                fail_insert: false,
                shutdown_in_review: shutdown,
            };
            let transcript = Transcript {
                text: "original".into(),
                ..Transcript::default()
            };
            let (status, _) = review_and_insert(
                &mut output,
                &transcript,
                &AtomicBool::new(false),
                || {},
                |_| panic!("must not save"),
            );
            assert_eq!(status, expected);
            assert!(output.inserted.is_empty());
        }
    }

    #[test]
    fn empty_or_already_cancelled_transcripts_never_open_editor() {
        for (text, cancelled, expected) in [("", false, "empty"), ("original", true, "cancelled")] {
            let mut output = Output {
                reviewed: Ok(None),
                inserted: vec![],
                fail_insert: false,
                shutdown_in_review: false,
            };
            let transcript = Transcript {
                text: text.into(),
                ..Transcript::default()
            };
            let (status, _) = review_and_insert(
                &mut output,
                &transcript,
                &AtomicBool::new(cancelled),
                || panic!("must not review"),
                |_| panic!("must not save"),
            );
            assert_eq!(status, expected);
        }
    }

    #[test]
    fn feedback_write_failure_does_not_discard_approved_text() {
        let mut output = Output {
            reviewed: Ok(Some("edited".into())),
            inserted: vec![],
            fail_insert: false,
            shutdown_in_review: false,
        };
        let transcript = Transcript {
            text: "original".into(),
            ..Transcript::default()
        };
        let (status, result) = review_and_insert(
            &mut output,
            &transcript,
            &AtomicBool::new(false),
            || {},
            |_| anyhow::bail!("disk full"),
        );
        assert_eq!(status, "inserted");
        assert!(result.is_ok());
        assert_eq!(output.inserted, ["edited"]);
    }

    #[test]
    fn control_requests_are_transport_independent() {
        let (control, incoming) = Controller::channel();
        let client = thread::spawn(move || control.request(Command::Status).unwrap());
        let request = incoming.recv().unwrap();
        assert!(matches!(request.command, Command::Status));
        request
            .reply
            .send(Status {
                state: State::Processing,
                error: None,
            })
            .unwrap();
        assert_eq!(client.join().unwrap().state, State::Processing);
    }
}

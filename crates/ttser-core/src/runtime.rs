use crate::{
    TextOutput,
    audio::{Cue, Recording, Sounds},
    config::Config,
    history,
    speech::Engine,
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
    Finished(Result<()>),
    Failed(String),
}

/// Runs the application on the calling thread. Keeps the output backend alive
/// between dictations so it can continue serving resources it owns.
pub fn run(
    config: Config,
    mut output: impl TextOutput,
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
            let history = match history::Entry::start(&config, &samples) {
                Ok(entry) => entry,
                Err(error) => {
                    eprintln!("Could not save dictation audio: {error:#}");
                    None
                }
            };
            let transcription = engine.transcribe(&samples);
            let (outcome, result) = match &transcription {
                Err(error) => ("transcription_failed", Err(anyhow::anyhow!("{error:#}"))),
                Ok(_) if cancelled.load(Ordering::Relaxed) => ("cancelled", Ok(())),
                Ok(transcript) if transcript.text.is_empty() => ("empty", Ok(())),
                Ok(transcript) => match output.insert(&transcript.text) {
                    Ok(()) => ("inserted", Ok(())),
                    Err(error) => ("insertion_failed", Err(error)),
                },
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

fn set_error(status: &mut Status, sounds: &Sounds, error: anyhow::Error) {
    eprintln!("Dictation error: {error:#}");
    status.error = Some(format!("{error:#}"));
    sounds.play(Cue::Error);
}

#[cfg(test)]
mod tests {
    use super::*;

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

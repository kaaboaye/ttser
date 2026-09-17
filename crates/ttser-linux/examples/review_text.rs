//! Isolated desktop test entry point, without a model or microphone.
//! Each stdin request is a big-endian u64 byte length followed by UTF-8 text.
use anyhow::Result;
use std::{
    io::{self, Read, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};
use ttser_core::{TextOutput, TranscriptReview};
use ttser_linux::desktop::X11TextOutput;

fn main() -> Result<()> {
    let cancelled = Arc::new(AtomicBool::new(false));
    let signal = cancelled.clone();
    ctrlc::set_handler(move || signal.store(true, Ordering::Relaxed))?;
    let (requests, input) = mpsc::channel();
    thread::spawn(move || {
        let mut stdin = io::stdin().lock();
        loop {
            let request = (|| -> Result<String> {
                let mut length = [0; 8];
                stdin.read_exact(&mut length)?;
                let mut bytes = vec![0; usize::try_from(u64::from_be_bytes(length))?];
                stdin.read_exact(&mut bytes)?;
                Ok(String::from_utf8(bytes)?)
            })();
            let failed = request.is_err();
            if requests.send(request).is_err() || failed {
                break;
            }
        }
    });
    let output = X11TextOutput::new(Duration::from_millis(100))?;
    // Match the daemon: construct the backend on the caller, then review on its worker.
    thread::spawn(move || run(output, input, cancelled))
        .join()
        .map_err(|_| anyhow::anyhow!("Review worker panicked"))?
}

fn run(
    mut output: X11TextOutput,
    input: mpsc::Receiver<Result<String>>,
    cancelled: Arc<AtomicBool>,
) -> Result<()> {
    while !cancelled.load(Ordering::Relaxed) {
        let text = match input.recv_timeout(Duration::from_millis(20)) {
            Ok(text) => text?,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        if let Some(text) = output.review(&text, &cancelled)? {
            output.insert(&text)?;
            println!("inserted");
        } else {
            println!("cancelled");
        }
        io::stdout().flush()?;
    }
    Ok(())
}

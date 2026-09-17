//! Isolated desktop test entry point, without a model or microphone.
use anyhow::Result;
use std::{
    io::{self, Read, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use ttser_core::{TextOutput, TranscriptReview};
use ttser_linux::desktop::X11TextOutput;

fn main() -> Result<()> {
    let mut text = String::new();
    io::stdin().read_to_string(&mut text)?;
    let cancelled = Arc::new(AtomicBool::new(false));
    let signal = cancelled.clone();
    ctrlc::set_handler(move || signal.store(true, Ordering::Relaxed))?;
    let mut output = X11TextOutput::new(Duration::from_millis(100))?;
    if let Some(text) = output.review(&text, &cancelled)? {
        output.insert(&text)?;
        println!("inserted");
    } else {
        println!("cancelled");
    }
    io::stdout().flush()?;
    while !cancelled.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}

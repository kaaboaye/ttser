//! Manual desktop smoke test. Focus a disposable text field before running it.
use anyhow::Result;
use std::{
    io::{self, Read, Write},
    time::Duration,
};
use ttser_core::TextOutput;
use ttser_linux::desktop::X11TextOutput;

fn main() -> Result<()> {
    let mut text = String::new();
    io::stdin().read_to_string(&mut text)?;
    let mut output = X11TextOutput::new(Duration::from_millis(300))?;
    let started = std::time::Instant::now();
    output.insert(&text)?;
    println!(
        "inserted in {}ms; terminate the helper to exit",
        started.elapsed().as_millis()
    );
    io::stdout().flush()?;
    // Restored selections need a live owner until the test has read them.
    loop {
        std::thread::park();
    }
}

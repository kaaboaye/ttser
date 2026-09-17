use std::{
    io::{self, BufRead, Write},
    path::PathBuf,
};
use ttser_core::observation::RecordingObserver;
use ttser_linux::context_probe::ContextRecorder;

fn main() -> anyhow::Result<()> {
    let directory = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("Usage: capture_context OUTPUT_DIRECTORY [--listen]"))?;
    let observer = ContextRecorder::new(directory)?;
    let capture = || {
        if let Some(observation) = observer.started() {
            observation.finished(None, None, "probe_only");
        }
    };
    if std::env::args().nth(2).as_deref() == Some("--listen") {
        println!("ready");
        io::stdout().flush()?;
        for line in io::stdin().lock().lines() {
            match line?.as_str() {
                "capture" => capture(),
                "quit" => break,
                _ => anyhow::bail!("Expected capture or quit"),
            }
        }
    } else {
        capture();
    }
    Ok(())
}

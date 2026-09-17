use crate::{desktop::X11TextOutput, hotkey::Hotkey, ipc};
use anyhow::{Context, Result};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};
use ttser_core::{
    config::Config,
    runtime::{self, Command, Controller},
};

pub fn serve(path: PathBuf, config: Config, hotkey_keycode: Option<u8>) -> Result<()> {
    let server = ipc::Server::bind(path.clone())?;
    let output = X11TextOutput::new(Duration::from_millis(config.paste_delay_ms))?;
    let hotkey = hotkey_keycode.map(Hotkey::new).transpose()?;
    let shutdown = Arc::new(AtomicBool::new(false));
    let signal = shutdown.clone();
    ctrlc::set_handler(move || signal.store(true, Ordering::Relaxed))?;
    let (control, commands) = Controller::channel();
    let keyboard = hotkey.map(|hotkey| {
        let controller = control.clone();
        let stopped = shutdown.clone();
        thread::spawn(move || {
            let result = hotkey.listen(controller, &stopped);
            if let Err(error) = &result {
                eprintln!("Hotkey listener failed: {error:#}");
                stopped.store(true, Ordering::Relaxed);
            }
            result
        })
    });
    let stopped = shutdown.clone();
    let transport = thread::spawn(move || {
        while !stopped.load(Ordering::Relaxed) {
            match server.listener.accept() {
                Ok((stream, _)) => {
                    let response = ipc::read_command(&stream).and_then(|line| {
                        let command = match line.as_str() {
                            "start" => Command::Start,
                            "stop" => Command::Stop,
                            "status" => Command::Status,
                            "shutdown" => Command::Shutdown,
                            _ => anyhow::bail!("Unknown command: {line}"),
                        };
                        control.request(command)
                    });
                    let response = match response {
                        Ok(status) => match status.error {
                            Some(error) => format!("error {error}"),
                            None => status.state.label().into(),
                        },
                        Err(error) => {
                            eprintln!("Control request failed: {error:#}");
                            format!("error {error:#}")
                        }
                    };
                    if let Err(error) = ipc::reply(&stream, &response) {
                        eprintln!("Control socket reply failed: {error:#}");
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10))
                }
                Err(error) => {
                    eprintln!("Control socket error: {error}");
                    stopped.store(true, Ordering::Relaxed);
                }
            }
        }
        // The socket lock must outlive the worker and its loaded model.
        server
    });
    eprintln!("Loading model; control socket: {}", path.display());
    let result = runtime::run(config, output, commands, shutdown.clone());
    shutdown.store(true, Ordering::Relaxed);
    let keyboard_result = keyboard
        .map(|keyboard| {
            keyboard
                .join()
                .map_err(|_| anyhow::anyhow!("Hotkey listener panicked"))?
        })
        .unwrap_or(Ok(()));
    let _server = transport
        .join()
        .map_err(|_| anyhow::anyhow!("Control transport panicked"))?;
    result
        .and(keyboard_result)
        .context("Dictation daemon stopped")
}

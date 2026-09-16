use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use ttser_core::{audio, config, speech};
#[cfg(target_os = "linux")]
use ttser_linux::{daemon, ipc};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Override the local control socket (also useful for isolated tests).
    #[arg(long, global = true)]
    socket: Option<PathBuf>,
    /// YAML configuration (default: $XDG_CONFIG_HOME/ttser/config.yaml).
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Keep the model loaded and accept push-to-talk commands.
    Serve {
        #[command(flatten)]
        model: ModelOptions,
        /// Exact input device name; otherwise use the system default.
        #[arg(long)]
        input_device: Option<String>,
        /// Stop and discard recordings longer than this many seconds.
        #[arg(long, value_parser = clap::value_parser!(u32).range(1..=3600))]
        max_seconds: Option<u32>,
    },
    /// Begin recording. Invoke on push-to-talk key press.
    Start,
    /// Finish recording and transcribe. Invoke on push-to-talk key release.
    Stop,
    /// Print the daemon state or its latest error.
    Status,
    /// Stop the daemon without inserting a pending transcript.
    Shutdown,
    /// List available microphone devices.
    Devices,
    /// Transcribe a WAV to stdout, without changing focus or clipboards.
    Transcribe {
        file: PathBuf,
        #[command(flatten)]
        model: ModelOptions,
    },
}

#[derive(clap::Args)]
pub struct ModelOptions {
    /// whisper.cpp GGML model; defaults to ~/.local/share/whisper/ggml-large-v3-turbo.bin.
    #[arg(long)]
    model: Option<PathBuf>,
    /// Use CPU even in a Vulkan-enabled build.
    #[arg(long)]
    cpu: bool,
    /// Allowed language codes: one fixes the language; no values permits all.
    #[arg(long, num_args = 0.., value_delimiter = ',')]
    languages: Option<Vec<String>>,
    /// Initial prompt providing vocabulary and transcription context.
    #[arg(long)]
    prompt: Option<String>,
    /// CPU threads used by Whisper.
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..=256))]
    threads: Option<u32>,
}

impl ModelOptions {
    fn apply(self, config: &mut config::Config) {
        if let Some(value) = self.model {
            config.model = Some(value);
        }
        if let Some(value) = self.languages {
            config.languages = value;
        }
        if let Some(value) = self.prompt {
            config.prompt = value;
        }
        if let Some(value) = self.threads {
            config.threads = value;
        }
        if self.cpu {
            config.cpu = true;
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut config = config::Config::load(cli.config.as_deref())?;
    match cli.command {
        Command::Serve {
            model,
            input_device,
            max_seconds,
        } => {
            model.apply(&mut config);
            if input_device.is_some() {
                config.input_device = input_device;
            }
            if let Some(value) = max_seconds {
                config.max_seconds = value;
            }
            config.validate()?;
            #[cfg(target_os = "linux")]
            return daemon::serve(
                ipc::socket_path(cli.socket.or(config.socket.clone()))?,
                config,
            );
            #[cfg(not(target_os = "linux"))]
            anyhow::bail!("Desktop dictation is currently implemented for Linux/X11 only")
        }
        Command::Devices => audio::list_devices(),
        Command::Transcribe { file, model } => {
            model.apply(&mut config);
            config.validate()?;
            let samples = audio::read_wav(&file)?;
            let mut engine = speech::Engine::new(&config)?;
            println!("{}", engine.transcribe(&samples)?.text);
            Ok(())
        }
        command => {
            #[cfg(not(target_os = "linux"))]
            {
                let _ = command;
                anyhow::bail!("Desktop dictation is currently implemented for Linux/X11 only");
            }
            #[cfg(target_os = "linux")]
            {
                let request = match command {
                    Command::Start => "start",
                    Command::Stop => "stop",
                    Command::Status => "status",
                    Command::Shutdown => "shutdown",
                    _ => unreachable!(),
                };
                let reply =
                    ipc::request(&ipc::socket_path(cli.socket.or(config.socket))?, request)?;
                if let Some(error) = reply.strip_prefix("error ") {
                    anyhow::bail!("{error}");
                }
                println!("{reply}");
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_overrides_prompt_without_resetting_other_settings() {
        let mut config = config::Config {
            languages: vec!["pl".into()],
            threads: 8,
            prompt: "YAML prompt".into(),
            ..Default::default()
        };
        let cli = Cli::try_parse_from(["ttser", "serve", "--prompt", "CLI prompt"]).unwrap();
        let Command::Serve { model, .. } = cli.command else {
            panic!()
        };
        model.apply(&mut config);
        assert_eq!(config.prompt, "CLI prompt");
        assert_eq!(config.languages, ["pl"]);
        assert_eq!(config.threads, 8);
    }

    #[test]
    fn cli_languages_replace_the_configured_list_including_with_an_empty_list() {
        for (args, expected) in [
            (
                vec!["ttser", "serve", "--languages", "pl,en"],
                vec!["pl", "en"],
            ),
            (vec!["ttser", "serve", "--languages", "en"], vec!["en"]),
            (vec!["ttser", "serve", "--languages"], vec![]),
        ] {
            let mut config = config::Config {
                languages: vec!["pl".into()],
                ..Default::default()
            };
            let Command::Serve { model, .. } = Cli::try_parse_from(args).unwrap().command else {
                panic!();
            };
            model.apply(&mut config);
            assert_eq!(config.languages, expected);
        }
    }
}

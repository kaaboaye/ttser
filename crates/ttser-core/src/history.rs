use crate::{config::Config, speech::Transcript};
use anyhow::{Context, Result};
use serde::Serialize;
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub struct Entry {
    directory: PathBuf,
    corrected_text: Option<String>,
    settings: Config,
    created_at_unix_ms: u64,
    sample_count: usize,
}

#[derive(Serialize)]
struct Record<'a> {
    version: u32,
    program_version: &'a str,
    created_at_unix_ms: u64,
    sample_rate: u32,
    sample_count: usize,
    processing_ms: u64,
    status: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    corrected_text: Option<&'a str>,
    settings: &'a Config,
    transcript: Option<&'a Transcript>,
    error: Option<&'a str>,
}

impl Entry {
    pub fn start(config: &Config, samples: &[f32]) -> Result<Option<Self>> {
        let Some(root) = &config.history_dir else {
            return Ok(None);
        };
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?;
        let directory = root.join(format!("{}-{}", now.as_nanos(), std::process::id()));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.recursive(true).create(root)?;
        builder.recursive(false).create(&directory)?;
        let file = create_private(&directory.join("audio.wav"))?;
        let mut wav = hound::WavWriter::new(
            file,
            hound::WavSpec {
                channels: 1,
                sample_rate: 16_000,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
        )?;
        for &sample in samples {
            wav.write_sample(sample)?;
        }
        wav.finalize()?;
        let mut settings = config.clone();
        settings.model = Some(config.model_path()?);
        let entry = Self {
            directory,
            corrected_text: None,
            settings,
            created_at_unix_ms: now.as_millis() as u64,
            sample_count: samples.len(),
        };
        entry.finish(None, "recorded", None, 0)?;
        Ok(Some(entry))
    }

    pub fn correct(
        &mut self,
        transcript: &Transcript,
        text: &str,
        processing_ms: u64,
    ) -> Result<()> {
        self.corrected_text = (text != transcript.text).then(|| text.to_owned());
        self.finish(Some(transcript), "reviewed", None, processing_ms)
    }

    pub fn finish(
        &self,
        transcript: Option<&Transcript>,
        status: &str,
        error: Option<&str>,
        processing_ms: u64,
    ) -> Result<()> {
        let record = Record {
            version: 3,
            program_version: env!("CARGO_PKG_VERSION"),
            created_at_unix_ms: self.created_at_unix_ms,
            sample_rate: 16_000,
            sample_count: self.sample_count,
            processing_ms,
            status,
            corrected_text: self.corrected_text.as_deref(),
            settings: &self.settings,
            transcript,
            error,
        };
        let temporary = self.directory.join("record.yaml.tmp");
        let mut file = create_private(&temporary)?;
        file.write_all(serde_yaml_ng::to_string(&record)?.as_bytes())?;
        drop(file);
        // Preserve the initial record if writing the completed result fails.
        fs::rename(temporary, self.directory.join("record.yaml"))
            .context("Saving dictation history")
    }
}

fn create_private(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_is_disabled_by_default() {
        assert!(Entry::start(&Config::default(), &[0.1]).unwrap().is_none());
    }

    #[test]
    fn corrections_live_in_record_and_survive_final_status_updates() {
        let temp = tempfile::tempdir().unwrap();
        let config = Config {
            history_dir: Some(temp.path().join("history")),
            model: Some(PathBuf::from("model.bin")),
            ..Config::default()
        };
        let transcript = Transcript {
            text: "Żółw 🐢".into(),
            raw_text: " Żółw 🐢.".into(),
            ..Transcript::default()
        };
        for correction in [None, Some("Żółw 🐢"), Some("Żółw 🐢\n"), Some("")] {
            let mut entry = Entry::start(&config, &[0.1]).unwrap().unwrap();
            let path = entry.directory.join("record.yaml");
            let read = || -> serde_yaml_ng::Value {
                serde_yaml_ng::from_str(&fs::read_to_string(&path).unwrap()).unwrap()
            };
            assert!(read().get("corrected_text").is_none());
            if let Some(text) = correction {
                entry.correct(&transcript, text, 10).unwrap();
            }
            for status in ["inserted", "insertion_failed"] {
                entry.finish(Some(&transcript), status, None, 20).unwrap();
                let record = read();
                assert_eq!(record["version"], 3);
                assert_eq!(record["status"], status);
                assert_eq!(record["transcript"]["text"], transcript.text);
                assert_eq!(record["transcript"]["raw_text"], transcript.raw_text);
                assert_eq!(
                    record
                        .get("corrected_text")
                        .and_then(|value| value.as_str()),
                    correction.filter(|text| *text != transcript.text)
                );
                assert_eq!(fs::read_dir(&entry.directory).unwrap().count(), 2);
            }
        }
    }

    #[test]
    #[ignore = "Requires a local model and the public whisper.cpp JFK sample"]
    fn real_speech_preserves_english_and_history_across_recordings() {
        let temp = tempfile::tempdir().unwrap();
        let config = Config {
            model: Some(
                std::env::var_os("TTSER_TEST_MODEL")
                    .expect("Set TTSER_TEST_MODEL")
                    .into(),
            ),
            history_dir: Some(temp.path().join("history")),
            languages: vec!["pl".into(), "en".into()],
            ..Config::default()
        };
        let wav = PathBuf::from(std::env::var_os("TTSER_TEST_WAV").expect("Set TTSER_TEST_WAV"));
        let samples = crate::audio::read_wav(&wav).unwrap();
        let mut engine = crate::speech::Engine::new(&config).unwrap();
        for attenuation in [1.0, 0.05] {
            let samples: Vec<f32> = samples.iter().map(|sample| sample * attenuation).collect();
            let entry = Entry::start(&config, &samples).unwrap().unwrap();
            let transcript = engine.transcribe(&samples).unwrap();
            assert_eq!(transcript.language.as_deref(), Some("en"));
            assert!(
                transcript
                    .text
                    .to_lowercase()
                    .contains("ask not what your country can do for you")
            );
            assert!(
                transcript.language_probabilities["en"] > transcript.language_probabilities["pl"]
            );
            entry
                .finish(Some(&transcript), "transcribed", None, 0)
                .unwrap();
            let record: serde_yaml_ng::Value = serde_yaml_ng::from_str(
                &fs::read_to_string(entry.directory.join("record.yaml")).unwrap(),
            )
            .unwrap();
            assert_eq!(record["transcript"]["text"], transcript.text);
            assert_eq!(record["transcript"]["raw_text"], transcript.raw_text);
            let levels = transcript.audio.as_ref().unwrap();
            assert_eq!(
                record["transcript"]["audio"]["gain"].as_f64().unwrap() as f32,
                levels.gain
            );
            if attenuation < 1.0 {
                assert!(levels.gain > 1.0 && levels.gain <= config.audio_max_gain);
            }
            assert_eq!(
                crate::audio::read_wav(&entry.directory.join("audio.wav")).unwrap(),
                samples
            );
        }
        assert_eq!(
            fs::read_dir(config.history_dir.unwrap()).unwrap().count(),
            2
        );
    }

    #[test]
    fn preserves_audio_text_settings_and_insertion_failure() {
        let temp = tempfile::tempdir().unwrap();
        let config = Config {
            history_dir: Some(temp.path().join("history")),
            model: Some(PathBuf::from("model.bin")),
            prompt: "Zażółć gęślą jaźń".into(),
            ..Config::default()
        };
        let samples = [-0.9, 0.1, 0.8];
        let entry = Entry::start(&config, &samples).unwrap().unwrap();
        let mut reader = hound::WavReader::open(entry.directory.join("audio.wav")).unwrap();
        assert_eq!(reader.spec().sample_rate, 16_000);
        assert_eq!(reader.spec().channels, 1);
        assert_eq!(
            reader
                .samples::<f32>()
                .collect::<Result<Vec<_>, _>>()
                .unwrap(),
            samples
        );
        let transcript = Transcript {
            text: "Żółw".into(),
            raw_text: " Żółw.".into(),
            language: Some("pl".into()),
            ..Transcript::default()
        };
        entry
            .finish(
                Some(&transcript),
                "insertion_failed",
                Some("Clipboard unavailable"),
                42,
            )
            .unwrap();
        let record: serde_yaml_ng::Value = serde_yaml_ng::from_str(
            &fs::read_to_string(entry.directory.join("record.yaml")).unwrap(),
        )
        .unwrap();
        assert_eq!(record["transcript"]["text"], "Żółw");
        assert_eq!(record["transcript"]["raw_text"], " Żółw.");
        assert_eq!(record["transcript"]["language"], "pl");
        assert_eq!(record["settings"]["prompt"], config.prompt);
        assert_eq!(record["status"], "insertion_failed");
        assert_eq!(record["error"], "Clipboard unavailable");
        assert_eq!(record["processing_ms"], 42);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&entry.directory).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(entry.directory.join("record.yaml"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        let another = Entry::start(&config, &samples).unwrap().unwrap();
        assert_ne!(entry.directory, another.directory);
    }
}

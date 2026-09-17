use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    env, fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    pub model: Option<PathBuf>,
    pub languages: Vec<String>,
    pub history_dir: Option<PathBuf>,
    pub feedback_dir: Option<PathBuf>,
    pub prompt: String,
    pub threads: u32,
    pub cpu: bool,
    pub input_device: Option<String>,
    pub audio_target_peak: f32,
    pub audio_max_gain: f32,
    pub max_seconds: u32,
    pub socket: Option<PathBuf>,
    pub paste_delay_ms: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            model: None,
            languages: Vec::new(),
            history_dir: None,
            feedback_dir: env::var_os("XDG_STATE_HOME")
                .map(PathBuf::from)
                .or_else(|| {
                    env::var_os("HOME")
                        .or_else(|| env::var_os("USERPROFILE"))
                        .map(|home| PathBuf::from(home).join(".local/state"))
                })
                .map(|state| state.join("ttser/feedback")),
            prompt: String::new(),
            threads: 4,
            cpu: false,
            input_device: None,
            audio_target_peak: 0.25,
            audio_max_gain: 10.0,
            max_seconds: 300,
            socket: None,
            paste_delay_ms: 300,
        }
    }
}

impl Config {
    pub fn load(explicit: Option<&Path>) -> Result<Self> {
        let default = env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|p| PathBuf::from(p).join(".config")))
            .map(|p| p.join("ttser/config.yaml"));
        let Some(path) = explicit.or(default.as_deref()) else {
            return Ok(Self::default());
        };
        if explicit.is_none() && !path.exists() {
            return Ok(Self::default());
        }
        Self::from_file(path)
    }

    fn from_file(path: &Path) -> Result<Self> {
        let yaml = fs::read_to_string(path)
            .with_context(|| format!("Reading config {}", path.display()))?;
        let mut config = Self::from_yaml(&yaml, |field| {
            eprintln!(
                "Warning: ignoring unknown config field {field:?} in {}",
                path.display()
            );
        })
        .with_context(|| format!("Parsing config {}", path.display()))?;
        for value in [
            &mut config.model,
            &mut config.socket,
            &mut config.history_dir,
            &mut config.feedback_dir,
        ]
        .into_iter()
        .flatten()
        {
            *value = expand_home(value)?;
            if value.is_relative() {
                *value = path.parent().unwrap_or(Path::new(".")).join(&*value);
            }
        }
        Ok(config)
    }

    fn from_yaml(yaml: &str, mut warn: impl FnMut(String)) -> Result<Self> {
        // Discard unknown values instead of retaining possible secrets in history.
        Ok(serde_ignored::deserialize(
            serde_yaml_ng::Deserializer::from_str(yaml),
            |path| warn(path.to_string()),
        )?)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.audio_target_peak.is_finite()
                && self.audio_target_peak > 0.0
                && self.audio_target_peak <= 1.0,
            "audio_target_peak must be greater than 0 and at most 1"
        );
        ensure!(
            self.audio_max_gain.is_finite() && self.audio_max_gain >= 1.0,
            "audio_max_gain must be finite and at least 1"
        );
        ensure!(
            (1..=256).contains(&self.threads),
            "threads must be between 1 and 256"
        );
        ensure!(
            (1..=3600).contains(&self.max_seconds),
            "max_seconds must be between 1 and 3600"
        );
        ensure!(
            (50..=5000).contains(&self.paste_delay_ms),
            "paste_delay_ms must be between 50 and 5000"
        );
        ensure!(
            !self.prompt.contains('\0'),
            "prompt cannot contain NUL characters"
        );
        for language in &self.languages {
            ensure!(
                !language.contains('\0') && whisper_rs::get_lang_id(language).is_some(),
                "Unknown language in languages: {language}"
            );
        }
        Ok(())
    }

    pub fn model_path(&self) -> Result<PathBuf> {
        if let Some(path) = &self.model {
            return expand_home(path);
        }
        let home =
            env::var_os("HOME").context("HOME is unset; specify model in YAML or --model")?;
        Ok(PathBuf::from(home).join(".local/share/whisper/ggml-large-v3-turbo.bin"))
    }
}

pub(crate) fn expand_home(path: &Path) -> Result<PathBuf> {
    if let Ok(rest) = path.strip_prefix("~") {
        return Ok(
            PathBuf::from(env::var_os("HOME").context("Cannot expand ~: HOME is unset")?)
                .join(rest),
        );
    }
    Ok(path.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaml_supports_multiline_prompt_and_relative_model() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        fs::write(&path, "model: models/test.bin\nhistory_dir: history\nfeedback_dir: corrections\nlanguages: [pl]\nthreads: 8\nprompt: |\n  Rust i Linux.\n  Zażółć gęślą jaźń.\n").unwrap();
        let config = Config::load(Some(&path)).unwrap();
        assert_eq!(config.model.unwrap(), dir.path().join("models/test.bin"));
        assert_eq!(config.history_dir.unwrap(), dir.path().join("history"));
        assert_eq!(config.feedback_dir.unwrap(), dir.path().join("corrections"));
        assert_eq!(config.prompt, "Rust i Linux.\nZażółć gęślą jaźń.\n");
        assert_eq!(config.languages, ["pl"]);
        assert_eq!(config.threads, 8);
        assert_eq!(config.max_seconds, 300);
        let restricted: Config = serde_yaml_ng::from_str("languages: [pl, en]").unwrap();
        restricted.validate().unwrap();
        assert_eq!(restricted.languages, ["pl", "en"]);
    }

    #[test]
    fn unknown_fields_warn_without_retaining_their_values() {
        let yaml = "threads: 8\nprompt: vocabulary\nunknown_test_field: secret-for-test\nextra: {nested: [1, 2]}\n";
        let mut warnings = Vec::new();
        let config = Config::from_yaml(yaml, |field| warnings.push(field)).unwrap();
        config.validate().unwrap();
        assert_eq!(config.threads, 8);
        assert_eq!(config.prompt, "vocabulary");
        assert_eq!(warnings, ["unknown_test_field", "extra"]);
        for output in [
            serde_yaml_ng::to_string(&config).unwrap(),
            format!("{config:?}"),
        ] {
            assert!(!output.contains("secret-for-test"));
            assert!(!output.contains("unknown_test_field"));
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        fs::write(&path, yaml).unwrap();
        assert_eq!(Config::load(Some(&path)).unwrap().threads, 8);
    }

    #[test]
    fn missing_files_and_invalid_known_values_are_errors() {
        assert!(Config::load(Some(Path::new("/nonexistent/ttser.yaml"))).is_err());
        assert!(Config::from_yaml("threads: wrong-type", |_| {}).is_err());
        assert!(Config::from_yaml("languages: [", |_| {}).is_err());
        assert!(Config::from_yaml("threads: 4\nthreads: 8", |_| {}).is_err());
        for yaml in [
            "threads: 0",
            "max_seconds: 0",
            "paste_delay_ms: 0",
            "audio_target_peak: 0",
            "audio_target_peak: 1.1",
            "audio_target_peak: .nan",
            "audio_max_gain: 0.5",
            "audio_max_gain: .inf",
            "languages: [pl, not-a-language]",
            "languages: [auto]",
            "languages: [\"pl\\0\"]",
        ] {
            assert!(
                serde_yaml_ng::from_str::<Config>(yaml)
                    .unwrap()
                    .validate()
                    .is_err()
            );
        }
    }
}

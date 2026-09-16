use anyhow::{Context, Result, ensure};
use serde::Serialize;
use std::collections::BTreeMap;
use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState,
};

use crate::config::Config;

#[derive(Debug, Default, Serialize)]
pub struct Transcript {
    pub text: String,
    pub raw_text: String,
    pub language: Option<String>,
    pub language_probabilities: BTreeMap<String, f32>,
}

pub struct Engine {
    state: WhisperState,
    languages: Vec<String>,
    threads: i32,
    prompt: String,
}

impl Engine {
    pub fn new(options: &Config) -> Result<Self> {
        options.validate()?;
        let path = options.model_path()?;
        ensure!(
            path.is_file(),
            "Model not found: {}. Pass --model PATH to an existing GGML model.",
            path.display()
        );
        let mut params = WhisperContextParameters::default();
        params.use_gpu(!options.cpu && cfg!(feature = "vulkan"));
        let context = WhisperContext::new_with_params(
            path.to_str().context("Model path must be UTF-8")?,
            params,
        )
        .context("Loading Whisper model")?;
        let state = context.create_state().context("Allocating Whisper state")?;
        Ok(Self {
            state,
            languages: options.languages.clone(),
            threads: options.threads as i32,
            prompt: options.prompt.clone(),
        })
    }

    pub fn transcribe(&mut self, samples: &[f32]) -> Result<Transcript> {
        // Very short/zero input otherwise tends to produce hallucinated subtitles.
        if samples.len() < 4_000 || samples.iter().all(|s| s.abs() < 0.00001) {
            return Ok(Transcript::default());
        }
        let mut language_probabilities = BTreeMap::new();
        let language = match self.languages.as_slice() {
            [] => None,
            [language] => Some(language.as_str()),
            languages => {
                // Restrict the language decision before decoding, rather than rewriting
                // a transcript already decoded with the wrong language token.
                self.state.pcm_to_mel(samples, self.threads as usize)?;
                let (_, probabilities) = self.state.lang_detect(0, self.threads as usize)?;
                for (id, &probability) in probabilities.iter().enumerate() {
                    if let Some(code) = whisper_rs::get_lang_str(id as i32) {
                        language_probabilities.insert(code.to_owned(), probability);
                    }
                }
                Some(select_language(languages, &probabilities)?)
            }
        };
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(self.threads);
        params.set_language(language);
        params.set_translate(false);
        params.set_no_context(true);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_print_special(false);
        params.set_temperature(0.0);
        params.set_suppress_blank(true);
        params.set_suppress_nst(true);
        params.set_initial_prompt(&self.prompt);
        self.state
            .full(params, samples)
            .context("Whisper transcription failed")?;
        let mut text = String::new();
        for segment in self.state.as_iter() {
            text.push_str(segment.to_str()?);
        }
        Ok(Transcript {
            text: normalize(&text),
            raw_text: text,
            language: whisper_rs::get_lang_str(self.state.full_lang_id_from_state())
                .map(str::to_owned),
            language_probabilities,
        })
    }
}

fn select_language<'a>(languages: &'a [String], probabilities: &[f32]) -> Result<&'a str> {
    let mut best: Option<(&str, f32)> = None;
    for language in languages {
        let id = whisper_rs::get_lang_id(language).context("Unknown detection language")?;
        let score = *probabilities
            .get(id as usize)
            .context("Missing language probability")?;
        ensure!(
            score.is_finite() && score >= 0.0,
            "Invalid language probability"
        );
        if best.is_none_or(|(_, previous)| score > previous) {
            best = Some((language, score));
        }
    }
    best.map(|(language, _)| language)
        .context("No detection languages configured")
}

pub fn normalize(text: &str) -> String {
    let text = text.replace(['\r', '\n'], "");
    let text = text.trim();
    let text = text.strip_suffix('.').unwrap_or(text).trim_end();
    if text == "[BLANK_AUDIO]" {
        String::new()
    } else {
        text.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection_restricts_candidates_without_forcing_polish_on_english() {
        let languages = vec!["pl".into(), "en".into()];
        let mut probabilities = vec![0.0; whisper_rs::get_lang_max_id() as usize + 1];
        let pl = whisper_rs::get_lang_id("pl").unwrap() as usize;
        let en = whisper_rs::get_lang_id("en").unwrap() as usize;
        let uk = whisper_rs::get_lang_id("uk").unwrap() as usize;
        probabilities[uk] = 0.8;
        probabilities[pl] = 0.15;
        probabilities[en] = 0.05;
        assert_eq!(select_language(&languages, &probabilities).unwrap(), "pl");
        probabilities[pl] = 0.05;
        probabilities[en] = 0.15;
        assert_eq!(select_language(&languages, &probabilities).unwrap(), "en");
        probabilities[pl] = 0.15;
        assert_eq!(select_language(&languages, &probabilities).unwrap(), "pl");
        assert!(select_language(&languages, &[]).is_err());
        probabilities[pl] = f32::NAN;
        assert!(select_language(&languages, &probabilities).is_err());
    }

    #[test]
    fn matches_the_scripts_text_cleanup() {
        assert_eq!(normalize("  Zażółć\n gęślą jaźń.\n"), "Zażółć gęślą jaźń");
        assert_eq!(normalize("Rust 🦀!"), "Rust 🦀!");
        assert_eq!(normalize("a\nb"), "ab");
        assert_eq!(normalize("  [BLANK_AUDIO].\n"), "");
        assert_eq!(normalize("foo.bar.."), "foo.bar.");
        assert_eq!(normalize("\r\n . \n"), "");
    }
}

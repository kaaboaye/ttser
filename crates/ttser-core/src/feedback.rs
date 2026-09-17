use crate::{
    config::{Config, expand_home},
    history,
    speech::Transcript,
};
use anyhow::Result;
use serde::Serialize;
use std::{
    fs,
    io::Write,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Serialize)]
struct Feedback<'a> {
    version: u32,
    created_at_unix_ms: u64,
    transcript: &'a Transcript,
    corrected_text: &'a str,
    history_directory: Option<&'a Path>,
}

pub(crate) fn save(
    config: &Config,
    transcript: &Transcript,
    corrected_text: &str,
    history_directory: Option<&Path>,
) -> Result<()> {
    if corrected_text == transcript.text {
        return Ok(());
    }
    let Some(root) = &config.feedback_dir else {
        return Ok(());
    };
    let root = expand_home(root)?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?;
    let record = Feedback {
        version: 1,
        created_at_unix_ms: now.as_millis() as u64,
        transcript,
        corrected_text,
        history_directory,
    };
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.recursive(true).create(&root)?;
    let name = format!("{}-{}", now.as_nanos(), std::process::id());
    let temporary = root.join(format!("{name}.tmp"));
    let mut file = history::create_private(&temporary)?;
    file.write_all(serde_yaml_ng::to_string(&record)?.as_bytes())?;
    drop(file);
    fs::rename(temporary, root.join(format!("{name}.yaml")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_changed_text_creates_feedback_and_preserves_exact_text() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("feedback");
        let config = Config {
            feedback_dir: Some(root.clone()),
            ..Config::default()
        };
        let transcript = Transcript {
            text: "Żółw 🐢".into(),
            raw_text: " Żółw 🐢.".into(),
            ..Transcript::default()
        };
        save(&config, &transcript, &transcript.text, None).unwrap();
        assert!(!root.exists());
        for corrected in ["Żółw 🐢\n", "", " Żółw 🐢"] {
            save(&config, &transcript, corrected, Some(temp.path())).unwrap();
        }
        let mut corrections = Vec::new();
        for entry in fs::read_dir(&root).unwrap() {
            let path = entry.unwrap().path();
            let record: serde_yaml_ng::Value =
                serde_yaml_ng::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(record["transcript"]["text"], transcript.text);
            assert_eq!(record["transcript"]["raw_text"], transcript.raw_text);
            assert_eq!(record["history_directory"].as_str(), temp.path().to_str());
            corrections.push(record["corrected_text"].as_str().unwrap().to_owned());
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    fs::metadata(path).unwrap().permissions().mode() & 0o777,
                    0o600
                );
                assert_eq!(
                    fs::metadata(&root).unwrap().permissions().mode() & 0o777,
                    0o700
                );
            }
        }
        corrections.sort();
        assert_eq!(corrections, ["", " Żółw 🐢", "Żółw 🐢\n"]);
        let disabled = Config {
            feedback_dir: None,
            ..config
        };
        save(&disabled, &transcript, "changed", None).unwrap();
        assert_eq!(fs::read_dir(root).unwrap().count(), 3);
    }
}

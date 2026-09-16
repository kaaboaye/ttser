# Audio normalization investigation

## Local comparison, September 16–17, 2026

Compared 11 existing recordings with the installed large-v3-turbo model and
Vulkan backend, preserving each recording's language and prompt settings.
Each recording was transcribed unchanged and at five target peaks: 0.1, 0.25,
0.5, 0.85 and 0.95 (66 transcriptions). Original peaks ranged from about 0.045
to 0.094. The experimental gain cap was 100x so every target could be reached;
the chosen 0.25 target needs at most 5.57x on this sample, below the 10x runtime cap.

Two references were explicitly corrected by the speaker; the other nine are
historical recognizer output, described by the speaker as generally correct.
Consequently these are comparisons against provisional references, not an
independent accuracy benchmark. Punctuation and case are excluded from the
word comparison; Polish diacritics are retained.

| Target peak | Recordings matching reference words | Word edits against references |
| --- | ---: | ---: |
| Original | 9/11 | 5 |
| 0.1 | 10/11 | 1 |
| 0.25 | 10/11 | 1 |
| 0.5 | 10/11 | 1 |
| 0.85 | 9/11 | 3 |
| 0.95 | 9/11 | 3 |

Reading the actual variants, rather than just the counts:

- The short corrected word became correct at every amplified level. At 0.5
  and above, the recognizer added a question mark.
- The other confirmed lexical error remained at every level.
- At 0.85 and 0.95, one sentence changed word order. Both versions are
  grammatical; the additional edit distance does not establish worse recognition.
- Other differences were commas. No further obvious language errors disappeared.

The default is **0.25 (approximately −12 dBFS), maximum gain 10x**. It is a
reasonable starting point within the tied 0.1–0.5 range, not a statistically
established optimum. Larger, independently corrected Polish and English samples
would be needed to distinguish these settings reliably. The implementation
uses one multiplier per recording before both language detection and decoding;
it also attenuates peaks above the target. It does not improve signal-to-noise
ratio or compensate for a single loud click dominating a recording's peak.

Raw recordings remain in the application's history. Full comparison transcripts
belong in gitignored `target/benchmarks/` or `/tmp`, separate from runtime state.
See [testing](testing.md#audio-level-comparison) for the reproducible harness.

## What other projects do

These observations concern the inspected application paths, not every engine,
operating-system microphone effect, or optional third-party dependency.

| Project | Observed behavior |
| --- | --- |
| [OpenSuperWhisper: PCM capture](https://github.com/Starmel/OpenSuperWhisper/blob/c8e6fe79d6851078940f459ab7dbc8ee39e2a97d/OpenSuperWhisper/PCMRecording.swift#L114), [Whisper input](https://github.com/Starmel/OpenSuperWhisper/blob/c8e6fe79d6851078940f459ab7dbc8ee39e2a97d/OpenSuperWhisper/Engines/WhisperEngine.swift#L641) | The inspected path converts/resamples audio and averages active channels. Its variable named `normalization` divides by channel count; it does not equalize peak volume. |
| [Handy: recorder](https://github.com/cjpais/Handy/blob/ba10ce1943ef34e93c09494027fc0b9ced2e8a44/src-tauri/src/audio_toolkit/audio/recorder.rs#L632), [transcription](https://github.com/cjpais/Handy/blob/ba10ce1943ef34e93c09494027fc0b9ced2e8a44/src-tauri/src/managers/transcription.rs#L1176) | Resampling and optional voice activity filtering; no peak gain stage in the inspected path. Visualizer gain is separate from recognizer input. |
| [Buzz: file input](https://github.com/chidiwilliams/buzz/blob/7d1929c58868bbcc9803b0df180fa3701103c040/buzz/whisper_audio.py), [live input](https://github.com/chidiwilliams/buzz/blob/7d1929c58868bbcc9803b0df180fa3701103c040/buzz/transcriber/recording_transcriber.py#L100) | Converts PCM integers to floats and measures RMS for silence decisions; no peak gain stage in these paths. Dividing int16 by 32768 converts units, without equalizing recording volume. |
| [Vibe: normalize](https://github.com/thewh1teagle/vibe/blob/4d1db65fd07d7927d7b4ef3a3d949539010147dc/desktop/src-tauri/src/ffmpeg.rs#L72), [server input](https://github.com/thewh1teagle/vibe/blob/4d1db65fd07d7927d7b4ef3a3d949539010147dc/server/crates/vibe-server/src/audio.rs) | `normalize` converts to mono 16 kHz PCM by default, without peak normalization. Additional filters can be supplied; server enhancement removes silence. A separate microphone/system mixing path has a limiter. |
| [OpenWhispr: capture constraints](https://github.com/OpenWhispr/openwhispr/blob/834a0771fea146bd3e0b693abd9193cbd288705c/src/helpers/audioManager.js#L990) | Explicitly disables browser automatic gain control, echo cancellation and noise suppression. The code explains that browser AGC can change Windows microphone volume. This is distinct from multiplying a captured buffer. |
| [whisper-guard 0.3.0](https://docs.rs/whisper-guard/0.3.0/src/whisper_guard/audio.rs.html#125-152) | For peaks above 0.0001 and below 0.1, targets 0.5 with a 100x gain cap. Other recordings are unchanged. |
| [Portavoz: normalization](https://github.com/johnny4young/portavoz/blob/c1cf1ef0b1e3f5fac53395f18f8f84056702dded/Sources/TranscriptionKit/VocabularyPrompt.swift#L4-L27), [call site](https://github.com/johnny4young/portavoz/blob/c1cf1ef0b1e3f5fac53395f18f8f84056702dded/Sources/TranscriptionKit/WhisperEngine.swift#L175) | Boosts peaks below 0.9 toward 0.9, capped at 20x; louder recordings are unchanged. Its motivation is quiet recordings falling below WhisperKit's energy-based VAD threshold. |

The projects do not establish a single optimal level. Whisper's author explains
that the model is not completely scale invariant and that its feature
normalization operates on the log-mel spectrogram, not by equalizing every
recording's peak: [Whisper discussion #428](https://github.com/openai/whisper/discussions/428).
The extreme scaling example in that discussion is not evidence for a particular
recommended peak or LUFS target.

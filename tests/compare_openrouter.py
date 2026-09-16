"""Upload explicitly selected history recordings to OpenRouter for comparison.

Requires PyYAML. Makes paid API requests; never runs as part of unit tests.
"""
import argparse
import base64
import hashlib
import io
import json
import os
import struct
import time
import urllib.error
import urllib.request
import wave
from datetime import datetime, timezone
from pathlib import Path

import yaml

from compare_audio_levels import distance, wav_samples, words

ENDPOINT = "https://openrouter.ai/api/v1/audio/transcriptions"
MODELS = ["microsoft/mai-transcribe-2", "openai/gpt-transcribe", "google/chirp-3"]


def audio_variant(samples, target):
    peak = max(map(abs, samples), default=0.0)
    gain = min(target / peak, 10.0) if target is not None and peak >= 0.00001 else 1.0
    # PCM16 WAV works across providers; both variants use identical quantization.
    pcm = [max(-32768, min(32767, round(sample * gain * 32768))) for sample in samples]
    output = io.BytesIO()
    with wave.open(output, "wb") as wav:
        wav.setnchannels(1)
        wav.setsampwidth(2)
        wav.setframerate(16000)
        wav.writeframes(struct.pack("<" + "h" * len(pcm), *pcm))
    return output.getvalue(), {"input_peak": peak, "gain": gain, "output_peak": peak * gain}


def request_body(model, audio, language):
    payload = {
        "model": model,
        "input_audio": {"data": base64.b64encode(audio).decode("ascii"), "format": "wav"},
        "response_format": "json",
    }
    if language is not None:
        payload["language"] = language
    return payload


def reference_edits(accepted, actual):
    return min(distance(words(expected), words(actual)) for expected in accepted)


def transcribe(key, payload):
    request = urllib.request.Request(
        ENDPOINT,
        data=json.dumps(payload).encode(),
        headers={"Authorization": "Bearer " + key, "Content-Type": "application/json"},
    )
    started = time.monotonic()
    try:
        with urllib.request.urlopen(request, timeout=90) as response:
            result = json.load(response)
            if not isinstance(result.get("text"), str):
                return {"status": "error", "error": "Response has no text field"}
            return {
                "status": "ok",
                "actual": result["text"],
                "usage": result.get("usage"),
                "generation_id": response.headers.get("X-Generation-Id"),
                "elapsed_seconds": time.monotonic() - started,
            }
    except urllib.error.HTTPError as error:
        # Do not persist an error body: a provider may echo credentials or audio.
        return {"status": "error", "http_status": error.code,
                "error": error.reason, "elapsed_seconds": time.monotonic() - started}
    except (urllib.error.URLError, TimeoutError, ValueError) as error:
        return {"status": "error", "error": type(error).__name__,
                "elapsed_seconds": time.monotonic() - started}


def save_report(output, report):
    (output / "report.json").write_text(json.dumps(report, ensure_ascii=False, indent=2))
    lines = ["# OpenRouter transcription comparison", "",
             "Two volume levels and automatic/Polish language. References were not sent to models.",
             "Single requests per condition; timing includes network and provider overhead.", ""]
    if report.get("assessment"):
        lines.extend([report["assessment"], ""])
    for recording in report["recordings"]:
        lines.extend(["## " + recording["id"], "", "Accepted references: " + " / ".join(recording["accepted_references"]), "",
                      "| Model | Volume | Language | Result | Seconds | Word edits |",
                      "| --- | --- | --- | --- | ---: | ---: |"])
        for row in report["rows"]:
            if row["recording"] != recording["id"]:
                continue
            text = row.get("actual", row.get("error", ""))
            text = text.replace("|", "\\|").replace("\n", " ")
            lines.append(f"| {row['model']} | {row['variant']} | {row['language'] or 'auto'} | "
                         f"{text} | {row.get('elapsed_seconds', 0):.2f} | {row.get('word_edits', '—')} |")
        lines.append("")
    (output / "comparison.md").write_text("\n".join(lines))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", type=Path, required=True, help="YAML containing openrouter_api_key")
    parser.add_argument("--history", type=Path, required=True)
    parser.add_argument("--recordings", nargs="+", required=True, help="Explicit history directory IDs to upload")
    parser.add_argument("--references", type=Path, required=True, help="JSON map of IDs to text or a list of accepted texts")
    parser.add_argument("--output", type=Path, required=True, help="New private directory under target/benchmarks or /tmp")
    args = parser.parse_args()
    key = yaml.safe_load(args.config.read_text()).get("openrouter_api_key")
    if not isinstance(key, str) or not key.strip():
        parser.error("Configuration needs a nonempty openrouter_api_key")
    corrections = json.loads(args.references.read_text())
    recordings = []
    for identifier in dict.fromkeys(args.recordings):
        if Path(identifier).name != identifier or identifier in (".", ".."):
            parser.error("Recordings must be directory names within history")
        path = args.history / identifier / "audio.wav"
        samples = wav_samples(path)
        accepted = corrections[identifier]
        if isinstance(accepted, str):
            accepted = [accepted]
        if not isinstance(accepted, list) or not accepted or not all(isinstance(s, str) and s.strip() for s in accepted):
            parser.error("Each reference must be text or a nonempty list of accepted texts")
        recordings.append((identifier, samples, {
            "id": identifier, "accepted_references": accepted,
            "duration_seconds": len(samples) / 16000,
            "source_sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
        }))
    os.umask(0o077)
    args.output.mkdir(parents=True, exist_ok=False, mode=0o700)
    report = {
        "created_at": datetime.now(timezone.utc).isoformat(), "endpoint": ENDPOINT,
        "audio_format": "PCM16 WAV, mono, 16000 Hz", "max_gain": 10.0,
        "reference_warning": "Word comparison preserves diacritics; grammatical variants require human assessment",
        "recordings": [metadata for _, _, metadata in recordings], "rows": [],
    }
    save_report(args.output, report)
    for identifier, samples, metadata in recordings:
        for label, target in [("original", None), ("peak_0.25", 0.25)]:
            audio, levels = audio_variant(samples, target)
            for language in [None, "pl"]:
                for model in MODELS:
                    result = transcribe(key.strip(), request_body(model, audio, language))
                    row = {"recording": identifier, "variant": label, "language": language,
                           "model": model, **levels, "audio_sha256": hashlib.sha256(audio).hexdigest(),
                           **result}
                    if result["status"] == "ok":
                        row["word_edits"] = reference_edits(metadata["accepted_references"], result["actual"])
                    report["rows"].append(row)
                    save_report(args.output, report)
                    print(f"{len(report['rows'])}/{len(recordings) * 12} {model} "
                          f"{identifier} {label} {language or 'auto'}: {result['status']}", flush=True)
                    if result.get("http_status") in (401, 402, 403):
                        raise SystemExit("Stopped after authorization/billing refusal; see report")


if __name__ == "__main__":
    main()

"""Compare peak normalization against local history references (requires PyYAML)."""
import argparse
import hashlib
import json
import math
import os
import re
import struct
import subprocess
import tempfile
from pathlib import Path

import yaml


def words(text):
    return re.findall(r"\w+", text.casefold())


def distance(expected, actual):
    previous = list(range(len(actual) + 1))
    for row, left in enumerate(expected, 1):
        current = [row]
        for column, right in enumerate(actual, 1):
            current.append(min(current[-1] + 1, previous[column] + 1, previous[column - 1] + (left != right)))
        previous = current
    return previous[-1]


def wav_samples(path):
    data = path.read_bytes()
    assert data[:4] == b"RIFF" and data[8:12] == b"WAVE"
    offset = 12
    audio_format = None
    while offset + 8 <= len(data):
        name, size = struct.unpack_from("<4sI", data, offset)
        block = data[offset + 8:offset + 8 + size]
        if name == b"fmt ":
            encoding, channels, rate, _, _, bits = struct.unpack_from("<HHIIHH", block)
            if encoding == 0xFFFE:
                encoding = struct.unpack_from("<H", block, 24)[0]
            audio_format = encoding, channels, rate, bits
        elif name == b"data":
            assert audio_format == (3, 1, 16000, 32), "Expected ttser float history WAV"
            samples = struct.unpack("<" + "f" * (len(block) // 4), block)
            assert all(math.isfinite(sample) for sample in samples)
            return samples
        offset += 8 + size + size % 2
    raise ValueError("No audio data")


def wav_peak(path):
    return max(map(abs, wav_samples(path)), default=0.0)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/release/ttser"))
    parser.add_argument("--history", type=Path, required=True)
    parser.add_argument("--recordings", nargs="+", help="Limit comparison to these history directory IDs")
    parser.add_argument("--output", type=Path, required=True, help="New directory under target/benchmarks or /tmp")
    parser.add_argument("--references", type=Path, help="JSON map of recording IDs to corrected text")
    parser.add_argument("--peaks", nargs="+", type=float, default=[0.1, 0.25, 0.5, 0.85, 0.95])
    parser.add_argument("--max-gain", type=float, default=100.0)
    args = parser.parse_args()
    assert all(0 < peak <= 1 for peak in args.peaks)
    assert math.isfinite(args.max_gain) and args.max_gain >= 1
    binary = args.binary.resolve()
    corrections = json.loads(args.references.read_text()) if args.references else {}
    if args.recordings:
        if any(Path(name).name != name or name in (".", "..") for name in args.recordings):
            parser.error("Recordings must be directory names within history")
        entries = [args.history / name / "record.yaml" for name in dict.fromkeys(args.recordings)]
    else:
        entries = sorted(args.history.glob("*/record.yaml"))
    args.output.mkdir(parents=True, exist_ok=False, mode=0o700)
    # Reports contain private transcripts; keep them out of version control.
    os.umask(0o077)
    report = {"reference_warning": "Uncorrected references are historical model output, not independent ground truth", "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(), "rows": []}
    variants = [("original", None)] + [(f"peak_{peak:g}", peak) for peak in args.peaks]
    with tempfile.TemporaryDirectory(dir=args.output) as temporary:
        config_file = Path(temporary) / "config.yaml"
        for index, entry in enumerate(entries, 1):
            record = yaml.safe_load(entry.read_text())
            if not record.get("transcript"):
                continue
            identifier = entry.parent.name
            wav = entry.parent / "audio.wav"
            peak = wav_peak(wav)
            assert 0 < peak <= 1, "Benchmark expects non-silent, unclipped recordings"
            expected = corrections.get(identifier, record["transcript"]["text"])
            settings = dict(record["settings"])
            settings["history_dir"] = None
            for label, target in variants:
                settings["audio_target_peak"] = peak if target is None else target
                settings["audio_max_gain"] = 1.0 if target is None else args.max_gain
                config_file.write_text(yaml.safe_dump(settings, allow_unicode=True))
                result = subprocess.run([str(binary), "--config", str(config_file), "transcribe", str(wav)], check=True, capture_output=True, text=True, timeout=120)
                actual = result.stdout.strip()
                gain = min(settings["audio_target_peak"] / peak, settings["audio_max_gain"])
                row = {"recording": identifier, "variant": label, "input_peak": peak, "gain": gain, "output_peak": peak * gain, "gain_capped": target is not None and gain == args.max_gain, "expected": expected, "reference_source": "correction" if identifier in corrections else "historical_output", "actual": actual, "word_edits": distance(words(expected), words(actual)), "reference_words": len(words(expected))}
                report["rows"].append(row)
                (args.output / "report.json").write_text(json.dumps(report, ensure_ascii=False, indent=2))
            print(f"{index}/{len(entries)} recordings compared", flush=True)
    for label, _ in variants:
        rows = [row for row in report["rows"] if row["variant"] == label]
        summary = {"variant": label, "word_edits": sum(row["word_edits"] for row in rows), "reference_words": sum(row["reference_words"] for row in rows), "exact_word_matches": sum(row["word_edits"] == 0 for row in rows), "recordings": len(rows)}
        print(json.dumps(summary), flush=True)


if __name__ == "__main__":
    main()

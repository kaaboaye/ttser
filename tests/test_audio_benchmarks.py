import base64
import io
import struct
import unittest
import wave
from unittest.mock import patch
from urllib.error import HTTPError

from compare_audio_levels import distance, words
from compare_openrouter import audio_variant, reference_edits, request_body, transcribe


class BenchmarkTests(unittest.TestCase):
    def test_pcm_conversion_keeps_duration_and_gain_without_mutating_input(self):
        samples = [-0.05, 0.0, 0.025, 0.1]
        for target, expected in [(None, [-1638, 0, 819, 3277]),
                                 (0.25, [-4096, 0, 2048, 8192])]:
            audio, _ = audio_variant(samples, target)
            with wave.open(io.BytesIO(audio)) as wav:
                self.assertEqual((wav.getnchannels(), wav.getsampwidth(), wav.getframerate()), (1, 2, 16000))
                self.assertEqual(wav.getnframes(), len(samples))
                self.assertEqual(list(struct.unpack("<4h", wav.readframes(4))), expected)
        self.assertEqual(samples, [-0.05, 0.0, 0.025, 0.1])
        self.assertEqual(audio_variant([0.001], 0.25)[1]["gain"], 10.0)
        self.assertEqual(audio_variant([0.0], 0.25)[1]["gain"], 1.0)

    def test_request_contains_audio_and_language_without_answer_hints(self):
        payload = request_body("google/chirp-3", b"audio", None)
        self.assertEqual(set(payload), {"model", "input_audio", "response_format"})
        self.assertEqual(base64.b64decode(payload["input_audio"]["data"]), b"audio")
        self.assertEqual(request_body("google/chirp-3", b"audio", "pl")["language"], "pl")

    def test_errors_do_not_persist_echoed_secrets_or_audio(self):
        error = HTTPError("https://openrouter.ai", 400, "Bad Request", {}, io.BytesIO(b"secret-audio-key"))
        with patch("urllib.request.urlopen", side_effect=error):
            result = transcribe("private-key", request_body("google/chirp-3", b"private-audio", None))
        self.assertEqual(result["http_status"], 400)
        self.assertNotIn("private", str(result))
        self.assertNotIn("secret", str(result))

    def test_word_distance_ignores_punctuation_but_preserves_polish_spelling(self):
        self.assertEqual(distance(words("Testuje."), words("testuje?")), 0)
        self.assertEqual(distance(words("testuje"), words("testuję")), 1)
        self.assertEqual(distance(words("poszukaj"), words("poszukań")), 1)
        self.assertEqual(distance([], words("dodatkowe słowo")), 2)
        self.assertEqual(reference_edits(["testuje", "testuję"], "Testuję."), 0)
        self.assertEqual(reference_edits(["testuje", "testuję"], "To jest to ja"), 4)


if __name__ == "__main__":
    unittest.main()

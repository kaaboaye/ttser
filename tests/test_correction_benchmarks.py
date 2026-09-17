import io
import json
import unittest
from unittest.mock import patch
from urllib.error import HTTPError

from compare_corrections import complete, request_body


class CorrectionBenchmarkTests(unittest.TestCase):
    def test_references_and_history_metadata_never_reach_provider(self):
        case = {"id": "private-case", "transcript": "Recognized text",
                "references": ["secret expected answer"], "history": "/private/history",
                "context": {"before_cursor": "Relevant context", "private_diagnostic": "secret"}}
        payload = request_body(case, "conservative", "low")
        user = json.loads(payload["messages"][1]["content"])
        self.assertEqual(user, {"transcript": "Recognized text", "context": {"before_cursor": "Relevant context"}})
        self.assertNotIn("secret", json.dumps(payload))
        self.assertEqual(payload["provider"], {"sort": "throughput"})

    def test_large_unicode_context_keeps_text_nearest_cursor_with_bounded_payload(self):
        context = {"before_cursor": "old" * 10000 + "ż" * 1500,
                   "after_cursor": "ą" * 500 + "far" * 10000,
                   "selected_text": "🙂" * 10000, "application": "a" * 10000,
                   "window_title": "b" * 10000}
        payload = request_body({"transcript": "test", "context": context}, "contextual", "low")
        actual = json.loads(payload["messages"][1]["content"])["context"]
        self.assertEqual(actual["before_cursor"], "ż" * 1500)
        self.assertEqual(actual["after_cursor"], "ą" * 500)
        self.assertEqual(sum(map(len, actual.values())), 2800)

    def test_incomplete_or_missing_answers_are_not_scored_as_success(self):
        for content, finish, expected in [("Done", "stop", "ok"), ("Done", "length", "invalid_response"),
                                          (None, "stop", "invalid_response"), ("", "stop", "invalid_response")]:
            response = {"choices": [{"message": {"content": content}, "finish_reason": finish}]}
            with patch("urllib.request.urlopen", return_value=io.BytesIO(json.dumps(response).encode())):
                self.assertEqual(complete("key", {})["status"], expected)

    def test_http_error_cannot_echo_credentials_or_private_context(self):
        error = HTTPError("https://openrouter.ai", 401, "private-key", {}, io.BytesIO(b"private-context"))
        with patch("urllib.request.urlopen", side_effect=error):
            actual = complete("private-key", {})
        self.assertEqual(actual["http_status"], 401)
        self.assertNotIn("private", str(actual))


if __name__ == "__main__":
    unittest.main()

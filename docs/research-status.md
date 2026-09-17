# Paused context and correction experiment

The branch preserves an optional Linux context probe and an offline text
correction benchmark. Correction is not integrated into the dictation pipeline.
The context probe is enabled explicitly with `serve --context-probe-dir`;
omit that option and restart the daemon to stop collecting context.

The experiment made 185 sequential OpenRouter requests to
`openai/gpt-oss-120b`, using throughput routing, four instruction variants,
bounded context, selected repetitions, and low/medium reasoning settings.
All requests were served by Cerebras. Complete-response latency was 0.41 seconds
at the median, 0.70 seconds at p95, and 1.79 seconds at the maximum. Reported
total cost was approximately USD 0.041. These are observations from one session,
not service guarantees.

Results did not justify replacing every transcript automatically. A simple
recognition error could be corrected consistently with one prompt, while a
harder programming-language reference was not recovered in any of 26 attempts.
Some correct inputs acquired inflection errors or unwanted JSON wrappers.
Mixed interface and dictation languages generally remained separate in the
tested cases, but this did not establish reliability for arbitrary inputs.
Synthetic stress cases and repeated failures are not a representative accuracy
sample. Reference answers were withheld from the model.

If resumed, evaluate correction suggestions on additional real examples before
enabling automatic insertion. A structured response contract can address output
wrappers, but cannot guarantee preservation of meaning or recover information
lost by speech recognition. Start with an empty Whisper prompt and evaluate
speech recognition and subsequent correction independently.

Only implementation, synthetic test fixtures, and aggregate observations belong
in version control. Recordings, captured application text, personal transcripts,
API credentials, and detailed benchmark reports remain local and excluded.
See [context capture](context-probe.md) and
[benchmark instructions](testing.md#text-correction-benchmark) to resume.

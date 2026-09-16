# OpenRouter spot check, September 17, 2026

Tested two speaker-corrected Polish recordings that local large-v3-turbo had
misrecognized. Each was sent to three models at original volume and peak 0.25,
with automatic language and an explicit Polish hint: 24 requests in total.
All returned text successfully. Input was mono 16 kHz PCM16 WAV; gain was capped
at 10x. Corrected references were never sent to the models.

| Model | Short corrected word | Longer corrected sentence | Median request time | Cost of 8 requests (USD) |
| --- | --- | --- | ---: | ---: |
| `microsoft/mai-transcribe-2` | Accepted wording | Corrected verb; includes an extra filler absent from the reference | 0.57 s | 0.000889 |
| `openai/gpt-transcribe` | Accepted wording | Matches reference words | 0.74 s | 0.002400 |
| `google/chirp-3` | Accepted wording | Matches reference words | 1.96 s | 0.008533 |

Total API-reported cost: **USD 0.011822**. Request times include network and
provider overhead. There was one request per condition, across two different
durations; these medians are observations, not a general speed ranking.

Changing volume did not change words within any model/language combination;
some commas differed. Google changed the ending of the short word when given
a Polish hint, but both spellings were accepted by the speaker. Microsoft's
filler could reflect an actual hesitation rather than a recognition error;
the corrected reference alone does not settle that.

These results are consistent with tolerance of volume changes, but do not
establish whether or how providers normalize audio internally. All three models
resolved the two known lexical problems. Two selected failure cases are too few
to rank general transcription quality or justify changing the local engine.

The [reproduction instructions](testing.md#openrouter-comparison-on-selected-recordings)
describe the opt-in harness. Full transcripts, references, source/payload hashes,
generation IDs and usage are retained in a local private report under gitignored
`target/benchmarks/`, separate from the application's runtime state.
Benchmark access does not add an OpenRouter runtime backend to the application.

## Follow-up: developer vocabulary

A third corrected recording (1.749 s), containing the word `commita`, was
compared in the same 12 remote conditions. MAI and Chirp consistently returned
the phonetic spelling `komita`; GPT Transcribe consistently returned `pomita`.
Neither volume nor a Polish hint changed those words. Additional API-reported
cost was USD 0.002956.

The local large-v3-turbo model split the intended word into two unrelated words
at the original volume and all five tested target peaks. Thus peak normalization
does not resolve this vocabulary error. The cloud outputs also do not exactly
match the speaker's requested spelling; the earlier two cases should not be
generalized to all dictation.

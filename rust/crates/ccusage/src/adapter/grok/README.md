# Grok adapter

Grok Build CLI (and Grok-in-Cursor via the same runtime) records billable token
usage in the global structured log `logs/unified.jsonl`. Session metadata lives
under `sessions/<url-encoded-cwd>/<session-id>/summary.json`.

## Commands

```sh
ccusage grok daily
ccusage grok monthly
ccusage grok session
ccusage grok daily --grok-home /backup/grok --json
```

`--mode display` is rejected because local Grok data has no precomputed
`costUSD`.

## Data paths

| Source           | Path                                               |
| ---------------- | -------------------------------------------------- |
| Base directory   | `GROK_HOME` or `~/.grok`                           |
| Token usage      | `logs/unified.jsonl` (`shell.turn.inference_done`) |
| Session metadata | `sessions/**/summary.json`                         |

Override with `--grok-home`, `GROK_HOME`, or JSON config `grok.defaults.grokHome`.

## Token mapping

| Grok `ctx` field                                    | ccusage field                                  |
| --------------------------------------------------- | ---------------------------------------------- |
| `prompt_tokens - cached_prompt_tokens` (saturating) | `input_tokens`                                 |
| `cached_prompt_tokens`                              | `cache_read_input_tokens`                      |
| `completion_tokens`                                 | `output_tokens`                                |
| `reasoning_tokens`                                  | `extra_total_tokens` (included in totals/cost) |

One `LoadedEntry` is emitted per `inference_done` line. Cached context is counted
on each inference, not rolled up per user turn.

## Pricing

Model lookup tries the raw `current_model_id`, then `xai/{model}`, then
`openrouter/x-ai/{model}`. Unknown models surface `missing_pricing_model` instead
of falling back to unrelated `grok-4.3` pricing.

## Limitations (V1)

- No `chat_history.jsonl` or `updates.jsonl` token parsing
- No subagent `tokens_used` aggregates
- No user-turn rollup across `loop_index` rounds
- Missing `unified.jsonl` returns an empty report (not an error)

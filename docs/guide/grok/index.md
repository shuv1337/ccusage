# Grok Data Source

ccusage can read Grok Build CLI usage from the local Grok runtime. Grok uses the
same unified and focused report model as Claude Code, Codex, OpenCode, Amp,
pi-agent, Kimi, GitHub Copilot CLI, and Gemini CLI.

## Usage

```sh
# Daily Grok usage
ccusage grok daily

# Monthly Grok usage
ccusage grok monthly

# Grok sessions
ccusage grok session

# Include Grok in the default all-source report
ccusage daily
```

## Data Location

The CLI reads token usage from `GROK_HOME` (defaults to `~/.grok`).

```sh
GROK_HOME="$HOME/.grok" ccusage grok daily
ccusage grok daily --grok-home /backup/grok-archive
```

Expected layout:

```text
~/.grok/
├── logs/unified.jsonl
└── sessions/<url-encoded-cwd>/<session-id>/summary.json
```

JSON config example:

```json
{
	"grok": {
		"defaults": {
			"grokHome": "/backup/grok-archive"
		}
	}
}
```

## Supported Reports

| Command                | Description                 | Related Report                          |
| ---------------------- | --------------------------- | --------------------------------------- |
| `ccusage grok daily`   | Group usage by day          | [Daily Usage](/guide/daily-reports)     |
| `ccusage grok monthly` | Group usage by month        | [Monthly Usage](/guide/monthly-reports) |
| `ccusage grok session` | Group usage by Grok session | [Session Usage](/guide/session-reports) |

## Token Mapping

| Grok `ctx` field                                    | ccusage field                   |
| --------------------------------------------------- | ------------------------------- |
| `prompt_tokens - cached_prompt_tokens` (saturating) | `input_tokens`                  |
| `cached_prompt_tokens`                              | `cache_read_input_tokens`       |
| `completion_tokens`                                 | `output_tokens`                 |
| `reasoning_tokens`                                  | extra totals (included in cost) |

Only `shell.turn.inference_done` lines with a session ID are included. Each
inference becomes one report row, so cached context is counted per inference
rather than rolled up per user turn.

## Cost Calculation

Grok rows do not store recorded USD cost locally, so ccusage estimates cost from
token counts and LiteLLM pricing. Use `--mode auto` or `--mode calculate`.
`--mode display` is rejected because there is no local precomputed `costUSD`.

## Environment Variables

| Variable    | Description                                          |
| ----------- | ---------------------------------------------------- |
| `GROK_HOME` | Override the Grok data directory (default `~/.grok`) |

## Troubleshooting

::: details No Grok usage data found
Ensure `logs/unified.jsonl` exists under `~/.grok` and contains
`shell.turn.inference_done` events for your date range. Set `GROK_HOME` or
`--grok-home` if your Grok data lives elsewhere.
:::

::: details Totals look higher than expected
Grok logs one row per model inference. A single user prompt with multiple tool
rounds produces multiple `inference_done` lines, and prompt cache hits are counted
on each inference.
:::

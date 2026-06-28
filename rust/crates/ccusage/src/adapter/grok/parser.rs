use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
    sync::Arc,
};

use jiff::tz::TimeZone as JiffTimeZone;
use serde::Deserialize;

use super::super::jsonl;
use crate::{
    LoadedEntry, PricingMap, Result, TimestampMs, TokenUsageRaw, UsageEntry, UsageMessage,
    calculate_cost_for_usage, cli::CostMode, fast::LinePrefilter, format_date_tz,
    missing_pricing_model_for_candidates, parse_ts_timestamp,
};

const INFERENCE_DONE_MSG: &str = "shell.turn.inference_done";
const DEFAULT_MODEL: &str = "grok-build";
const UNKNOWN_PROJECT: &str = "unknown";

#[derive(Debug, Deserialize)]
struct UnifiedLogLine {
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    ts: Option<String>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    sid: Option<String>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    msg: Option<String>,
    ctx: Option<InferenceCtx>,
}

#[derive(Debug, Deserialize)]
struct InferenceCtx {
    #[serde(default)]
    loop_index: Option<u64>,
    #[serde(default)]
    prompt_tokens: Option<u64>,
    #[serde(default)]
    cached_prompt_tokens: Option<u64>,
    #[serde(default)]
    completion_tokens: Option<u64>,
    #[serde(default)]
    reasoning_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct SummaryInfo {
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    id: Option<String>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    cwd: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SummaryRecord {
    info: Option<SummaryInfo>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    current_model_id: Option<String>,
}

#[derive(Debug, Clone)]
pub(super) struct SessionMeta {
    pub(super) cwd: String,
    pub(super) model_id: String,
}

#[derive(Debug, Clone)]
pub(super) struct GrokUsageEntry {
    pub(super) timestamp: TimestampMs,
    pub(super) timestamp_text: String,
    pub(super) session_id: String,
    pub(super) project: String,
    pub(super) model: String,
    pub(super) usage: TokenUsageRaw,
    pub(super) reasoning_tokens: u64,
    pub(super) entry_id: String,
}

pub(super) fn build_session_index(sessions_root: &Path) -> HashMap<String, SessionMeta> {
    let mut index = HashMap::new();
    let Ok(entries) = fs::read_dir(sessions_root) else {
        return index;
    };
    for encoded_dir in entries.flatten() {
        let encoded_path = encoded_dir.path();
        if !encoded_path.is_dir() {
            continue;
        }
        let Ok(session_entries) = fs::read_dir(&encoded_path) else {
            continue;
        };
        let encoded_cwd = encoded_dir.file_name().to_str().map(str::to_string);
        for session_dir in session_entries.flatten() {
            let summary_path = session_dir.path().join("summary.json");
            if !summary_path.is_file() {
                continue;
            }
            let Ok(content) = fs::read_to_string(&summary_path) else {
                continue;
            };
            let Ok(record) = serde_json::from_str::<SummaryRecord>(&content) else {
                continue;
            };
            let Some(info) = record.info.as_ref() else {
                continue;
            };
            let Some(session_id) = info
                .id
                .as_ref()
                .filter(|id| !id.is_empty())
                .map(|id| id.to_string())
            else {
                continue;
            };
            let cwd = info
                .cwd
                .clone()
                .filter(|cwd| !cwd.is_empty())
                .or_else(|| encoded_cwd.as_ref().map(|value| percent_decode_cwd(value)))
                .unwrap_or_else(|| UNKNOWN_PROJECT.to_string());
            let model_id = record
                .current_model_id
                .filter(|model| !model.is_empty())
                .unwrap_or_else(|| DEFAULT_MODEL.to_string());
            index.insert(session_id, SessionMeta { cwd, model_id });
        }
    }
    index
}

pub(super) fn parse_unified_log(
    path: &Path,
    index: &HashMap<String, SessionMeta>,
) -> Result<Vec<GrokUsageEntry>> {
    let content = match fs::read(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let prefilter = LinePrefilter::all(&[br#""shell.turn.inference_done""#]);
    Ok(jsonl::records::<UnifiedLogLine>(&content, Some(&prefilter))
        .filter_map(|line| line_to_entry(&line, index))
        .collect())
}

fn line_to_entry(
    line: &UnifiedLogLine,
    index: &HashMap<String, SessionMeta>,
) -> Option<GrokUsageEntry> {
    if line.msg.as_deref() != Some(INFERENCE_DONE_MSG) {
        return None;
    }
    let sid = line.sid.as_ref()?;
    let ts = line.ts.as_ref()?;
    let ctx = line.ctx.as_ref()?;
    if !ctx_has_required_token_fields(ctx) {
        return None;
    }
    let timestamp = parse_ts_timestamp(ts)?;
    let usage = map_token_usage(ctx);
    let reasoning_tokens = ctx.reasoning_tokens.unwrap_or(0);
    if crate::total_usage_tokens(usage) + reasoning_tokens == 0 {
        return None;
    }
    let meta = index.get(sid);
    let model = meta
        .map(|meta| meta.model_id.clone())
        .unwrap_or_else(|| DEFAULT_MODEL.to_string());
    let project = meta
        .map(|meta| meta.cwd.clone())
        .unwrap_or_else(|| UNKNOWN_PROJECT.to_string());
    let entry_id = format!("grok:{sid}:{ts}:{}", ctx.loop_index.unwrap_or(0));
    Some(GrokUsageEntry {
        timestamp,
        timestamp_text: crate::format_rfc3339_millis(timestamp),
        session_id: sid.clone(),
        project,
        model,
        usage,
        reasoning_tokens,
        entry_id,
    })
}

fn ctx_has_required_token_fields(ctx: &InferenceCtx) -> bool {
    ctx.loop_index.is_some()
        && ctx.prompt_tokens.is_some()
        && ctx.cached_prompt_tokens.is_some()
        && ctx.completion_tokens.is_some()
}

fn map_token_usage(ctx: &InferenceCtx) -> TokenUsageRaw {
    let prompt_tokens = ctx.prompt_tokens.unwrap_or(0);
    let cached_prompt_tokens = ctx.cached_prompt_tokens.unwrap_or(0);
    TokenUsageRaw {
        input_tokens: prompt_tokens.saturating_sub(cached_prompt_tokens),
        output_tokens: ctx.completion_tokens.unwrap_or(0),
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: cached_prompt_tokens,
        speed: None,
        cache_creation: None,
    }
}

fn percent_decode_cwd(encoded: &str) -> String {
    let mut bytes = Vec::with_capacity(encoded.len());
    let raw = encoded.as_bytes();
    let mut index = 0;
    while index < raw.len() {
        if raw[index] == b'%'
            && index + 2 < raw.len()
            && let Ok(byte) = u8::from_str_radix(
                std::str::from_utf8(&raw[index + 1..index + 3]).unwrap_or(""),
                16,
            )
        {
            bytes.push(byte);
            index += 3;
            continue;
        }
        bytes.push(raw[index]);
        index += 1;
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

pub(super) fn grok_entry_to_loaded(
    entry: GrokUsageEntry,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: &PricingMap,
) -> LoadedEntry {
    let cost_usage = TokenUsageRaw {
        output_tokens: entry.usage.output_tokens + entry.reasoning_tokens,
        cache_creation: None,
        ..entry.usage
    };
    let cost = calculate_grok_cost(&entry, mode, pricing, cost_usage);
    let missing_pricing_model = missing_grok_pricing(&entry, mode, pricing, cost_usage);
    let data = UsageEntry {
        session_id: Some(entry.session_id.clone()),
        timestamp: entry.timestamp_text,
        version: None,
        message: UsageMessage {
            usage: entry.usage,
            model: Some(entry.model.clone()),
            id: Some(entry.entry_id),
        },
        cost_usd: None,
        request_id: None,
        is_api_error_message: None,
        is_sidechain: None,
    };
    LoadedEntry {
        date: format_date_tz(entry.timestamp, tz),
        timestamp: entry.timestamp,
        project: Arc::from("grok"),
        session_id: Arc::from(entry.session_id),
        project_path: Arc::from(entry.project.as_str()),
        cost,
        extra_total_tokens: entry.reasoning_tokens,
        credits: None,
        message_count: None,
        model: Some(entry.model),
        usage_limit_reset_time: None,
        missing_pricing_model,
        data,
    }
}

fn calculate_grok_cost(
    entry: &GrokUsageEntry,
    mode: CostMode,
    pricing: &PricingMap,
    usage: TokenUsageRaw,
) -> f64 {
    match mode {
        CostMode::Display => 0.0,
        CostMode::Auto | CostMode::Calculate => {
            for candidate in grok_model_candidates(&entry.model) {
                if pricing.find(&candidate).is_some() {
                    return calculate_cost_for_usage(
                        Some(&candidate),
                        usage,
                        None,
                        CostMode::Calculate,
                        Some(pricing),
                    );
                }
            }
            0.0
        }
    }
}

fn missing_grok_pricing(
    entry: &GrokUsageEntry,
    mode: CostMode,
    pricing: &PricingMap,
    usage: TokenUsageRaw,
) -> Option<String> {
    if mode == CostMode::Display {
        return None;
    }
    missing_pricing_model_for_candidates(
        &entry.model,
        grok_model_candidates(&entry.model),
        crate::total_usage_tokens(usage),
        Some(pricing),
    )
}

pub(super) fn grok_model_candidates(model: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    [
        model.to_string(),
        format!("xai/{model}"),
        format!("openrouter/x-ai/{model}"),
    ]
    .into_iter()
    .filter_map(|candidate| {
        if seen.contains(&candidate) {
            None
        } else {
            seen.insert(candidate.clone());
            Some(candidate)
        }
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccusage_test_support::fs_fixture;

    const SID_A: &str = "019f0000-0000-7000-8000-000000000001";
    const SID_B: &str = "019f0000-0000-7000-8000-000000000002";

    fn fixture_root() -> ccusage_test_support::Fixture {
        fs_fixture!({
            "logs/unified.jsonl": [
                r#"{"ts":"2026-06-26T10:00:00.000Z","src":"shell","sid":"019f0000-0000-7000-8000-000000000001","msg":"shell.turn.inference_done","ctx":{"loop_index":1,"prompt_tokens":100,"cached_prompt_tokens":30,"completion_tokens":20,"reasoning_tokens":5}}"#,
                r#"{"ts":"2026-06-26T10:01:00.000Z","src":"shell","sid":"019f0000-0000-7000-8000-000000000001","msg":"shell.turn.inference_done","ctx":{"loop_index":2,"prompt_tokens":30,"cached_prompt_tokens":100,"completion_tokens":10,"reasoning_tokens":0}}"#,
                r#"{"ts":"2026-06-26T11:00:00.000Z","src":"shell","sid":"019f0000-0000-7000-8000-000000000002","msg":"shell.turn.inference_done","ctx":{"loop_index":1,"prompt_tokens":50,"cached_prompt_tokens":0,"completion_tokens":25,"reasoning_tokens":0}}"#,
                r#"{"ts":"2026-06-26T12:00:00.000Z","src":"shell","msg":"shell.turn.prompt_received"}"#,
                r#"{"ts":"2026-06-26T13:00:00.000Z","src":"shell","msg":"shell.turn.inference_done","ctx":{"loop_index":1,"prompt_tokens":10,"cached_prompt_tokens":0,"completion_tokens":1,"reasoning_tokens":0}}"#,
            ]
            .join("\n"),
            "sessions/%2Ftmp%2Fproject/019f0000-0000-7000-8000-000000000001/summary.json": serde_json::json!({
                "info": {
                    "id": "019f0000-0000-7000-8000-000000000001",
                    "cwd": "/tmp/project"
                },
                "current_model_id": "grok-composer-2.5-fast",
                "generated_title": "Fixture session"
            }).to_string(),
        })
    }

    #[test]
    fn maps_cache_split_with_saturation() {
        let fixture = fixture_root();
        let index = build_session_index(&fixture.path("sessions"));
        let entries = parse_unified_log(&fixture.path("logs/unified.jsonl"), &index).unwrap();

        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].usage.input_tokens, 70);
        assert_eq!(entries[0].usage.cache_read_input_tokens, 30);
        assert_eq!(entries[0].usage.output_tokens, 20);
        assert_eq!(entries[0].reasoning_tokens, 5);
        assert_eq!(entries[1].usage.input_tokens, 0);
        assert_eq!(entries[1].usage.cache_read_input_tokens, 100);
    }

    #[test]
    fn skips_inference_done_without_sid() {
        let fixture = fixture_root();
        let entries =
            parse_unified_log(&fixture.path("logs/unified.jsonl"), &HashMap::new()).unwrap();
        assert_eq!(entries.len(), 3);
    }

    #[test]
    fn joins_model_from_summary_and_uses_fallback_for_unknown_sid() {
        let fixture = fixture_root();
        let index = build_session_index(&fixture.path("sessions"));
        let entries = parse_unified_log(&fixture.path("logs/unified.jsonl"), &index).unwrap();

        assert_eq!(entries[0].model, "grok-composer-2.5-fast");
        assert_eq!(entries[2].model, DEFAULT_MODEL);
        assert_eq!(entries[2].project, UNKNOWN_PROJECT);
        assert!(!entries[0].model.starts_with("[grok]"));
    }

    #[test]
    fn grok_entry_to_loaded_includes_reasoning_in_cost_usage() {
        let pricing = PricingMap::load_embedded();
        let entry = GrokUsageEntry {
            timestamp: parse_ts_timestamp("2026-06-26T10:00:00.000Z").unwrap(),
            timestamp_text: "2026-06-26T10:00:00.000Z".to_string(),
            session_id: SID_A.to_string(),
            project: "/tmp/project".to_string(),
            model: "grok-composer-2.5-fast".to_string(),
            usage: TokenUsageRaw {
                input_tokens: 70,
                output_tokens: 20,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 30,
                speed: None,
                cache_creation: None,
            },
            reasoning_tokens: 5,
            entry_id: format!("grok:{SID_A}:2026-06-26T10:00:00.000Z:1"),
        };

        let loaded = grok_entry_to_loaded(entry, None, CostMode::Calculate, &pricing);

        assert_eq!(loaded.extra_total_tokens, 5);
        assert_eq!(loaded.model.as_deref(), Some("grok-composer-2.5-fast"));
        assert!(loaded.cost > 0.0);
        assert!(loaded.missing_pricing_model.is_none());
    }

    #[test]
    fn unknown_model_surfaces_missing_pricing_without_grok_43_fallback() {
        let pricing = PricingMap::load_embedded();
        let entry = GrokUsageEntry {
            timestamp: parse_ts_timestamp("2026-06-26T10:00:00.000Z").unwrap(),
            timestamp_text: "2026-06-26T10:00:00.000Z".to_string(),
            session_id: SID_B.to_string(),
            project: UNKNOWN_PROJECT.to_string(),
            model: "grok-made-up-model".to_string(),
            usage: TokenUsageRaw {
                input_tokens: 10,
                output_tokens: 5,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
                speed: None,
                cache_creation: None,
            },
            reasoning_tokens: 0,
            entry_id: format!("grok:{SID_B}:2026-06-26T10:00:00.000Z:1"),
        };

        let loaded = grok_entry_to_loaded(entry, None, CostMode::Calculate, &pricing);

        assert_eq!(loaded.cost, 0.0);
        assert_eq!(
            loaded.missing_pricing_model.as_deref(),
            Some("grok-made-up-model")
        );
    }

    #[test]
    fn builds_stable_entry_ids() {
        let fixture = fixture_root();
        let entries =
            parse_unified_log(&fixture.path("logs/unified.jsonl"), &HashMap::new()).unwrap();
        assert_eq!(
            entries[0].entry_id,
            "grok:019f0000-0000-7000-8000-000000000001:2026-06-26T10:00:00.000Z:1"
        );
    }

    #[test]
    fn session_index_walks_encoded_cwd_directories() {
        let fixture = fixture_root();
        let index = build_session_index(&fixture.path("sessions"));
        let meta = index.get(SID_A).unwrap();
        assert_eq!(meta.cwd, "/tmp/project");
        assert_eq!(meta.model_id, "grok-composer-2.5-fast");
    }

    #[test]
    fn percent_decode_cwd_handles_non_ascii_utf8() {
        assert_eq!(percent_decode_cwd("%E4%B8%AD%E6%96%87"), "中文");
    }

    #[test]
    fn session_index_decodes_non_ascii_cwd_from_encoded_directory() {
        let fixture = fs_fixture!({
            "sessions/%E4%B8%AD%E6%96%87/019f0000-0000-7000-8000-000000000099/summary.json": serde_json::json!({
                "info": {
                    "id": "019f0000-0000-7000-8000-000000000099"
                },
                "current_model_id": "grok-build"
            }).to_string(),
        });
        let index = build_session_index(&fixture.path("sessions"));
        let meta = index.get("019f0000-0000-7000-8000-000000000099").unwrap();
        assert_eq!(meta.cwd, "中文");
    }

    #[test]
    fn skips_inference_done_when_required_token_fields_are_missing() {
        let fixture = fs_fixture!({
            "logs/unified.jsonl": r#"{"ts":"2026-06-26T10:00:00.000Z","src":"shell","sid":"019f0000-0000-7000-8000-000000000001","msg":"shell.turn.inference_done","ctx":{"loop_index":1,"cached_prompt_tokens":10,"completion_tokens":5,"reasoning_tokens":0}}"#,
        });
        let entries =
            parse_unified_log(&fixture.path("logs/unified.jsonl"), &HashMap::new()).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn accepts_inference_done_without_reasoning_tokens_field() {
        let fixture = fs_fixture!({
            "logs/unified.jsonl": r#"{"ts":"2026-06-26T10:00:00.000Z","src":"shell","sid":"019f0000-0000-7000-8000-000000000001","msg":"shell.turn.inference_done","ctx":{"loop_index":1,"prompt_tokens":10,"cached_prompt_tokens":0,"completion_tokens":5}}"#,
        });
        let entries =
            parse_unified_log(&fixture.path("logs/unified.jsonl"), &HashMap::new()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].reasoning_tokens, 0);
    }

    #[test]
    fn grok_entry_to_loaded_surfaces_session_cwd_as_project_path() {
        let pricing = PricingMap::load_embedded();
        let entry = GrokUsageEntry {
            timestamp: parse_ts_timestamp("2026-06-26T10:00:00.000Z").unwrap(),
            timestamp_text: "2026-06-26T10:00:00.000Z".to_string(),
            session_id: SID_A.to_string(),
            project: "/tmp/project".to_string(),
            model: "grok-composer-2.5-fast".to_string(),
            usage: TokenUsageRaw {
                input_tokens: 10,
                output_tokens: 5,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
                speed: None,
                cache_creation: None,
            },
            reasoning_tokens: 0,
            entry_id: format!("grok:{SID_A}:2026-06-26T10:00:00.000Z:1"),
        };

        let loaded = grok_entry_to_loaded(entry, None, CostMode::Calculate, &pricing);

        assert_eq!(loaded.project_path.as_ref(), "/tmp/project");
    }
}

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::{
    BucketKind, LoadedEntry, Result, SessionAccumulator, adapter::opencode, cli::AgentReportKind,
    cli::WeekDay, summarize_by_key, summarize_summaries_by_bucket, totals_json,
};

pub(crate) fn report_from_rows(rows: &[crate::UsageSummary], kind: AgentReportKind) -> Value {
    let rows_json = rows
        .iter()
        .map(|row| opencode::agent_summary_json(row, kind, kind == AgentReportKind::Session))
        .collect::<Vec<_>>();
    json!({
        rows_key(kind): rows_json,
        "totals": totals_json(rows),
    })
}

pub(crate) fn summarize_entries(
    entries: &[LoadedEntry],
    kind: AgentReportKind,
) -> Result<Vec<crate::UsageSummary>> {
    match kind {
        AgentReportKind::Daily => summarize_by_key(
            entries,
            |entry| entry.date.clone(),
            |date| (date.to_string(), None),
        ),
        AgentReportKind::Monthly => {
            let daily = summarize_entries(entries, AgentReportKind::Daily)?;
            Ok(summarize_summaries_by_bucket(
                &daily,
                BucketKind::Monthly,
                WeekDay::Sunday,
            ))
        }
        AgentReportKind::Session => {
            let mut groups = BTreeMap::<String, SessionAccumulator>::new();
            for entry in entries {
                groups
                    .entry(entry.session_id.to_string())
                    .or_default()
                    .add_entry(entry);
            }
            groups
                .into_values()
                .map(|group| group.into_summary())
                .collect()
        }
        AgentReportKind::Weekly => {
            let daily = summarize_entries(entries, AgentReportKind::Daily)?;
            Ok(summarize_summaries_by_bucket(
                &daily,
                BucketKind::Weekly,
                WeekDay::Sunday,
            ))
        }
    }
}

fn rows_key(kind: AgentReportKind) -> &'static str {
    match kind {
        AgentReportKind::Daily => "daily",
        AgentReportKind::Weekly => "weekly",
        AgentReportKind::Monthly => "monthly",
        AgentReportKind::Session => "sessions",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PricingMap, cli::SharedArgs, filter_loaded_entries_by_date};

    use super::super::loader::load_entries_with_home;
    use super::super::paths::GROK_HOME_ENV;
    use ccusage_test_support::{EnvVarGuard, fs_fixture};

    #[test]
    fn session_report_includes_project_path_and_activity() {
        let fixture = fs_fixture!({
            "logs/unified.jsonl": [
                r#"{"ts":"2026-06-26T10:00:00.000Z","src":"shell","sid":"019f0000-0000-7000-8000-000000000001","msg":"shell.turn.inference_done","ctx":{"loop_index":1,"prompt_tokens":100,"cached_prompt_tokens":0,"completion_tokens":20,"reasoning_tokens":0}}"#,
                r#"{"ts":"2026-06-26T11:00:00.000Z","src":"shell","sid":"019f0000-0000-7000-8000-000000000001","msg":"shell.turn.inference_done","ctx":{"loop_index":2,"prompt_tokens":50,"cached_prompt_tokens":0,"completion_tokens":10,"reasoning_tokens":0}}"#,
            ]
            .join("\n"),
            "sessions/%2Ftmp%2Fproject/019f0000-0000-7000-8000-000000000001/summary.json": r#"{"info":{"id":"019f0000-0000-7000-8000-000000000001","cwd":"/tmp/project"},"current_model_id":"grok-composer-2.5-fast"}"#,
        });
        let _env = EnvVarGuard::set(GROK_HOME_ENV, fixture.root());
        let entries =
            load_entries_with_home(&SharedArgs::default(), None, &PricingMap::load_embedded())
                .unwrap();
        let rows = summarize_entries(&entries, AgentReportKind::Session).unwrap();
        let report = report_from_rows(&rows, AgentReportKind::Session);

        let session = report
            .get("sessions")
            .and_then(Value::as_array)
            .and_then(|rows| rows.first())
            .expect("session row");
        assert_eq!(
            session.get("sessionId").and_then(Value::as_str),
            Some("019f0000-0000-7000-8000-000000000001")
        );
        assert_eq!(
            session.get("projectPath").and_then(Value::as_str),
            Some("/tmp/project")
        );
        assert!(session.get("firstActivity").is_some());
        assert!(session.get("lastActivity").is_some());
    }

    #[test]
    fn session_summaries_preserve_date_filtered_entries() {
        let fixture = fs_fixture!({
            "logs/unified.jsonl": [
                r#"{"ts":"2026-06-26T10:00:00.000Z","src":"shell","sid":"019f0000-0000-7000-8000-000000000001","msg":"shell.turn.inference_done","ctx":{"loop_index":1,"prompt_tokens":100,"cached_prompt_tokens":0,"completion_tokens":20,"reasoning_tokens":0}}"#,
                r#"{"ts":"2026-06-27T10:00:00.000Z","src":"shell","sid":"019f0000-0000-7000-8000-000000000001","msg":"shell.turn.inference_done","ctx":{"loop_index":2,"prompt_tokens":50,"cached_prompt_tokens":0,"completion_tokens":10,"reasoning_tokens":0}}"#,
            ]
            .join("\n"),
            "sessions/%2Ftmp%2Fproject/019f0000-0000-7000-8000-000000000001/summary.json": r#"{"info":{"id":"019f0000-0000-7000-8000-000000000001","cwd":"/tmp/project"},"current_model_id":"grok-composer-2.5-fast"}"#,
        });
        let shared = SharedArgs {
            since: Some("20260627".to_string()),
            until: Some("20260627".to_string()),
            ..SharedArgs::default()
        };
        let mut entries = load_entries_with_home(
            &shared,
            Some(fixture.root().to_str().unwrap()),
            &PricingMap::load_embedded(),
        )
        .unwrap();
        filter_loaded_entries_by_date(&mut entries, &shared);
        let rows = summarize_entries(&entries, AgentReportKind::Session).unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].input_tokens, 50);
        assert_eq!(rows[0].output_tokens, 10);
    }
}

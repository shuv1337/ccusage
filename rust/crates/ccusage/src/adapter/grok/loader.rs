use crate::{LoadedEntry, PricingMap, Result, cli::SharedArgs, parse_tz};

use super::{
    parser::{build_session_index, grok_entry_to_loaded, parse_unified_log},
    paths::{grok_home, sessions_root, unified_log_path},
};

pub(crate) fn load_entries_with_home(
    shared: &SharedArgs,
    grok_home_override: Option<&str>,
    pricing: &PricingMap,
) -> Result<Vec<LoadedEntry>> {
    crate::progress::track_usage_load(crate::progress::UsageLoadAgent::Grok, shared.json, || {
        load_entries_inner(shared, grok_home_override, pricing)
    })
}

fn load_entries_inner(
    shared: &SharedArgs,
    grok_home_override: Option<&str>,
    pricing: &PricingMap,
) -> Result<Vec<LoadedEntry>> {
    let tz = parse_tz(shared.timezone.as_deref());
    let home = grok_home(grok_home_override)?;
    let log_path = unified_log_path(&home);
    if !log_path.is_file() {
        return Ok(Vec::new());
    }
    let index = build_session_index(&sessions_root(&home));
    let entries = parse_unified_log(&log_path, &index)?;
    let mut loaded = entries
        .into_iter()
        .map(|entry| grok_entry_to_loaded(entry, tz.as_ref(), shared.mode, pricing))
        .collect::<Vec<_>>();
    loaded.sort_by_key(|entry| entry.timestamp);
    Ok(loaded)
}

#[cfg(test)]
mod tests {
    use super::super::paths::GROK_HOME_ENV;
    use super::*;
    use crate::filter_loaded_entries_by_date;
    use ccusage_test_support::{EnvVarGuard, fs_fixture};

    fn grok_fixture() -> ccusage_test_support::Fixture {
        fs_fixture!({
            "logs/unified.jsonl": [
                r#"{"ts":"2026-06-26T10:00:00.000Z","src":"shell","sid":"019f0000-0000-7000-8000-000000000001","msg":"shell.turn.inference_done","ctx":{"loop_index":1,"prompt_tokens":100,"cached_prompt_tokens":0,"completion_tokens":20,"reasoning_tokens":0}}"#,
                r#"{"ts":"2026-06-27T10:00:00.000Z","src":"shell","sid":"019f0000-0000-7000-8000-000000000001","msg":"shell.turn.inference_done","ctx":{"loop_index":2,"prompt_tokens":50,"cached_prompt_tokens":0,"completion_tokens":10,"reasoning_tokens":0}}"#,
            ]
            .join("\n"),
            "sessions/%2Ftmp%2Fproject/019f0000-0000-7000-8000-000000000001/summary.json": r#"{"info":{"id":"019f0000-0000-7000-8000-000000000001","cwd":"/tmp/project"},"current_model_id":"grok-composer-2.5-fast"}"#,
        })
    }

    #[test]
    fn loads_expected_entry_count_from_fixture() {
        let fixture = grok_fixture();
        let _env = EnvVarGuard::set(GROK_HOME_ENV, fixture.root());
        let entries =
            load_entries_with_home(&SharedArgs::default(), None, &PricingMap::load_embedded())
                .unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].date, "2026-06-26");
        assert_eq!(entries[1].date, "2026-06-27");
    }

    #[test]
    fn respects_since_and_until_filters() {
        let fixture = grok_fixture();
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
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].date, "2026-06-27");
    }

    #[test]
    fn grok_home_override_beats_env() {
        let fixture = grok_fixture();
        let other = fs_fixture!({});
        let _env = EnvVarGuard::set(GROK_HOME_ENV, other.root());
        let entries = load_entries_with_home(
            &SharedArgs::default(),
            Some(fixture.root().to_str().unwrap()),
            &PricingMap::load_embedded(),
        )
        .unwrap();
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn missing_unified_log_returns_empty_vec() {
        let fixture = fs_fixture!({});
        let entries = load_entries_with_home(
            &SharedArgs::default(),
            Some(fixture.root().to_str().unwrap()),
            &PricingMap::load_embedded(),
        )
        .unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    #[ignore = "requires local ~/.grok"]
    fn smoke_loads_real_grok_home() {
        let entries =
            load_entries_with_home(&SharedArgs::default(), None, &PricingMap::load_embedded())
                .unwrap();
        assert!(!entries.is_empty());
    }
}

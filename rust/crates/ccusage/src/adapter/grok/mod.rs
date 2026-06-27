mod loader;
mod parser;
mod paths;
mod report;

use crate::{
    PricingMap, Result,
    adapter::opencode,
    cli::{AgentCommandArgs, CostMode},
    filter_loaded_entries_by_date, print_json_or_jq, print_usage_table, sort_summaries, wants_json,
};

pub(crate) use loader::load_entries_with_home;
pub(crate) use report::{report_from_rows, summarize_entries};

pub(crate) fn run(args: AgentCommandArgs) -> Result<()> {
    if args.shared.mode == CostMode::Display {
        return Err(crate::cli_error(
            "Grok does not store precomputed costUSD locally; use --mode auto or --mode calculate",
        ));
    }
    let shared = args.shared;
    let pricing = PricingMap::load_with_overrides(
        shared.offline,
        crate::log_level() != Some(0),
        shared.pricing_overrides.iter(),
    );
    let mut entries = load_entries_with_home(&shared, args.grok_home.as_deref(), &pricing)?;
    filter_loaded_entries_by_date(&mut entries, &shared);
    let mut rows = summarize_entries(&entries, args.kind)?;
    sort_summaries(&mut rows, &shared.order, |row| {
        opencode::summary_period(row)
    });
    if wants_json(&shared) {
        return print_json_or_jq(
            report_from_rows(&rows, args.kind),
            shared.jq.as_deref(),
            shared.no_cost,
        );
    }
    print_usage_table(
        "Grok Token Usage Report",
        opencode::first_column(args.kind),
        &rows,
        &shared,
        false,
        None,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{AgentCommandArgs, AgentReportKind, CodexSpeed, SharedArgs};

    #[test]
    fn rejects_display_mode_before_loading() {
        let args = AgentCommandArgs {
            shared: SharedArgs {
                mode: CostMode::Display,
                ..SharedArgs::default()
            },
            kind: AgentReportKind::Daily,
            pi_path: None,
            open_claw_path: None,
            grok_home: None,
            codex_speed: CodexSpeed::Auto,
        };

        let error = run(args).unwrap_err().to_string();

        assert!(error.contains("precomputed costUSD"));
    }
}

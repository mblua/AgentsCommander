//! `context-history` (#3532 P04): print the context-history report as JSON.
//!
//! The report core is pure; this edge validates arguments, resolves the
//! Project AC Root, reads the retained snapshot once and streams every page of
//! agents as one schema 2 JSON document on stdout. Errors go to stderr with
//! exit code 1 and never produce an empty successful result.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use clap::Args;

use crate::config::ac_root::existing_ac_root;
use crate::context_history::config::load_history_config;
use crate::context_history::report::{
    build_report, LaunchFilter, ReportContext, ReportError, ReportQuery,
};
use crate::context_history::store::{read_history, ReadAttempt, StoreError};
use crate::context_history::HistoryRecord;

#[derive(Args, Debug, Clone, PartialEq, Eq)]
#[command(after_help = "\
SELECTION: A logical session is selected when it STARTED inside [--from, --to). \
A session that started earlier is excluded even if it changed inside the period. \
A selected session contributes ALL of its retained positive changes, including \
changes observed after --to. Sessions whose start is unknown are excluded and \
counted in coverage.unknownStartSessionsCount.\n\n\
VALUES: Context usage is observed about every 5 seconds; only positive changes \
count. mean = positiveSum / recordsCount over records, never a mean of means; \
null when there are no records.\n\n\
LAUNCH METADATA: --coding-agent, --model and --effort match the values captured \
from the launch command exactly. --unknown-* matches sessions where that value \
was not captured; it is not the same as omitting the filter.\n\n\
COVERAGE: Retention, storage caps and observation gaps can remove data. \
coverage.incomplete and per-session flags mark retained data of incomplete \
sessions; totals are never extrapolated.\n\n\
OUTPUT: One JSON object, schemaVersion 2, with requestedPeriod, generatedAtUtcMs, \
coverage, totalAgents and agents. A missing history database prints an empty \
report. Times are RFC3339 with an explicit timezone; the period is at most 31 days.\n\n\
EXAMPLES:\n  \
  {bin} context-history --project C:/repos/demo --from 2026-11-01T00:00:00Z \
--to 2026-11-08T00:00:00Z\n  \
  {bin} context-history --project /repos/demo --from 2026-11-01T00:00:00-03:00 \
--to 2026-11-02T00:00:00-03:00 --agent dev-rust --unknown-model")]
pub struct ContextHistoryArgs {
    /// Absolute path of the AC project
    #[arg(long)]
    pub project: PathBuf,
    /// Period start (inclusive), RFC3339 with timezone
    #[arg(long)]
    pub from: String,
    /// Period end (exclusive), RFC3339 with timezone
    #[arg(long)]
    pub to: String,
    /// Exact agent key (bare agent name)
    #[arg(long)]
    pub agent: Option<String>,
    /// Exact team id; only sessions attributed to exactly this team
    #[arg(long)]
    pub team: Option<String>,
    /// Exact coding agent id captured at launch
    #[arg(long, conflicts_with = "unknown_coding_agent")]
    pub coding_agent: Option<String>,
    /// Only sessions whose coding agent was not captured
    #[arg(long)]
    pub unknown_coding_agent: bool,
    /// Exact model captured at launch
    #[arg(long, conflicts_with = "unknown_model")]
    pub model: Option<String>,
    /// Only sessions whose model was not captured
    #[arg(long)]
    pub unknown_model: bool,
    /// Exact effort captured at launch
    #[arg(long, conflicts_with = "unknown_effort")]
    pub effort: Option<String>,
    /// Only sessions whose effort was not captured
    #[arg(long)]
    pub unknown_effort: bool,
}

/// Why the command failed; each maps to stderr and exit code 1.
#[derive(Debug)]
enum CliError {
    Usage(String),
    Report(ReportError),
    Store(StoreError),
    Io(io::Error),
}

impl From<io::Error> for CliError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<ReportError> for CliError {
    fn from(error: ReportError) -> Self {
        Self::Report(error)
    }
}

fn launch_filter(exact: Option<&String>, unknown: bool) -> LaunchFilter {
    match exact {
        Some(value) => LaunchFilter::Exact(value.clone()),
        None if unknown => LaunchFilter::Unknown,
        None => LaunchFilter::Any,
    }
}

fn parse_time(flag: &str, text: &str) -> Result<i64, CliError> {
    chrono::DateTime::parse_from_rfc3339(text)
        .map(|time| time.timestamp_millis())
        .map_err(|error| {
            CliError::Usage(format!("--{flag} must be RFC3339 with a timezone: {error}"))
        })
}

fn project_text(project: &Path) -> Result<String, CliError> {
    if !project.is_absolute() {
        return Err(CliError::Usage(
            "--project must be an absolute path".to_string(),
        ));
    }
    project
        .to_str()
        .map(str::to_string)
        .ok_or_else(|| CliError::Usage("--project must be valid UTF-8".to_string()))
}

/// The validated query for these arguments; no IO.
fn query_from_args(args: &ContextHistoryArgs) -> Result<ReportQuery, CliError> {
    let query = ReportQuery {
        project_path: project_text(&args.project)?,
        from_utc_ms: parse_time("from", &args.from)?,
        to_utc_ms: parse_time("to", &args.to)?,
        agent_key: args.agent.clone(),
        team_id: args.team.clone(),
        coding_agent_id: launch_filter(args.coding_agent.as_ref(), args.unknown_coding_agent),
        model: launch_filter(args.model.as_ref(), args.unknown_model),
        effort: launch_filter(args.effort.as_ref(), args.unknown_effort),
        cursor: None,
    };
    query.validate()?;
    Ok(query)
}

/// Read the retained snapshot; a hot journal gets the store's one bounded recovery.
fn read_records(ac_root: &Path) -> Result<Vec<HistoryRecord>, CliError> {
    match read_history(ac_root, ReadAttempt::ReadOnly) {
        Err(StoreError::RecoveryRequired { .. }) => {
            read_history(ac_root, ReadAttempt::AllowRecovery).map_err(CliError::Store)
        }
        other => other.map_err(CliError::Store),
    }
}

/// Stream every agent page as one JSON object; pages share one snapshot.
fn write_report(
    records: &[HistoryRecord],
    query: &ReportQuery,
    context: ReportContext,
    out: &mut dyn Write,
) -> Result<(), CliError> {
    let first = build_report(records, query, context)?;
    let header = serde_json::to_string(&first.header).map_err(io::Error::from)?;
    let open = header.strip_suffix('}').unwrap_or(&header);
    write!(
        out,
        "{open},\"totalAgents\":{},\"agents\":[",
        first.total_agents
    )?;
    let mut page = first;
    let mut written = 0_u64;
    loop {
        for agent in &page.agents_page {
            if written > 0 {
                out.write_all(b",")?;
            }
            serde_json::to_writer(&mut *out, agent).map_err(io::Error::from)?;
            written += 1;
        }
        let Some(cursor) = page.next_agent_cursor.take() else {
            break;
        };
        let mut next = query.clone();
        next.cursor = Some(cursor);
        page = build_report(records, &next, context)?;
    }
    writeln!(out, "]}}")?;
    out.flush()?;
    Ok(())
}

fn run(args: &ContextHistoryArgs, now_utc_ms: i64, out: &mut dyn Write) -> Result<(), CliError> {
    let query = query_from_args(args)?;
    let ac_root = existing_ac_root(&args.project).ok_or_else(|| {
        CliError::Usage(format!(
            "--project '{}' has no .ac Project AC Root",
            args.project.display()
        ))
    })?;
    let context = ReportContext {
        retention_days: load_history_config(&ac_root).config.retention_days,
        generated_at_utc_ms: now_utc_ms,
    };
    let records = read_records(&ac_root)?;
    write_report(&records, &query, context, out)
}

/// Exit code for a result; `BrokenPipe` exits successfully like `cli_println!`.
fn exit_code(result: Result<(), CliError>) -> i32 {
    let Err(error) = result else {
        return 0;
    };
    if matches!(&error, CliError::Io(io_error) if io_error.kind() == io::ErrorKind::BrokenPipe) {
        return 0;
    }
    let message = match error {
        CliError::Usage(message) => message,
        CliError::Report(error) => error.to_string(),
        CliError::Store(error) => format!("context history unavailable: {error:?}"),
        CliError::Io(error) => format!("failed writing to stdout: {error}"),
    };
    eprintln!("Error: {message}");
    1
}

pub fn execute(args: ContextHistoryArgs) -> i32 {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    exit_code(run(&args, chrono::Utc::now().timestamp_millis(), &mut out))
}

#[cfg(test)]
mod tests;

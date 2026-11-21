//! Remote CLI subcommands (plan.md §6.7): `rules`, `labels`, `seed`, `torrent`
//! and `cmd`, executed against a running daemon over gRPC
//! `CommandService.Execute` — the same command map as XML-RPC/REST (§6.4), so
//! every surface keeps parity.
//!
//! Layering (each layer is testable without a network):
//! 1. **Parsing**: [`parse_remote_command`] turns `keyword + args` into a typed
//!    [`RemoteCommand`] (manual scanner; `--flag=value` and `--flag value` both
//!    work). Unknown keywords yield `Ok(None)` so the router can fall through
//!    to the legacy FILES/magnet behaviour unchanged.
//! 2. **Request building**: [`build_calls`] maps a parsed command onto a
//!    sequence of [`RpcCall`]s (command name + typed args + optional target
//!    hash). Pure except for reading `--file`/`import` documents from disk.
//!    `labels policy` is a read-modify-write flow: the builder emits the
//!    initial `label.policy.get`; the executor merges the flags with
//!    [`merge_policy_json`] and appends [`policy_set_call`]/[`save_path_call`].
//! 3. **Transport + rendering**: [`handle_remote_subcommand`] connects through
//!    [`rt_tui_client::GrpcBridge`] (endpoint discovery from the active TOML
//!    config when `--endpoint` is absent) and prints a table by default or raw
//!    JSON with `--json`. Errors propagate as `Err` (caller: stderr + exit≠0).
//!
//! Trust is server-decided (local Unix socket = trusted; TCP needs the
//! configured bearer token), never taken from the client payload — so the
//! discovery prefers `[rpc] grpc_socket` over `grpc_port`.
//!
//! Wire conventions (§6.4 / `rules_commands.rs`):
//! - complex types (Rule, SeedPolicy, lists, history, export) travel as JSON
//!   strings;
//! - ratios travel as strings (`Object` has no float variant); `0` = unlimited
//!   at the global level;
//! - per-torrent limits use `d.seed_{ratio,time,idle,action}.set` with the
//!   gRPC `target_hash` field (`Target::Download`), values as
//!   `inherit|unlimited|<number>` strings;
//! - `cmd` mimics the XML-RPC first-parameter target convention: a leading
//!   40-hex-digit hash (optionally `:d`) becomes `target_hash`, remaining args
//!   are typed (integer-looking → int, otherwise string).

use std::path::PathBuf;
use std::time::Duration;

use rt_grpc::{object_value, CommandResponse, ObjectValue};
use rt_rules::SeedPolicy;
pub use rt_rules::{LimitMode, LimitValue, SeedAction};
use rt_tui_client::GrpcBridge;

use crate::toml_config::RtorrentTomlConfig;

/// Timeout for the connect attempt and every RPC (a forced `rules.check` pass
/// can take a while on large sessions).
const RPC_TIMEOUT: Duration = Duration::from_secs(30);

/// Default history window, matching `rules_commands.rs` / REST.
const HISTORY_DEFAULT_LIMIT: usize = 100;

const USAGE_RULES: &str = "usage: rstorrent rules list [--json] | add --file <path> | \
     add --json '<json>' [--allow-destructive] | remove <id> | enable <id> | disable <id> | \
     get <id> [--json] | test --file <path> [--limit N] [--json] | check [--json] | \
     history [--limit N] [--json] | export | import <file>";
const USAGE_LABELS: &str = "usage: rstorrent labels list [--json] | add <name> [--color C] | \
     remove <name> | policy <name> [--ratio R|inherit|unlimited] [--time T] [--idle I] \
     [--action A|inherit] [--save-path P] [--json]";
const USAGE_SEED: &str = "usage: rstorrent seed show [--json] | set [--ratio R] [--time T] \
     [--idle I] [--mode any|all] [--action A] [--enabled true|false] [--check-interval S] [--json]";
const USAGE_TORRENT: &str = "usage: rstorrent torrent limits <hash> [--ratio R|inherit|unlimited] \
     [--time T|inherit|unlimited] [--idle I|inherit|unlimited] [--action A|inherit] [--json]";
const USAGE_CMD: &str = "usage: rstorrent cmd <command> [args...]";

/// Everything that can go wrong in the remote CLI. Variants carry enough
/// context for a readable stderr message without a backtrace.
#[derive(Debug, thiserror::Error)]
pub enum RemoteError {
    /// Bad user input: unknown flag, missing value, invalid number, …
    #[error("{0}")]
    Input(String),
    /// A `--file` / `import` document could not be read.
    #[error("cannot read {path}: {source}")]
    Read {
        /// Document path as given on the command line.
        path: String,
        /// Underlying I/O failure.
        #[source]
        source: std::io::Error,
    },
    /// A JSON document (rule, policy, export blob) failed to parse.
    #[error("invalid JSON ({what}): {source}")]
    Json {
        /// Which document failed.
        what: &'static str,
        /// Underlying serde failure.
        #[source]
        source: serde_json::Error,
    },
    /// The discovered config file is not valid TOML.
    #[error("cannot parse config {path}: {source}")]
    Config {
        /// Config path.
        path: String,
        /// Underlying TOML failure.
        #[source]
        source: toml::de::Error,
    },
    /// Transport-level failure from the reused TUI client bridge.
    #[error(transparent)]
    Bridge(#[from] rt_tui_client::BridgeError),
    /// The daemon executed the command and reported a failure.
    #[error("daemon rejected `{command}`: {message}")]
    Command {
        /// Command name that failed.
        command: String,
        /// Server-side error message.
        message: String,
    },
    /// The daemon answered successfully but with an unexpected payload shape.
    #[error("unexpected response from `{command}`: {detail}")]
    Response {
        /// Command name.
        command: String,
        /// What was expected vs. found.
        detail: String,
    },
}

fn input_err(msg: impl Into<String>) -> RemoteError {
    RemoteError::Input(msg.into())
}

// ---- Duration / limit / action parsers ----

/// Parses a CLI duration into whole seconds: `120` (bare seconds), `30s`,
/// `30m`, `2h`, `7d`. Surrounding whitespace is tolerated; negative values,
/// fractions, compound forms (`1h30m`) and unknown units are rejected.
///
/// # Errors
///
/// [`RemoteError::Input`] when the text is not `<digits>[s|m|h|d]` or the
/// result would overflow `u64`.
pub fn parse_duration(text: &str) -> Result<u64, RemoteError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(input_err("empty duration (expected e.g. 120, 30m, 2h, 7d)"));
    }
    let digits_end = trimmed
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(trimmed.len());
    let (number, unit) = trimmed.split_at(digits_end);
    if number.is_empty() {
        return Err(input_err(format!(
            "invalid duration '{text}': expected a number followed by an optional unit (s/m/h/d)"
        )));
    }
    let value: u64 = number
        .parse()
        .map_err(|_| input_err(format!("invalid duration '{text}': number out of range")))?;
    let multiplier: u64 = match unit {
        "" | "s" => 1,
        "m" => 60,
        "h" => 3_600,
        "d" => 86_400,
        other => {
            return Err(input_err(format!(
                "invalid duration unit '{other}' in '{text}' (expected s, m, h or d)"
            )))
        }
    };
    value
        .checked_mul(multiplier)
        .ok_or_else(|| input_err(format!("duration '{text}' overflows u64 seconds")))
}

/// Parses a ratio limit: `inherit`, `unlimited` or a finite number ≥ 0
/// (`1.0` = 100%, plan §6.1 unit table).
///
/// # Errors
///
/// [`RemoteError::Input`] for anything else (including `NaN`/negatives).
pub fn parse_ratio_limit(text: &str) -> Result<LimitValue<f64>, RemoteError> {
    match text.trim() {
        "inherit" => Ok(LimitValue::Inherit),
        "unlimited" => Ok(LimitValue::Unlimited),
        other => {
            let ratio: f64 = other.parse().map_err(|_| {
                input_err(format!(
                    "invalid ratio '{text}': expected inherit|unlimited|<number>"
                ))
            })?;
            if !ratio.is_finite() || ratio < 0.0 {
                return Err(input_err(format!(
                    "invalid ratio '{text}': must be a finite number >= 0"
                )));
            }
            Ok(LimitValue::Set(ratio))
        }
    }
}

/// Parses a seconds limit: `inherit`, `unlimited` or a duration
/// ([`parse_duration`]).
///
/// # Errors
///
/// [`RemoteError::Input`] when the duration part is invalid.
pub fn parse_secs_limit(text: &str) -> Result<LimitValue<u64>, RemoteError> {
    match text.trim() {
        "inherit" => Ok(LimitValue::Inherit),
        "unlimited" => Ok(LimitValue::Unlimited),
        _ => Ok(LimitValue::Set(parse_duration(text)?)),
    }
}

/// Global-level ratio: a number or `unlimited` (→ `0`, the §6.4 wire form).
/// `inherit` is rejected — there is no level above global.
fn parse_global_ratio(text: &str) -> Result<f64, RemoteError> {
    match parse_ratio_limit(text)? {
        LimitValue::Set(ratio) => Ok(ratio),
        LimitValue::Unlimited => Ok(0.0),
        LimitValue::Inherit => Err(input_err(
            "global seed ratio cannot be 'inherit' (use a number or 'unlimited')",
        )),
    }
}

/// Global-level seconds: a duration or `unlimited` (→ `0`).
fn parse_global_secs(text: &str) -> Result<u64, RemoteError> {
    match parse_secs_limit(text)? {
        LimitValue::Set(secs) => Ok(secs),
        LimitValue::Unlimited => Ok(0),
        LimitValue::Inherit => Err(input_err(
            "global seed limit cannot be 'inherit' (use a duration or 'unlimited')",
        )),
    }
}

/// An `--action` flag value: an explicit seed action, or `inherit` where the
/// level supports it (label policy / per-torrent override).
#[derive(Debug, Clone, PartialEq)]
pub enum ActionFlag {
    /// Clear the override / defer to the level above.
    Inherit,
    /// An explicit, validated action.
    Set(SeedAction),
}

/// Validates an action string against the shared sidecar parser
/// (`stop|close|remove|remove_with_data|throttle:<name>|super_seed|notify|command:<str>`),
/// so CLI, RPC and persistence never diverge.
fn parse_action_flag(text: &str, allow_inherit: bool) -> Result<ActionFlag, RemoteError> {
    let trimmed = text.trim();
    if trimmed == "inherit" {
        if allow_inherit {
            return Ok(ActionFlag::Inherit);
        }
        return Err(input_err("global seed action cannot be 'inherit'"));
    }
    let action = rt_core::download::parse_seed_action(trimmed).ok_or_else(|| {
        input_err(format!(
            "invalid action '{text}' (expected stop|close|remove|remove_with_data|\
             throttle:<name>|super_seed|notify|command:<str>)"
        ))
    })?;
    Ok(ActionFlag::Set(action))
}

/// RPC string form of a limit value (`d.seed_*.set` / sidecar convention).
fn limit_wire<T: std::fmt::Display>(value: &LimitValue<T>) -> String {
    match value {
        LimitValue::Inherit => "inherit".to_string(),
        LimitValue::Unlimited => "unlimited".to_string(),
        LimitValue::Set(value) => value.to_string(),
    }
}

/// Validates a torrent hash (40 hex digits) and normalises it to lowercase.
fn validate_hash(text: &str) -> Result<String, RemoteError> {
    let trimmed = text.trim();
    if trimmed.len() == 40 && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
        Ok(trimmed.to_ascii_lowercase())
    } else {
        Err(input_err(format!(
            "invalid torrent hash '{text}': expected 40 hex digits"
        )))
    }
}

// ---- Typed command model (parsing output) ----

/// Where a rule JSON document comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsonSource {
    /// Read from a file at execution time.
    File(PathBuf),
    /// Given inline via `--json '<json>'`.
    Inline(String),
}

/// `rules …` subcommands (§6.7).
#[derive(Debug, Clone, PartialEq)]
pub enum RulesCmd {
    /// `rules list [--json]`
    List,
    /// `rules add --file <path> | --json '<json>' [--allow-destructive]`
    Add {
        /// Rule document source.
        source: JsonSource,
        /// Sets `allow_destructive: true` on the rule before submission
        /// (per-rule gate, plan §5.5; the global gate stays untouched).
        allow_destructive: bool,
    },
    /// `rules remove <id>`
    Remove {
        /// Rule id.
        id: u64,
    },
    /// `rules enable <id>`
    Enable {
        /// Rule id.
        id: u64,
    },
    /// `rules disable <id>`
    Disable {
        /// Rule id.
        id: u64,
    },
    /// `rules get <id> [--json]`
    Get {
        /// Rule id.
        id: u64,
    },
    /// `rules test --file <path> [--limit N] [--json]` — dry-run, zero mutations.
    Test {
        /// Rule document source.
        source: JsonSource,
        /// Client-side cap on the returned matches (the RPC `rules.test`
        /// takes only the rule; REST applies the same truncation).
        limit: Option<usize>,
    },
    /// `rules check [--json]` — force one pass, print fired decisions.
    Check,
    /// `rules history [--limit N] [--json]`
    History {
        /// Window size; `None` → server default (100, capped at 500).
        limit: Option<usize>,
    },
    /// `rules export` — raw JSON backup blob on stdout.
    Export,
    /// `rules import <file>`
    Import {
        /// Backup blob path.
        file: PathBuf,
    },
}

/// `labels …` subcommands (§6.7).
#[derive(Debug, Clone, PartialEq)]
pub enum LabelsCmd {
    /// `labels list [--json]`
    List,
    /// `labels add <name> [--color C]`
    Add {
        /// Label name (== `d.custom1`).
        name: String,
        /// Optional `#rrggbb` colour for the UI.
        color: Option<String>,
    },
    /// `labels remove <name>`
    Remove {
        /// Label name.
        name: String,
    },
    /// `labels policy <name> [flags]` — partial update merged into the
    /// existing policy (read-modify-write).
    Policy {
        /// Label name.
        name: String,
        /// Fields to change; absent flags keep their current value.
        flags: PolicyFlags,
    },
}

/// Optional `labels policy` fields. `None` = flag absent (keep current).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PolicyFlags {
    /// `--ratio R|inherit|unlimited`
    pub ratio: Option<LimitValue<f64>>,
    /// `--time T|inherit|unlimited`
    pub time: Option<LimitValue<u64>>,
    /// `--idle I|inherit|unlimited`
    pub idle: Option<LimitValue<u64>>,
    /// `--action A|inherit`
    pub action: Option<ActionFlag>,
    /// `--save-path P` (separate `label.save_path.set` command).
    pub save_path: Option<String>,
}

impl PolicyFlags {
    /// Whether any policy field (not just save_path) was given.
    fn has_policy_fields(&self) -> bool {
        self.ratio.is_some() || self.time.is_some() || self.idle.is_some() || self.action.is_some()
    }
}

/// `seed set` fields (§6.7). Each maps to one `seed.*.set` command.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SeedSetArgs {
    /// `--ratio R` (f64, `0` = unlimited).
    pub ratio: Option<f64>,
    /// `--time T` (seconds, `0` = unlimited).
    pub time: Option<u64>,
    /// `--idle I` (seconds, `0` = unlimited).
    pub idle: Option<u64>,
    /// `--mode any|all`
    pub mode: Option<LimitMode>,
    /// `--action A`
    pub action: Option<SeedAction>,
    /// `--enabled true|false`
    pub enabled: Option<bool>,
    /// `--check-interval S` (seconds, ≥ 1).
    pub check_interval: Option<u64>,
}

impl SeedSetArgs {
    fn is_empty(&self) -> bool {
        self.ratio.is_none()
            && self.time.is_none()
            && self.idle.is_none()
            && self.mode.is_none()
            && self.action.is_none()
            && self.enabled.is_none()
            && self.check_interval.is_none()
    }
}

/// `torrent limits <hash> [flags]` (§6.7): per-torrent overrides via
/// `d.seed_*.set` with `Target::Download(hash)`, then `d.seed_limits` shows
/// the effective values + sources.
#[derive(Debug, Clone, PartialEq)]
pub struct TorrentLimitsArgs {
    /// 40-hex-digit info hash.
    pub hash: String,
    /// `--ratio R|inherit|unlimited`
    pub ratio: Option<LimitValue<f64>>,
    /// `--time T|inherit|unlimited`
    pub time: Option<LimitValue<u64>>,
    /// `--idle I|inherit|unlimited`
    pub idle: Option<LimitValue<u64>>,
    /// `--action A|inherit`
    pub action: Option<ActionFlag>,
}

/// The dispatchable part of a remote subcommand.
#[derive(Debug, Clone, PartialEq)]
pub enum CommandKind {
    /// `rules …`
    Rules(RulesCmd),
    /// `labels …`
    Labels(LabelsCmd),
    /// `seed show`
    SeedShow,
    /// `seed set …`
    SeedSet(SeedSetArgs),
    /// `torrent limits …`
    TorrentLimits(TorrentLimitsArgs),
    /// `cmd <command> [args...]` — raw RPC escape hatch.
    Raw {
        /// RPC command name.
        command: String,
        /// Raw arguments (no flag interpretation).
        args: Vec<String>,
    },
}

/// A fully parsed remote subcommand.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteCommand {
    /// What to do.
    pub kind: CommandKind,
    /// `--json`: machine-readable output instead of a table.
    pub json: bool,
}

// ---- Argument scanning helpers ----

/// One scanned token: a long flag (with optional `=value`) or a positional.
#[derive(Debug)]
enum Token<'a> {
    Flag(&'a str, Option<&'a str>),
    Positional(&'a str),
}

fn tokenize(args: &[String]) -> Vec<Token<'_>> {
    args.iter()
        .map(|arg| {
            let text = arg.as_str();
            if text.starts_with("--") {
                match text.split_once('=') {
                    Some((name, value)) => Token::Flag(name, Some(value)),
                    None => Token::Flag(text, None),
                }
            } else {
                Token::Positional(text)
            }
        })
        .collect()
}

/// Resolves a flag's value: the `=value` form, or the next token.
fn flag_value<'a>(
    tokens: &[Token<'a>],
    index: &mut usize,
    inline: Option<&'a str>,
    flag: &str,
) -> Result<&'a str, RemoteError> {
    if let Some(value) = inline {
        return Ok(value);
    }
    *index += 1;
    match tokens.get(*index) {
        Some(Token::Positional(value)) => Ok(value),
        Some(Token::Flag(_, _)) => Err(input_err(format!("{flag} requires a value (got a flag)"))),
        None => Err(input_err(format!("{flag} requires a value"))),
    }
}

fn parse_id(text: &str) -> Result<u64, RemoteError> {
    text.trim()
        .parse::<u64>()
        .map_err(|_| input_err(format!("invalid rule id '{text}': expected a number")))
}

fn parse_limit(text: &str, flag: &str) -> Result<usize, RemoteError> {
    text.trim().parse::<usize>().map_err(|_| {
        input_err(format!(
            "invalid {flag} value '{text}': expected a number >= 0"
        ))
    })
}

fn parse_bool(text: &str, flag: &str) -> Result<bool, RemoteError> {
    match text.trim() {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        other => Err(input_err(format!(
            "invalid {flag} value '{other}': expected true|false"
        ))),
    }
}

fn parse_mode(text: &str) -> Result<LimitMode, RemoteError> {
    match text.trim() {
        "any" => Ok(LimitMode::Any),
        "all" => Ok(LimitMode::All),
        other => Err(input_err(format!(
            "invalid mode '{other}': expected any|all"
        ))),
    }
}

/// Rejects leftover tokens after a subcommand finished parsing.
fn expect_end(tokens: &[Token<'_>], index: usize, usage: &str) -> Result<(), RemoteError> {
    match tokens.get(index) {
        None => Ok(()),
        Some(Token::Positional(extra)) => {
            Err(input_err(format!("unexpected argument '{extra}'\n{usage}")))
        }
        Some(Token::Flag(flag, _)) => Err(input_err(format!("unknown flag '{flag}'\n{usage}"))),
    }
}

/// Collects exactly `count` positionals or fails with the usage line.
fn positionals<'a>(
    tokens: &[Token<'a>],
    count: usize,
    what: &str,
    usage: &str,
) -> Result<Vec<&'a str>, RemoteError> {
    let mut found = Vec::new();
    for token in tokens {
        match token {
            Token::Positional(value) => found.push(*value),
            Token::Flag(flag, _) => {
                return Err(input_err(format!("unknown flag '{flag}'\n{usage}")))
            }
        }
    }
    if found.len() != count {
        return Err(input_err(format!("expected {what}\n{usage}")));
    }
    Ok(found)
}

// ---- Subcommand parsers ----

/// Parses a remote keyword + its arguments into a typed command.
///
/// Returns `Ok(None)` for keywords outside `{rules, labels, seed, torrent,
/// cmd}` so the caller keeps the legacy FILES/magnet behaviour.
///
/// # Errors
///
/// [`RemoteError::Input`] with a usage hint for malformed subcommands.
pub fn parse_remote_command(
    keyword: &str,
    args: &[String],
) -> Result<Option<RemoteCommand>, RemoteError> {
    let tokens = tokenize(args);
    let (kind, json) = match keyword {
        "rules" => parse_rules(&tokens)?,
        "labels" => parse_labels(&tokens)?,
        "seed" => parse_seed(&tokens)?,
        "torrent" => parse_torrent(&tokens)?,
        "cmd" => {
            // Raw RPC args: no flag interpretation at all (`--json` included).
            let command = args
                .first()
                .map(String::as_str)
                .filter(|c| !c.trim().is_empty())
                .ok_or_else(|| input_err(format!("missing command\n{USAGE_CMD}")))?;
            (
                CommandKind::Raw {
                    command: command.to_string(),
                    args: args[1..].to_vec(),
                },
                false,
            )
        }
        _ => return Ok(None),
    };
    Ok(Some(RemoteCommand { kind, json }))
}

/// Parses `--json` as an output switch (no value allowed).
fn json_switch(inline: Option<&str>) -> Result<bool, RemoteError> {
    if inline.is_some() {
        return Err(input_err("--json is a switch and does not take a value"));
    }
    Ok(true)
}

fn parse_rules(tokens: &[Token<'_>]) -> Result<(CommandKind, bool), RemoteError> {
    let sub = match tokens.first() {
        Some(Token::Positional(sub)) => *sub,
        _ => {
            return Err(input_err(format!(
                "missing rules subcommand\n{USAGE_RULES}"
            )))
        }
    };
    let rest = &tokens[1..];
    let mut json = false;
    let cmd = match sub {
        "list" => {
            let mut index = 0;
            while let Some(token) = rest.get(index) {
                match token {
                    Token::Flag("--json", inline) => json = json_switch(*inline)?,
                    other => return Err(unexpected_token(other, USAGE_RULES)),
                }
                index += 1;
            }
            RulesCmd::List
        }
        "add" => parse_rules_add(rest)?,
        "remove" | "enable" | "disable" => {
            let values = positionals(rest, 1, "a rule id", USAGE_RULES)?;
            let id = parse_id(values[0])?;
            match sub {
                "remove" => RulesCmd::Remove { id },
                "enable" => RulesCmd::Enable { id },
                _ => RulesCmd::Disable { id },
            }
        }
        "get" => {
            let mut id = None;
            let mut index = 0;
            while let Some(token) = rest.get(index) {
                match token {
                    Token::Flag("--json", inline) => json = json_switch(*inline)?,
                    Token::Positional(value) if id.is_none() => id = Some(parse_id(value)?),
                    other => return Err(unexpected_token(other, USAGE_RULES)),
                }
                index += 1;
            }
            let id = id.ok_or_else(|| input_err(format!("expected a rule id\n{USAGE_RULES}")))?;
            RulesCmd::Get { id }
        }
        "test" => parse_rules_test(rest, &mut json)?,
        "check" => {
            let mut index = 0;
            while let Some(token) = rest.get(index) {
                match token {
                    Token::Flag("--json", inline) => json = json_switch(*inline)?,
                    other => return Err(unexpected_token(other, USAGE_RULES)),
                }
                index += 1;
            }
            RulesCmd::Check
        }
        "history" => {
            let mut limit = None;
            let mut index = 0;
            while let Some(token) = rest.get(index) {
                match token {
                    Token::Flag("--limit", inline) => {
                        limit = Some(parse_limit(
                            flag_value(rest, &mut index, *inline, "--limit")?,
                            "--limit",
                        )?);
                    }
                    Token::Flag("--json", inline) => json = json_switch(*inline)?,
                    other => return Err(unexpected_token(other, USAGE_RULES)),
                }
                index += 1;
            }
            RulesCmd::History { limit }
        }
        "export" => {
            expect_end(rest, 0, USAGE_RULES)?;
            RulesCmd::Export
        }
        "import" => {
            let values = positionals(rest, 1, "a backup file path", USAGE_RULES)?;
            RulesCmd::Import {
                file: PathBuf::from(values[0]),
            }
        }
        other => {
            return Err(input_err(format!(
                "unknown rules subcommand '{other}'\n{USAGE_RULES}"
            )))
        }
    };
    Ok((CommandKind::Rules(cmd), json))
}

fn unexpected_token(token: &Token<'_>, usage: &str) -> RemoteError {
    match token {
        Token::Flag(flag, _) => input_err(format!("unknown flag '{flag}'\n{usage}")),
        Token::Positional(value) => input_err(format!("unexpected argument '{value}'\n{usage}")),
    }
}

fn parse_rules_add(tokens: &[Token<'_>]) -> Result<RulesCmd, RemoteError> {
    let mut source: Option<JsonSource> = None;
    let mut allow_destructive = false;
    let mut index = 0;
    while let Some(token) = tokens.get(index) {
        match token {
            Token::Flag("--file", inline) => {
                let path = flag_value(tokens, &mut index, *inline, "--file")?;
                set_source(&mut source, JsonSource::File(PathBuf::from(path)))?;
            }
            Token::Flag("--json", inline) => {
                let text = flag_value(tokens, &mut index, *inline, "--json")?;
                set_source(&mut source, JsonSource::Inline(text.to_string()))?;
            }
            Token::Flag("--allow-destructive", None) => allow_destructive = true,
            other => return Err(unexpected_token(other, USAGE_RULES)),
        }
        index += 1;
    }
    let source = source.ok_or_else(|| {
        input_err(format!(
            "rules add requires --file <path> or --json '<json>'\n{USAGE_RULES}"
        ))
    })?;
    Ok(RulesCmd::Add {
        source,
        allow_destructive,
    })
}

fn set_source(slot: &mut Option<JsonSource>, source: JsonSource) -> Result<(), RemoteError> {
    if slot.is_some() {
        return Err(input_err(
            "give exactly one of --file <path> or --json '<json>'",
        ));
    }
    *slot = Some(source);
    Ok(())
}

fn parse_rules_test(tokens: &[Token<'_>], json: &mut bool) -> Result<RulesCmd, RemoteError> {
    let mut source: Option<JsonSource> = None;
    let mut limit = None;
    let mut index = 0;
    while let Some(token) = tokens.get(index) {
        match token {
            Token::Flag("--file", inline) => {
                let path = flag_value(tokens, &mut index, *inline, "--file")?;
                set_source(&mut source, JsonSource::File(PathBuf::from(path)))?;
            }
            Token::Flag("--limit", inline) => {
                limit = Some(parse_limit(
                    flag_value(tokens, &mut index, *inline, "--limit")?,
                    "--limit",
                )?);
            }
            Token::Flag("--json", inline) => *json = json_switch(*inline)?,
            other => return Err(unexpected_token(other, USAGE_RULES)),
        }
        index += 1;
    }
    let source = source
        .ok_or_else(|| input_err(format!("rules test requires --file <path>\n{USAGE_RULES}")))?;
    Ok(RulesCmd::Test { source, limit })
}

fn parse_labels(tokens: &[Token<'_>]) -> Result<(CommandKind, bool), RemoteError> {
    let sub = match tokens.first() {
        Some(Token::Positional(sub)) => *sub,
        _ => {
            return Err(input_err(format!(
                "missing labels subcommand\n{USAGE_LABELS}"
            )))
        }
    };
    let rest = &tokens[1..];
    let mut json = false;
    let cmd = match sub {
        "list" => {
            let mut index = 0;
            while let Some(token) = rest.get(index) {
                match token {
                    Token::Flag("--json", inline) => json = json_switch(*inline)?,
                    other => return Err(unexpected_token(other, USAGE_LABELS)),
                }
                index += 1;
            }
            LabelsCmd::List
        }
        "add" => {
            let mut name = None;
            let mut color = None;
            let mut index = 0;
            while let Some(token) = rest.get(index) {
                match token {
                    Token::Flag("--color", inline) => {
                        color = Some(flag_value(rest, &mut index, *inline, "--color")?.to_string());
                    }
                    Token::Flag("--json", inline) => json = json_switch(*inline)?,
                    Token::Positional(value) if name.is_none() => name = Some(*value),
                    other => return Err(unexpected_token(other, USAGE_LABELS)),
                }
                index += 1;
            }
            let name = non_empty_name(name, USAGE_LABELS)?;
            LabelsCmd::Add { name, color }
        }
        "remove" => {
            let values = positionals(rest, 1, "a label name", USAGE_LABELS)?;
            LabelsCmd::Remove {
                name: non_empty_name(Some(values[0]), USAGE_LABELS)?,
            }
        }
        "policy" => parse_labels_policy(rest, &mut json)?,
        other => {
            return Err(input_err(format!(
                "unknown labels subcommand '{other}'\n{USAGE_LABELS}"
            )))
        }
    };
    Ok((CommandKind::Labels(cmd), json))
}

fn non_empty_name(name: Option<&str>, usage: &str) -> Result<String, RemoteError> {
    let name = name
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .ok_or_else(|| input_err(format!("missing label name\n{usage}")))?;
    Ok(name.to_string())
}

fn parse_labels_policy(tokens: &[Token<'_>], json: &mut bool) -> Result<LabelsCmd, RemoteError> {
    let mut name = None;
    let mut flags = PolicyFlags::default();
    let mut index = 0;
    while let Some(token) = tokens.get(index) {
        match token {
            Token::Flag("--ratio", inline) => {
                flags.ratio = Some(parse_ratio_limit(flag_value(
                    tokens, &mut index, *inline, "--ratio",
                )?)?);
            }
            Token::Flag("--time", inline) => {
                flags.time = Some(parse_secs_limit(flag_value(
                    tokens, &mut index, *inline, "--time",
                )?)?);
            }
            Token::Flag("--idle", inline) => {
                flags.idle = Some(parse_secs_limit(flag_value(
                    tokens, &mut index, *inline, "--idle",
                )?)?);
            }
            Token::Flag("--action", inline) => {
                flags.action = Some(parse_action_flag(
                    flag_value(tokens, &mut index, *inline, "--action")?,
                    true,
                )?);
            }
            Token::Flag("--save-path", inline) => {
                flags.save_path =
                    Some(flag_value(tokens, &mut index, *inline, "--save-path")?.to_string());
            }
            Token::Flag("--json", inline) => *json = json_switch(*inline)?,
            Token::Positional(value) if name.is_none() => name = Some(*value),
            other => return Err(unexpected_token(other, USAGE_LABELS)),
        }
        index += 1;
    }
    let name = non_empty_name(name, USAGE_LABELS)?;
    Ok(LabelsCmd::Policy { name, flags })
}

fn parse_seed(tokens: &[Token<'_>]) -> Result<(CommandKind, bool), RemoteError> {
    let sub = match tokens.first() {
        Some(Token::Positional(sub)) => *sub,
        _ => return Err(input_err(format!("missing seed subcommand\n{USAGE_SEED}"))),
    };
    let rest = &tokens[1..];
    let mut json = false;
    match sub {
        "show" => {
            let mut index = 0;
            while let Some(token) = rest.get(index) {
                match token {
                    Token::Flag("--json", inline) => json = json_switch(*inline)?,
                    other => return Err(unexpected_token(other, USAGE_SEED)),
                }
                index += 1;
            }
            Ok((CommandKind::SeedShow, json))
        }
        "set" => {
            let mut args = SeedSetArgs::default();
            let mut index = 0;
            while let Some(token) = rest.get(index) {
                match token {
                    Token::Flag("--ratio", inline) => {
                        args.ratio = Some(parse_global_ratio(flag_value(
                            rest, &mut index, *inline, "--ratio",
                        )?)?);
                    }
                    Token::Flag("--time", inline) => {
                        args.time = Some(parse_global_secs(flag_value(
                            rest, &mut index, *inline, "--time",
                        )?)?);
                    }
                    Token::Flag("--idle", inline) => {
                        args.idle = Some(parse_global_secs(flag_value(
                            rest, &mut index, *inline, "--idle",
                        )?)?);
                    }
                    Token::Flag("--mode", inline) => {
                        args.mode = Some(parse_mode(flag_value(
                            rest, &mut index, *inline, "--mode",
                        )?)?);
                    }
                    Token::Flag("--action", inline) => {
                        let flag = parse_action_flag(
                            flag_value(rest, &mut index, *inline, "--action")?,
                            false,
                        )?;
                        match flag {
                            ActionFlag::Set(action) => args.action = Some(action),
                            ActionFlag::Inherit => {
                                return Err(input_err("global seed action cannot be 'inherit'"))
                            }
                        }
                    }
                    Token::Flag("--enabled", inline) => {
                        args.enabled = Some(parse_bool(
                            flag_value(rest, &mut index, *inline, "--enabled")?,
                            "--enabled",
                        )?);
                    }
                    Token::Flag("--check-interval", inline) => {
                        let secs = parse_duration(flag_value(
                            rest,
                            &mut index,
                            *inline,
                            "--check-interval",
                        )?)?;
                        if secs < 1 {
                            return Err(input_err("--check-interval must be >= 1 second"));
                        }
                        args.check_interval = Some(secs);
                    }
                    Token::Flag("--json", inline) => json = json_switch(*inline)?,
                    other => return Err(unexpected_token(other, USAGE_SEED)),
                }
                index += 1;
            }
            if args.is_empty() {
                return Err(input_err(format!(
                    "seed set requires at least one flag\n{USAGE_SEED}"
                )));
            }
            Ok((CommandKind::SeedSet(args), json))
        }
        other => Err(input_err(format!(
            "unknown seed subcommand '{other}'\n{USAGE_SEED}"
        ))),
    }
}

fn parse_torrent(tokens: &[Token<'_>]) -> Result<(CommandKind, bool), RemoteError> {
    let sub = match tokens.first() {
        Some(Token::Positional(sub)) => *sub,
        _ => {
            return Err(input_err(format!(
                "missing torrent subcommand\n{USAGE_TORRENT}"
            )))
        }
    };
    if sub != "limits" {
        return Err(input_err(format!(
            "unknown torrent subcommand '{sub}'\n{USAGE_TORRENT}"
        )));
    }
    let rest = &tokens[1..];
    let mut hash = None;
    let mut ratio = None;
    let mut time = None;
    let mut idle = None;
    let mut action = None;
    let mut json = false;
    let mut index = 0;
    while let Some(token) = rest.get(index) {
        match token {
            Token::Flag("--ratio", inline) => {
                ratio = Some(parse_ratio_limit(flag_value(
                    rest, &mut index, *inline, "--ratio",
                )?)?);
            }
            Token::Flag("--time", inline) => {
                time = Some(parse_secs_limit(flag_value(
                    rest, &mut index, *inline, "--time",
                )?)?);
            }
            Token::Flag("--idle", inline) => {
                idle = Some(parse_secs_limit(flag_value(
                    rest, &mut index, *inline, "--idle",
                )?)?);
            }
            Token::Flag("--action", inline) => {
                action = Some(parse_action_flag(
                    flag_value(rest, &mut index, *inline, "--action")?,
                    true,
                )?);
            }
            Token::Flag("--json", inline) => json = json_switch(*inline)?,
            Token::Positional(value) if hash.is_none() => hash = Some(*value),
            other => return Err(unexpected_token(other, USAGE_TORRENT)),
        }
        index += 1;
    }
    let hash = validate_hash(
        hash.ok_or_else(|| input_err(format!("missing torrent hash\n{USAGE_TORRENT}")))?,
    )?;
    Ok((
        CommandKind::TorrentLimits(TorrentLimitsArgs {
            hash,
            ratio,
            time,
            idle,
            action,
        }),
        json,
    ))
}

// ---- Request building (pure, network-free) ----

/// A typed RPC argument (`ObjectValue` without the proto dependency in tests).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RpcArg {
    /// String argument (JSON documents, ratios, enum strings, …).
    Text(String),
    /// Integer argument (ids, seconds, switches).
    Int(i64),
}

/// One `CommandService.Execute` invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcCall {
    /// Command name from the shared map (§6.4).
    pub command: String,
    /// Positional arguments.
    pub args: Vec<RpcArg>,
    /// Download-scoped target (`target_hash` → `Target::Download`), if any.
    pub target: Option<String>,
}

impl RpcCall {
    fn new(command: impl Into<String>, args: Vec<RpcArg>) -> Self {
        Self {
            command: command.into(),
            args,
            target: None,
        }
    }

    fn targeted(command: impl Into<String>, args: Vec<RpcArg>, target: &str) -> Self {
        Self {
            command: command.into(),
            args,
            target: Some(target.to_string()),
        }
    }

    fn to_object_values(&self) -> Vec<ObjectValue> {
        self.args
            .iter()
            .map(|arg| ObjectValue {
                kind: Some(match arg {
                    RpcArg::Text(text) => object_value::Kind::StringValue(text.clone()),
                    RpcArg::Int(value) => object_value::Kind::IntValue(*value),
                }),
            })
            .collect()
    }
}

/// Maps a parsed command onto the RPC calls that implement it (§6.4).
///
/// Pure except for reading `--file`/`import` documents. Multi-step flows:
/// - `labels policy`: returns the initial `label.policy.get`; the executor
///   merges flags ([`merge_policy_json`]) and appends [`policy_set_call`] /
///   [`save_path_call`], then re-reads the policy for display.
/// - `seed set` / `torrent limits`: one setter per given flag, followed by a
///   final read (`seed.show` / `d.seed_limits`) for display.
///
/// # Errors
///
/// [`RemoteError::Read`] / [`RemoteError::Json`] for bad documents,
/// [`RemoteError::Input`] for unsupported raw targets.
pub fn build_calls(command: &RemoteCommand) -> Result<Vec<RpcCall>, RemoteError> {
    match &command.kind {
        CommandKind::Rules(rules) => build_rules_calls(rules),
        CommandKind::Labels(labels) => build_labels_calls(labels),
        CommandKind::SeedShow => Ok(vec![RpcCall::new("seed.show", vec![])]),
        CommandKind::SeedSet(args) => Ok(build_seed_set_calls(args)),
        CommandKind::TorrentLimits(args) => Ok(build_torrent_limits_calls(args)),
        CommandKind::Raw { command, args } => {
            let (target, args) = split_raw_target(args)?;
            Ok(vec![RpcCall {
                command: command.clone(),
                args,
                target,
            }])
        }
    }
}

fn read_json_source(source: &JsonSource) -> Result<String, RemoteError> {
    match source {
        JsonSource::Inline(text) => Ok(text.clone()),
        JsonSource::File(path) => {
            std::fs::read_to_string(path).map_err(|source| RemoteError::Read {
                path: path.display().to_string(),
                source,
            })
        }
    }
}

fn build_rules_calls(rules: &RulesCmd) -> Result<Vec<RpcCall>, RemoteError> {
    let call = match rules {
        RulesCmd::List => RpcCall::new("rules.list", vec![]),
        RulesCmd::Add {
            source,
            allow_destructive,
        } => {
            let text = read_json_source(source)?;
            let text = inject_allow_destructive(&text, *allow_destructive)?;
            RpcCall::new("rules.add", vec![RpcArg::Text(text)])
        }
        RulesCmd::Remove { id } => RpcCall::new("rules.remove", vec![RpcArg::Int(*id as i64)]),
        RulesCmd::Enable { id } => RpcCall::new("rules.enable", vec![RpcArg::Int(*id as i64)]),
        RulesCmd::Disable { id } => RpcCall::new("rules.disable", vec![RpcArg::Int(*id as i64)]),
        RulesCmd::Get { id } => RpcCall::new("rules.get", vec![RpcArg::Int(*id as i64)]),
        RulesCmd::Test { source, .. } => {
            let text = read_json_source(source)?;
            RpcCall::new("rules.test", vec![RpcArg::Text(text)])
        }
        RulesCmd::Check => RpcCall::new("rules.check", vec![]),
        RulesCmd::History { limit } => RpcCall::new(
            "rules.history",
            vec![RpcArg::Int(limit.unwrap_or(HISTORY_DEFAULT_LIMIT) as i64)],
        ),
        RulesCmd::Export => RpcCall::new("rules.export", vec![]),
        RulesCmd::Import { file } => {
            let text = std::fs::read_to_string(file).map_err(|source| RemoteError::Read {
                path: file.display().to_string(),
                source,
            })?;
            RpcCall::new("rules.import", vec![RpcArg::Text(text)])
        }
    };
    Ok(vec![call])
}

fn build_labels_calls(labels: &LabelsCmd) -> Result<Vec<RpcCall>, RemoteError> {
    let call = match labels {
        LabelsCmd::List => RpcCall::new("label.list", vec![]),
        LabelsCmd::Add { name, color } => {
            let mut args = vec![RpcArg::Text(name.clone())];
            if let Some(color) = color {
                args.push(RpcArg::Text(color.clone()));
            }
            RpcCall::new("label.add", args)
        }
        LabelsCmd::Remove { name } => {
            RpcCall::new("label.remove", vec![RpcArg::Text(name.clone())])
        }
        LabelsCmd::Policy { name, .. } => {
            // Read-modify-write: the executor merges the flags into this
            // policy and appends the setter calls (see merge_policy_json).
            RpcCall::new("label.policy.get", vec![RpcArg::Text(name.clone())])
        }
    };
    Ok(vec![call])
}

fn build_seed_set_calls(args: &SeedSetArgs) -> Vec<RpcCall> {
    let mut calls = Vec::new();
    if let Some(enabled) = args.enabled {
        calls.push(RpcCall::new(
            "seed.enabled.set",
            vec![RpcArg::Int(i64::from(enabled))],
        ));
    }
    if let Some(ratio) = args.ratio {
        // f64 travels as a string: ObjectValue has no float variant and the
        // server's parse_ratio accepts (double-flagged) strings alike.
        calls.push(RpcCall::new(
            "seed.ratio.set",
            vec![RpcArg::Text(ratio.to_string())],
        ));
    }
    if let Some(secs) = args.time {
        calls.push(RpcCall::new(
            "seed.time.set",
            vec![RpcArg::Int(secs as i64)],
        ));
    }
    if let Some(secs) = args.idle {
        calls.push(RpcCall::new(
            "seed.idle.set",
            vec![RpcArg::Int(secs as i64)],
        ));
    }
    if let Some(mode) = args.mode {
        let text = match mode {
            LimitMode::Any => "any",
            LimitMode::All => "all",
        };
        calls.push(RpcCall::new(
            "seed.mode.set",
            vec![RpcArg::Text(text.to_string())],
        ));
    }
    if let Some(action) = &args.action {
        calls.push(RpcCall::new(
            "seed.action.set",
            vec![RpcArg::Text(rt_core::download::format_seed_action(action))],
        ));
    }
    if let Some(secs) = args.check_interval {
        calls.push(RpcCall::new(
            "seed.check_interval.set",
            vec![RpcArg::Int(secs as i64)],
        ));
    }
    calls
}

fn build_torrent_limits_calls(args: &TorrentLimitsArgs) -> Vec<RpcCall> {
    let mut calls = Vec::new();
    if let Some(ratio) = &args.ratio {
        calls.push(RpcCall::targeted(
            "d.seed_ratio.set",
            vec![RpcArg::Text(limit_wire(ratio))],
            &args.hash,
        ));
    }
    if let Some(time) = &args.time {
        calls.push(RpcCall::targeted(
            "d.seed_time.set",
            vec![RpcArg::Text(limit_wire(time))],
            &args.hash,
        ));
    }
    if let Some(idle) = &args.idle {
        calls.push(RpcCall::targeted(
            "d.seed_idle.set",
            vec![RpcArg::Text(limit_wire(idle))],
            &args.hash,
        ));
    }
    if let Some(action) = &args.action {
        let text = match action {
            ActionFlag::Inherit => "inherit".to_string(),
            ActionFlag::Set(action) => rt_core::download::format_seed_action(action),
        };
        calls.push(RpcCall::targeted(
            "d.seed_action.set",
            vec![RpcArg::Text(text)],
            &args.hash,
        ));
    }
    // Always finish with the effective-limits read (values + sources).
    calls.push(RpcCall::targeted("d.seed_limits", vec![], &args.hash));
    calls
}

/// `cmd` target convention (XML-RPC parity): a leading 40-hex-digit hash,
/// optionally suffixed `:d`, becomes the download target; other suffixes
/// (`:f0`, `:t1`, …) are rejected because gRPC `target_hash` only carries
/// download targets. Remaining args are typed: integer-looking → int.
fn split_raw_target(args: &[String]) -> Result<(Option<String>, Vec<RpcArg>), RemoteError> {
    let Some(first) = args.first() else {
        return Ok((None, Vec::new()));
    };
    let (hash_part, suffix) = match first.split_once(':') {
        Some((hash, suffix)) => (hash, Some(suffix)),
        None => (first.as_str(), None),
    };
    let is_hash = hash_part.len() == 40 && hash_part.chars().all(|c| c.is_ascii_hexdigit());
    if is_hash {
        match suffix {
            None | Some("d") => {
                let target = hash_part.to_ascii_lowercase();
                let rest = args[1..]
                    .iter()
                    .map(|arg| typed_arg(arg.as_str()))
                    .collect();
                return Ok((Some(target), rest));
            }
            Some(other) => {
                return Err(input_err(format!(
                    "unsupported target suffix ':{other}' in '{first}': \
                     gRPC Execute supports download targets only (<hash> or <hash>:d)"
                )))
            }
        }
    }
    Ok((
        None,
        args.iter().map(|arg| typed_arg(arg.as_str())).collect(),
    ))
}

fn typed_arg(text: &str) -> RpcArg {
    match text.parse::<i64>() {
        Ok(value) => RpcArg::Int(value),
        Err(_) => RpcArg::Text(text.to_string()),
    }
}

/// Sets `allow_destructive: true` on a rule JSON document (per-rule gate,
/// plan §5.5). With `allow = false` the document passes through untouched;
/// the server still validates the full rule.
///
/// # Errors
///
/// [`RemoteError::Json`] when the document is not a JSON object.
pub fn inject_allow_destructive(json: &str, allow: bool) -> Result<String, RemoteError> {
    if !allow {
        return Ok(json.to_string());
    }
    let mut value: serde_json::Value =
        serde_json::from_str(json).map_err(|source| RemoteError::Json {
            what: "rule",
            source,
        })?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| input_err("rule JSON must be an object"))?;
    object.insert(
        "allow_destructive".to_string(),
        serde_json::Value::Bool(true),
    );
    serde_json::to_string(&value).map_err(|source| RemoteError::Json {
        what: "rule",
        source,
    })
}

/// Merges `labels policy` flags into an existing policy JSON (partial update:
/// absent flags keep their current value). Returns the new `SeedPolicy` JSON
/// for `label.policy.set`.
///
/// # Errors
///
/// [`RemoteError::Json`] when the existing policy is not valid `SeedPolicy`
/// JSON.
pub fn merge_policy_json(existing: &str, flags: &PolicyFlags) -> Result<String, RemoteError> {
    let mut policy: SeedPolicy =
        serde_json::from_str(existing).map_err(|source| RemoteError::Json {
            what: "existing label policy",
            source,
        })?;
    if let Some(ratio) = &flags.ratio {
        policy.ratio = ratio.clone();
    }
    if let Some(time) = &flags.time {
        policy.seeding_time_secs = time.clone();
    }
    if let Some(idle) = &flags.idle {
        policy.inactive_secs = idle.clone();
    }
    if let Some(action) = &flags.action {
        policy.action = match action {
            ActionFlag::Inherit => None,
            ActionFlag::Set(action) => Some(action.clone()),
        };
    }
    serde_json::to_string(&policy).map_err(|source| RemoteError::Json {
        what: "label policy",
        source,
    })
}

/// The `label.policy.set` call for a merged policy (`<name>,<json>` travels
/// as a two-element argument list; the server splits only the first comma).
pub fn policy_set_call(name: &str, policy_json: &str) -> RpcCall {
    RpcCall::new(
        "label.policy.set",
        vec![
            RpcArg::Text(name.to_string()),
            RpcArg::Text(policy_json.to_string()),
        ],
    )
}

/// The `label.save_path.set` call (`<name>,<path>`).
pub fn save_path_call(name: &str, path: &str) -> RpcCall {
    RpcCall::new(
        "label.save_path.set",
        vec![
            RpcArg::Text(name.to_string()),
            RpcArg::Text(path.to_string()),
        ],
    )
}

/// Caps the `matches` array of a `rules.test` dry-run result client-side
/// (the RPC takes only the rule JSON; REST truncates the same way). The
/// `no_data` list is preserved untouched.
///
/// # Errors
///
/// [`RemoteError::Json`] when the result is not valid JSON.
pub fn truncate_test_matches(json: &str, limit: Option<usize>) -> Result<String, RemoteError> {
    let Some(limit) = limit else {
        return Ok(json.to_string());
    };
    let mut value: serde_json::Value =
        serde_json::from_str(json).map_err(|source| RemoteError::Json {
            what: "dry-run result",
            source,
        })?;
    if let Some(matches) = value
        .get_mut("matches")
        .and_then(serde_json::Value::as_array_mut)
    {
        matches.truncate(limit);
    }
    serde_json::to_string(&value).map_err(|source| RemoteError::Json {
        what: "dry-run result",
        source,
    })
}

// ---- Endpoint resolution ----

/// Resolves the daemon endpoint: the explicit `--endpoint` value (accepts
/// `host:port`, `tcp://…`, `unix:///…` and `grpc://…`), or discovery from the
/// active TOML config (`[rpc] grpc_socket` preferred — the Unix socket is
/// trusted — then `[rpc] grpc_port` on loopback).
///
/// # Errors
///
/// [`RemoteError::Input`] with a REST/curl hint when no endpoint can be
/// determined, or the explicit value is malformed.
pub fn resolve_endpoint(explicit: Option<&str>) -> Result<String, RemoteError> {
    match explicit {
        Some(raw) => {
            let trimmed = raw.trim();
            let normalized = match trimmed.strip_prefix("grpc://") {
                Some(rest) => format!("tcp://{rest}"),
                None => trimmed.to_string(),
            };
            let parsed: rt_tui_client::Endpoint = normalized
                .parse()
                .map_err(|err| input_err(format!("invalid --endpoint '{raw}': {err}")))?;
            Ok(parsed.to_string())
        }
        None => discover_endpoint(),
    }
}

fn discover_endpoint() -> Result<String, RemoteError> {
    let hint = "pass --endpoint <host:port|tcp://host:port|unix:///path/to.sock>, or enable \
                gRPC in rstorrent.toml ([rpc] grpc_port / grpc_socket); REST alternative: \
                curl -H \"Authorization: Bearer <token>\" http://127.0.0.1:8080/api/rules";
    let path = crate::setup::find_config_file(false)
        .ok_or_else(|| input_err(format!("no config file found; {hint}")))?;
    if path.extension().and_then(|ext| ext.to_str()) != Some("toml") {
        return Err(input_err(format!(
            "config {} is not a TOML file; cannot discover the gRPC endpoint; {hint}",
            path.display()
        )));
    }
    let content = std::fs::read_to_string(&path).map_err(|source| RemoteError::Read {
        path: path.display().to_string(),
        source,
    })?;
    let config = RtorrentTomlConfig::parse(&content).map_err(|source| RemoteError::Config {
        path: path.display().to_string(),
        source,
    })?;
    if let Some(socket) = config
        .rpc
        .grpc_socket
        .as_deref()
        .map(str::trim)
        .filter(|socket| !socket.is_empty())
    {
        // Prefer the Unix socket: the server trusts local peers, so rule
        // mutations work without a bearer token.
        let endpoint = if socket.starts_with("unix://") {
            socket.to_string()
        } else {
            format!("unix://{socket}")
        };
        return Ok(endpoint);
    }
    if let Some(port) = config.rpc.grpc_port {
        return Ok(format!("127.0.0.1:{port}"));
    }
    Err(input_err(format!(
        "gRPC is not enabled in {}; {hint}",
        path.display()
    )))
}

// ---- Transport ----

/// Entry point for the keyword router (wired by the orchestrator in
/// `lib.rs`): handles `rules`/`labels`/`seed`/`torrent`/`cmd` against a remote
/// daemon. Returns `Ok(true)` when the keyword was handled, `Ok(false)` when
/// it is not a remote keyword (legacy FILES behaviour must continue).
///
/// `args` are the positional words after the keyword; `endpoint` is the
/// `--endpoint` flag value (or `None` → config discovery).
///
/// # Errors
///
/// Parse failures, endpoint problems, transport failures and daemon-side
/// command errors — the caller prints them to stderr and exits non-zero.
pub fn handle_remote_subcommand(
    keyword: &str,
    args: &[String],
    endpoint: Option<&str>,
    token: Option<&str>,
) -> Result<bool, Box<dyn std::error::Error>> {
    let Some(command) = parse_remote_command(keyword, args)? else {
        return Ok(false);
    };
    let endpoint = resolve_endpoint(endpoint)?;
    let token = resolve_token(token);
    run_blocking(async move {
        let mut bridge = GrpcBridge::connect_with_token(&endpoint, RPC_TIMEOUT, token).await?;
        execute(&command, &mut bridge).await
    })??;
    Ok(true)
}

/// Fallback router for raw argv when clap rejects subcommand flags
/// (`--json`, `--file`, `--ratio`, … are not main-Cli arguments). Finds the
/// first keyword family, extracts `--endpoint`/`--token` anywhere, and passes
/// the remaining tokens to the subcommand parser. Returns `None` when no
/// keyword is present so the original clap error can surface.
pub fn try_route_raw(raw: &[String]) -> Option<Result<(), Box<dyn std::error::Error>>> {
    let pos = raw
        .iter()
        .position(|a| matches!(a.as_str(), "rules" | "labels" | "seed" | "torrent" | "cmd"))?;
    let keyword = raw[pos].clone();
    let mut endpoint: Option<String> = None;
    let mut token: Option<String> = None;
    let mut rest: Vec<String> = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        let arg = raw[i].as_str();
        if arg == "--endpoint" {
            endpoint = raw.get(i + 1).cloned();
            i += 2;
            continue;
        }
        if arg == "--token" {
            token = raw.get(i + 1).cloned();
            i += 2;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--endpoint=") {
            endpoint = Some(value.to_string());
            i += 1;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--token=") {
            token = Some(value.to_string());
            i += 1;
            continue;
        }
        if i > pos {
            rest.push(raw[i].clone());
        }
        i += 1;
    }
    Some(
        handle_remote_subcommand(&keyword, &rest, endpoint.as_deref(), token.as_deref())
            .map(|_| ()),
    )
}

/// Resolves the gRPC bearer token: explicit `--token` wins, otherwise the
/// active TOML config's `[rpc] grpc_token`. Unix sockets need no token.
pub fn resolve_token(explicit: Option<&str>) -> Option<String> {
    if let Some(token) = explicit.map(str::trim).filter(|t| !t.is_empty()) {
        return Some(token.to_string());
    }
    let path = crate::setup::find_config_file(false)?;
    if path.extension().and_then(|ext| ext.to_str()) != Some("toml") {
        return None;
    }
    let content = std::fs::read_to_string(&path).ok()?;
    let config = RtorrentTomlConfig::parse(&content).ok()?;
    config
        .rpc
        .grpc_token
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

/// Bridges sync → async: on the daemon's multi-thread runtime (the normal
/// call site, `run_app`) use `block_in_place` like `rules.check`; outside any
/// runtime (tests, tools) spin up a private current-thread runtime.
fn run_blocking<F: std::future::Future>(future: F) -> Result<F::Output, RemoteError> {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            Ok(tokio::task::block_in_place(|| handle.block_on(future)))
        }
        Ok(_) => Err(input_err(
            "remote subcommands require the multi-thread Tokio runtime (run via rstorrent)",
        )),
        Err(_) => {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|err| input_err(format!("cannot create a Tokio runtime: {err}")))?;
            Ok(runtime.block_on(future))
        }
    }
}

async fn execute_call(
    bridge: &mut GrpcBridge,
    call: &RpcCall,
) -> Result<CommandResponse, RemoteError> {
    let response = bridge
        .execute(
            &call.command,
            call.to_object_values(),
            call.target.as_deref(),
        )
        .await?;
    if !response.success {
        return Err(RemoteError::Command {
            command: call.command.clone(),
            message: response.error_message,
        });
    }
    Ok(response)
}

fn string_result(response: &CommandResponse, command: &str) -> Result<String, RemoteError> {
    match response
        .result
        .as_ref()
        .and_then(|value| value.kind.as_ref())
    {
        Some(object_value::Kind::StringValue(text)) => Ok(text.clone()),
        _ => Err(RemoteError::Response {
            command: command.to_string(),
            detail: "expected a string result".to_string(),
        }),
    }
}

fn int_result(response: &CommandResponse, command: &str) -> Result<i64, RemoteError> {
    match response
        .result
        .as_ref()
        .and_then(|value| value.kind.as_ref())
    {
        Some(object_value::Kind::IntValue(value)) => Ok(*value),
        _ => Err(RemoteError::Response {
            command: command.to_string(),
            detail: "expected an integer result".to_string(),
        }),
    }
}

async fn execute(command: &RemoteCommand, bridge: &mut GrpcBridge) -> Result<(), RemoteError> {
    // Multi-step flow: label policy read-modify-write.
    if let CommandKind::Labels(LabelsCmd::Policy { name, flags }) = &command.kind {
        return execute_label_policy(name, flags, command.json, bridge).await;
    }

    let calls = build_calls(command)?;

    // Setter sequences followed by a display read.
    match &command.kind {
        CommandKind::SeedSet(_) => {
            for call in &calls {
                execute_call(bridge, call).await?;
            }
            let response = execute_call(bridge, &RpcCall::new("seed.show", vec![])).await?;
            return render_map_result(&response, command.json, "seed.show");
        }
        CommandKind::TorrentLimits(_) => {
            let (setters, getter) = calls.split_at(calls.len() - 1);
            for call in setters {
                execute_call(bridge, call).await?;
            }
            let response = execute_call(bridge, &getter[0]).await?;
            return render_map_result(&response, command.json, "d.seed_limits");
        }
        _ => {}
    }

    let response = execute_call(bridge, &calls[0]).await?;
    match &command.kind {
        CommandKind::Rules(RulesCmd::Add { .. }) => {
            println!("Added rule {}", int_result(&response, "rules.add")?);
            Ok(())
        }
        CommandKind::Rules(RulesCmd::Import { .. }) => {
            println!("Imported labels and rules");
            Ok(())
        }
        CommandKind::Rules(RulesCmd::Export) => {
            // Raw backup blob: `rstorrent rules export > backup.json`.
            println!("{}", string_result(&response, "rules.export")?);
            Ok(())
        }
        CommandKind::Rules(RulesCmd::Test { limit, .. }) => {
            let text = string_result(&response, "rules.test")?;
            let text = truncate_test_matches(&text, *limit)?;
            render_json_text(&text, command.json, TableKind::DryRun)
        }
        CommandKind::Rules(RulesCmd::List) => render_json_text(
            &string_result(&response, "rules.list")?,
            command.json,
            TableKind::Rules,
        ),
        CommandKind::Rules(RulesCmd::Get { .. }) => render_json_text(
            &string_result(&response, "rules.get")?,
            command.json,
            TableKind::Pretty,
        ),
        CommandKind::Rules(RulesCmd::Check) => render_json_text(
            &string_result(&response, "rules.check")?,
            command.json,
            TableKind::History,
        ),
        CommandKind::Rules(RulesCmd::History { .. }) => render_json_text(
            &string_result(&response, "rules.history")?,
            command.json,
            TableKind::History,
        ),
        CommandKind::Labels(LabelsCmd::List) => render_json_text(
            &string_result(&response, "label.list")?,
            command.json,
            TableKind::Labels,
        ),
        CommandKind::SeedShow => render_map_result(&response, command.json, "seed.show"),
        CommandKind::Raw { .. } => {
            render_raw_result(&response, command.json);
            Ok(())
        }
        // Silent successes: rules remove/enable/disable, labels add/remove
        // (and the setter flows above, which return early).
        CommandKind::Rules(_)
        | CommandKind::Labels(_)
        | CommandKind::SeedSet(_)
        | CommandKind::TorrentLimits(_) => Ok(()),
    }
}

async fn execute_label_policy(
    name: &str,
    flags: &PolicyFlags,
    json: bool,
    bridge: &mut GrpcBridge,
) -> Result<(), RemoteError> {
    let get = RpcCall::new("label.policy.get", vec![RpcArg::Text(name.to_string())]);
    let response = execute_call(bridge, &get).await?;
    let existing = string_result(&response, "label.policy.get")?;

    if flags.has_policy_fields() {
        let merged = merge_policy_json(&existing, flags)?;
        execute_call(bridge, &policy_set_call(name, &merged)).await?;
    }
    if let Some(path) = &flags.save_path {
        execute_call(bridge, &save_path_call(name, path)).await?;
    }

    let response = execute_call(bridge, &get).await?;
    let updated = string_result(&response, "label.policy.get")?;
    render_json_text(&updated, json, TableKind::KeyValue)
}

// ---- Rendering ----

/// Which table shape a JSON result gets when `--json` is absent.
#[derive(Debug, Clone, Copy)]
enum TableKind {
    Rules,
    Labels,
    History,
    DryRun,
    KeyValue,
    Pretty,
}

fn render_json_text(text: &str, json: bool, kind: TableKind) -> Result<(), RemoteError> {
    if json {
        println!("{text}");
        return Ok(());
    }
    let value: serde_json::Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(_) => {
            // Not JSON after all: print it verbatim.
            println!("{text}");
            return Ok(());
        }
    };
    let rendered = match kind {
        TableKind::Rules => render_rules_table(&value),
        TableKind::Labels => render_labels_table(&value),
        TableKind::History => render_history_table(&value),
        TableKind::DryRun => render_dry_run_table(&value),
        TableKind::KeyValue => render_key_value_table(&value),
        TableKind::Pretty => {
            serde_json::to_string_pretty(&value).unwrap_or_else(|_| text.to_string())
        }
    };
    println!("{rendered}");
    Ok(())
}

fn render_map_result(
    response: &CommandResponse,
    json: bool,
    command: &str,
) -> Result<(), RemoteError> {
    let value = response
        .result
        .as_ref()
        .map(object_to_json)
        .ok_or_else(|| RemoteError::Response {
            command: command.to_string(),
            detail: "expected a result payload".to_string(),
        })?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&value).map_err(|source| RemoteError::Json {
                what: "result",
                source
            })?
        );
    } else {
        println!("{}", render_key_value_table(&value));
    }
    Ok(())
}

fn render_raw_result(response: &CommandResponse, json: bool) {
    let Some(result) = response.result.as_ref() else {
        return;
    };
    match result.kind.as_ref() {
        Some(object_value::Kind::StringValue(text)) => println!("{text}"),
        Some(object_value::Kind::IntValue(value)) => println!("{value}"),
        Some(object_value::Kind::BoolValue(value)) => println!("{value}"),
        Some(object_value::Kind::BytesValue(bytes)) => {
            println!("{}", String::from_utf8_lossy(bytes));
        }
        Some(object_value::Kind::NullValue(_)) | None => {}
        Some(object_value::Kind::ListValue(_) | object_value::Kind::MapValue(_)) => {
            let value = object_to_json(result);
            let rendered = if json {
                serde_json::to_string(&value)
            } else {
                serde_json::to_string_pretty(&value)
            };
            match rendered {
                Ok(text) => println!("{text}"),
                Err(_) => println!("{value:?}"),
            }
        }
    }
}

/// Converts a proto `ObjectValue` into JSON for display (maps/lists recurse;
/// bytes render lossily as text).
fn object_to_json(value: &ObjectValue) -> serde_json::Value {
    use serde_json::Value;
    match value.kind.as_ref() {
        Some(object_value::Kind::IntValue(value)) => Value::from(*value),
        Some(object_value::Kind::StringValue(text)) => Value::from(text.clone()),
        Some(object_value::Kind::BoolValue(value)) => Value::from(*value),
        Some(object_value::Kind::BytesValue(bytes)) => {
            Value::from(String::from_utf8_lossy(bytes).into_owned())
        }
        Some(object_value::Kind::ListValue(list)) => {
            Value::Array(list.values.iter().map(object_to_json).collect())
        }
        Some(object_value::Kind::MapValue(map)) => Value::Object(
            map.fields
                .iter()
                .map(|(key, value)| (key.clone(), object_to_json(value)))
                .collect(),
        ),
        Some(object_value::Kind::NullValue(_)) | None => Value::Null,
    }
}

/// Simple aligned-column table (two-space gutter, char-count widths).
fn render_table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let mut widths: Vec<usize> = headers
        .iter()
        .map(|header| header.chars().count())
        .collect();
    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            if let Some(width) = widths.get_mut(index) {
                *width = (*width).max(cell.chars().count());
            }
        }
    }
    let mut out = String::new();
    let mut push_row = |cells: &[String]| {
        for (index, cell) in cells.iter().enumerate() {
            if index > 0 {
                out.push_str("  ");
            }
            let width = widths.get(index).copied().unwrap_or(0);
            out.push_str(&format!("{cell:<width$}"));
        }
        // Trim the padding of the last column.
        while out.ends_with(' ') {
            out.pop();
        }
        out.push('\n');
    };
    let header_cells: Vec<String> = headers.iter().map(|header| header.to_string()).collect();
    push_row(&header_cells);
    for row in rows {
        push_row(row);
    }
    if rows.is_empty() {
        out.push_str("(no entries)\n");
    }
    out.trim_end().to_string()
}

fn field_str<'a>(value: &'a serde_json::Value, key: &str) -> &'a str {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
}

fn field_i64(value: &serde_json::Value, key: &str) -> i64 {
    value
        .get(key)
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0)
}

fn field_bool(value: &serde_json::Value, key: &str) -> bool {
    value
        .get(key)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// Compact scalar rendering; compounds fall back to truncated JSON.
fn scalar_text(value: &serde_json::Value) -> String {
    use serde_json::Value;
    match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        Value::Bool(flag) => flag.to_string(),
        Value::Null => "-".to_string(),
        compound => truncate_text(&compound.to_string(), 60),
    }
}

fn truncate_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut shortened: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    shortened.push('…');
    shortened
}

/// `LimitValue` JSON (`"inherit"`/`"unlimited"`/`{"set":v}`) or a policy
/// action (`null` = inherit) as a display string.
fn limit_text(value: Option<&serde_json::Value>) -> String {
    use serde_json::Value;
    match value {
        None | Some(Value::Null) => "inherit".to_string(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Object(map)) if map.contains_key("set") => {
            scalar_text(map.get("set").unwrap_or(&Value::Null))
        }
        Some(other) => scalar_text(other),
    }
}

/// Externally-tagged enums (`scope`, `trigger`, action payloads) as
/// `name` / `name:payload` strings.
fn tagged_text(value: &serde_json::Value) -> String {
    use serde_json::Value;
    match value {
        Value::String(text) => text.clone(),
        Value::Object(map) if map.len() == 1 => match map.iter().next() {
            Some((key, Value::Null)) => key.clone(),
            Some((key, scalar @ (Value::String(_) | Value::Number(_) | Value::Bool(_)))) => {
                format!("{key}:{}", scalar_text(scalar))
            }
            Some((key, compound)) => format!("{key}:{}", truncate_text(&compound.to_string(), 40)),
            None => String::new(),
        },
        other => truncate_text(&other.to_string(), 40),
    }
}

fn action_names(value: Option<&serde_json::Value>) -> String {
    match value.and_then(serde_json::Value::as_array) {
        Some(actions) => actions
            .iter()
            .map(tagged_text)
            .collect::<Vec<_>>()
            .join(","),
        None => "-".to_string(),
    }
}

fn render_rules_table(value: &serde_json::Value) -> String {
    let Some(rules) = value.as_array() else {
        return serde_json::to_string_pretty(value).unwrap_or_default();
    };
    let rows: Vec<Vec<String>> = rules
        .iter()
        .map(|rule| {
            vec![
                field_i64(rule, "id").to_string(),
                field_bool(rule, "enabled").to_string(),
                field_i64(rule, "priority").to_string(),
                truncate_text(field_str(rule, "name"), 40),
                tagged_text(rule.get("scope").unwrap_or(&serde_json::Value::Null)),
                tagged_text(rule.get("trigger").unwrap_or(&serde_json::Value::Null)),
                action_names(rule.get("actions")),
            ]
        })
        .collect();
    render_table(
        &[
            "ID", "ENABLED", "PRIORITY", "NAME", "SCOPE", "TRIGGER", "ACTIONS",
        ],
        &rows,
    )
}

fn render_labels_table(value: &serde_json::Value) -> String {
    let Some(labels) = value.as_array() else {
        return serde_json::to_string_pretty(value).unwrap_or_default();
    };
    let rows: Vec<Vec<String>> = labels
        .iter()
        .map(|label| {
            let policy = label.get("policy").unwrap_or(&serde_json::Value::Null);
            vec![
                field_str(label, "name").to_string(),
                field_str(label, "color").to_string(),
                limit_text(policy.get("ratio")),
                limit_text(policy.get("seeding_time_secs")),
                limit_text(policy.get("inactive_secs")),
                limit_text(policy.get("action")),
                field_str(label, "save_path").to_string(),
            ]
        })
        .collect();
    render_table(
        &[
            "NAME",
            "COLOR",
            "RATIO",
            "TIME",
            "IDLE",
            "ACTION",
            "SAVE_PATH",
        ],
        &rows,
    )
}

fn render_history_table(value: &serde_json::Value) -> String {
    let Some(entries) = value.as_array() else {
        return serde_json::to_string_pretty(value).unwrap_or_default();
    };
    let rows: Vec<Vec<String>> = entries
        .iter()
        .map(|entry| {
            vec![
                field_i64(entry, "at").to_string(),
                truncate_text(field_str(entry, "rule_name"), 30),
                truncate_text(field_str(entry, "torrent_name"), 40),
                field_str(entry, "status").to_string(),
                action_names(entry.get("actions")),
                truncate_text(field_str(entry, "reason"), 60),
            ]
        })
        .collect();
    render_table(
        &["AT", "RULE", "TORRENT", "STATUS", "ACTIONS", "REASON"],
        &rows,
    )
}

fn render_dry_run_table(value: &serde_json::Value) -> String {
    let mut out = String::new();
    let matches = value.get("matches").and_then(serde_json::Value::as_array);
    let rows: Vec<Vec<String>> = matches
        .map(|matches| {
            matches
                .iter()
                .map(|m| {
                    vec![
                        field_str(m, "torrent_hash").to_string(),
                        truncate_text(field_str(m, "name"), 40),
                        action_names(m.get("actions")),
                        truncate_text(field_str(m, "reason"), 60),
                    ]
                })
                .collect()
        })
        .unwrap_or_default();
    out.push_str(&render_table(&["HASH", "NAME", "ACTIONS", "REASON"], &rows));
    if let Some(no_data) = value.get("no_data").and_then(serde_json::Value::as_array) {
        if !no_data.is_empty() {
            out.push_str("\nno swarm data (condition not evaluated):\n");
            for hash in no_data {
                out.push_str(&format!("  {}\n", scalar_text(hash)));
            }
        }
    }
    out.push_str("\nNothing was changed (dry-run).");
    out
}

fn render_key_value_table(value: &serde_json::Value) -> String {
    let Some(map) = value.as_object() else {
        return serde_json::to_string_pretty(value).unwrap_or_default();
    };
    let rows: Vec<Vec<String>> = map
        .iter()
        .map(|(key, value)| vec![key.clone(), scalar_text(value)])
        .collect();
    render_table(&["KEY", "VALUE"], &rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_units_convert_to_seconds() {
        assert_eq!(parse_duration("120").unwrap(), 120);
        assert_eq!(parse_duration("30s").unwrap(), 30);
        assert_eq!(parse_duration("30m").unwrap(), 1_800);
        assert_eq!(parse_duration("2h").unwrap(), 7_200);
        assert_eq!(parse_duration("7d").unwrap(), 604_800);
    }

    #[test]
    fn limit_values_parse_the_three_forms() {
        assert_eq!(parse_ratio_limit("inherit").unwrap(), LimitValue::Inherit);
        assert_eq!(
            parse_ratio_limit("unlimited").unwrap(),
            LimitValue::Unlimited
        );
        assert_eq!(parse_ratio_limit("2.5").unwrap(), LimitValue::Set(2.5));
        assert_eq!(parse_secs_limit("7d").unwrap(), LimitValue::Set(604_800));
        assert!(parse_ratio_limit("-1").is_err());
        assert!(parse_ratio_limit("NaN").is_err());
        assert!(parse_secs_limit("10x").is_err());
    }

    #[test]
    fn raw_target_extraction_follows_xmlrpc_convention() {
        let hash = "1111222233334444555566667777888899990000";
        let (target, args) = split_raw_target(&[hash.to_string(), "extra".to_string()]).unwrap();
        assert_eq!(target.as_deref(), Some(hash));
        assert_eq!(args, vec![RpcArg::Text("extra".to_string())]);

        let (target, args) = split_raw_target(&["20".to_string()]).unwrap();
        assert_eq!(target, None);
        assert_eq!(args, vec![RpcArg::Int(20)]);

        assert!(split_raw_target(&[format!("{hash}:f0")]).is_err());
    }
}

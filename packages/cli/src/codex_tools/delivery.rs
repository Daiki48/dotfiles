//! `codex-delivery` の安全な Rust 実装。
//!
//! 外部コマンド、GitHub の応答、managed state はすべて fail-closed で扱う。
//! このモジュールは installer から呼び出せるよう `entrypoint` のみを公開する。

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::env;
use std::ffi::{CStr, OsString};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::{process, trust};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

const MANIFEST_VERSION: i64 = 1;
const RECEIPT_VERSION: i64 = 5;
const STATE_VERSION: i64 = 1;
const MAX_FILE_BYTES: u64 = 256 * 1024;
const MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
const MAX_PAGES: usize = 20;
const MAX_ITEMS: usize = 10_000;
const MAX_MAIN_SYNC_RECOVERY_PATHS: usize = 512;
const MAX_RULESETS: usize = 20;
const COMMAND_TIMEOUT: u64 = 45;
const OPERATION_TIMEOUT: u64 = 300;
const MAIN_SYNC_SANDBOX_RETRY_READY: &str = "main-sync-sandbox-retry-ready";
const MAIN_SYNC_SANDBOX_RETRY_CONSUMED: &str = "main-sync-sandbox-retry-consumed";
const SYSTEM_PATH: &str = "/usr/bin:/bin";
const GIT_BINARY: &str = "/usr/bin/git";
const GH_BINARY: &str = "/usr/bin/gh";
const SSH_BINARY: &str = "/usr/bin/ssh";
const STRICT_GATE_MODE: &str = "strict-ruleset";
const FREE_PRIVATE_GATE_MODE: &str = "github-free-private";
const LOOP_LEDGER_V1_MARKER: &str = "<!-- codex-loop-ledger:v1 -->";
const LOOP_LEDGER_V2_MARKER: &str = "<!-- codex-loop-ledger:v2 -->";
const THREAD_QUERY: &str = "query($owner:String!,$name:String!,$number:Int!,$cursor:String){repository(owner:$owner,name:$name){pullRequest(number:$number){reviewThreads(first:100,after:$cursor){nodes{isResolved}pageInfo{hasNextPage,endCursor}}}}}";
const REVIEW_QUERY: &str = "query($owner:String!,$name:String!,$number:Int!,$cursor:String){repository(owner:$owner,name:$name){pullRequest(number:$number){reviews(first:100,after:$cursor){nodes{id,state,submittedAt,author{login}}pageInfo{hasNextPage,endCursor}}}}}";
const DECISION_QUERY: &str = "query($owner:String!,$name:String!,$number:Int!){repository(owner:$owner,name:$name){pullRequest(number:$number){reviewDecision}}}";

#[derive(Debug, Clone, PartialEq, Eq)]
struct DeliveryError(String);

impl fmt::Display for DeliveryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for DeliveryError {}
type Result<T> = std::result::Result<T, DeliveryError>;

fn error(message: impl Into<String>) -> DeliveryError {
    DeliveryError(message.into())
}

fn object(entries: impl IntoIterator<Item = (impl Into<String>, Value)>) -> Value {
    Value::Object(entries.into_iter().map(|(k, v)| (k.into(), v)).collect())
}
fn string(value: impl Into<String>) -> Value {
    Value::String(value.into())
}
fn object_ref(value: &Value) -> Result<&Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| error("JSON objectが必要です"))
}
fn get<'a>(value: &'a Value, key: &str) -> Result<&'a Value> {
    object_ref(value)?
        .get(key)
        .ok_or_else(|| error(format!("JSON field `{key}`が不足しています")))
}
fn bool_value(value: &Value, key: &str) -> Result<bool> {
    get(value, key)?
        .as_bool()
        .ok_or_else(|| error(format!("{key}の型が不正です")))
}
fn str_value(value: &Value, key: &str) -> Result<String> {
    get(value, key)?
        .as_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| error(format!("{key}の型が不正です")))
}
fn int_value(value: &Value, key: &str) -> Result<i64> {
    get(value, key)?
        .as_i64()
        .ok_or_else(|| error(format!("{key}の型が不正です")))
}
fn value_keys(value: &Value) -> Result<HashSet<String>> {
    Ok(object_ref(value)?.keys().cloned().collect())
}
fn expected_keys(value: &Value, keys: &[&str], label: &str) -> Result<()> {
    let actual = value_keys(value)?;
    let expected: HashSet<String> = keys.iter().map(|v| (*v).to_string()).collect();
    if actual != expected {
        return Err(error(format!("{label} schemaが一致しません")));
    }
    Ok(())
}
fn now() -> String {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let days = duration.as_secs() / 86_400;
    let seconds = duration.as_secs() % 86_400;
    let (year, month, day) = civil_from_days(days as i64);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:09}Z",
        seconds / 3600,
        (seconds % 3600) / 60,
        seconds % 60,
        duration.subsec_nanos()
    )
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    (year + if month <= 2 { 1 } else { 0 }, month, day)
}

fn is_hex(value: &str) -> bool {
    value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
fn oid(value: &str, name: &str) -> Result<String> {
    if (value.len() == 40 || value.len() == 64) && is_hex(value) {
        Ok(value.to_ascii_lowercase())
    } else {
        Err(error(format!("{name}が正しいSHAではありません")))
    }
}
fn task_id(value: &str) -> Result<String> {
    let valid = if let Some(rest) = value.strip_prefix("issue-") {
        !rest.is_empty() && !rest.starts_with('0') && rest.bytes().all(|b| b.is_ascii_digit())
    } else if let Some(rest) = value.strip_prefix("task-") {
        (1..=64).contains(&rest.len())
            && rest
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            && rest
                .bytes()
                .next()
                .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    } else {
        false
    };
    if valid {
        Ok(value.to_string())
    } else {
        Err(error("task IDが安全ではありません"))
    }
}
fn repository_name(value: &str) -> bool {
    let mut parts = value.split('/');
    parts.next().is_some_and(valid_repo_part)
        && parts.next().is_some_and(valid_repo_part)
        && parts.next().is_none()
}
fn valid_repo_part(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.')
}
fn branch_name(value: &str) -> bool {
    let Some((prefix, rest)) = value.split_once('/') else {
        return false;
    };
    [
        "feat", "feature", "fix", "refactor", "docs", "test", "chore", "ci", "build", "perf",
        "style", "hotfix", "update",
    ]
    .contains(&prefix)
        && !rest.is_empty()
        && rest
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && rest.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'_' || b == b'-'
        })
}
fn plan_id(value: &str) -> bool {
    let Some((prefix, version)) = value.rsplit_once("-v") else {
        return false;
    };
    !prefix.is_empty()
        && prefix.len() >= 8
        && prefix.len() <= 128
        && prefix
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphanumeric())
        && prefix
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        && !version.is_empty()
        && !version.starts_with('0')
        && version.bytes().all(|b| b.is_ascii_digit())
}
fn repo_key(repository: &str) -> Result<String> {
    if !repository_name(repository) {
        return Err(error("GitHub repository名が安全ではありません"));
    }
    let (owner, name) = repository
        .split_once('/')
        .ok_or_else(|| error("repositoryが不正です"))?;
    Ok(format!(
        "{}-{}--{}-{}",
        owner.len(),
        owner.to_ascii_lowercase(),
        name.len(),
        name.to_ascii_lowercase()
    ))
}
fn gate_mode(value: &str) -> Result<&'static str> {
    match value {
        STRICT_GATE_MODE => Ok(STRICT_GATE_MODE),
        FREE_PRIVATE_GATE_MODE => Ok(FREE_PRIVATE_GATE_MODE),
        _ => Err(error("gate modeが不正です")),
    }
}
fn risk(value: &str) -> Result<&'static str> {
    match value {
        "low" => Ok("low"),
        "medium" => Ok("medium"),
        "high" => Ok("high"),
        "critical" => Ok("critical"),
        _ => Err(error("riskはlow/medium/high/criticalのいずれかです")),
    }
}
fn decision(value: &str) -> Result<&'static str> {
    match value {
        "autonomous" => Ok("autonomous"),
        "human-approved" => Ok("human-approved"),
        _ => Err(error("receipt decisionが不正です")),
    }
}
fn risk_level(value: &str) -> i32 {
    ["low", "medium", "high", "critical"]
        .iter()
        .position(|v| *v == value)
        .unwrap_or(99) as i32
}

thread_local! { static DEADLINE: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) }; }
thread_local! { static HELD_LOCKS: std::cell::RefCell<HashSet<(String, String)>> = std::cell::RefCell::new(HashSet::new()); }

fn with_deadline<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    let previous = DEADLINE.with(|cell| {
        let old = cell.get();
        let current = Instant::now() + Duration::from_secs(OPERATION_TIMEOUT);
        cell.set(Some(old.map_or(current, |v| v.min(current))));
        old
    });
    let result = f();
    DEADLINE.with(|cell| cell.set(previous));
    result
}
fn remaining(timeout: Duration) -> Result<Duration> {
    DEADLINE.with(|cell| {
        let Some(deadline) = cell.get() else {
            return Ok(timeout);
        };
        let rest = deadline.saturating_duration_since(Instant::now());
        if rest.is_zero() {
            Err(error("delivery操作全体がtimeoutしました"))
        } else {
            Ok(rest.min(timeout))
        }
    })
}

fn safe_environment(command: &mut Command, gh_config: Option<&Path>) {
    let removes: Vec<OsString> = env::vars_os()
        .filter_map(|(key, _)| {
            let key_text = key.to_string_lossy();
            (key_text.starts_with("GIT_")
                || key_text.starts_with("GH_")
                || key_text == "GITHUB_TOKEN"
                || key_text == "GITHUB_ENTERPRISE_TOKEN"
                || key_text == "SSH_ASKPASS"
                || key_text == "GIT_ASKPASS"
                || key_text.eq_ignore_ascii_case("HTTP_PROXY")
                || key_text.eq_ignore_ascii_case("HTTPS_PROXY")
                || key_text.eq_ignore_ascii_case("ALL_PROXY")
                || key_text.eq_ignore_ascii_case("NO_PROXY")
                || key_text.eq_ignore_ascii_case("SSL_CERT_FILE")
                || key_text.eq_ignore_ascii_case("SSL_CERT_DIR"))
            .then_some(key)
        })
        .collect();
    for key in removes {
        command.env_remove(key);
    }
    let overridden: Vec<OsString> = command
        .get_envs()
        .filter(|(key, value)| {
            value.is_some()
                && (key.to_string_lossy().starts_with("GIT_")
                    || key.to_string_lossy().starts_with("GH_")
                    || key.to_string_lossy() == "GITHUB_TOKEN"
                    || key.to_string_lossy() == "GITHUB_ENTERPRISE_TOKEN"
                    || key.to_string_lossy() == "SSH_ASKPASS"
                    || key.to_string_lossy() == "GIT_ASKPASS"
                    || key.to_string_lossy().eq_ignore_ascii_case("HTTP_PROXY")
                    || key.to_string_lossy().eq_ignore_ascii_case("HTTPS_PROXY")
                    || key.to_string_lossy().eq_ignore_ascii_case("ALL_PROXY")
                    || key.to_string_lossy().eq_ignore_ascii_case("NO_PROXY")
                    || key.to_string_lossy().eq_ignore_ascii_case("SSL_CERT_FILE")
                    || key.to_string_lossy().eq_ignore_ascii_case("SSL_CERT_DIR"))
        })
        .map(|(key, _)| key.to_os_string())
        .collect();
    for key in overridden {
        command.env_remove(key);
    }
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_SSH_VARIANT", "ssh")
        .env("GH_HOST", "github.com")
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env("PATH", SYSTEM_PATH);
    if let Some(path) = gh_config {
        command.env("GH_CONFIG_DIR", path);
    }
}

fn trusted_binary(path: &str, name: &str) -> Result<String> {
    trust::trusted_system_binary(path, name).map_err(error)
}
fn git_command(args: &[String]) -> Result<Vec<String>> {
    trusted_binary(GH_BINARY, "GitHub CLI")?;
    let ssh = trusted_binary(SSH_BINARY, "SSH")?;
    let mut result = vec![
        trusted_binary(GIT_BINARY, "Git")?,
        "-c".into(),
        "core.hooksPath=/dev/null".into(),
        "-c".into(),
        format!("core.sshCommand={ssh} -F /dev/null"),
        "-c".into(),
        "core.gitProxy=none".into(),
        "-c".into(),
        "core.askPass=".into(),
        "-c".into(),
        "core.fsmonitor=false".into(),
        "-c".into(),
        "core.pager=cat".into(),
        "-c".into(),
        "credential.helper=".into(),
        "-c".into(),
        "credential.https://github.com.helper=!/usr/bin/gh auth git-credential".into(),
        "-c".into(),
        "diff.external=".into(),
        "-c".into(),
        "submodule.recurse=false".into(),
        "-c".into(),
        "fetch.recurseSubmodules=false".into(),
        "-c".into(),
        "push.recurseSubmodules=no".into(),
        "-c".into(),
        "protocol.ext.allow=never".into(),
        "-c".into(),
        "protocol.file.allow=never".into(),
        "-c".into(),
        "protocol.git.allow=never".into(),
        "-c".into(),
        "http.sslVerify=true".into(),
        "-c".into(),
        "remote.origin.uploadpack=git-upload-pack".into(),
        "-c".into(),
        "remote.origin.receivepack=git-receive-pack".into(),
        "-c".into(),
        "remote.origin.proxy=".into(),
    ];
    if args.iter().any(|arg| arg.is_empty()) {
        return Err(error("外部commandを安全に構成できません"));
    }
    result.extend(args.iter().cloned());
    Ok(result)
}

struct Captured {
    status: std::process::ExitStatus,
    stdout: String,
    stderr: String,
}
fn run(command: &[String], cwd: &Path, timeout: Duration, max_output: usize) -> Result<Captured> {
    run_with_config(command, cwd, timeout, max_output, None)
}
fn run_with_config(
    command: &[String],
    cwd: &Path,
    timeout: Duration,
    max_output: usize,
    gh_config: Option<&Path>,
) -> Result<Captured> {
    if command.is_empty() || command.iter().any(String::is_empty) {
        return Err(error("外部commandを安全に構成できません"));
    }
    let git_command = command.first().is_some_and(|value| value == GIT_BINARY);
    if git_command {
        validate_local_git_config(cwd)?;
    }
    let automatic_sandbox = if git_command && gh_config.is_none() {
        let sandbox = GhSandbox::create(cwd)?;
        sandbox.snapshot_args(&[])?;
        Some(sandbox)
    } else {
        None
    };
    let gh_config = gh_config.or_else(|| automatic_sandbox.as_ref().map(|v| v.path.as_path()));
    let timeout = remaining(timeout)?;
    let mut command_builder = Command::new(&command[0]);
    command_builder.args(&command[1..]).current_dir(cwd);
    safe_environment(&mut command_builder, gh_config);
    let output = process::run_with_limit(&mut command_builder, timeout, max_output)
        .map_err(|_| error("外部commandの実行またはcaptureに失敗しました"))?;
    Ok(Captured {
        status: output.status,
        stdout: String::from_utf8(output.stdout)
            .map_err(|_| error("外部commandの応答がUTF-8ではありません"))?,
        stderr: String::from_utf8(output.stderr)
            .map_err(|_| error("外部commandのerror応答がUTF-8ではありません"))?,
    })
}
fn git(cwd: &Path, args: &[&str], check: bool) -> Result<String> {
    let args: Vec<String> = args.iter().map(|v| (*v).to_string()).collect();
    let command = git_command(&args)?;
    let result = run(
        &command,
        cwd,
        Duration::from_secs(COMMAND_TIMEOUT),
        MAX_OUTPUT_BYTES,
    )?;
    if check && !result.status.success() {
        return Err(error("Git commandに失敗しました"));
    }
    Ok(result.stdout)
}
fn gh(cwd: &Path, args: &[String], check: bool) -> Result<String> {
    let sandbox = GhSandbox::create(cwd)?;
    let args = sandbox.snapshot_args(args)?;
    let mut command = vec![trusted_binary(GH_BINARY, "GitHub CLI")?];
    command.extend(args);
    let result = run_with_config(
        &command,
        cwd,
        Duration::from_secs(COMMAND_TIMEOUT),
        MAX_OUTPUT_BYTES,
        Some(&sandbox.path),
    )?;
    if check && !result.status.success() {
        return Err(error("GitHub CLI commandに失敗しました"));
    }
    Ok(result.stdout)
}

fn dangerous_local_git_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key == "include.path"
        || key.starts_with("includeif.")
        || key == "core.sshcommand"
        || key == "core.gitproxy"
        || key == "core.fsmonitor"
        || key == "core.askpass"
        || key == "core.pager"
        || key == "diff.external"
        || key.starts_with("credential.") && key.ends_with(".helper")
        || key == "submodule.recurse"
        || key == "fetch.recursesubmodules"
        || key == "push.recursesubmodules"
        || key == "http.proxy"
        || key == "http.sslcainfo"
        || key == "http.sslverify"
        || key.starts_with("http.")
            && (key.ends_with(".proxy")
                || key.ends_with(".extraheader")
                || key.ends_with(".proxycommand")
                || key.ends_with(".sslcainfo")
                || key.ends_with(".sslverify")
                || key.ends_with(".sslcert")
                || key.ends_with(".sslkey"))
        || key.starts_with("pager.")
        || key.starts_with("filter.")
            && (key.ends_with(".process")
                || key.ends_with(".clean")
                || key.ends_with(".smudge")
                || key.ends_with(".required"))
        || key.starts_with("merge.") && key.ends_with(".driver")
        || key.starts_with("remote.")
            && (key.ends_with(".proxy")
                || key.ends_with(".uploadpack")
                || key.ends_with(".receivepack")
                || key.ends_with(".pushurl"))
        || key.starts_with("url.")
            && (key.ends_with(".insteadof") || key.ends_with(".pushinsteadof"))
        || key.starts_with("protocol.") && key.ends_with(".allow")
}

fn validate_local_git_config(cwd: &Path) -> Result<()> {
    let git = trusted_binary(GIT_BINARY, "Git")?;
    let mut command = Command::new(git);
    command
        .current_dir(cwd)
        .arg("config")
        .arg("--local")
        .arg("--null")
        .arg("--name-only")
        .arg("--get-regexp")
        .arg(".*")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    safe_environment(&mut command, None);
    let output = process::run_with_limit(
        &mut command,
        remaining(Duration::from_secs(COMMAND_TIMEOUT))?,
        MAX_OUTPUT_BYTES,
    )
    .map_err(|_| error("local Git configの検査に失敗しました"))?;
    // `git config --local` exits 1 when no key matches. Every other
    // non-success status is an unreadable local config.
    if !output.status.success() && output.status.code() != Some(1) {
        return Err(error("local Git configを安全に検査できません"));
    }
    let text = String::from_utf8(output.stdout)
        .map_err(|_| error("local Git configの応答がUTF-8ではありません"))?;
    if text
        .split('\0')
        .filter(|key| !key.is_empty())
        .any(dangerous_local_git_key)
    {
        return Err(error(
            "local Git configに外部実行またはtransport変更設定があります",
        ));
    }
    Ok(())
}
fn parse_json(text: &str, label: &str) -> Result<Value> {
    if text.is_empty() || text.len() > MAX_OUTPUT_BYTES {
        return Err(error(format!("{label}の応答が空または大きすぎます")));
    }
    serde_json::from_str(text).map_err(|_| error(format!("{label}のJSONを安全に解析できません")))
}
fn gh_json(cwd: &Path, args: &[String]) -> Result<Value> {
    parse_json(&gh(cwd, args, true)?, "GitHub")
}

fn sha256(value: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(value.as_ref()))
}

fn chunked_hex(value: &Value, bytes: usize, label: &str) -> Result<String> {
    let chunks = value
        .as_array()
        .ok_or_else(|| error(format!("{label}はhex chunk配列である必要があります")))?;
    if chunks.len() != bytes / 4
        || chunks.iter().any(|chunk| {
            !chunk.as_str().is_some_and(|value| {
                value.len() == 8
                    && value.bytes().all(|byte| byte.is_ascii_hexdigit())
                    && value == value.to_ascii_lowercase()
            })
        })
    {
        return Err(error(format!("{label}のhex chunkが不正です")));
    }
    Ok(chunks.iter().filter_map(Value::as_str).collect::<String>())
}

fn non_empty_string_array(value: &Value, label: &str) -> Result<()> {
    let values = value
        .as_array()
        .ok_or_else(|| error(format!("{label}は配列である必要があります")))?;
    if values.is_empty()
        || values.len() > MAX_ITEMS
        || values.iter().any(|value| {
            !value
                .as_str()
                .is_some_and(|text| !text.is_empty() && !text.contains(['\n', '\r']))
        })
    {
        return Err(error(format!("{label}の値が不正です")));
    }
    Ok(())
}

fn ledger_json(body: &str, marker: &str) -> Result<Value> {
    if body.matches(marker).count() != 1 {
        return Err(error("loop ledger markerが一意ではありません"));
    }
    let prefix = format!("{marker}\n");
    let json = body
        .strip_prefix(&prefix)
        .ok_or_else(|| error("loop ledger markerはcomment先頭の独立行に必要です"))?;
    parse_json(json, "loop ledger")
}

fn finding_fingerprint(finding: &Value) -> Result<String> {
    let invariant = str_value(finding, "invariant_id")?;
    let path = str_value(finding, "cause_path")?;
    let failure_class = str_value(finding, "failure_class")?;
    if invariant.is_empty()
        || failure_class.is_empty()
        || path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
        || [invariant.as_str(), failure_class.as_str(), path.as_str()]
            .iter()
            .any(|value| value.contains(['\n', '\r']))
    {
        return Err(error("finding fingerprint preimageが不正です"));
    }
    let canonical = object([
        ("cause_path", string(path)),
        ("failure_class", string(failure_class)),
        ("invariant_id", string(invariant)),
    ]);
    let bytes = serde_json::to_vec(&canonical)
        .map_err(|_| error("finding fingerprintをcanonical serializeできません"))?;
    Ok(sha256(bytes))
}

#[derive(Debug, Clone)]
struct LedgerFinding {
    immutable_digest: String,
    status: String,
    attempt: i64,
    first_head: String,
}

#[derive(Debug)]
struct LedgerCheckpoint {
    schema: i64,
    plan_version: i64,
    round: i64,
    head_before: String,
    head_after: String,
    predecessor_id: Option<i64>,
    predecessor_digest: Option<String>,
    findings: BTreeMap<String, LedgerFinding>,
}

fn finding_immutable_digest(finding: &Value) -> Result<String> {
    let canonical = object([
        ("cause_path", string(str_value(finding, "cause_path")?)),
        (
            "failure_class",
            string(str_value(finding, "failure_class")?),
        ),
        (
            "first_head",
            string(chunked_hex(get(finding, "first_head")?, 20, "first head")?),
        ),
        ("impact", string(str_value(finding, "impact")?)),
        ("invariant_id", string(str_value(finding, "invariant_id")?)),
        (
            "post_fix_condition",
            string(str_value(finding, "post_fix_condition")?),
        ),
        ("reproduction", string(str_value(finding, "reproduction")?)),
        ("severity", string(str_value(finding, "severity")?)),
    ]);
    let bytes = serde_json::to_vec(&canonical)
        .map_err(|_| error("finding identityをcanonical serializeできません"))?;
    Ok(sha256(bytes))
}

fn failure_signature(signature: &Value) -> Result<String> {
    expected_keys(
        signature,
        &[
            "id",
            "operation",
            "target",
            "error_class",
            "input_digest",
            "external_state_digest",
        ],
        "failure signature",
    )?;
    let operation = str_value(signature, "operation")?;
    let target = str_value(signature, "target")?;
    let error_class = str_value(signature, "error_class")?;
    if [operation.as_str(), target.as_str(), error_class.as_str()]
        .iter()
        .any(|value| value.is_empty() || value.contains(['\n', '\r']))
    {
        return Err(error("failure signature preimageが不正です"));
    }
    let canonical = object([
        ("error_class", string(error_class)),
        (
            "external_state_digest",
            string(chunked_hex(
                get(signature, "external_state_digest")?,
                32,
                "external state digest",
            )?),
        ),
        (
            "input_digest",
            string(chunked_hex(
                get(signature, "input_digest")?,
                32,
                "input digest",
            )?),
        ),
        ("operation", string(operation)),
        ("target", string(target)),
    ]);
    let bytes = serde_json::to_vec(&canonical)
        .map_err(|_| error("failure signatureをcanonical serializeできません"))?;
    Ok(sha256(bytes))
}

fn task_from_parts(payload: &Value) -> Result<String> {
    let task_parts = get(payload, "task_id_parts")?
        .as_array()
        .ok_or_else(|| error("loop ledger task ID partsが不正です"))?;
    if task_parts.len() < 2
        || task_parts.len() > 8
        || task_parts.iter().any(|part| {
            !part.as_str().is_some_and(|text| {
                !text.is_empty()
                    && text
                        .bytes()
                        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
            })
        })
    {
        return Err(error("loop ledger task ID partsが不正です"));
    }
    Ok(task_parts
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join("-"))
}

fn validate_loop_ledger_payload(
    payload: &Value,
    task: &str,
    plan: &str,
    repository: &str,
    pr: i64,
    expected_head: Option<&str>,
    require_resolved: bool,
) -> Result<LedgerCheckpoint> {
    expected_keys(
        payload,
        &[
            "schema",
            "task_id_parts",
            "plan_id",
            "plan_version",
            "repository",
            "pr",
            "round",
            "head_before",
            "head_after",
            "previous",
            "findings",
            "failure_signatures",
            "progress_events",
            "diagnostic",
        ],
        "loop ledger",
    )?;
    let schema = int_value(payload, "schema")?;
    let head_after = oid(
        &chunked_hex(get(payload, "head_after")?, 20, "head after")?,
        "head",
    )?;
    if ![2, 3].contains(&schema)
        || task_from_parts(payload)? != task
        || str_value(payload, "plan_id")? != plan
        || int_value(payload, "plan_version")? < 1
        || str_value(payload, "repository")? != repository
        || int_value(payload, "pr")? != pr
        || int_value(payload, "round")? < 1
        || expected_head.is_some_and(|head| head_after != head)
    {
        return Err(error("loop ledger identity/headが一致しません"));
    }
    let head_before = oid(
        &chunked_hex(get(payload, "head_before")?, 20, "head before")?,
        "head before",
    )?;

    let findings = get(payload, "findings")?
        .as_array()
        .ok_or_else(|| error("loop ledger findingsが不正です"))?;
    if findings.len() > MAX_ITEMS {
        return Err(error("loop ledger findingsが多すぎます"));
    }
    let mut normalized_findings = BTreeMap::new();
    for finding in findings {
        expected_keys(
            finding,
            &[
                "fingerprint",
                "invariant_id",
                "cause_path",
                "failure_class",
                "first_head",
                "severity",
                "status",
                "attempt",
                "reproduction",
                "impact",
                "post_fix_condition",
                "tests",
                "evidence",
            ],
            "loop ledger finding",
        )?;
        let stored_fingerprint =
            chunked_hex(get(finding, "fingerprint")?, 32, "finding fingerprint")?;
        if stored_fingerprint != finding_fingerprint(finding)? {
            return Err(error(
                "finding fingerprintとcanonical preimageが一致しません",
            ));
        }
        let first_head = oid(
            &chunked_hex(get(finding, "first_head")?, 20, "finding first head")?,
            "finding first head",
        )?;
        for key in [
            "invariant_id",
            "cause_path",
            "failure_class",
            "reproduction",
            "impact",
            "post_fix_condition",
        ] {
            if str_value(finding, key)?.is_empty() {
                return Err(error(format!("loop ledger findingの{key}が空です")));
            }
        }
        let status = str_value(finding, "status")?;
        if !["low", "medium", "high", "critical"]
            .contains(&str_value(finding, "severity")?.as_str())
            || !["new", "recurring", "resolved", "false_positive", "blocked"]
                .contains(&status.as_str())
            || (require_resolved && !["resolved", "false_positive"].contains(&status.as_str()))
            || int_value(finding, "attempt")? < 0
        {
            return Err(error(
                "loop ledger findingに未解消または不正な状態があります",
            ));
        }
        non_empty_string_array(get(finding, "tests")?, "finding tests")?;
        non_empty_string_array(get(finding, "evidence")?, "finding evidence")?;
        if normalized_findings
            .insert(
                stored_fingerprint,
                LedgerFinding {
                    immutable_digest: finding_immutable_digest(finding)?,
                    status,
                    attempt: int_value(finding, "attempt")?,
                    first_head,
                },
            )
            .is_some()
        {
            return Err(error("loop ledger finding fingerprintが重複しています"));
        }
    }
    let signatures = get(payload, "failure_signatures")?
        .as_array()
        .ok_or_else(|| error("loop ledger failure signaturesが不正です"))?;
    if signatures.len() > MAX_ITEMS {
        return Err(error("loop ledger failure signaturesが多すぎます"));
    }
    for signature in signatures {
        if schema == 2 {
            let _ = chunked_hex(signature, 32, "legacy failure signature")?;
        } else {
            let stored = chunked_hex(get(signature, "id")?, 32, "failure signature ID")?;
            if stored != failure_signature(signature)? {
                return Err(error("failure signatureとcanonical preimageが一致しません"));
            }
        }
    }
    if schema == 2 {
        non_empty_string_array(get(payload, "progress_events")?, "legacy progress events")?;
    } else {
        let events = get(payload, "progress_events")?
            .as_array()
            .ok_or_else(|| error("progress eventsが不正です"))?;
        if events.len() > MAX_ITEMS {
            return Err(error("progress eventsが多すぎます"));
        }
        for event in events {
            expected_keys(event, &["kind", "finding", "evidence"], "progress event")?;
            if ![
                "finding_resolved",
                "test_transition",
                "evidence_narrowed",
                "false_positive",
            ]
            .contains(&str_value(event, "kind")?.as_str())
            {
                return Err(error("progress event kindが不正です"));
            }
            let fingerprint = chunked_hex(get(event, "finding")?, 32, "progress finding")?;
            if !normalized_findings.contains_key(&fingerprint) {
                return Err(error("progress eventのfindingがledgerにありません"));
            }
            non_empty_string_array(get(event, "evidence")?, "progress evidence")?;
        }
    }

    let diagnostic = get(payload, "diagnostic")?;
    if schema == 2 {
        expected_keys(
            diagnostic,
            &["used", "max_tool_calls", "deadline_minutes", "outcome"],
            "legacy loop ledger diagnostic",
        )?;
        let _ = bool_value(diagnostic, "used")?;
        if !(1..=100).contains(&int_value(diagnostic, "max_tool_calls")?)
            || !(1..=120).contains(&int_value(diagnostic, "deadline_minutes")?)
            || str_value(diagnostic, "outcome")?.is_empty()
        {
            return Err(error("legacy loop ledger diagnosticが不正です"));
        }
    } else {
        expected_keys(
            diagnostic,
            &[
                "used",
                "budget_source",
                "max_tool_calls",
                "tool_calls_used",
                "deadline_minutes",
                "outcome",
            ],
            "loop ledger diagnostic",
        )?;
        let used = bool_value(diagnostic, "used")?;
        let calls = int_value(diagnostic, "tool_calls_used")?;
        if !["rollout_budget", "runtime", "policy"]
            .contains(&str_value(diagnostic, "budget_source")?.as_str())
            || int_value(diagnostic, "max_tool_calls")? != 12
            || int_value(diagnostic, "deadline_minutes")? != 30
            || !(0..=12).contains(&calls)
            || used != (calls > 0)
            || str_value(diagnostic, "outcome")?.is_empty()
        {
            return Err(error("loop ledger diagnostic予算または結果が不正です"));
        }
    }

    let previous = get(payload, "previous")?;
    expected_keys(
        previous,
        &["comment_id", "body_sha256"],
        "loop ledger predecessor",
    )?;
    let predecessor = int_value(previous, "comment_id")?;
    if predecessor < 1 {
        return Err(error("loop ledger predecessor IDが不正です"));
    }
    Ok(LedgerCheckpoint {
        schema,
        plan_version: int_value(payload, "plan_version")?,
        round: int_value(payload, "round")?,
        head_before,
        head_after,
        predecessor_id: Some(predecessor),
        predecessor_digest: Some(chunked_hex(
            get(previous, "body_sha256")?,
            32,
            "predecessor digest",
        )?),
        findings: normalized_findings,
    })
}

fn validate_v1_bootstrap(
    body: &str,
    task: &str,
    plan: &str,
    repository: &str,
    pr: i64,
) -> Result<LedgerCheckpoint> {
    let suffix = body
        .strip_prefix(&format!("{LOOP_LEDGER_V1_MARKER}\n```json\n"))
        .and_then(|value| value.trim_end().strip_suffix("```"))
        .ok_or_else(|| error("v1 loop ledger bootstrap形式が不正です"))?;
    let payload = parse_json(suffix.trim_end(), "v1 loop ledger bootstrap")?;
    let task_parts = get(&payload, "task_id_parts")?
        .as_array()
        .ok_or_else(|| error("v1 loop ledger task IDが不正です"))?;
    let reconstructed = task_parts
        .iter()
        .map(|part| {
            part.as_str()
                .ok_or_else(|| error("v1 loop ledger task IDが不正です"))
        })
        .collect::<Result<Vec<_>>>()?
        .join("-");
    if int_value(&payload, "schema_version")? != 1
        || reconstructed != task
        || str_value(&payload, "plan_id")? != plan
        || int_value(&payload, "plan_version")? < 1
        || str_value(&payload, "repository")? != repository
        || int_value(&payload, "pr")? != pr
        || int_value(&payload, "round")? != 1
    {
        return Err(error("v1 loop ledger bootstrap identityが不正です"));
    }
    if let Some(findings) = payload.get("findings") {
        let findings = findings
            .as_array()
            .ok_or_else(|| error("v1 loop ledger findingsが不正です"))?;
        if findings.len() > MAX_ITEMS {
            return Err(error("v1 loop ledger findingsが多すぎます"));
        }
        for finding in findings {
            let _ = chunked_hex(get(finding, "id")?, 32, "v1 finding ID")?;
            if str_value(finding, "invariant_id")?.is_empty()
                || !["resolved", "false_positive"].contains(&str_value(finding, "status")?.as_str())
                || int_value(finding, "attempt")? < 0
                || str_value(finding, "evidence")?.is_empty()
            {
                return Err(error(
                    "v1 bootstrapはterminal findingだけを移行履歴として許可します",
                ));
            }
        }
    }
    Ok(LedgerCheckpoint {
        schema: 1,
        plan_version: int_value(&payload, "plan_version")?,
        round: 1,
        head_before: oid(
            &chunked_hex(get(&payload, "head_before")?, 20, "v1 head before")?,
            "v1 head before",
        )?,
        head_after: oid(
            &chunked_hex(get(&payload, "head_after")?, 20, "v1 head after")?,
            "v1 head after",
        )?,
        predecessor_id: None,
        predecessor_digest: None,
        findings: BTreeMap::new(),
    })
}

fn validate_finding_transition(
    previous: &LedgerCheckpoint,
    current: &LedgerCheckpoint,
) -> Result<()> {
    for (fingerprint, old) in &previous.findings {
        let new = current
            .findings
            .get(fingerprint)
            .ok_or_else(|| error("loop ledgerで過去findingが欠落しています"))?;
        if old.immutable_digest != new.immutable_digest
            || old.first_head != new.first_head
            || old.attempt > new.attempt
            || (["resolved", "false_positive"].contains(&old.status.as_str())
                && old.status != new.status)
            || (old.status == "blocked"
                && !["blocked", "resolved", "false_positive"].contains(&new.status.as_str()))
            || (old.status == "recurring" && new.status == "new")
        {
            return Err(error(
                "loop ledger findingのidentityまたは状態遷移が不正です",
            ));
        }
    }
    Ok(())
}

fn validate_loop_ledger_comments(
    pages: &Value,
    current_login: &str,
    task: &str,
    plan: &str,
    repository: &str,
    pr: i64,
    head: &str,
) -> Result<(i64, String, HashSet<String>, i64)> {
    let pages = pages
        .as_array()
        .ok_or_else(|| error("PR comment pagination応答が不正です"))?;
    if pages.len() > MAX_PAGES {
        return Err(error("PR comment pagination上限を超えました"));
    }
    let mut comments = Vec::new();
    for page in pages {
        let values = page
            .as_array()
            .ok_or_else(|| error("PR comment pageが不正です"))?;
        comments.extend(values.iter());
        if comments.len() > MAX_ITEMS {
            return Err(error("PR commentが多すぎます"));
        }
    }
    let mut ledgers: Vec<_> = comments
        .into_iter()
        .filter(|comment| {
            comment
                .get("body")
                .and_then(Value::as_str)
                .is_some_and(|body| {
                    body.starts_with(LOOP_LEDGER_V1_MARKER)
                        || body.starts_with(LOOP_LEDGER_V2_MARKER)
                })
        })
        .collect();
    ledgers.sort_by_key(|comment| comment.get("id").and_then(Value::as_i64).unwrap_or(0));
    if ledgers.is_empty() {
        return Err(error("loop ledger commentがありません"));
    }
    for comment in &ledgers {
        if comment.get("id").and_then(Value::as_i64).is_none()
            || comment.get("created_at").and_then(Value::as_str).is_none()
            || comment.get("created_at") != comment.get("updated_at")
            || !comment
                .get("user")
                .and_then(|value| value.get("login"))
                .and_then(Value::as_str)
                .is_some_and(|login| login.eq_ignore_ascii_case(current_login))
        {
            return Err(error("loop ledger author/time/comment IDが不正です"));
        }
    }
    let mut previous: Option<(i64, &str, LedgerCheckpoint)> = None;
    let mut reachable_heads = HashSet::new();
    for (index, comment) in ledgers.iter().enumerate() {
        let id = comment
            .get("id")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        let body = comment
            .get("body")
            .and_then(Value::as_str)
            .ok_or_else(|| error("loop ledger bodyが不正です"))?;
        let is_latest = index + 1 == ledgers.len();
        let checkpoint = if index == 0 {
            if !body.starts_with(&format!("{LOOP_LEDGER_V1_MARKER}\n")) {
                return Err(error(
                    "loop ledger chainはv1 bootstrapから開始する必要があります",
                ));
            }
            validate_v1_bootstrap(body, task, plan, repository, pr)?
        } else {
            if !body.starts_with(&format!("{LOOP_LEDGER_V2_MARKER}\n")) {
                return Err(error(
                    "v1 bootstrapはloop ledger chainの先頭だけに許可されます",
                ));
            }
            let payload = ledger_json(body, LOOP_LEDGER_V2_MARKER)?;
            let checkpoint = validate_loop_ledger_payload(
                &payload,
                task,
                plan,
                repository,
                pr,
                is_latest.then_some(head),
                is_latest,
            )?;
            if is_latest && checkpoint.schema != 3 {
                return Err(error("最新loop ledgerはschema 3である必要があります"));
            }
            checkpoint
        };
        if let Some((previous_id, previous_body, previous_checkpoint)) = &previous {
            if checkpoint.predecessor_id != Some(*previous_id)
                || checkpoint.predecessor_digest.as_deref() != Some(&sha256(previous_body))
                || checkpoint.round != previous_checkpoint.round + 1
                || checkpoint.head_before != previous_checkpoint.head_after
                || checkpoint.plan_version < previous_checkpoint.plan_version
                || (previous_checkpoint.schema == 3 && checkpoint.schema != 3)
            {
                return Err(error(
                    "loop ledger predecessor/round/head/schema/Plan version chainが一致しません",
                ));
            }
            validate_finding_transition(previous_checkpoint, &checkpoint)?;
        }
        reachable_heads.insert(checkpoint.head_before.clone());
        reachable_heads.insert(checkpoint.head_after.clone());
        reachable_heads.extend(
            checkpoint
                .findings
                .values()
                .map(|finding| finding.first_head.clone()),
        );
        previous = Some((id, body, checkpoint));
    }
    let latest_checkpoint = &previous
        .as_ref()
        .ok_or_else(|| error("loop ledger latest checkpointがありません"))?
        .2;
    if latest_checkpoint.schema != 3 || latest_checkpoint.head_after != head {
        return Err(error(
            "最新loop ledgerはcurrent headに一致するschema 3である必要があります",
        ));
    }
    let latest = *ledgers.last().unwrap();
    let latest_body = latest
        .get("body")
        .and_then(Value::as_str)
        .unwrap_or_default();
    Ok((
        latest.get("id").and_then(Value::as_i64).unwrap_or_default(),
        sha256(latest_body),
        reachable_heads,
        latest_checkpoint.plan_version,
    ))
}

fn loop_ledger(
    root: &Path,
    task: &str,
    plan: &str,
    repository: &str,
    pr: i64,
    head: &str,
    expected_plan_version: i64,
) -> Result<(i64, String, i64)> {
    let comments = gh_json(
        root,
        &[
            "api".into(),
            format!("repos/{repository}/issues/{pr}/comments?per_page=100"),
            "--paginate".into(),
            "--slurp".into(),
            "--header".into(),
            "Accept: application/vnd.github+json".into(),
        ],
    )?;
    let user = gh_json(root, &["api".into(), "user".into()])?;
    let login = str_value(&user, "login")?;
    let (comment_id, digest, reachable_heads, plan_version) =
        validate_loop_ledger_comments(&comments, &login, task, plan, repository, pr, head)?;
    if plan_version != expected_plan_version {
        return Err(error("最新loop ledgerのPlan versionが指定値と一致しません"));
    }
    for candidate in reachable_heads {
        git(
            root,
            &["cat-file", "-e", &format!("{candidate}^{{commit}}")],
            true,
        )?;
        git(
            root,
            &["merge-base", "--is-ancestor", &candidate, head],
            true,
        )?;
    }
    Ok((comment_id, digest, plan_version))
}

fn current_user_home() -> Result<PathBuf> {
    #[cfg(unix)]
    let candidate = unsafe {
        let entry = libc::getpwuid(libc::getuid());
        if entry.is_null() {
            None
        } else {
            CStr::from_ptr((*entry).pw_dir)
                .to_str()
                .ok()
                .map(PathBuf::from)
        }
    };
    #[cfg(not(unix))]
    let candidate = env::var_os("HOME").map(PathBuf::from);
    let candidate =
        candidate.ok_or_else(|| error("current userのhome directoryを確認できません"))?;
    validate_absolute_path(&candidate, "home directory")
}

fn validate_absolute_path(path: &Path, label: &str) -> Result<PathBuf> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(error(format!("{label}は安全な絶対pathにしてください")));
    }
    Ok(path.to_path_buf())
}

fn resolve_codex_home(configured: Option<PathBuf>, home: PathBuf) -> Result<PathBuf> {
    let candidate = configured
        .map(|path| validate_absolute_path(&path, "CODEX_HOME"))
        .transpose()?
        .unwrap_or_else(|| home.join(".codex"));
    if !candidate.is_absolute()
        || candidate
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(error("CODEX_HOMEは安全な絶対pathにしてください"));
    }
    let mut current = PathBuf::from("/");
    for component in candidate.components().skip(1) {
        current.push(component.as_os_str());
        if let Ok(metadata) = fs::symlink_metadata(&current) {
            if metadata.file_type().is_symlink() {
                return Err(error("CODEX_HOMEのsymlink componentを拒否しました"));
            }
            if current != candidate && !metadata.is_dir() {
                return Err(error("CODEX_HOMEの親がdirectoryではありません"));
            }
        } else if current != candidate {
            return Err(error("CODEX_HOMEを安全に検査できません"));
        }
    }
    Ok(candidate)
}
fn codex_home() -> Result<PathBuf> {
    resolve_codex_home(
        env::var_os("CODEX_HOME").map(PathBuf::from),
        current_user_home()?,
    )
}
fn safe_directory(path: &Path) -> Result<()> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| error("managed directoryを確認できません"))?;
    #[cfg(unix)]
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.uid() != unsafe { libc::getuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err(error("managed directoryがprivateではありません"));
    }
    #[cfg(not(unix))]
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(error("managed directoryが安全ではありません"));
    }
    Ok(())
}
fn managed_root(repository: &str) -> Result<PathBuf> {
    let root = codex_home()?.join("worktrees").join(repo_key(repository)?);
    safe_directory(
        root.parent()
            .and_then(Path::parent)
            .ok_or_else(|| error("managed rootが不正です"))?,
    )?;
    safe_directory(
        root.parent()
            .ok_or_else(|| error("managed rootが不正です"))?,
    )?;
    safe_directory(&root)?;
    Ok(root)
}
fn state_root(repository: &str) -> Result<PathBuf> {
    let path = managed_root(repository)?.join(".state");
    safe_directory(&path)?;
    Ok(path)
}
fn snapshot_path(repository: &str, task: &str) -> Result<PathBuf> {
    let path = state_root(repository)?.join(format!("{task}.json"));
    task_id(task)?;
    Ok(path)
}
fn state_path(repository: &str, task: &str) -> Result<PathBuf> {
    let path = state_root(repository)?.join(format!("{task}.delivery.json"));
    task_id(task)?;
    Ok(path)
}
fn receipt_path(repository: &str, task: &str, head: &str) -> Result<PathBuf> {
    let path = state_root(repository)?.join(format!("{task}.receipt.{}.json", oid(head, "SHA")?));
    task_id(task)?;
    Ok(path)
}

fn open_private_regular(path: &Path, label: &str) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let file = options
        .open(path)
        .map_err(|_| error(format!("{label}を安全に開けません")))?;
    let metadata = file
        .metadata()
        .map_err(|_| error(format!("{label}を安全に検査できません")))?;
    #[cfg(unix)]
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.uid() != unsafe { libc::getuid() }
        || metadata.mode() & 0o077 != 0
        || metadata.len() > MAX_FILE_BYTES
    {
        return Err(error(format!(
            "{label}が安全なprivate regular fileではありません"
        )));
    }
    #[cfg(not(unix))]
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
        return Err(error(format!(
            "{label}が安全なprivate regular fileではありません"
        )));
    }
    Ok(file)
}

fn read_private_bytes(path: &Path, label: &str) -> Result<Vec<u8>> {
    let mut file = open_private_regular(path, label)?;
    let before = file
        .metadata()
        .map_err(|_| error(format!("{label}を安全に検査できません")))?;
    let mut bytes = Vec::with_capacity(before.len().min(MAX_FILE_BYTES) as usize);
    file.read_to_end(&mut bytes)
        .map_err(|_| error(format!("{label}を安全に読み込めません")))?;
    let after = file
        .metadata()
        .map_err(|_| error(format!("{label}を安全に検査できません")))?;
    if before.len() != after.len() || bytes.len() as u64 != after.len() {
        return Err(error(format!("{label}の読み込み中に内容が変化しました")));
    }
    Ok(bytes)
}

fn read_private_text(path: &Path, label: &str) -> Result<String> {
    let bytes = read_private_bytes(path, label)?;
    String::from_utf8(bytes).map_err(|_| error(format!("{label}がUTF-8ではありません")))
}

fn read_json_file(path: &Path, label: &str) -> Result<Value> {
    let text = read_private_text(path, label)?;
    parse_json(&text, label)
}

#[cfg(unix)]
fn sync_directory(path: &Path, label: &str) -> Result<()> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let directory = options
        .open(path)
        .map_err(|_| error(format!("{label}のparent directoryを開けません")))?;
    directory.sync_all().map_err(|_| {
        error(format!(
            "{label}のparent directoryをdurableに保存できません"
        ))
    })
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path, _label: &str) -> Result<()> {
    Ok(())
}

fn private_temp_parent() -> Result<PathBuf> {
    #[cfg(unix)]
    let path = PathBuf::from("/tmp");
    #[cfg(not(unix))]
    let path = env::temp_dir();
    let metadata = fs::symlink_metadata(&path)
        .map_err(|_| error("private temporary directoryを確認できません"))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(error("private temporary directoryが安全ではありません"));
    }
    Ok(path)
}

fn gh_config_candidates() -> Result<Vec<PathBuf>> {
    let mut candidates = Vec::new();
    if let Some(value) = env::var_os("GH_CONFIG_DIR")
        && let Ok(path) = validate_absolute_path(&PathBuf::from(value), "GH_CONFIG_DIR")
    {
        candidates.push(path);
    }
    if let Some(value) = env::var_os("XDG_CONFIG_HOME")
        && let Ok(path) = validate_absolute_path(&PathBuf::from(value), "XDG_CONFIG_HOME")
    {
        candidates.push(path.join("gh"));
    }
    candidates.push(current_user_home()?.join(".config/gh"));
    Ok(candidates)
}

fn gh_auth_hosts() -> Result<Option<Vec<u8>>> {
    for directory in gh_config_candidates()? {
        let source = directory.join("hosts.yml");
        if source.exists() {
            return read_private_bytes(&source, "GitHub auth hosts.yml").map(Some);
        }
    }
    Ok(None)
}

struct GhSandbox {
    path: PathBuf,
}

impl GhSandbox {
    fn create(_cwd: &Path) -> Result<Self> {
        // Keep the sandbox in the OS temporary directory rather than under a
        // repository-controlled path. `cwd` is retained in the signature so
        // Git and GitHub invocations share one construction contract.
        let parent = private_temp_parent()?;
        let suffix = format!(
            "codex-delivery-gh-{}-{}",
            std::process::id(),
            now().replace(|c: char| !c.is_ascii_digit(), "")
        );
        let mut path = parent.join(suffix);
        let mut created = false;
        for attempt in 0..8 {
            if attempt > 0 {
                path = parent.join(format!(
                    "{}-{attempt}",
                    path.file_name()
                        .and_then(|v| v.to_str())
                        .unwrap_or("sandbox")
                ));
            }
            match fs::create_dir(&path) {
                Ok(()) => {
                    created = true;
                    break;
                }
                Err(cause) if cause.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(_) => break,
            }
        }
        if !created {
            return Err(error(
                "GitHub CLIのprivate config directoryを作成できません",
            ));
        }
        let sandbox = Self { path };
        #[cfg(unix)]
        fs::set_permissions(&sandbox.path, fs::Permissions::from_mode(0o700))
            .map_err(|_| error("GitHub CLIのprivate config directoryを保護できません"))?;
        if let Some(hosts) = gh_auth_hosts()? {
            sandbox.write_auth_hosts(&hosts)?;
        }
        sync_directory(&sandbox.path, "GitHub CLI private config")?;
        Ok(sandbox)
    }

    fn write_private(&self, name: &str, bytes: &[u8], label: &str) -> Result<PathBuf> {
        let path = self.path.join(name);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            options.mode(0o400);
            options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        }
        let mut file = options
            .open(&path)
            .map_err(|_| error(format!("{label}のprivate snapshotを作成できません")))?;
        file.write_all(bytes)
            .map_err(|_| error(format!("{label}のprivate snapshotを作成できません")))?;
        file.sync_all().map_err(|_| {
            error(format!(
                "{label}のprivate snapshotをdurableに保存できません"
            ))
        })?;
        #[cfg(unix)]
        file.set_permissions(fs::Permissions::from_mode(0o400))
            .map_err(|_| error(format!("{label}のprivate snapshotを固定できません")))?;
        Ok(path)
    }

    fn write_auth_hosts(&self, bytes: &[u8]) -> Result<PathBuf> {
        let path = self.write_private("hosts.yml", bytes, "GitHub auth hosts.yml")?;
        #[cfg(unix)]
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .map_err(|_| error("GitHub auth hosts.ymlのprivate snapshotを保護できません"))?;
        Ok(path)
    }

    fn snapshot_args(&self, args: &[String]) -> Result<Vec<String>> {
        let mut result = Vec::with_capacity(args.len());
        let mut index = 0;
        while index < args.len() {
            let argument = &args[index];
            if argument == "--body-file" {
                let source = args
                    .get(index + 1)
                    .ok_or_else(|| error("GitHub CLI body-fileのpathが不足しています"))?;
                let bytes = read_private_bytes(Path::new(source), "GitHub CLI body-file")?;
                let snapshot_name = format!("body-file-{index}");
                let snapshot =
                    self.write_private(&snapshot_name, &bytes, "GitHub CLI body-file")?;
                result.push(argument.clone());
                result.push(snapshot.display().to_string());
                index += 2;
                continue;
            }
            if let Some(source) = argument.strip_prefix("--body-file=") {
                if source.is_empty() {
                    return Err(error("GitHub CLI body-fileのpathが不足しています"));
                }
                let bytes = read_private_bytes(Path::new(source), "GitHub CLI body-file")?;
                let snapshot_name = format!("body-file-{index}");
                let snapshot =
                    self.write_private(&snapshot_name, &bytes, "GitHub CLI body-file")?;
                result.push(format!("--body-file={}", snapshot.display()));
            } else {
                result.push(argument.clone());
            }
            index += 1;
        }
        sync_directory(&self.path, "GitHub CLI private snapshot")?;
        Ok(result)
    }
}

impl Drop for GhSandbox {
    fn drop(&mut self) {
        #[cfg(unix)]
        let _ = fs::set_permissions(&self.path, fs::Permissions::from_mode(0o700));
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn atomic_json(path: &Path, value: &Value) -> Result<()> {
    let parent = path
        .parent()
        .filter(|value| value.is_dir() && !value.is_symlink())
        .ok_or_else(|| error("state parentが安全ではありません"))?;
    #[cfg(unix)]
    let parent_handle = {
        let mut options = OpenOptions::new();
        options
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
        options
            .open(parent)
            .map_err(|_| error("state parentをdurableに保存できません"))?
    };
    if path.file_name().is_none() {
        return Err(error("state parentが安全ではありません"));
    }
    let suffix = format!(
        ".tmp.{}.{}",
        std::process::id(),
        now().replace(|c: char| !c.is_ascii_digit(), "")
    );
    let temporary = path.with_file_name(format!(
        ".{}{}",
        path.file_name().and_then(|v| v.to_str()).unwrap_or("state"),
        suffix
    ));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options
            .open(&temporary)
            .map_err(|_| error("receipt/stateをatomic保存できません"))?;
        let bytes = serde_json::to_vec_pretty(value)
            .map_err(|_| error("receipt/stateをJSON化できません"))?;
        file.write_all(&bytes)
            .map_err(|_| error("receipt/stateをatomic保存できません"))?;
        file.write_all(b"\n")
            .map_err(|_| error("receipt/stateをatomic保存できません"))?;
        file.sync_all()
            .map_err(|_| error("receipt/stateをatomic保存できません"))?;
        fs::rename(&temporary, path).map_err(|_| error("receipt/stateをatomic保存できません"))?;
        #[cfg(unix)]
        parent_handle
            .sync_all()
            .map_err(|_| error("receipt/stateのparent directoryをdurableに保存できません"))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

struct TaskLock {
    _file: Option<File>,
    key: (String, String),
}
impl Drop for TaskLock {
    fn drop(&mut self) {
        HELD_LOCKS.with(|locks| {
            locks.borrow_mut().remove(&self.key);
        });
    }
}
fn task_lock(repository: &str, task: &str) -> Result<TaskLock> {
    task_id(task)?;
    let key = (repository.to_ascii_lowercase(), task.to_string());
    let already = HELD_LOCKS.with(|locks| locks.borrow().contains(&key));
    if already {
        return Ok(TaskLock { _file: None, key });
    }
    let lock_root = managed_root(repository)?.join(".locks");
    safe_directory(&lock_root)?;
    let path = lock_root.join("lifecycle.lock");
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        options.mode(0o600);
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options
        .open(path)
        .map_err(|_| error("task lifecycle lockを取得できません"))?;
    #[cfg(unix)]
    {
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|_| error("task lifecycle lockを取得できません"))?;
        let deadline = DEADLINE
            .with(|v| v.get())
            .unwrap_or_else(|| Instant::now() + Duration::from_secs(OPERATION_TIMEOUT));
        loop {
            let result = unsafe {
                libc::flock(
                    std::os::fd::AsRawFd::as_raw_fd(&file),
                    libc::LOCK_EX | libc::LOCK_NB,
                )
            };
            if result == 0 {
                break;
            }
            if Instant::now() >= deadline {
                return Err(error("task lifecycle lockの取得がtimeoutしました"));
            }
            thread::sleep(Duration::from_millis(50));
        }
    }
    HELD_LOCKS.with(|locks| {
        locks.borrow_mut().insert(key.clone());
    });
    Ok(TaskLock {
        _file: Some(file),
        key,
    })
}

fn github_repository(remote: &str) -> Option<String> {
    let value = remote.trim().trim_end_matches('/');
    for prefix in [
        "https://github.com/",
        "ssh://git@github.com/",
        "git@github.com:",
    ] {
        if let Some(rest) = value.strip_prefix(prefix) {
            let candidate = rest.strip_suffix(".git").unwrap_or(rest);
            if repository_name(candidate) {
                return Some(candidate.to_string());
            }
        }
    }
    None
}
fn repository(root: &Path) -> Result<String> {
    let top = git(root, &["rev-parse", "--show-toplevel"], true)?
        .trim()
        .to_string();
    if top.is_empty() || fs::canonicalize(&top).ok() != fs::canonicalize(root).ok() {
        return Err(error("current checkout rootを確認できません"));
    }
    let fetch: Vec<_> = git(root, &["remote", "get-url", "--all", "origin"], true)?
        .lines()
        .map(str::to_string)
        .collect();
    let push: Vec<_> = git(
        root,
        &["remote", "get-url", "--push", "--all", "origin"],
        true,
    )?
    .lines()
    .map(str::to_string)
    .collect();
    if fetch.len() != 1 || push.len() != 1 {
        return Err(error(
            "originのfetch/push URLはそれぞれ1件に限定してください",
        ));
    }
    let fetched = github_repository(&fetch[0])
        .ok_or_else(|| error("originは同一github.com repositoryに固定してください"))?;
    let pushed = github_repository(&push[0])
        .ok_or_else(|| error("originは同一github.com repositoryに固定してください"))?;
    if !fetched.eq_ignore_ascii_case(&pushed) {
        return Err(error("originは同一github.com repositoryに固定してください"));
    }
    Ok(fetched)
}

fn canonical_remote_url(root: &Path, expected_repository: &str) -> Result<String> {
    let current = repository(root)?;
    if !current.eq_ignore_ascii_case(expected_repository) {
        return Err(error("origin repository identityが検査後に変化しました"));
    }
    Ok(format!("https://github.com/{expected_repository}.git"))
}

fn manifest(root: &Path, task: &str, repository_name_value: &str) -> Result<(Value, PathBuf)> {
    let path = snapshot_path(repository_name_value, task)?;
    let payload = read_json_file(&path, "worktree manifest")?;
    expected_keys(
        &payload,
        &[
            "version",
            "status",
            "task_id",
            "repository",
            "common_git_dir",
            "github_name",
            "branch",
            "base",
            "base_oid",
            "worktree",
            "created_at",
            "detail",
        ],
        "worktree manifest",
    )?;
    if int_value(&payload, "version")? != MANIFEST_VERSION
        || str_value(&payload, "status")? != "ready"
        || str_value(&payload, "task_id")? != task
        || str_value(&payload, "repository")? != root.display().to_string()
        || !str_value(&payload, "github_name")?.eq_ignore_ascii_case(repository_name_value)
        || !branch_name(&str_value(&payload, "branch")?)
        || str_value(&payload, "base")? != "origin/main"
        || oid(&str_value(&payload, "base_oid")?, "base SHA").is_err()
        || str_value(&payload, "created_at")?.is_empty()
    {
        return Err(error(
            "worktree manifestの値がcurrent repositoryと一致しません",
        ));
    }
    let managed = managed_root(repository_name_value)?;
    let expected = managed.join(task);
    if str_value(&payload, "worktree")? != expected.display().to_string() {
        return Err(error("manifest worktreeがmanaged root外です"));
    }
    let common = git(root, &["rev-parse", "--git-common-dir"], true)?
        .trim()
        .to_string();
    if common.is_empty()
        || fs::canonicalize(&common).ok()
            != fs::canonicalize(str_value(&payload, "common_git_dir")?).ok()
    {
        return Err(error("manifest common git directoryが一致しません"));
    }
    Ok((payload, expected))
}
fn worktree(root: &Path, manifest: &Value, expected: &Path) -> Result<()> {
    if expected.is_symlink() || !expected.is_dir() {
        return Err(error("managed worktreeが安全なdirectoryではありません"));
    }
    let top = git(expected, &["rev-parse", "--show-toplevel"], true)?
        .trim()
        .to_string();
    let common = git(expected, &["rev-parse", "--git-common-dir"], true)?
        .trim()
        .to_string();
    if fs::canonicalize(&top).ok() != fs::canonicalize(expected).ok()
        || fs::canonicalize(&common).ok()
            != fs::canonicalize(str_value(manifest, "common_git_dir")?).ok()
        || git(expected, &["rev-parse", "--abbrev-ref", "HEAD"], true)?.trim()
            != str_value(manifest, "branch")?
    {
        return Err(error(
            "managed worktreeがmanifest repositoryに属していません",
        ));
    }
    let _ = root;
    Ok(())
}
fn worktree_clean_head(worktree: &Path, expected: &str) -> Result<()> {
    if !git(
        worktree,
        &[
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--ignored=matching",
        ],
        true,
    )?
    .is_empty()
    {
        return Err(error("managed worktreeがcleanではありません"));
    }
    if oid(
        git(worktree, &["rev-parse", "HEAD"], true)?.trim(),
        "managed worktree head",
    )? != oid(expected, "SHA")?
    {
        return Err(error("managed worktree headがreceiptと一致しません"));
    }
    Ok(())
}
fn changed_files(worktree: &Path, base: &str, head: &str) -> Result<Vec<String>> {
    let output = git(
        worktree,
        &["diff", "--name-only", "--no-renames", "-z", base, head],
        true,
    )?;
    let values: Vec<String> = output
        .split('\0')
        .filter(|v| !v.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    if values.len() > MAX_ITEMS || values.iter().any(|v| v.contains(['\n', '\r'])) {
        return Err(error("changed-files応答を安全に解析できません"));
    }
    Ok(values)
}
fn safety_path(path: &str) -> bool {
    path == "AGENTS.md"
        || path == "Cargo.toml"
        || path == "Cargo.lock"
        || [".codex/", ".github/", ".agents/skills/", "packages/cli/"]
            .iter()
            .any(|prefix| path.starts_with(prefix))
        || [".codex/config.base.toml", "packages/cli/src/codex.rs"].contains(&path)
}

fn runtime_readonly_path(path: &str) -> bool {
    path.starts_with(".codex/") || path.starts_with(".agents/")
}

fn sandbox_readonly_sync_failure(stderr: &str, changed: &[String]) -> bool {
    let protected = changed
        .iter()
        .filter(|path| runtime_readonly_path(path))
        .collect::<Vec<_>>();
    !protected.is_empty()
        && stderr.lines().any(|line| {
            let permission_denied = [
                "Permission denied",
                "Read-only file system",
                "Operation not permitted",
            ]
            .iter()
            .any(|message| line.contains(message));
            permission_denied && protected.iter().any(|path| line.contains(path.as_str()))
        })
}

fn validate_main_sync_retry(stage: &str, last_error: &str, sandbox_retry: bool) -> Result<()> {
    if sandbox_retry {
        if stage != "merged" || last_error != MAIN_SYNC_SANDBOX_RETRY_READY {
            return Err(error(
                "sandbox外のfinish再試行は検証済みのmain同期拒否に1回だけ使用できます",
            ));
        }
    } else if stage == "merged"
        && [
            MAIN_SYNC_SANDBOX_RETRY_READY,
            MAIN_SYNC_SANDBOX_RETRY_CONSUMED,
        ]
        .contains(&last_error)
    {
        return Err(error(
            "main同期のsandbox拒否は検証済みです。finishを直接再実行できません",
        ));
    }
    Ok(())
}

fn receipt_decision(value: &Value) -> Result<String> {
    if let Some(raw) = value.get("decision") {
        return decision(
            raw.as_str()
                .ok_or_else(|| error("receipt decisionが不正です"))?,
        )
        .map(str::to_string);
    }
    match value.get("human_approved") {
        Some(v) if v.as_bool().is_some() => Ok(if v.as_bool() == Some(true) {
            "human-approved"
        } else {
            "autonomous"
        }
        .to_string()),
        _ => Err(error("receipt decisionが不足しています")),
    }
}
fn receipt_gate_mode(value: &Value) -> Result<String> {
    gate_mode(
        value
            .get("gate_mode")
            .and_then(Value::as_str)
            .unwrap_or(STRICT_GATE_MODE),
    )
    .map(str::to_string)
}
fn validate_legacy_decision(
    version: i64,
    risk_value: &str,
    decision_value: &str,
    gate: &str,
) -> Result<()> {
    let _ = risk(risk_value)?;
    let _ = decision(decision_value)?;
    if (version == 1 || version == 2)
        && ["high", "critical"].contains(&risk_value)
        && decision_value != "human-approved"
    {
        return Err(error(
            "legacy high/critical receiptにはhuman approvalが必要です",
        ));
    }
    if (version == 1 || version == 2)
        && gate == FREE_PRIVATE_GATE_MODE
        && (!["high", "critical"].contains(&risk_value) || decision_value != "human-approved")
    {
        return Err(error(
            "legacy github-free-private receiptにはhigh/criticalとhuman approvalが必要です",
        ));
    }
    Ok(())
}
fn validate_gate_repository(repository: &str, gate: &str) -> Result<()> {
    gate_mode(gate)?;
    if !repository_name(repository) {
        return Err(error("gate repository名が不正です"));
    }
    Ok(())
}
fn receipt_has_current_plan_version(payload: &Value) -> bool {
    payload.get("version").and_then(Value::as_i64) == Some(RECEIPT_VERSION)
        && payload.get("plan_version").is_some()
}
fn receipt(
    payload: &Value,
    root: &Path,
    task: &str,
    head_value: &str,
    repository_value: Option<&str>,
) -> Result<Value> {
    let object_ref = object_ref(payload)?;
    let common = [
        "version",
        "kind",
        "task_id",
        "repository",
        "pr",
        "head_sha",
        "risk",
        "plan_id",
        "actionable",
        "tests_passed",
        "changed_files",
        "created_at",
    ];
    let version = int_value(payload, "version")?;
    let current_repository = match repository_value {
        Some(value) => value.to_owned(),
        None => repository(root)?,
    };
    let mut normalized = object_ref.clone();
    if version == 1 {
        let mut keys = common.to_vec();
        keys.extend([
            "human_approved",
            "neutral_review_passed",
            "adversarial_review_passed",
        ]);
        expected_keys(payload, &keys, "legacy receipt")?;
        if payload
            .get("human_approved")
            .and_then(Value::as_bool)
            .is_none()
        {
            return Err(error("legacy receipt approvalが不正です"));
        }
        normalized.insert("gate_mode".into(), string(STRICT_GATE_MODE));
        normalized.insert("decision".into(), string(receipt_decision(payload)?));
        normalized.remove("human_approved");
    } else if version == 2 {
        let mut keys = common.to_vec();
        keys.extend([
            "human_approved",
            "gate_mode",
            "neutral_review_passed",
            "adversarial_review_passed",
        ]);
        expected_keys(payload, &keys, "legacy receipt")?;
        if payload
            .get("human_approved")
            .and_then(Value::as_bool)
            .is_none()
        {
            return Err(error("legacy receipt approvalが不正です"));
        }
        normalized.insert("decision".into(), string(receipt_decision(payload)?));
        normalized.remove("human_approved");
    } else if version == 3 {
        let mut keys = common.to_vec();
        keys.extend([
            "decision",
            "gate_mode",
            "neutral_review_passed",
            "adversarial_review_passed",
        ]);
        expected_keys(payload, &keys, "legacy receipt")?;
    } else if version == 4 {
        let mut keys = common.to_vec();
        keys.extend([
            "decision",
            "gate_mode",
            "independent_review_passed",
            "specialist_review_passed",
        ]);
        expected_keys(payload, &keys, "legacy receipt")?;
    } else if version == RECEIPT_VERSION {
        let mut keys = common.to_vec();
        keys.extend([
            "decision",
            "gate_mode",
            "independent_review_passed",
            "specialist_review_passed",
            "ledger_comment_id",
            "ledger_body_sha256",
        ]);
        if receipt_has_current_plan_version(payload) {
            keys.push("plan_version");
            expected_keys(payload, &keys, "receipt")?;
        } else {
            expected_keys(payload, &keys, "legacy v5 receipt")?;
        }
    } else {
        return Err(error("receipt schemaが一致しません"));
    }
    let normalized = Value::Object(normalized);
    if str_value(&normalized, "kind")? != "review"
        || str_value(&normalized, "task_id")? != task
        || str_value(&normalized, "repository")? != current_repository
    {
        return Err(error("receipt task/repositoryが一致しません"));
    }
    let gate = receipt_gate_mode(&normalized)?;
    validate_gate_repository(&current_repository, &gate)?;
    let pr = int_value(&normalized, "pr")?;
    if !(1..=999_999_999).contains(&pr) {
        return Err(error("receipt PR番号が不正です"));
    }
    if str_value(&normalized, "head_sha")? != head_value {
        return Err(error("receipt headが不正です"));
    }
    let risk_value = risk(&str_value(&normalized, "risk")?)?.to_string();
    let decision_value = receipt_decision(&normalized)?;
    if gate == FREE_PRIVATE_GATE_MODE && !["high", "critical"].contains(&risk_value.as_str()) {
        return Err(error(
            "github-free-private receiptにはhigh/critical riskが必要です",
        ));
    }
    if !plan_id(&str_value(&normalized, "plan_id")?) {
        return Err(error("receipt Plan IDが不正です"));
    }
    let review_evidence_valid = if version <= 3 {
        bool_value(&normalized, "neutral_review_passed")?
            && bool_value(&normalized, "adversarial_review_passed")?
    } else {
        let specialist_required = ["high", "critical"].contains(&risk_value.as_str());
        bool_value(&normalized, "independent_review_passed")?
            && bool_value(&normalized, "specialist_review_passed")? == specialist_required
    };
    if int_value(&normalized, "actionable")? != 0
        || !bool_value(&normalized, "tests_passed")?
        || !review_evidence_valid
    {
        return Err(error("receiptの必須検証flagが不足しています"));
    }
    if version == RECEIPT_VERSION
        && (int_value(&normalized, "ledger_comment_id")? < 1
            || receipt_has_current_plan_version(&normalized)
                && int_value(&normalized, "plan_version")? < 1
            || oid(
                &str_value(&normalized, "ledger_body_sha256")?,
                "ledger digest",
            )?
            .len()
                != 64)
    {
        return Err(error("receiptのloop ledger証拠が不正です"));
    }
    let changed = get(&normalized, "changed_files")?
        .as_array()
        .ok_or_else(|| error("receipt changed-filesが不正です"))?;
    if changed.len() > MAX_ITEMS || changed.iter().any(|v| v.as_str().is_none()) {
        return Err(error("receipt changed-filesの型が不正です"));
    }
    validate_legacy_decision(version, &risk_value, &decision_value, &gate)?;
    Ok(normalized)
}
fn load_receipt(
    root: &Path,
    task: &str,
    head: &str,
    repository_value: Option<&str>,
) -> Result<Value> {
    let repository = match repository_value {
        Some(value) => value.to_owned(),
        None => repository(root)?,
    };
    let path = receipt_path(&repository, task, head)?;
    let payload = read_json_file(&path, "receipt")?;
    receipt(&payload, root, task, &oid(head, "SHA")?, Some(&repository))
}
fn validate_review_evidence_flags(
    risk_value: &str,
    tests: bool,
    independent: bool,
    specialist: bool,
) -> Result<()> {
    let specialist_required = ["high", "critical"].contains(&risk(risk_value)?);
    if !tests || !independent || specialist != specialist_required {
        return Err(error(
            "tests/independent reviewとriskに応じたspecialist reviewの完了flagが必要です",
        ));
    }
    Ok(())
}
fn review_receipt_scope_matches(existing: &Value, proposed: &Value) -> bool {
    let mut keys = vec![
        "kind",
        "task_id",
        "repository",
        "pr",
        "head_sha",
        "plan_id",
        "actionable",
        "tests_passed",
        "changed_files",
        "gate_mode",
    ];
    if receipt_has_current_plan_version(existing) {
        keys.extend(["ledger_comment_id", "ledger_body_sha256", "plan_version"]);
    }
    keys.iter()
        .all(|key| existing.get(*key) == proposed.get(*key))
}
fn review_request_is_idempotent(
    existing_risk: &str,
    requested_risk: &str,
    existing_decision: &str,
    approved: bool,
) -> bool {
    let requested_decision = if approved {
        "human-approved"
    } else {
        "autonomous"
    };
    existing_risk == requested_risk && existing_decision == requested_decision
}
#[allow(clippy::too_many_arguments)]
fn write_review_locked(
    root: &Path,
    task: &str,
    pr: i64,
    head_value: &str,
    risk_value: &str,
    plan: &str,
    plan_version: i64,
    approved: bool,
    tests: bool,
    independent: bool,
    specialist: bool,
    gate: &str,
) -> Result<Value> {
    gate_mode(gate)?;
    risk(risk_value)?;
    if !(1..=999_999_999).contains(&pr) || !plan_id(plan) || !(1..=999_999).contains(&plan_version)
    {
        return Err(error(
            "PR番号、Plan ID、またはPlan versionが安全ではありません",
        ));
    }
    validate_review_evidence_flags(risk_value, tests, independent, specialist)?;
    let repository = repository(root)?;
    validate_gate_repository(&repository, gate)?;
    let (manifest_value, worktree_path) = manifest(root, task, &repository)?;
    worktree(root, &manifest_value, &worktree_path)?;
    let current = oid(
        git(&worktree_path, &["rev-parse", "HEAD"], true)?.trim(),
        "current head",
    )?;
    let head = oid(head_value, "SHA")?;
    if current != head {
        return Err(error("指定headがworktreeのcurrent HEADと一致しません"));
    }
    worktree_clean_head(&worktree_path, &head)?;
    let changed = changed_files(
        &worktree_path,
        &str_value(&manifest_value, "base_oid")?,
        &head,
    )?;
    if changed.iter().any(|v| safety_path(v)) && !["high", "critical"].contains(&risk_value) {
        return Err(error("安全境界差分にはhigh/critical riskが必要です"));
    }
    if gate == FREE_PRIVATE_GATE_MODE && !["high", "critical"].contains(&risk_value) {
        return Err(error(
            "github-free-private receiptにはhigh/critical riskが必要です",
        ));
    }
    let (ledger_comment_id, ledger_body_sha256, plan_version) =
        loop_ledger(root, task, plan, &repository, pr, &head, plan_version)?;
    let payload = object([
        ("version", Value::Number(RECEIPT_VERSION.into())),
        ("kind", string("review")),
        ("task_id", string(task)),
        ("repository", string(&repository)),
        ("pr", Value::Number(pr.into())),
        ("head_sha", string(&head)),
        ("risk", string(risk_value)),
        ("plan_id", string(plan)),
        ("actionable", Value::Number(0.into())),
        (
            "decision",
            string(if approved {
                "human-approved"
            } else {
                "autonomous"
            }),
        ),
        ("tests_passed", Value::Bool(true)),
        ("independent_review_passed", Value::Bool(true)),
        ("specialist_review_passed", Value::Bool(specialist)),
        (
            "changed_files",
            Value::Array(changed.into_iter().map(Value::String).collect()),
        ),
        ("created_at", string(now())),
        ("gate_mode", string(gate)),
        ("ledger_comment_id", Value::Number(ledger_comment_id.into())),
        ("ledger_body_sha256", string(ledger_body_sha256)),
        ("plan_version", Value::Number(plan_version.into())),
    ]);
    let path = receipt_path(&repository, task, &head)?;
    if path.exists() || path.is_symlink() {
        let existing = load_receipt(root, task, &head, Some(&repository))?;
        if !review_receipt_scope_matches(&existing, &payload) {
            return Err(error(
                "同じheadのreceiptのfield/mode/Plan/evidenceが異なります",
            ));
        }
        let old_risk = str_value(&existing, "risk")?;
        let old_decision = receipt_decision(&existing)?;
        if risk_level(risk_value) < risk_level(&old_risk)
            || (!approved && old_decision == "human-approved")
        {
            return Err(error("同じheadのreceiptをdowngradeできません"));
        }
        if receipt_has_current_plan_version(&existing)
            && review_request_is_idempotent(&old_risk, risk_value, &old_decision, approved)
        {
            return Ok(existing);
        }
        atomic_json(&path, &payload)?;
        Ok(payload)
    } else {
        atomic_json(&path, &payload)?;
        Ok(payload)
    }
}
#[allow(clippy::too_many_arguments)]
fn write_review(
    root: &Path,
    task: &str,
    pr: i64,
    head: &str,
    risk_value: &str,
    plan: &str,
    plan_version: i64,
    approved: bool,
    tests: bool,
    independent: bool,
    specialist: bool,
    gate: &str,
) -> Result<Value> {
    task_id(task)?;
    let repository = repository(root)?;
    let _lock = task_lock(&repository, task)?;
    write_review_locked(
        root,
        task,
        pr,
        head,
        risk_value,
        plan,
        plan_version,
        approved,
        tests,
        independent,
        specialist,
        gate,
    )
}

fn pr_view(root: &Path, repository: &str, pr: i64) -> Result<Value> {
    let args = vec!["pr".into(), "view".into(), pr.to_string(), "--repo".into(), repository.into(), "--json".into(), "number,state,isDraft,headRefOid,headRefName,baseRefName,mergeable,mergeStateStatus,mergedAt,headRepository,headRepositoryOwner,isCrossRepository,autoMergeRequest".into()];
    let value = gh_json(root, &args)?;
    let object = object_ref(&value)?;
    for key in [
        "number",
        "state",
        "isDraft",
        "headRefOid",
        "headRefName",
        "baseRefName",
        "mergeable",
        "mergeStateStatus",
        "mergedAt",
        "headRepository",
        "headRepositoryOwner",
        "isCrossRepository",
        "autoMergeRequest",
    ] {
        if !object.contains_key(key) {
            return Err(error("PR応答の必須fieldが不足しています"));
        }
    }
    if int_value(&value, "number")? != pr || value.get("isDraft").and_then(Value::as_bool).is_none()
    {
        return Err(error("PR番号またはdraft stateが不正です"));
    }
    Ok(value)
}
fn default_branch(root: &Path, repository: &str) -> Result<()> {
    let args = vec![
        "api".into(),
        format!("repos/{repository}"),
        "--header".into(),
        "Accept: application/vnd.github+json".into(),
    ];
    if str_value(&gh_json(root, &args)?, "default_branch")? != "main" {
        return Err(error("live repositoryのdefault branchがmainではありません"));
    }
    Ok(())
}
fn free_private_repository(root: &Path, repository: &str) -> Result<()> {
    validate_gate_repository(repository, FREE_PRIVATE_GATE_MODE)?;
    let args = vec![
        "api".into(),
        format!("repos/{repository}"),
        "--header".into(),
        "Accept: application/vnd.github+json".into(),
    ];
    let value = gh_json(root, &args)?;
    let expected: [(&str, Value); 8] = [
        ("full_name", string(repository)),
        ("private", Value::Bool(true)),
        ("visibility", string("private")),
        ("default_branch", string("main")),
        ("archived", Value::Bool(false)),
        ("disabled", Value::Bool(false)),
        ("allow_merge_commit", Value::Bool(true)),
        ("allow_auto_merge", Value::Bool(false)),
    ];
    for (key, expected_value) in expected {
        if value.get(key) != Some(&expected_value) {
            return Err(error(
                "github-free-privateのlive repository policyが一致しません",
            ));
        }
    }
    Ok(())
}
fn same_head_repository(view: &Value, repository: &str) -> bool {
    let source = view
        .get("headRepository")
        .and_then(|v| v.get("nameWithOwner"))
        .and_then(Value::as_str);
    let owner = view
        .get("headRepositoryOwner")
        .and_then(|v| v.get("login"))
        .and_then(Value::as_str);
    let owner_name = repository.split('/').next().unwrap_or("");
    source.is_some_and(|v| v.eq_ignore_ascii_case(repository))
        && owner.is_some_and(|v| v.eq_ignore_ascii_case(owner_name))
}

fn check_required_ci(root: &Path, repository: &str, head: &str) -> Result<()> {
    let args = vec![
        "api".into(),
        format!("repos/{repository}/commits/{head}/check-runs"),
        "--paginate".into(),
        "--slurp".into(),
        "--header".into(),
        "Accept: application/vnd.github+json".into(),
    ];
    let value = gh_json(root, &args)?;
    validate_check_runs(&value, head)
}
fn validate_check_runs(value: &Value, head: &str) -> Result<()> {
    let pages = value
        .as_array()
        .ok_or_else(|| error("check-runs pagination応答が不正です"))?;
    if pages.len() > MAX_PAGES {
        return Err(error("check-runs pagination応答が不正です"));
    }
    let mut runs = Vec::new();
    for page in pages {
        let items = page
            .get("check_runs")
            .and_then(Value::as_array)
            .ok_or_else(|| error("check-runs pageが不正です"))?;
        runs.extend(items.iter());
        if runs.len() > MAX_ITEMS {
            return Err(error("check-runsが多すぎます"));
        }
    }
    let candidates: Vec<_> = runs
        .into_iter()
        .filter(|run| run.get("name").and_then(Value::as_str) == Some("required-ci"))
        .collect();
    if candidates.len() != 1 {
        return Err(error(
            "required-ci checkは同名重複なしの1件である必要があります",
        ));
    }
    let run = candidates[0];
    if run
        .get("app")
        .and_then(|v| v.get("id"))
        .and_then(Value::as_i64)
        != Some(15368)
        || run.get("head_sha").and_then(Value::as_str) != Some(head)
        || run.get("status").and_then(Value::as_str) != Some("completed")
        || run.get("conclusion").and_then(Value::as_str) != Some("success")
        || run.get("completed_at").and_then(Value::as_str).is_none()
    {
        return Err(error(
            "required-ciのapp/status/conclusion/headが安全条件に一致しません",
        ));
    }
    Ok(())
}
fn graphql(
    root: &Path,
    repository: &str,
    query: &str,
    variables: &[(&str, Option<String>)],
) -> Result<Value> {
    let (owner, name) = repository
        .split_once('/')
        .ok_or_else(|| error("repositoryが不正です"))?;
    let mut args = vec![
        "api".into(),
        "graphql".into(),
        "-f".into(),
        format!("query={query}"),
        "-F".into(),
        format!("owner={owner}"),
        "-F".into(),
        format!("name={name}"),
    ];
    for (key, value) in variables {
        if let Some(value) = value {
            args.push(if value.parse::<i64>().is_ok() {
                "-F".into()
            } else {
                "-f".into()
            });
            args.push(format!("{key}={value}"));
        }
    }
    let data = gh_json(root, &args)?;
    if data.get("errors").is_some() || !data.get("data").is_some_and(Value::is_object) {
        return Err(error("GraphQL応答にerrorがあります"));
    }
    Ok(data["data"].clone())
}
fn review_safety(root: &Path, repository: &str, pr: i64) -> Result<()> {
    let mut cursor: Option<String> = None;
    let mut pages = 0;
    let mut items = 0;
    loop {
        let data = graphql(
            root,
            repository,
            THREAD_QUERY,
            &[("number", Some(pr.to_string())), ("cursor", cursor.clone())],
        )?;
        let connection = data
            .get("repository")
            .and_then(|v| v.get("pullRequest"))
            .and_then(|v| v.get("reviewThreads"))
            .ok_or_else(|| error("review threadのGraphQL schemaが不正です"))?;
        let nodes = connection
            .get("nodes")
            .and_then(Value::as_array)
            .ok_or_else(|| error("review threadのpaginationが不正です"))?;
        let info = connection
            .get("pageInfo")
            .and_then(Value::as_object)
            .ok_or_else(|| error("review threadのpaginationが不正です"))?;
        pages += 1;
        items += nodes.len();
        if pages > MAX_PAGES || items > MAX_ITEMS {
            return Err(error("review threadのpagination上限を超えました"));
        }
        if nodes
            .iter()
            .any(|node| node.get("isResolved") != Some(&Value::Bool(true)))
        {
            return Err(error("未解決review threadがあります"));
        }
        if info.get("hasNextPage") == Some(&Value::Bool(false)) {
            break;
        }
        let next = info
            .get("endCursor")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| error("review thread pagination cursorが不正です"))?;
        if cursor.as_deref() == Some(next) {
            return Err(error("review thread pagination cursorが不正です"));
        }
        cursor = Some(next.to_string());
    }
    cursor = None;
    pages = 0;
    items = 0;
    let mut effective: HashMap<String, (String, String)> = HashMap::new();
    loop {
        let data = graphql(
            root,
            repository,
            REVIEW_QUERY,
            &[("number", Some(pr.to_string())), ("cursor", cursor.clone())],
        )?;
        let connection = data
            .get("repository")
            .and_then(|v| v.get("pullRequest"))
            .and_then(|v| v.get("reviews"))
            .ok_or_else(|| error("reviewのGraphQL schemaが不正です"))?;
        let nodes = connection
            .get("nodes")
            .and_then(Value::as_array)
            .ok_or_else(|| error("reviewのpaginationが不正です"))?;
        let info = connection
            .get("pageInfo")
            .and_then(Value::as_object)
            .ok_or_else(|| error("reviewのpaginationが不正です"))?;
        pages += 1;
        items += nodes.len();
        if pages > MAX_PAGES || items > MAX_ITEMS {
            return Err(error("reviewのpagination上限を超えました"));
        }
        for node in nodes {
            let state = node
                .get("state")
                .and_then(Value::as_str)
                .ok_or_else(|| error("review stateが不明または未確定です"))?;
            if !["APPROVED", "CHANGES_REQUESTED", "COMMENTED", "DISMISSED"].contains(&state) {
                return Err(error("review stateが不明または未確定です"));
            }
            let author = node
                .get("author")
                .and_then(Value::as_object)
                .ok_or_else(|| error("review author/submittedAtが不正です"))?;
            let login = author
                .get("login")
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
                .ok_or_else(|| error("review author/submittedAtが不正です"))?;
            let submitted = node
                .get("submittedAt")
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
                .ok_or_else(|| error("review author/submittedAtが不正です"))?;
            if state == "COMMENTED" {
                continue;
            }
            let key = login.to_ascii_lowercase();
            if let Some((previous_time, previous_state)) = effective.get(&key) {
                if previous_time == submitted && previous_state != state {
                    return Err(error("同一reviewerの有効review順序を確定できません"));
                }
                if submitted > previous_time.as_str() {
                    effective.insert(key, (submitted.to_string(), state.to_string()));
                }
            } else {
                effective.insert(key, (submitted.to_string(), state.to_string()));
            }
        }
        if info.get("hasNextPage") == Some(&Value::Bool(false)) {
            break;
        }
        let next = info
            .get("endCursor")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| error("review pagination cursorが不正です"))?;
        if cursor.as_deref() == Some(next) {
            return Err(error("review pagination cursorが不正です"));
        }
        cursor = Some(next.to_string());
    }
    if effective
        .values()
        .any(|(_, state)| state == "CHANGES_REQUESTED")
    {
        return Err(error("現在有効なCHANGES_REQUESTED reviewがあります"));
    }
    let data = graphql(
        root,
        repository,
        DECISION_QUERY,
        &[("number", Some(pr.to_string()))],
    )?;
    let decision_value = data
        .get("repository")
        .and_then(|v| v.get("pullRequest"))
        .and_then(|v| v.get("reviewDecision"))
        .ok_or_else(|| error("review decisionのGraphQL schemaが不正です"))?;
    if !decision_value.is_null() && decision_value.as_str() != Some("APPROVED") {
        return Err(error(
            "現在有効なreview decisionがdeliveryを許可していません",
        ));
    }
    Ok(())
}
fn ruleset(root: &Path, repository: &str) -> Result<()> {
    let args = vec![
        "api".into(),
        format!("repos/{repository}/rulesets?includes_parents=true"),
        "--header".into(),
        "Accept: application/vnd.github+json".into(),
    ];
    let value = gh_json(root, &args)?;
    let items = value
        .as_array()
        .ok_or_else(|| error("Ruleset応答が不正です"))?;
    if items.len() > MAX_ITEMS {
        return Err(error("Ruleset応答が不正です"));
    }
    let mut ids = Vec::new();
    for item in items {
        if item.get("target").and_then(Value::as_str) == Some("branch")
            && item.get("enforcement").and_then(Value::as_str) == Some("active")
        {
            let id = item
                .get("id")
                .and_then(Value::as_i64)
                .filter(|v| *v > 0)
                .ok_or_else(|| error("active Rulesetのnumeric IDが不正です"))?;
            ids.push(id);
        }
    }
    if ids.len() > MAX_RULESETS {
        return Err(error("Ruleset候補が多すぎます"));
    }
    let mut matches = Vec::new();
    for id in ids {
        let detail_args = vec![
            "api".into(),
            format!("repos/{repository}/rulesets/{id}?includes_parents=true"),
            "--header".into(),
            "Accept: application/vnd.github+json".into(),
        ];
        let detail = gh_json(root, &detail_args)?;
        if detail.get("id").and_then(Value::as_i64) != Some(id)
            || !detail
                .get("source")
                .and_then(Value::as_str)
                .is_some_and(|v| v.eq_ignore_ascii_case(repository))
            || detail.get("source_type").and_then(Value::as_str) != Some("Repository")
        {
            return Err(error(
                "Ruleset detailのID/sourceがcurrent repositoryと一致しません",
            ));
        }
        let refs = detail.get("conditions").and_then(|v| v.get("ref_name"));
        if detail.get("target").and_then(Value::as_str) == Some("branch")
            && detail.get("enforcement").and_then(Value::as_str) == Some("active")
            && refs.and_then(|v| v.get("include"))
                == Some(&Value::Array(vec![string("~DEFAULT_BRANCH")]))
            && refs.and_then(|v| v.get("exclude")) == Some(&Value::Array(Vec::new()))
        {
            matches.push(detail);
        }
    }
    if matches.len() != 1 {
        return Err(error(
            "default branch向けactive Rulesetを一意に確認できません",
        ));
    }
    let selected = &matches[0];
    if selected.get("bypass_actors") != Some(&Value::Array(Vec::new()))
        || selected
            .get("current_user_can_bypass")
            .and_then(Value::as_str)
            != Some("never")
    {
        return Err(error("Ruleset bypass actorは許可されません"));
    }
    let rules = selected
        .get("rules")
        .and_then(Value::as_array)
        .ok_or_else(|| error("Ruleset rulesが不正です"))?;
    let mut by_type = HashMap::new();
    for rule in rules {
        let kind = rule
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| error("Ruleset rulesが不正です"))?;
        if by_type.insert(kind, rule).is_some() {
            return Err(error("Rulesetの必須ruleが不足または重複しています"));
        }
    }
    for required in [
        "deletion",
        "non_fast_forward",
        "pull_request",
        "required_status_checks",
    ] {
        if !by_type.contains_key(required) {
            return Err(error("Rulesetの必須ruleが不足または重複しています"));
        }
    }
    let pull = by_type["pull_request"]
        .get("parameters")
        .ok_or_else(|| error("Rulesetがmerge-only/thread resolutionではありません"))?;
    if pull.get("allowed_merge_methods") != Some(&Value::Array(vec![string("merge")]))
        || pull.get("required_review_thread_resolution") != Some(&Value::Bool(true))
    {
        return Err(error("Rulesetがmerge-only/thread resolutionではありません"));
    }
    let checks = by_type["required_status_checks"]
        .get("parameters")
        .ok_or_else(|| error("Ruleset required-ci strict/appが不正です"))?;
    let required = checks
        .get("required_status_checks")
        .and_then(Value::as_array)
        .ok_or_else(|| error("Ruleset required-ci strict/appが不正です"))?;
    if checks.get("strict_required_status_checks_policy") != Some(&Value::Bool(true))
        || required.len() != 1
        || required[0].get("context").and_then(Value::as_str) != Some("required-ci")
        || required[0].get("integration_id").and_then(Value::as_i64) != Some(15368)
    {
        return Err(error("Ruleset required-ci strict/appが不正です"));
    }
    Ok(())
}

fn fetch_main(root: &Path, repository: &str) -> Result<String> {
    let args_vec = vec![
        "fetch".into(),
        canonical_remote_url(root, repository)?,
        "refs/heads/main:refs/remotes/origin/main".into(),
    ];
    if !run(
        &git_command(&args_vec)?,
        root,
        Duration::from_secs(COMMAND_TIMEOUT),
        MAX_OUTPUT_BYTES,
    )?
    .status
    .success()
    {
        return Err(error("origin/mainをfetchできません"));
    }
    oid(
        git(root, &["rev-parse", "refs/remotes/origin/main"], true)?.trim(),
        "origin/main",
    )
}
fn validate_receipt_loop_ledger(root: &Path, receipt_value: &Value) -> Result<()> {
    if int_value(receipt_value, "version")? < RECEIPT_VERSION
        || !receipt_has_current_plan_version(receipt_value)
    {
        return Err(error(
            "legacy receiptは読み取り互換専用です。current headをv5で再reviewしてください",
        ));
    }
    let task = str_value(receipt_value, "task_id")?;
    let plan = str_value(receipt_value, "plan_id")?;
    let repository = str_value(receipt_value, "repository")?;
    let pr = int_value(receipt_value, "pr")?;
    let head = str_value(receipt_value, "head_sha")?;
    let receipt_plan_version = int_value(receipt_value, "plan_version")?;
    let (comment_id, body_sha256, plan_version) = loop_ledger(
        root,
        &task,
        &plan,
        &repository,
        pr,
        &head,
        receipt_plan_version,
    )?;
    if comment_id != int_value(receipt_value, "ledger_comment_id")?
        || body_sha256 != str_value(receipt_value, "ledger_body_sha256")?
        || plan_version != int_value(receipt_value, "plan_version")?
    {
        return Err(error(
            "loop ledgerがreview receipt記録後に欠落または変更されています",
        ));
    }
    Ok(())
}
fn validate_delivery(
    root: &Path,
    receipt_value: &Value,
    allow_draft: bool,
    expected_branch: Option<&str>,
    inspected_base: Option<&mut Vec<String>>,
) -> Result<Value> {
    let repository = str_value(receipt_value, "repository")?;
    let pr = int_value(receipt_value, "pr")?;
    let head = str_value(receipt_value, "head_sha")?;
    let gate = receipt_gate_mode(receipt_value)?;
    validate_gate_repository(&repository, &gate)?;
    let view = pr_view(root, &repository, pr)?;
    let draft = bool_value(&view, "isDraft")?;
    let status_ok = if allow_draft && draft {
        ["DRAFT", "CLEAN"].contains(
            &view
                .get("mergeStateStatus")
                .and_then(Value::as_str)
                .unwrap_or(""),
        )
    } else {
        view.get("mergeStateStatus").and_then(Value::as_str) == Some("CLEAN")
    };
    if str_value(&view, "state")? != "OPEN"
        || (draft && !allow_draft)
        || oid(&str_value(&view, "headRefOid")?, "PR head")? != oid(&head, "SHA")?
        || str_value(&view, "baseRefName")? != "main"
        || expected_branch
            .is_some_and(|v| view.get("headRefName").and_then(Value::as_str) != Some(v))
        || view.get("isCrossRepository") != Some(&Value::Bool(false))
        || !same_head_repository(&view, &repository)
        || !view.get("autoMergeRequest").is_some_and(Value::is_null)
        || view.get("mergeable").and_then(Value::as_str) != Some("MERGEABLE")
        || !status_ok
    {
        return Err(error(
            "PR state/head/base/source/auto-merge/mergeabilityが安全条件に一致しません",
        ));
    }
    default_branch(root, &repository)?;
    validate_receipt_loop_ledger(root, receipt_value)?;
    let live = fetch_main(root, &repository)?;
    if let Some(base) = inspected_base {
        base.push(live.clone());
    }
    let args = ["merge-base", "--is-ancestor", &live, &head];
    let args_vec = args.iter().map(|v| (*v).to_string()).collect::<Vec<_>>();
    if !run(
        &git_command(&args_vec)?,
        root,
        Duration::from_secs(COMMAND_TIMEOUT),
        MAX_OUTPUT_BYTES,
    )?
    .status
    .success()
    {
        return Err(error(
            "live origin/mainがreceipt headのancestorではありません",
        ));
    }
    check_required_ci(root, &repository, &head)?;
    review_safety(root, &repository, pr)?;
    if gate == FREE_PRIVATE_GATE_MODE {
        free_private_repository(root, &repository)?;
    } else {
        ruleset(root, &repository)?;
    }
    Ok(view)
}
fn match_cli_receipt(
    receipt_value: &Value,
    pr: Option<i64>,
    head: Option<&str>,
    plan: Option<&str>,
    plan_version: Option<i64>,
    gate: &str,
) -> Result<()> {
    if receipt_gate_mode(receipt_value)? != gate_mode(gate)? {
        return Err(error("指定gate modeがreceiptと一致しません"));
    }
    if pr.is_some_and(|v| receipt_value.get("pr").and_then(Value::as_i64) != Some(v))
        || head.is_some_and(|v| receipt_value.get("head_sha").and_then(Value::as_str) != Some(v))
        || plan.is_some_and(|v| receipt_value.get("plan_id").and_then(Value::as_str) != Some(v))
        || plan_version
            .is_some_and(|v| receipt_value.get("plan_version").and_then(Value::as_i64) != Some(v))
    {
        return Err(error(
            "指定PR/head/Plan ID/Plan versionがreceiptと一致しません",
        ));
    }
    Ok(())
}
fn validate_receipt_evidence(
    worktree_path: &Path,
    manifest_value: &Value,
    receipt_value: &Value,
) -> Result<()> {
    let repository = str_value(receipt_value, "repository")?;
    let gate = receipt_gate_mode(receipt_value)?;
    validate_gate_repository(&repository, &gate)?;
    let version = int_value(receipt_value, "version").unwrap_or(0);
    let decision_value = receipt_decision(receipt_value)?;
    let risk_value = risk(&str_value(receipt_value, "risk")?)?;
    if version == 1 || version == 2 {
        validate_legacy_decision(version, risk_value, &decision_value, &gate)?;
    }
    if gate == FREE_PRIVATE_GATE_MODE && !["high", "critical"].contains(&risk_value) {
        return Err(error(
            "github-free-private receiptにはhigh/critical riskが必要です",
        ));
    }
    let changed = changed_files(
        worktree_path,
        &str_value(manifest_value, "base_oid")?,
        &str_value(receipt_value, "head_sha")?,
    )?;
    let expected = get(receipt_value, "changed_files")?
        .as_array()
        .ok_or_else(|| error("receipt changed-filesが不正です"))?;
    let expected: Vec<_> = expected.iter().filter_map(Value::as_str).collect();
    if changed.iter().map(String::as_str).collect::<Vec<_>>() != expected {
        return Err(error(
            "receiptのchanged-filesが固定headの差分と一致しません",
        ));
    }
    if ["low", "medium"].contains(&risk_value) && changed.iter().any(|v| safety_path(v)) {
        return Err(error(
            "安全境界差分をlow/medium receiptでdeliveryできません",
        ));
    }
    Ok(())
}
fn save_stage(
    repository: &str,
    task: &str,
    state: &Value,
    stage: &str,
    last_error: &str,
) -> Result<Value> {
    let mut state = object_ref(state)?.clone();
    state.insert("stage".into(), string(stage));
    state.insert("last_error".into(), string(last_error));
    state.insert("updated_at".into(), string(now()));
    let value = Value::Object(state);
    atomic_json(&state_path(repository, task)?, &value)?;
    Ok(value)
}
fn load_state(repository: &str, task: &str, receipt_value: &Value, branch: &str) -> Result<Value> {
    let path = state_path(repository, task)?;
    if !path.exists() {
        return Err(error("delivery stateが存在しません"));
    }
    let state = read_json_file(&path, "delivery state")?;
    expected_keys(
        &state,
        &[
            "version",
            "kind",
            "task_id",
            "repository",
            "pr",
            "head_sha",
            "branch",
            "stage",
            "updated_at",
            "last_error",
        ],
        "delivery state",
    )?;
    if int_value(&state, "version")? != STATE_VERSION
        || str_value(&state, "kind")? != "delivery"
        || str_value(&state, "task_id")? != task
        || str_value(&state, "repository")? != str_value(receipt_value, "repository")?
        || int_value(&state, "pr")? != int_value(receipt_value, "pr")?
        || str_value(&state, "head_sha")? != str_value(receipt_value, "head_sha")?
        || str_value(&state, "branch")? != branch
        || ![
            "merge_started",
            "merged",
            "main_synced",
            "remote_delete_started",
            "remote_deleted",
            "worktree_unlock_started",
            "worktree_removed",
            "completed",
        ]
        .contains(&str_value(&state, "stage")?.as_str())
        || get(&state, "last_error")?.as_str().is_none()
    {
        return Err(error("delivery stateがreceipt/manifestと一致しません"));
    }
    Ok(state)
}
fn worktree_records(root: &Path) -> Result<Vec<HashMap<String, String>>> {
    let output = git(root, &["worktree", "list", "--porcelain", "-z"], true)?;
    let mut records = Vec::new();
    let mut record = HashMap::new();
    for item in output.split('\0') {
        if item.is_empty() {
            if !record.is_empty() {
                records.push(record);
                record = HashMap::new();
            }
            continue;
        }
        let (key, value) = item
            .split_once(' ')
            .ok_or_else(|| error("worktree metadataを安全に解析できません"))?;
        if record.insert(key.to_string(), value.to_string()).is_some() {
            return Err(error("worktree metadataを安全に解析できません"));
        }
    }
    if !record.is_empty() {
        records.push(record);
    }
    if records.len() > MAX_ITEMS {
        return Err(error("worktreeが多すぎます"));
    }
    Ok(records)
}
fn remote_branch(root: &Path, repository: &str, branch: &str) -> Result<Option<String>> {
    let remote = canonical_remote_url(root, repository)?;
    let output = git(
        root,
        &[
            "ls-remote",
            "--heads",
            &remote,
            &format!("refs/heads/{branch}"),
        ],
        true,
    )?;
    let lines: Vec<_> = output.lines().collect();
    if lines.is_empty() {
        return Ok(None);
    }
    if lines.len() != 1 {
        return Err(error("remote branch応答が重複しています"));
    }
    let (sha, refname) = lines[0]
        .split_once('\t')
        .ok_or_else(|| error("remote branch応答を安全に解析できません"))?;
    if refname != format!("refs/heads/{branch}") {
        return Err(error("remote branch応答を安全に解析できません"));
    }
    Ok(Some(oid(sha, "remote branch")?))
}
fn assert_main_clean(root: &Path) -> Result<()> {
    if git(root, &["symbolic-ref", "--short", "HEAD"], true)?.trim() != "main"
        || !git(
            root,
            &["status", "--porcelain=v1", "--untracked-files=all"],
            true,
        )?
        .is_empty()
    {
        return Err(error(
            "人間用checkoutがmainではありませんまたはcleanではありません",
        ));
    }
    Ok(())
}

struct MainSyncCandidate {
    path: String,
    target_blob: String,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    mode: u32,
}

fn recover_interrupted_main_sync_with_hook(
    root: &Path,
    target: &str,
    expected_head: &str,
    before_restore: impl FnOnce(),
) -> Result<()> {
    let status = git(
        root,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
        true,
    )?;
    if status.is_empty() {
        return Ok(());
    }
    #[cfg(not(unix))]
    return Err(error(
        "main同期の中断復旧はatomicなinode検証がないplatformでは実行できません",
    ));
    if git(root, &["symbolic-ref", "--short", "HEAD"], true)?.trim() != "main" {
        return Err(error("main同期の中断状態をmain以外では復旧できません"));
    }
    let paths = status
        .split('\0')
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let bytes = entry.as_bytes();
            if bytes.len() < 4 || bytes[0] != b' ' || bytes[1] != b'M' || bytes[2] != b' ' {
                return Err(error(
                    "main同期の中断状態に未追跡、staged、削除、renameまたはtype変更があります",
                ));
            }
            let path = &entry[3..];
            let candidate = Path::new(path);
            if path.is_empty()
                || candidate.is_absolute()
                || candidate
                    .components()
                    .any(|component| !matches!(component, std::path::Component::Normal(_)))
            {
                return Err(error("main同期の中断pathが安全ではありません"));
            }
            Ok(path.to_string())
        })
        .collect::<Result<Vec<_>>>()?;
    if paths.is_empty() || paths.len() > MAX_MAIN_SYNC_RECOVERY_PATHS {
        return Err(error("main同期の中断path数が許可範囲外です"));
    }

    let mut candidates = Vec::with_capacity(paths.len());
    for path in &paths {
        let mut current = root.to_path_buf();
        let components = Path::new(path).components().collect::<Vec<_>>();
        for (index, component) in components.iter().enumerate() {
            let std::path::Component::Normal(part) = component else {
                return Err(error("main同期の中断pathが安全ではありません"));
            };
            current.push(part);
            let metadata = fs::symlink_metadata(&current)
                .map_err(|_| error("main同期の中断pathを確認できません"))?;
            if index + 1 < components.len() {
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(error("main同期の中断pathにsymlink parentがあります"));
                }
            } else if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(error("main同期の中断pathがregular fileではありません"));
            }
        }

        let listing = git(root, &["ls-tree", "-z", target, "--", path], true)?;
        let listing = listing
            .strip_suffix('\0')
            .filter(|value| !value.contains('\0'))
            .ok_or_else(|| error("main同期target treeを一意に確認できません"))?;
        let (header, listed_path) = listing
            .split_once('\t')
            .ok_or_else(|| error("main同期target treeを解析できません"))?;
        let fields = header.split_whitespace().collect::<Vec<_>>();
        if fields.len() != 3
            || !["100644", "100755"].contains(&fields[0])
            || fields[1] != "blob"
            || oid(fields[2], "main同期target blob").is_err()
            || listed_path != path
        {
            return Err(error("main同期target entryが安全ではありません"));
        }
        let metadata = fs::symlink_metadata(&current)
            .map_err(|_| error("main同期の中断path modeを確認できません"))?;
        #[cfg(unix)]
        if (metadata.mode() & 0o111 != 0) != (fields[0] == "100755") {
            return Err(error("main同期の中断path modeがtargetと一致しません"));
        }
        let working_blob = git(root, &["hash-object", "--no-filters", "--", path], true)?;
        if working_blob.trim() != fields[2] {
            return Err(error(
                "main同期の中断pathにtargetと異なるlocal変更があります",
            ));
        }
        candidates.push(MainSyncCandidate {
            path: path.clone(),
            target_blob: fields[2].to_string(),
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
            #[cfg(unix)]
            mode: metadata.mode(),
        });
    }

    before_restore();
    for (index, candidate) in candidates.iter().enumerate() {
        if git(root, &["symbolic-ref", "--short", "HEAD"], true)?.trim() != "main" {
            return Err(error("main同期の復旧前にcheckout branchが変化しました"));
        }
        if git(root, &["rev-parse", "HEAD"], true)?.trim() != expected_head {
            return Err(error("main同期の復旧前にHEADが変化しました"));
        }
        let expected_status = candidates[index..]
            .iter()
            .map(|entry| format!(" M {}\0", entry.path))
            .collect::<String>();
        if git(
            root,
            &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
            true,
        )? != expected_status
        {
            return Err(error("main同期の復旧前にworking tree状態が変化しました"));
        }
        let current = root.join(&candidate.path);
        let metadata = fs::symlink_metadata(&current)
            .map_err(|_| error("main同期の復旧前にpathが変化しました"))?;
        #[cfg(unix)]
        if metadata.file_type().is_symlink()
            || !metadata.is_file()
            || metadata.dev() != candidate.device
            || metadata.ino() != candidate.inode
            || metadata.mode() != candidate.mode
        {
            return Err(error("main同期の復旧前にinodeまたはmodeが変化しました"));
        }
        if git(
            root,
            &["hash-object", "--no-filters", "--", &candidate.path],
            true,
        )?
        .trim()
            != candidate.target_blob
        {
            return Err(error("main同期の復旧前にfile内容が変化しました"));
        }
        git(
            root,
            &[
                "restore",
                "--source=HEAD",
                "--worktree",
                "--",
                &candidate.path,
            ],
            true,
        )?;
    }
    assert_main_clean(root)
}

fn recover_interrupted_main_sync(root: &Path, target: &str, expected_head: &str) -> Result<()> {
    recover_interrupted_main_sync_with_hook(root, target, expected_head, || {})
}

fn prepare_main_sync(root: &Path, target: &str) -> Result<()> {
    let before = git(root, &["rev-parse", "HEAD"], true)?.trim().to_string();
    let args = ["merge-base", "--is-ancestor", &before, target];
    let av = args
        .iter()
        .map(|value| (*value).to_string())
        .collect::<Vec<_>>();
    if !run(
        &git_command(&av)?,
        root,
        Duration::from_secs(COMMAND_TIMEOUT),
        MAX_OUTPUT_BYTES,
    )?
    .status
    .success()
    {
        return Err(error("人間用mainに未pushのlocal commitがあります"));
    }
    recover_interrupted_main_sync(root, target, &before)
}

fn assert_main_synced(root: &Path, repository: &str) -> Result<()> {
    assert_main_clean(root)?;
    let remote = fetch_main(root, repository)?;
    if oid(
        git(root, &["rev-parse", "HEAD"], true)?.trim(),
        "人間用main head",
    )? != remote
    {
        return Err(error("人間用mainがorigin/mainと一致しません"));
    }
    Ok(())
}
fn assert_merge_base_unchanged(root: &Path, repository: &str, inspected: &[String]) -> Result<()> {
    let last = inspected
        .last()
        .ok_or_else(|| error("merge直前の検査済みbaseがありません"))?;
    if fetch_main(root, repository)? != *last {
        return Err(error(
            "merge直前にorigin/mainが検査済みbaseから変化しました",
        ));
    }
    Ok(())
}

fn deliver_locked(
    root: &Path,
    task: &str,
    expected_pr: i64,
    expected_head: &str,
    expected_plan: &str,
    expected_plan_version: i64,
    expected_gate: &str,
) -> Result<Value> {
    let repository = repository(root)?;
    let (manifest_value, worktree_path) = manifest(root, task, &repository)?;
    let head = oid(expected_head, "指定head")?;
    let receipt_value = load_receipt(root, task, &head, Some(&repository))?;
    match_cli_receipt(
        &receipt_value,
        Some(expected_pr),
        Some(expected_head),
        Some(expected_plan),
        Some(expected_plan_version),
        expected_gate,
    )?;
    if str_value(&receipt_value, "head_sha")? == str_value(&manifest_value, "base_oid")? {
        return Err(error("baseと同じheadはdeliveryできません"));
    }
    let _lock = task_lock(&repository, task)?;
    worktree(root, &manifest_value, &worktree_path)?;
    worktree_clean_head(&worktree_path, &head)?;
    validate_receipt_evidence(&worktree_path, &manifest_value, &receipt_value)?;
    let mut inspected = Vec::new();
    validate_delivery(
        root,
        &receipt_value,
        true,
        Some(&str_value(&manifest_value, "branch")?),
        Some(&mut inspected),
    )?;
    let initial = pr_view(root, &repository, expected_pr)?;
    if bool_value(&initial, "isDraft")? {
        gh(
            root,
            &[
                "pr".into(),
                "ready".into(),
                expected_pr.to_string(),
                "--repo".into(),
                repository.clone(),
            ],
            true,
        )?;
    }
    validate_delivery(
        root,
        &receipt_value,
        false,
        Some(&str_value(&manifest_value, "branch")?),
        Some(&mut inspected),
    )?;
    let latest = pr_view(root, &repository, expected_pr)?;
    if str_value(&latest, "state")? != "OPEN"
        || bool_value(&latest, "isDraft")?
        || oid(&str_value(&latest, "headRefOid")?, "PR head")? != head
        || str_value(&latest, "headRefName")? != str_value(&manifest_value, "branch")?
        || str_value(&latest, "baseRefName")? != "main"
        || latest.get("isCrossRepository") != Some(&Value::Bool(false))
        || !same_head_repository(&latest, &repository)
        || !latest.get("autoMergeRequest").is_some_and(Value::is_null)
        || str_value(&latest, "mergeable")? != "MERGEABLE"
        || str_value(&latest, "mergeStateStatus")? != "CLEAN"
    {
        return Err(error("merge直前にPR identity/mergeabilityが変化しました"));
    }
    worktree(root, &manifest_value, &worktree_path)?;
    worktree_clean_head(&worktree_path, &head)?;
    validate_receipt_evidence(&worktree_path, &manifest_value, &receipt_value)?;
    validate_delivery(
        root,
        &receipt_value,
        false,
        Some(&str_value(&manifest_value, "branch")?),
        Some(&mut inspected),
    )?;
    let path = state_path(&repository, task)?;
    let state = if path.exists() || path.is_symlink() {
        let current = load_state(
            &repository,
            task,
            &receipt_value,
            &str_value(&manifest_value, "branch")?,
        )?;
        if str_value(&current, "stage")? != "merge_started" {
            return Err(error("既存delivery stateがmerge再試行可能ではありません"));
        }
        save_stage(&repository, task, &current, "merge_started", "")?
    } else {
        let state = object([
            ("version", Value::Number(STATE_VERSION.into())),
            ("kind", string("delivery")),
            ("task_id", string(task)),
            ("repository", string(&repository)),
            ("pr", Value::Number(expected_pr.into())),
            ("head_sha", string(&head)),
            ("branch", string(str_value(&manifest_value, "branch")?)),
            ("stage", string("merge_started")),
            ("updated_at", string(now())),
            ("last_error", string("")),
        ]);
        atomic_json(&path, &state)?;
        state
    };
    assert_merge_base_unchanged(root, &repository, &inspected)?;
    gh(
        root,
        &[
            "pr".into(),
            "merge".into(),
            expected_pr.to_string(),
            "--repo".into(),
            repository.clone(),
            "--merge".into(),
            "--match-head-commit".into(),
            head.clone(),
        ],
        true,
    )?;
    let merged = pr_view(root, &repository, expected_pr)?;
    if str_value(&merged, "state")? != "MERGED"
        || oid(&str_value(&merged, "headRefOid")?, "merged PR head")? != head
    {
        return Err(error("PR merge後のstate/headを確認できません"));
    }
    save_stage(&repository, task, &state, "merged", "")
}
#[allow(clippy::too_many_arguments)]
fn finish_locked(
    root: &Path,
    task: &str,
    expected_pr: i64,
    expected_head: &str,
    expected_plan: &str,
    expected_plan_version: i64,
    expected_gate: &str,
    sandbox_retry: bool,
) -> Result<Value> {
    let repository = repository(root)?;
    let (manifest_value, worktree_path) = manifest(root, task, &repository)?;
    let head = oid(expected_head, "指定head")?;
    let receipt_value = load_receipt(root, task, &head, Some(&repository))?;
    match_cli_receipt(
        &receipt_value,
        Some(expected_pr),
        Some(expected_head),
        Some(expected_plan),
        Some(expected_plan_version),
        expected_gate,
    )?;
    let mut state = load_state(
        &repository,
        task,
        &receipt_value,
        &str_value(&manifest_value, "branch")?,
    )?;
    let _lock = task_lock(&repository, task)?;
    let initial_stage = str_value(&state, "stage")?;
    let initial_error = str_value(&state, "last_error")?;
    validate_main_sync_retry(&initial_stage, &initial_error, sandbox_retry)?;
    let view = pr_view(root, &repository, expected_pr)?;
    if str_value(&view, "state")? != "MERGED"
        || oid(&str_value(&view, "headRefOid")?, "merged PR head")? != head
        || str_value(&view, "headRefName")? != str_value(&manifest_value, "branch")?
        || str_value(&view, "baseRefName")? != "main"
        || view.get("isCrossRepository") != Some(&Value::Bool(false))
        || !same_head_repository(&view, &repository)
        || !view.get("autoMergeRequest").is_some_and(Value::is_null)
    {
        return Err(error(
            "PRがreceiptのhead/branch/sourceでmergedされていません",
        ));
    }
    default_branch(root, &repository)?;
    let gate = receipt_gate_mode(&receipt_value)?;
    let stage = str_value(&state, "stage")?;
    if finish_requires_live_ledger(&stage) {
        validate_receipt_loop_ledger(root, &receipt_value)?;
    }
    if ["merge_started", "merged"].contains(&stage.as_str()) {
        worktree(root, &manifest_value, &worktree_path)?;
        worktree_clean_head(&worktree_path, &head)?;
        validate_receipt_evidence(&worktree_path, &manifest_value, &receipt_value)?;
    }
    if stage == "merge_started" {
        check_required_ci(root, &repository, &head)?;
        review_safety(root, &repository, expected_pr)?;
        if gate == FREE_PRIVATE_GATE_MODE {
            free_private_repository(root, &repository)?;
        } else {
            ruleset(root, &repository)?;
        }
        state = save_stage(&repository, task, &state, "merged", "")?;
    } else if gate == FREE_PRIVATE_GATE_MODE && stage != "completed" {
        free_private_repository(root, &repository)?;
    }
    let stage = str_value(&state, "stage")?;
    if stage != "merged" {
        assert_main_clean(root)?;
    }
    if stage == "completed" {
        if remote_branch(root, &repository, &str_value(&manifest_value, "branch")?)?.is_some()
            || worktree_path.exists()
            || worktree_path.is_symlink()
        {
            return Err(error("completed stateのcleanup対象が再出現しました"));
        }
        return Ok(state);
    }
    if stage == "merged" {
        let remote_main = fetch_main(root, &repository)?;
        let args = [
            "merge-base",
            "--is-ancestor",
            &head,
            "refs/remotes/origin/main",
        ];
        let av = args.iter().map(|v| (*v).to_string()).collect::<Vec<_>>();
        if !run(
            &git_command(&av)?,
            root,
            Duration::from_secs(COMMAND_TIMEOUT),
            MAX_OUTPUT_BYTES,
        )?
        .status
        .success()
        {
            return Err(error("origin/mainがmerged headへ到達していません"));
        }
        prepare_main_sync(root, &remote_main)?;
        assert_main_clean(root)?;
        if sandbox_retry {
            state = save_stage(
                &repository,
                task,
                &state,
                "merged",
                MAIN_SYNC_SANDBOX_RETRY_CONSUMED,
            )?;
        }
        let args = ["merge", "--ff-only", "refs/remotes/origin/main"];
        let av = args.iter().map(|v| (*v).to_string()).collect::<Vec<_>>();
        let merge = run(
            &git_command(&av)?,
            root,
            Duration::from_secs(COMMAND_TIMEOUT),
            MAX_OUTPUT_BYTES,
        )?;
        if !merge.status.success() {
            if !sandbox_retry {
                let changed = get(&receipt_value, "changed_files")?
                    .as_array()
                    .ok_or_else(|| error("receipt changed-filesが不正です"))?
                    .iter()
                    .map(|value| {
                        value
                            .as_str()
                            .map(ToOwned::to_owned)
                            .ok_or_else(|| error("receipt changed-filesが不正です"))
                    })
                    .collect::<Result<Vec<_>>>()?;
                if sandbox_readonly_sync_failure(&merge.stderr, &changed) {
                    save_stage(
                        &repository,
                        task,
                        &state,
                        "merged",
                        MAIN_SYNC_SANDBOX_RETRY_READY,
                    )?;
                    return Err(error(
                        "runtimeのread-only拒否を確認しました。--sandbox-retry付きfinishをsandbox外で1回だけ再試行できます",
                    ));
                }
            }
            return Err(error("人間用mainをff-only syncできません"));
        }
        assert_main_synced(root, &repository)?;
        state = save_stage(&repository, task, &state, "main_synced", "")?;
    } else {
        assert_main_synced(root, &repository)?;
    }
    let stage = str_value(&state, "stage")?;
    if ["main_synced", "remote_delete_started"].contains(&stage.as_str()) {
        assert_main_synced(root, &repository)?;
        let branch = str_value(&manifest_value, "branch")?;
        let remote = remote_branch(root, &repository, &branch)?;
        if remote.as_deref().is_some_and(|v| v != head) {
            return Err(error("remote task branchがreceipt headから変化しています"));
        }
        let records = worktree_records(root)?;
        let target: Vec<_> = records
            .iter()
            .filter(|r| r.get("worktree") == Some(&worktree_path.display().to_string()))
            .collect();
        let branch_records: Vec<_> = records
            .iter()
            .filter(|r| {
                r.get("branch") == Some(&format!("refs/heads/{branch}"))
                    || r.get("branch") == Some(&branch)
            })
            .collect();
        if target.len() != 1 || branch_records.len() != 1 {
            return Err(error(
                "task worktreeまたはbranchの使用状況が一意ではありません",
            ));
        }
        worktree(root, &manifest_value, &worktree_path)?;
        worktree_clean_head(&worktree_path, &head)?;
        if remote.is_some() {
            state = save_stage(&repository, task, &state, "remote_delete_started", "")?;
            let remote_url = canonical_remote_url(root, &repository)?;
            let args = [
                "push".into(),
                format!("--force-with-lease=refs/heads/{branch}:{head}"),
                remote_url,
                "--delete".into(),
                branch.clone(),
            ];
            if !git(
                root,
                &args.iter().map(String::as_str).collect::<Vec<_>>(),
                true,
            )
            .is_ok()
            {
                return Err(error("remote task branchを削除できません"));
            }
            if remote_branch(root, &repository, &branch)?.is_some() {
                return Err(error("remote task branchを削除後も確認できました"));
            }
        }
        state = save_stage(&repository, task, &state, "remote_deleted", "")?;
    }
    let stage = str_value(&state, "stage")?;
    if ["remote_deleted", "worktree_unlock_started"].contains(&stage.as_str()) {
        assert_main_synced(root, &repository)?;
        let branch = str_value(&manifest_value, "branch")?;
        if remote_branch(root, &repository, &branch)?.is_some() {
            return Err(error("remote task branchがcleanup中に再出現しました"));
        }
        let records = worktree_records(root)?;
        let target: Vec<_> = records
            .iter()
            .filter(|r| r.get("worktree") == Some(&worktree_path.display().to_string()))
            .collect();
        if !target.is_empty() {
            if target.len() != 1 {
                return Err(error("cleanup対象worktreeの使用状況が一意ではありません"));
            }
            let reason = target[0].get("locked").map(String::as_str);
            if stage == "remote_deleted" && reason != Some(&format!("codex-task:{task}")) {
                return Err(error("task worktreeのlock reasonが一致しません"));
            }
            if stage == "worktree_unlock_started"
                && reason.is_some_and(|v| v != format!("codex-task:{task}"))
            {
                return Err(error("unlock後のworktree lock reasonが想定外です"));
            }
            if reason.is_some() {
                state = save_stage(&repository, task, &state, "worktree_unlock_started", "")?;
                let args = [
                    "worktree",
                    "unlock",
                    "--",
                    &worktree_path.display().to_string(),
                ];
                let av = args.iter().map(|v| (*v).to_string()).collect::<Vec<_>>();
                if !git(
                    root,
                    &av.iter().map(String::as_str).collect::<Vec<_>>(),
                    true,
                )
                .is_ok()
                {
                    return Err(error("worktree unlockに失敗しました"));
                }
                let after = worktree_records(root)?;
                let unlocked = after
                    .iter()
                    .find(|r| r.get("worktree") == Some(&worktree_path.display().to_string()))
                    .ok_or_else(|| error("worktree unlock後の状態を確認できません"))?;
                if unlocked.get("locked").is_some() {
                    return Err(error("worktree unlock後のlock状態を確認できません"));
                }
                assert_main_synced(root, &repository)?;
                worktree(root, &manifest_value, &worktree_path)?;
                worktree_clean_head(&worktree_path, &head)?;
            }
            let args = [
                "worktree",
                "remove",
                "--",
                &worktree_path.display().to_string(),
            ];
            let av = args.iter().map(|v| (*v).to_string()).collect::<Vec<_>>();
            git(
                root,
                &av.iter().map(String::as_str).collect::<Vec<_>>(),
                true,
            )?;
        } else if worktree_path.exists() || worktree_path.is_symlink() {
            return Err(error("Git metadataにないworktree実体を削除しません"));
        }
        state = save_stage(&repository, task, &state, "worktree_removed", "")?;
    }
    if str_value(&state, "stage")? == "worktree_removed" {
        let branch = str_value(&manifest_value, "branch")?;
        if remote_branch(root, &repository, &branch)?.is_some() {
            return Err(error("remote task branchがcleanup中に再出現しました"));
        }
        let args = [
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ];
        let av = args.iter().map(|v| (*v).to_string()).collect::<Vec<_>>();
        let local = run(
            &git_command(&av)?,
            root,
            Duration::from_secs(COMMAND_TIMEOUT),
            MAX_OUTPUT_BYTES,
        )?;
        if local.status.success() {
            if oid(
                git(root, &["rev-parse", &format!("refs/heads/{branch}")], true)?.trim(),
                "local task branch",
            )? != head
            {
                return Err(error("再出現したlocal branchがreceipt headと一致しません"));
            }
            git(root, &["branch", "-d", "--", &branch], true)?;
        }
        state = save_stage(&repository, task, &state, "completed", "")?;
    }
    Ok(state)
}
fn deliver(
    root: &Path,
    task: &str,
    pr: i64,
    head: &str,
    plan: &str,
    plan_version: i64,
    gate: &str,
) -> Result<Value> {
    task_id(task)?;
    with_deadline(|| {
        let repository = repository(root)?;
        let _lock = task_lock(&repository, task)?;
        deliver_locked(root, task, pr, head, plan, plan_version, gate)
    })
}
fn finish_requires_live_ledger(stage: &str) -> bool {
    stage != "completed"
}

#[allow(clippy::too_many_arguments)]
fn finish(
    root: &Path,
    task: &str,
    pr: i64,
    head: &str,
    plan: &str,
    plan_version: i64,
    gate: &str,
    sandbox_retry: bool,
) -> Result<Value> {
    task_id(task)?;
    with_deadline(|| {
        let repository = repository(root)?;
        let _lock = task_lock(&repository, task)?;
        finish_locked(
            root,
            task,
            pr,
            head,
            plan,
            plan_version,
            gate,
            sandbox_retry,
        )
    })
}

fn canonical_helper_path() -> Result<PathBuf> {
    #[cfg(unix)]
    unsafe {
        let entry = libc::getpwuid(libc::getuid());
        if entry.is_null() {
            return Err(error("current userのhome directoryを確認できません"));
        }
        let home = CStr::from_ptr((*entry).pw_dir)
            .to_str()
            .map_err(|_| error("current userのhome directoryを確認できません"))?;
        Ok(PathBuf::from(home).join(".local/bin/codex-delivery"))
    }
    #[cfg(not(unix))]
    {
        Err(error("current userのhome directoryを確認できません"))
    }
}
fn resolve_invocation_path(argv0: &Path, path_value: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    if argv0.is_absolute() || argv0.components().count() > 1 {
        return fs::symlink_metadata(argv0)
            .ok()
            .map(|_| argv0.to_path_buf());
    }
    let path_value = path_value?;
    for directory in env::split_paths(path_value) {
        let candidate = if directory.as_os_str().is_empty() {
            PathBuf::from(argv0)
        } else {
            directory.join(argv0)
        };
        let Some(metadata) = fs::symlink_metadata(&candidate).ok() else {
            continue;
        };
        if metadata.is_file() || metadata.file_type().is_symlink() {
            return Some(candidate);
        }
    }
    None
}
fn require_canonical_invocation() -> Result<()> {
    let canonical = canonical_helper_path()?;
    let metadata = fs::symlink_metadata(&canonical)
        .map_err(|_| error("codex-deliveryのcanonical install先を確認できません"))?;
    let argv0 = env::args_os()
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| error("codex-deliveryのcanonical install先を確認できません"))?;
    let invoked = resolve_invocation_path(&argv0, env::var_os("PATH").as_deref())
        .ok_or_else(|| error("codex-deliveryのcanonical install先を確認できません"))?;
    let invoked_metadata = fs::symlink_metadata(&invoked)
        .map_err(|_| error("codex-deliveryの起動pathを確認できません"))?;
    let invoked_canonical = fs::canonicalize(&invoked)
        .map_err(|_| error("codex-deliveryの起動pathをcanonicalizeできません"))?;
    let canonical_canonical = fs::canonicalize(&canonical)
        .map_err(|_| error("codex-deliveryのcanonical install先を確認できません"))?;
    #[cfg(unix)]
    if invoked_canonical != canonical_canonical
        || invoked_metadata.file_type().is_symlink()
        || canonical.is_symlink()
        || !metadata.is_file()
        || metadata.uid() != unsafe { libc::getuid() }
        || metadata.mode() & 0o077 != 0
        || metadata.mode() & 0o111 == 0
    {
        return Err(error(
            "codex-deliveryはcanonical install先から直接実行してください",
        ));
    }
    #[cfg(not(unix))]
    if invoked_canonical != canonical_canonical
        || invoked_metadata.file_type().is_symlink()
        || canonical.is_symlink()
        || !metadata.is_file()
    {
        return Err(error(
            "codex-deliveryはcanonical install先から直接実行してください",
        ));
    }
    Ok(())
}
#[derive(Debug)]
struct CliArgs {
    command: String,
    task: String,
    pr: i64,
    head: String,
    risk: Option<String>,
    plan: String,
    plan_version: i64,
    tests: bool,
    independent: bool,
    specialist: bool,
    sandbox_retry: bool,
    gate: String,
}
fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<CliArgs> {
    let mut iter = args.into_iter();
    let command = iter
        .next()
        .ok_or_else(|| error("commandが必要です"))?
        .into_string()
        .map_err(|_| error("commandがUTF-8ではありません"))?;
    if !["record-review", "approve-review", "deliver", "finish"].contains(&command.as_str()) {
        return Err(error("不明なcommandです"));
    }
    let mut values: BTreeMap<String, String> = BTreeMap::new();
    let mut flags = HashSet::new();
    let mut current = iter;
    while let Some(raw) = current.next() {
        let key = raw
            .into_string()
            .map_err(|_| error("argumentがUTF-8ではありません"))?;
        let normalized = key
            .strip_prefix("--")
            .ok_or_else(|| error("argumentが不正です"))?
            .to_string();
        let flag_options = [
            "tests-passed",
            "independent-review-passed",
            "specialist-review-passed",
            "sandbox-retry",
        ];
        let value_options = [
            "task-id",
            "pr",
            "head",
            "risk",
            "plan-id",
            "plan-version",
            "gate-mode",
        ];
        if flag_options.contains(&normalized.as_str()) {
            if !flags.insert(normalized) {
                return Err(error("argumentが重複しています"));
            }
        } else if value_options.contains(&normalized.as_str()) {
            let value = current
                .next()
                .ok_or_else(|| error(format!("--{normalized}の値が不足しています")))?
                .into_string()
                .map_err(|_| error("argumentがUTF-8ではありません"))?;
            if values.insert(normalized.clone(), value).is_some() {
                return Err(error("argumentが重複しています"));
            }
        } else {
            return Err(error(format!("--{normalized}は許可されていません")));
        }
    }
    let task = task_id(
        values
            .get("task-id")
            .ok_or_else(|| error("--task-idが必要です"))?,
    )?;
    let head = values
        .get("head")
        .ok_or_else(|| error("--headが必要です"))?
        .clone();
    oid(&head, "head")?;
    let pr = values
        .get("pr")
        .ok_or_else(|| error("--prが必要です"))?
        .parse::<i64>()
        .map_err(|_| error("PR番号が不正です"))?;
    if pr < 1 {
        return Err(error("PR番号が不正です"));
    }
    let plan = values
        .get("plan-id")
        .ok_or_else(|| error("--plan-idが必要です"))?
        .clone();
    if !plan_id(&plan) {
        return Err(error("Plan IDが安全ではありません"));
    }
    let plan_version = values
        .get("plan-version")
        .ok_or_else(|| error("--plan-versionが必要です"))?
        .parse::<i64>()
        .map_err(|_| error("Plan versionが不正です"))?;
    if !(1..=999_999).contains(&plan_version) {
        return Err(error("Plan versionが不正です"));
    }
    let gate = values
        .get("gate-mode")
        .map(String::as_str)
        .unwrap_or(STRICT_GATE_MODE)
        .to_string();
    gate_mode(&gate)?;
    if values
        .get("gate-mode")
        .is_some_and(|v| v == STRICT_GATE_MODE)
    {
        return Err(error(
            "strict-rulesetは省略時の既定値としてのみ指定できます",
        ));
    }
    if command == "record-review" || command == "approve-review" {
        if flags.contains("sandbox-retry") {
            return Err(error("review記録へsandbox再試行を指定できません"));
        }
        let risk_value = values
            .get("risk")
            .ok_or_else(|| error("--riskが必要です"))?
            .clone();
        risk(&risk_value)?;
        let tests = flags.contains("tests-passed");
        let independent = flags.contains("independent-review-passed");
        let specialist = flags.contains("specialist-review-passed");
        validate_review_evidence_flags(&risk_value, tests, independent, specialist)?;
        Ok(CliArgs {
            command,
            task,
            pr,
            head,
            risk: Some(risk_value),
            plan,
            plan_version,
            tests,
            independent,
            specialist,
            sandbox_retry: false,
            gate,
        })
    } else {
        let sandbox_retry = flags.remove("sandbox-retry");
        if values.contains_key("risk")
            || !flags.is_empty()
            || (command == "deliver" && sandbox_retry)
        {
            return Err(error("deliver/finishのargumentが不正です"));
        }
        Ok(CliArgs {
            command,
            task,
            pr,
            head,
            risk: None,
            plan,
            plan_version,
            tests: false,
            independent: false,
            specialist: false,
            sandbox_retry,
            gate,
        })
    }
}

/// canonical installed binary から起動する entrypoint。成功0、検証/操作失敗1。
pub fn entrypoint(args: impl IntoIterator<Item = OsString>) -> i32 {
    let result = (|| -> Result<Value> {
        require_canonical_invocation()?;
        // 呼び出し側が argv[0] を除いた引数列を渡す契約。
        let parsed = parse_args(args);
        let parsed = parsed?;
        let root =
            env::current_dir().map_err(|_| error("current checkout rootを確認できません"))?;
        match parsed.command.as_str() {
            "record-review" => write_review(
                &root,
                &parsed.task,
                parsed.pr,
                &parsed.head,
                parsed.risk.as_deref().unwrap_or(""),
                &parsed.plan,
                parsed.plan_version,
                false,
                parsed.tests,
                parsed.independent,
                parsed.specialist,
                &parsed.gate,
            ),
            "approve-review" => write_review(
                &root,
                &parsed.task,
                parsed.pr,
                &parsed.head,
                parsed.risk.as_deref().unwrap_or(""),
                &parsed.plan,
                parsed.plan_version,
                true,
                parsed.tests,
                parsed.independent,
                parsed.specialist,
                &parsed.gate,
            ),
            "deliver" => deliver(
                &root,
                &parsed.task,
                parsed.pr,
                &parsed.head,
                &parsed.plan,
                parsed.plan_version,
                &parsed.gate,
            ),
            "finish" => finish(
                &root,
                &parsed.task,
                parsed.pr,
                &parsed.head,
                &parsed.plan,
                parsed.plan_version,
                &parsed.gate,
                parsed.sandbox_retry,
            ),
            _ => Err(error("不明なcommandです")),
        }
    })();
    match result {
        Ok(value) => match serde_json::to_string(&value) {
            Ok(text) => {
                println!("{text}");
                0
            }
            Err(_) => 1,
        },
        Err(err) => {
            eprintln!("codex-delivery: {err}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::{OsStr, OsString};
    use std::fs;
    use std::sync::{Mutex, OnceLock};

    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    fn receipt_v3() -> Value {
        object([
            ("version", Value::Number(3.into())),
            ("kind", string("review")),
            ("task_id", string("issue-24")),
            ("repository", string("owner/repo")),
            ("pr", Value::Number(24.into())),
            ("head_sha", string("b".repeat(40))),
            ("risk", string("low")),
            ("plan_id", string("CODEX-DELIVERY-TEST-v1")),
            ("actionable", Value::Number(0.into())),
            ("decision", string("autonomous")),
            ("tests_passed", Value::Bool(true)),
            ("neutral_review_passed", Value::Bool(true)),
            ("adversarial_review_passed", Value::Bool(true)),
            ("changed_files", Value::Array(vec![string("src/main.rs")])),
            ("created_at", string("now")),
            ("gate_mode", string(STRICT_GATE_MODE)),
        ])
    }

    fn receipt_v5(risk_value: &str, specialist: bool) -> Value {
        object([
            ("version", Value::Number(RECEIPT_VERSION.into())),
            ("kind", string("review")),
            ("task_id", string("issue-24")),
            ("repository", string("owner/repo")),
            ("pr", Value::Number(24.into())),
            ("head_sha", string("b".repeat(40))),
            ("risk", string(risk_value)),
            ("plan_id", string("CODEX-DELIVERY-TEST-v1")),
            ("actionable", Value::Number(0.into())),
            ("decision", string("autonomous")),
            ("tests_passed", Value::Bool(true)),
            ("independent_review_passed", Value::Bool(true)),
            ("specialist_review_passed", Value::Bool(specialist)),
            ("changed_files", Value::Array(vec![string("src/main.rs")])),
            ("created_at", string("now")),
            ("gate_mode", string(STRICT_GATE_MODE)),
            ("ledger_comment_id", Value::Number(42.into())),
            ("ledger_body_sha256", string("c".repeat(64))),
            ("plan_version", Value::Number(2.into())),
        ])
    }

    fn legacy_receipt_v5(risk_value: &str, specialist: bool) -> Value {
        let mut receipt = receipt_v5(risk_value, specialist);
        receipt.as_object_mut().unwrap().remove("plan_version");
        receipt
    }

    fn chunks(value: &str) -> Value {
        Value::Array(
            value
                .as_bytes()
                .chunks(8)
                .map(|chunk| string(std::str::from_utf8(chunk).unwrap()))
                .collect(),
        )
    }

    fn ledger_comment(id: i64, body: String, updated_at: &str) -> Value {
        object([
            ("id", Value::Number(id.into())),
            ("body", string(body)),
            ("created_at", string("2026-08-27T00:00:00Z")),
            ("updated_at", string(updated_at)),
            ("user", object([("login", string("owner"))])),
        ])
    }

    fn ledger_fixture(status: &str) -> Value {
        let bootstrap_body = format!(
            "{LOOP_LEDGER_V1_MARKER}\n```json\n{}\n```\n",
            serde_json::to_string(&object([
                ("schema_version", Value::Number(1.into())),
                (
                    "task_id_parts",
                    Value::Array(vec![string("issue"), string("24")]),
                ),
                ("plan_id", string("CODEX-DELIVERY-TEST-v1")),
                ("plan_version", Value::Number(1.into())),
                ("repository", string("owner/repo")),
                ("pr", Value::Number(24.into())),
                ("round", Value::Number(1.into())),
                ("head_before", chunks(&"a".repeat(40))),
                ("head_after", chunks(&"a".repeat(40))),
            ]))
            .unwrap()
        );
        let mut finding = object([
            ("invariant_id", string("DELIVERY-NO-BLOCKED-LEDGER")),
            (
                "cause_path",
                string("packages/cli/src/codex_tools/delivery.rs"),
            ),
            ("failure_class", string("blocked-ledger")),
            ("first_head", chunks(&"a".repeat(40))),
            ("severity", string("high")),
            ("status", string(status)),
            ("attempt", Value::Number(1.into())),
            ("reproduction", string("blocked findingを記録する")),
            ("impact", string("deliveryが通過し得る")),
            ("post_fix_condition", string("helperが拒否する")),
            ("tests", Value::Array(vec![string("ledger gate test")])),
            ("evidence", Value::Array(vec![string("unit test passed")])),
        ]);
        let fingerprint = finding_fingerprint(&finding).unwrap();
        finding
            .as_object_mut()
            .unwrap()
            .insert("fingerprint".into(), chunks(&fingerprint));
        let payload = object([
            ("schema", Value::Number(3.into())),
            (
                "task_id_parts",
                Value::Array(vec![string("issue"), string("24")]),
            ),
            ("plan_id", string("CODEX-DELIVERY-TEST-v1")),
            ("plan_version", Value::Number(1.into())),
            ("repository", string("owner/repo")),
            ("pr", Value::Number(24.into())),
            ("round", Value::Number(2.into())),
            ("head_before", chunks(&"a".repeat(40))),
            ("head_after", chunks(&"b".repeat(40))),
            (
                "previous",
                object([
                    ("comment_id", Value::Number(10.into())),
                    ("body_sha256", chunks(&sha256(&bootstrap_body))),
                ]),
            ),
            ("findings", Value::Array(vec![finding])),
            ("failure_signatures", Value::Array(Vec::new())),
            (
                "progress_events",
                Value::Array(vec![object([
                    ("kind", string("finding_resolved")),
                    ("finding", chunks(&fingerprint)),
                    ("evidence", Value::Array(vec![string("ledger gate test")])),
                ])]),
            ),
            (
                "diagnostic",
                object([
                    ("used", Value::Bool(true)),
                    ("budget_source", string("policy")),
                    ("max_tool_calls", Value::Number(12.into())),
                    ("tool_calls_used", Value::Number(4.into())),
                    ("deadline_minutes", Value::Number(30.into())),
                    ("outcome", string("root causeを修正")),
                ]),
            ),
        ]);
        let current_body = format!(
            "{LOOP_LEDGER_V2_MARKER}\n{}",
            serde_json::to_string(&payload).unwrap()
        );
        Value::Array(vec![Value::Array(vec![
            ledger_comment(10, bootstrap_body, "2026-08-27T00:00:00Z"),
            ledger_comment(11, current_body, "2026-08-27T00:00:00Z"),
        ])])
    }

    fn mutate_latest_ledger(pages: &mut Value, mutate: impl FnOnce(&mut Value)) {
        let comments = pages.as_array_mut().unwrap()[0].as_array_mut().unwrap();
        let latest = comments.last_mut().unwrap().as_object_mut().unwrap();
        let body = latest.get("body").and_then(Value::as_str).unwrap();
        let mut payload = ledger_json(body, LOOP_LEDGER_V2_MARKER).unwrap();
        mutate(&mut payload);
        latest.insert(
            "body".into(),
            string(format!(
                "{LOOP_LEDGER_V2_MARKER}\n{}",
                serde_json::to_string(&payload).unwrap()
            )),
        );
    }

    fn three_checkpoint_ledger() -> Value {
        let mut pages = ledger_fixture("resolved");
        let comments = pages.as_array_mut().unwrap()[0].as_array_mut().unwrap();
        let bootstrap_body = comments[0]["body"].as_str().unwrap().to_string();
        let latest_body = comments[1]["body"].as_str().unwrap();
        let mut intermediate = ledger_json(latest_body, LOOP_LEDGER_V2_MARKER).unwrap();
        let intermediate_object = intermediate.as_object_mut().unwrap();
        intermediate_object.insert("schema".into(), Value::Number(2.into()));
        intermediate_object.insert("head_after".into(), chunks(&"a".repeat(40)));
        intermediate_object.insert(
            "progress_events".into(),
            Value::Array(vec![string("legacy checkpoint")]),
        );
        intermediate_object.insert(
            "diagnostic".into(),
            object([
                ("used", Value::Bool(true)),
                ("max_tool_calls", Value::Number(12.into())),
                ("deadline_minutes", Value::Number(30.into())),
                ("outcome", string("legacy checkpoint")),
            ]),
        );
        let intermediate_body = format!(
            "{LOOP_LEDGER_V2_MARKER}\n{}",
            serde_json::to_string(&intermediate).unwrap()
        );
        let mut latest = ledger_json(latest_body, LOOP_LEDGER_V2_MARKER).unwrap();
        let latest_object = latest.as_object_mut().unwrap();
        latest_object.insert("round".into(), Value::Number(3.into()));
        latest_object.insert("head_before".into(), chunks(&"a".repeat(40)));
        latest_object.insert(
            "previous".into(),
            object([
                ("comment_id", Value::Number(11.into())),
                ("body_sha256", chunks(&sha256(&intermediate_body))),
            ]),
        );
        let current_body = format!(
            "{LOOP_LEDGER_V2_MARKER}\n{}",
            serde_json::to_string(&latest).unwrap()
        );
        *comments = vec![
            ledger_comment(10, bootstrap_body, "2026-08-27T00:00:00Z"),
            ledger_comment(11, intermediate_body, "2026-08-27T00:00:00Z"),
            ledger_comment(12, current_body, "2026-08-27T00:00:00Z"),
        ];
        pages
    }

    fn with_codex_home<T>(f: impl FnOnce(&Path) -> T) -> T {
        let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let home = env::temp_dir().join(format!("codex-delivery-rust-{suffix}"));
        let managed = home.join("worktrees").join(repo_key("owner/repo").unwrap());
        fs::create_dir_all(managed.join(".state")).unwrap();
        fs::create_dir_all(managed.join(".locks")).unwrap();
        for path in [
            home.as_path(),
            home.join("worktrees").as_path(),
            managed.as_path(),
            managed.join(".state").as_path(),
            managed.join(".locks").as_path(),
        ] {
            set_private_dir(path);
        }
        // Rust 2024 marks process-wide environment mutation unsafe.  Tests are
        // serialized by ENV_LOCK and restore the value before returning.
        let previous = env::var_os("CODEX_HOME");
        unsafe {
            env::set_var("CODEX_HOME", &home);
        }
        let result = f(&home);
        if let Some(value) = previous {
            unsafe {
                env::set_var("CODEX_HOME", value);
            }
        } else {
            unsafe {
                env::remove_var("CODEX_HOME");
            }
        }
        let _ = fs::remove_dir_all(home);
        result
    }

    fn set_private_dir(path: &Path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
    }

    #[test]
    fn codex_home_defaults_to_dot_codex_below_the_account_home() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let home = env::temp_dir().join(format!("codex-delivery-home-{suffix}"));
        fs::create_dir(&home).expect("create account home fixture");

        assert_eq!(
            resolve_codex_home(None, home.clone()).expect("resolve default CODEX_HOME"),
            home.join(".codex")
        );

        fs::remove_dir(home).expect("remove account home fixture");
    }

    fn write_private(path: &Path, value: &Value) {
        fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        }
    }

    #[test]
    fn validation_rejects_unsafe_identifiers() {
        assert!(task_id("issue-0").is_err());
        assert!(task_id("issue-24").is_ok());
        assert!(task_id("task-abc-1").is_ok());
        assert!(task_id("task-A").is_err());
        assert!(oid(&"a".repeat(40), "head").is_ok());
        assert!(oid("x", "head").is_err());
        assert!(repository_name("owner/repo"));
        assert!(!repository_name("evil/http://x"));
        assert!(branch_name("feat/issue-24"));
        assert!(!branch_name("main"));
        assert!(plan_id("CODEX-DELIVERY-TEST-v1"));
        assert!(plan_id("loop-engineering-circuit-breaker-v1"));
        assert!(!plan_id("bad-v0"));
    }

    #[test]
    fn remote_url_is_bound_to_the_initial_repository_identity() {
        let mut root = std::env::current_dir().expect("current repository");
        while !root.join(".git").exists() {
            assert!(root.pop(), "repository root is present");
        }
        assert_eq!(
            canonical_remote_url(&root, "Daiki48/dotfiles").expect("stable repository identity"),
            "https://github.com/Daiki48/dotfiles.git"
        );
        assert!(canonical_remote_url(&root, "other/repository").is_err());
    }
    #[test]
    fn receipt_v3_requires_separate_decision_and_gate() {
        let value = object([
            ("version", Value::Number(3.into())),
            ("decision", string("autonomous")),
            ("gate_mode", string(STRICT_GATE_MODE)),
        ]);
        assert_eq!(receipt_decision(&value).unwrap_or_default(), "autonomous");
        assert!(receipt_decision(&object([("human_approved", Value::Bool(false))])).is_ok());
        assert!(receipt_decision(&object([("decision", string("invalid"))])).is_err());
    }

    #[test]
    fn receipt_v5_requires_standard_review_and_risk_matched_specialist_review() {
        for (risk_value, specialist) in [
            ("low", false),
            ("medium", false),
            ("high", true),
            ("critical", true),
        ] {
            assert!(
                receipt(
                    &receipt_v5(risk_value, specialist),
                    Path::new("/unused"),
                    "issue-24",
                    &"b".repeat(40),
                    Some("owner/repo"),
                )
                .is_ok()
            );
        }
        assert!(
            receipt(
                &receipt_v5("low", true),
                Path::new("/unused"),
                "issue-24",
                &"b".repeat(40),
                Some("owner/repo"),
            )
            .is_err()
        );
        assert!(
            receipt(
                &receipt_v5("high", false),
                Path::new("/unused"),
                "issue-24",
                &"b".repeat(40),
                Some("owner/repo"),
            )
            .is_err()
        );
    }

    #[test]
    fn loop_ledger_requires_a_complete_unedited_resolved_chain() {
        let valid = ledger_fixture("resolved");
        let evidence = validate_loop_ledger_comments(
            &valid,
            "owner",
            "issue-24",
            "CODEX-DELIVERY-TEST-v1",
            "owner/repo",
            24,
            &"b".repeat(40),
        )
        .expect("valid loop ledger");
        assert_eq!(evidence.0, 11);

        assert!(
            validate_loop_ledger_comments(
                &ledger_fixture("blocked"),
                "owner",
                "issue-24",
                "CODEX-DELIVERY-TEST-v1",
                "owner/repo",
                24,
                &"b".repeat(40),
            )
            .is_err()
        );

        let mut missing = valid.clone();
        missing.as_array_mut().unwrap()[0]
            .as_array_mut()
            .unwrap()
            .remove(0);
        assert!(
            validate_loop_ledger_comments(
                &missing,
                "owner",
                "issue-24",
                "CODEX-DELIVERY-TEST-v1",
                "owner/repo",
                24,
                &"b".repeat(40),
            )
            .is_err()
        );

        let mut edited = valid;
        edited.as_array_mut().unwrap()[0].as_array_mut().unwrap()[1]
            .as_object_mut()
            .unwrap()
            .insert("updated_at".into(), string("2026-08-27T00:00:01Z"));
        assert!(
            validate_loop_ledger_comments(
                &edited,
                "owner",
                "issue-24",
                "CODEX-DELIVERY-TEST-v1",
                "owner/repo",
                24,
                &"b".repeat(40),
            )
            .is_err()
        );
    }

    #[test]
    fn loop_ledger_allows_an_empty_clean_checkpoint_and_rejects_forged_evidence() {
        let mut clean = ledger_fixture("resolved");
        mutate_latest_ledger(&mut clean, |payload| {
            payload
                .as_object_mut()
                .unwrap()
                .insert("findings".into(), Value::Array(Vec::new()));
            payload
                .as_object_mut()
                .unwrap()
                .insert("progress_events".into(), Value::Array(Vec::new()));
        });
        assert!(
            validate_loop_ledger_comments(
                &clean,
                "owner",
                "issue-24",
                "CODEX-DELIVERY-TEST-v1",
                "owner/repo",
                24,
                &"b".repeat(40),
            )
            .is_ok()
        );

        let mut forged_signature = ledger_fixture("resolved");
        mutate_latest_ledger(&mut forged_signature, |payload| {
            payload.as_object_mut().unwrap().insert(
                "failure_signatures".into(),
                Value::Array(vec![object([
                    ("id", chunks(&"d".repeat(64))),
                    ("operation", string("test")),
                    ("target", string("delivery")),
                    ("error_class", string("failure")),
                    ("input_digest", chunks(&"e".repeat(64))),
                    ("external_state_digest", chunks(&"f".repeat(64))),
                ])]),
            );
        });
        assert!(
            validate_loop_ledger_comments(
                &forged_signature,
                "owner",
                "issue-24",
                "CODEX-DELIVERY-TEST-v1",
                "owner/repo",
                24,
                &"b".repeat(40),
            )
            .is_err()
        );

        let mut excessive_budget = ledger_fixture("resolved");
        mutate_latest_ledger(&mut excessive_budget, |payload| {
            payload["diagnostic"]["max_tool_calls"] = Value::Number(100.into());
        });
        assert!(
            validate_loop_ledger_comments(
                &excessive_budget,
                "owner",
                "issue-24",
                "CODEX-DELIVERY-TEST-v1",
                "owner/repo",
                24,
                &"b".repeat(40),
            )
            .is_err()
        );
    }

    #[test]
    fn loop_ledger_validates_every_predecessor_and_preserves_findings() {
        let valid = three_checkpoint_ledger();
        assert!(
            validate_loop_ledger_comments(
                &valid,
                "owner",
                "issue-24",
                "CODEX-DELIVERY-TEST-v1",
                "owner/repo",
                24,
                &"b".repeat(40),
            )
            .is_ok()
        );

        let mut wrong_identity = valid.clone();
        let comments = wrong_identity.as_array_mut().unwrap()[0]
            .as_array_mut()
            .unwrap();
        let body = comments[1]["body"].as_str().unwrap();
        let mut payload = ledger_json(body, LOOP_LEDGER_V2_MARKER).unwrap();
        payload["repository"] = string("other/repo");
        let forged_body = format!(
            "{LOOP_LEDGER_V2_MARKER}\n{}",
            serde_json::to_string(&payload).unwrap()
        );
        comments[1]["body"] = string(&forged_body);
        let latest_body = comments[2]["body"].as_str().unwrap();
        let mut latest_payload = ledger_json(latest_body, LOOP_LEDGER_V2_MARKER).unwrap();
        latest_payload["previous"]["body_sha256"] = chunks(&sha256(forged_body));
        comments[2]["body"] = string(format!(
            "{LOOP_LEDGER_V2_MARKER}\n{}",
            serde_json::to_string(&latest_payload).unwrap()
        ));
        assert!(
            validate_loop_ledger_comments(
                &wrong_identity,
                "owner",
                "issue-24",
                "CODEX-DELIVERY-TEST-v1",
                "owner/repo",
                24,
                &"b".repeat(40),
            )
            .is_err()
        );

        let mut missing_finding = valid;
        mutate_latest_ledger(&mut missing_finding, |payload| {
            payload["findings"] = Value::Array(Vec::new());
            payload["progress_events"] = Value::Array(Vec::new());
        });
        assert!(
            validate_loop_ledger_comments(
                &missing_finding,
                "owner",
                "issue-24",
                "CODEX-DELIVERY-TEST-v1",
                "owner/repo",
                24,
                &"b".repeat(40),
            )
            .is_err()
        );
    }

    #[test]
    fn loop_ledger_requires_latest_schema3_and_terminal_v1_findings() {
        let mut only_v1 = ledger_fixture("resolved");
        only_v1.as_array_mut().unwrap()[0]
            .as_array_mut()
            .unwrap()
            .truncate(1);
        assert!(
            validate_loop_ledger_comments(
                &only_v1,
                "owner",
                "issue-24",
                "CODEX-DELIVERY-TEST-v1",
                "owner/repo",
                24,
                &"b".repeat(40),
            )
            .is_err()
        );

        let mut unresolved_v1 = ledger_fixture("resolved");
        let comments = unresolved_v1.as_array_mut().unwrap()[0]
            .as_array_mut()
            .unwrap();
        let bootstrap_body = comments[0]["body"].as_str().unwrap();
        let suffix = bootstrap_body
            .strip_prefix(&format!("{LOOP_LEDGER_V1_MARKER}\n```json\n"))
            .unwrap()
            .trim_end()
            .strip_suffix("```")
            .unwrap();
        let mut bootstrap = parse_json(suffix.trim_end(), "test bootstrap").unwrap();
        bootstrap["findings"] = Value::Array(vec![object([
            ("id", chunks(&"d".repeat(64))),
            ("invariant_id", string("UNRESOLVED-V1")),
            ("status", string("blocked")),
            ("attempt", Value::Number(1.into())),
            ("evidence", string("still blocked")),
        ])]);
        let forged_bootstrap = format!(
            "{LOOP_LEDGER_V1_MARKER}\n```json\n{}\n```\n",
            serde_json::to_string(&bootstrap).unwrap()
        );
        comments[0]["body"] = string(&forged_bootstrap);
        let current_body = comments[1]["body"].as_str().unwrap();
        let mut current = ledger_json(current_body, LOOP_LEDGER_V2_MARKER).unwrap();
        current["previous"]["body_sha256"] = chunks(&sha256(forged_bootstrap));
        comments[1]["body"] = string(format!(
            "{LOOP_LEDGER_V2_MARKER}\n{}",
            serde_json::to_string(&current).unwrap()
        ));
        assert!(
            validate_loop_ledger_comments(
                &unresolved_v1,
                "owner",
                "issue-24",
                "CODEX-DELIVERY-TEST-v1",
                "owner/repo",
                24,
                &"b".repeat(40),
            )
            .is_err()
        );
    }

    #[test]
    fn loop_ledger_rejects_schema_downgrade_and_plan_version_regression() {
        let mut downgrade = three_checkpoint_ledger();
        let comments = downgrade.as_array_mut().unwrap()[0].as_array_mut().unwrap();
        let schema3_body = comments[2]["body"].as_str().unwrap().to_string();
        let mut schema2 = ledger_json(&schema3_body, LOOP_LEDGER_V2_MARKER).unwrap();
        schema2["schema"] = Value::Number(2.into());
        schema2["round"] = Value::Number(4.into());
        schema2["head_before"] = chunks(&"b".repeat(40));
        schema2["head_after"] = chunks(&"b".repeat(40));
        schema2["previous"] = object([
            ("comment_id", Value::Number(12.into())),
            ("body_sha256", chunks(&sha256(&schema3_body))),
        ]);
        schema2["progress_events"] = Value::Array(vec![string("legacy downgrade")]);
        schema2["diagnostic"] = object([
            ("used", Value::Bool(false)),
            ("max_tool_calls", Value::Number(12.into())),
            ("deadline_minutes", Value::Number(30.into())),
            ("outcome", string("legacy downgrade")),
        ]);
        let schema2_body = format!(
            "{LOOP_LEDGER_V2_MARKER}\n{}",
            serde_json::to_string(&schema2).unwrap()
        );
        let mut final_schema3 = ledger_json(&schema3_body, LOOP_LEDGER_V2_MARKER).unwrap();
        final_schema3["round"] = Value::Number(5.into());
        final_schema3["head_before"] = chunks(&"b".repeat(40));
        final_schema3["previous"] = object([
            ("comment_id", Value::Number(13.into())),
            ("body_sha256", chunks(&sha256(&schema2_body))),
        ]);
        let final_body = format!(
            "{LOOP_LEDGER_V2_MARKER}\n{}",
            serde_json::to_string(&final_schema3).unwrap()
        );
        comments.push(ledger_comment(13, schema2_body, "2026-08-27T00:00:00Z"));
        comments.push(ledger_comment(14, final_body, "2026-08-27T00:00:00Z"));
        assert!(
            validate_loop_ledger_comments(
                &downgrade,
                "owner",
                "issue-24",
                "CODEX-DELIVERY-TEST-v1",
                "owner/repo",
                24,
                &"b".repeat(40),
            )
            .is_err()
        );

        let mut version_regression = three_checkpoint_ledger();
        let comments = version_regression.as_array_mut().unwrap()[0]
            .as_array_mut()
            .unwrap();
        let intermediate_body = comments[1]["body"].as_str().unwrap();
        let mut intermediate = ledger_json(intermediate_body, LOOP_LEDGER_V2_MARKER).unwrap();
        intermediate["plan_version"] = Value::Number(2.into());
        let forged_intermediate = format!(
            "{LOOP_LEDGER_V2_MARKER}\n{}",
            serde_json::to_string(&intermediate).unwrap()
        );
        comments[1]["body"] = string(&forged_intermediate);
        let latest_body = comments[2]["body"].as_str().unwrap();
        let mut latest = ledger_json(latest_body, LOOP_LEDGER_V2_MARKER).unwrap();
        latest["plan_version"] = Value::Number(1.into());
        latest["previous"]["body_sha256"] = chunks(&sha256(forged_intermediate));
        comments[2]["body"] = string(format!(
            "{LOOP_LEDGER_V2_MARKER}\n{}",
            serde_json::to_string(&latest).unwrap()
        ));
        assert!(
            validate_loop_ledger_comments(
                &version_regression,
                "owner",
                "issue-24",
                "CODEX-DELIVERY-TEST-v1",
                "owner/repo",
                24,
                &"b".repeat(40),
            )
            .is_err()
        );
    }

    #[test]
    fn legacy_receipt_cannot_bypass_the_v5_ledger_scope() {
        assert!(review_receipt_scope_matches(
            &receipt_v3(),
            &receipt_v5("low", false)
        ));
        let current = receipt_v5("low", false);
        let mut different_plan_version = current.clone();
        different_plan_version["plan_version"] = Value::Number(3.into());
        assert!(!review_receipt_scope_matches(
            &current,
            &different_plan_version
        ));
        let legacy_v5 = legacy_receipt_v5("low", false);
        assert!(review_receipt_scope_matches(&legacy_v5, &current));
        assert!(!receipt_has_current_plan_version(&legacy_v5));
        assert!(
            receipt(
                &legacy_v5,
                Path::new("/unused"),
                "issue-24",
                &"b".repeat(40),
                Some("owner/repo"),
            )
            .is_ok()
        );
        assert!(validate_receipt_loop_ledger(Path::new("/unused"), &legacy_v5).is_err());
        assert!(validate_receipt_loop_ledger(Path::new("/unused"), &receipt_v3()).is_err());
    }

    #[test]
    fn finish_revalidates_the_live_ledger_until_cleanup_is_complete() {
        for stage in [
            "merge_started",
            "merged",
            "main_synced",
            "remote_branch_deleted",
            "worktree_removed",
        ] {
            assert!(finish_requires_live_ledger(stage), "stage: {stage}");
        }
        assert!(!finish_requires_live_ledger("completed"));
    }

    #[test]
    fn main_sync_recovers_only_target_matching_partial_files() {
        fn git_at(root: &Path, args: &[&str]) -> std::process::Output {
            std::process::Command::new("/usr/bin/git")
                .args(["-c", "core.hooksPath=/dev/null", "-c", "diff.external="])
                .args(args)
                .current_dir(root)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .output()
                .expect("run git")
        }
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = env::temp_dir().join(format!("codex-delivery-ff-{suffix}"));
        fs::create_dir(&root).unwrap();
        assert!(
            git_at(&root, &["init", "-q", "-b", "main"])
                .status
                .success()
        );
        fs::write(root.join("a"), "old-a\n").unwrap();
        fs::write(root.join("b"), "old-b\n").unwrap();
        assert!(git_at(&root, &["add", "a", "b"]).status.success());
        assert!(
            git_at(
                &root,
                &[
                    "-c",
                    "user.name=test",
                    "-c",
                    "user.email=test@example.com",
                    "commit",
                    "-qm",
                    "base"
                ]
            )
            .status
            .success()
        );
        let base = String::from_utf8(git_at(&root, &["rev-parse", "HEAD"]).stdout).unwrap();
        assert!(
            git_at(&root, &["checkout", "-qb", "target"])
                .status
                .success()
        );
        fs::write(root.join("a"), "new-a\n").unwrap();
        fs::write(root.join("b"), "new-b\n").unwrap();
        assert!(git_at(&root, &["add", "a", "b"]).status.success());
        assert!(
            git_at(
                &root,
                &[
                    "-c",
                    "user.name=test",
                    "-c",
                    "user.email=test@example.com",
                    "commit",
                    "-qm",
                    "target"
                ]
            )
            .status
            .success()
        );
        let target = String::from_utf8(git_at(&root, &["rev-parse", "HEAD"]).stdout).unwrap();
        assert!(git_at(&root, &["checkout", "-q", "main"]).status.success());
        assert!(
            git_at(&root, &["reset", "--hard", base.trim()])
                .status
                .success()
        );
        fs::write(root.join("a"), "new-a\n").unwrap();
        prepare_main_sync(&root, target.trim()).expect("recover partial sync");
        assert_eq!(fs::read_to_string(root.join("a")).unwrap(), "old-a\n");
        let output = git_at(&root, &["merge", "--ff-only", target.trim()]);
        assert!(output.status.success());
        assert!(
            git_at(&root, &["status", "--porcelain=v1"])
                .stdout
                .is_empty()
        );

        assert!(
            git_at(&root, &["reset", "--hard", base.trim()])
                .status
                .success()
        );
        fs::write(root.join("a"), "new-a\n").unwrap();
        assert!(
            recover_interrupted_main_sync_with_hook(&root, target.trim(), base.trim(), || {
                fs::write(root.join("a"), "changed-after-validation\n").unwrap()
            },)
            .is_err()
        );
        assert_eq!(
            fs::read_to_string(root.join("a")).unwrap(),
            "changed-after-validation\n"
        );

        assert!(
            git_at(&root, &["reset", "--hard", base.trim()])
                .status
                .success()
        );
        fs::write(root.join("a"), "new-a\n").unwrap();
        assert!(
            recover_interrupted_main_sync_with_hook(&root, target.trim(), base.trim(), || {
                fs::write(root.join("racing-commit"), "new HEAD\n").unwrap();
                assert!(git_at(&root, &["add", "racing-commit"]).status.success());
                assert!(
                    git_at(
                        &root,
                        &[
                            "-c",
                            "user.name=test",
                            "-c",
                            "user.email=test@example.com",
                            "commit",
                            "-qm",
                            "racing HEAD"
                        ]
                    )
                    .status
                    .success()
                );
            },)
            .is_err()
        );
        assert_eq!(fs::read_to_string(root.join("a")).unwrap(), "new-a\n");

        assert!(
            git_at(&root, &["reset", "--hard", base.trim()])
                .status
                .success()
        );
        assert!(
            git_at(&root, &["checkout", "-qb", "feature-race"])
                .status
                .success()
        );
        assert!(git_at(&root, &["checkout", "-q", "main"]).status.success());
        fs::write(root.join("a"), "new-a\n").unwrap();
        assert!(
            recover_interrupted_main_sync_with_hook(&root, target.trim(), base.trim(), || {
                assert!(
                    git_at(&root, &["checkout", "-q", "feature-race"])
                        .status
                        .success()
                );
            },)
            .is_err()
        );
        assert_eq!(fs::read_to_string(root.join("a")).unwrap(), "new-a\n");
        assert_eq!(
            String::from_utf8(git_at(&root, &["symbolic-ref", "--short", "HEAD"]).stdout)
                .unwrap()
                .trim(),
            "feature-race"
        );
        assert!(git_at(&root, &["checkout", "-q", "main"]).status.success());

        assert!(
            git_at(&root, &["reset", "--hard", base.trim()])
                .status
                .success()
        );
        fs::write(root.join("a"), "unique-local-change\n").unwrap();
        assert!(prepare_main_sync(&root, target.trim()).is_err());
        assert_eq!(
            fs::read_to_string(root.join("a")).unwrap(),
            "unique-local-change\n"
        );

        assert!(
            git_at(&root, &["reset", "--hard", base.trim()])
                .status
                .success()
        );
        fs::write(root.join("a"), "new-a\n").unwrap();
        assert!(git_at(&root, &["add", "a"]).status.success());
        assert!(prepare_main_sync(&root, target.trim()).is_err());
        assert!(
            !git_at(&root, &["diff", "--cached", "--quiet"])
                .status
                .success()
        );

        assert!(
            git_at(&root, &["reset", "--hard", base.trim()])
                .status
                .success()
        );
        fs::write(root.join("a"), "new-a\n").unwrap();
        fs::write(root.join("untracked"), "must survive\n").unwrap();
        assert!(prepare_main_sync(&root, target.trim()).is_err());
        assert_eq!(
            fs::read_to_string(root.join("untracked")).unwrap(),
            "must survive\n"
        );

        fs::remove_file(root.join("untracked")).unwrap();
        assert!(
            git_at(&root, &["reset", "--hard", base.trim()])
                .status
                .success()
        );
        fs::write(root.join("local-only"), "local commit\n").unwrap();
        assert!(git_at(&root, &["add", "local-only"]).status.success());
        assert!(
            git_at(
                &root,
                &[
                    "-c",
                    "user.name=test",
                    "-c",
                    "user.email=test@example.com",
                    "commit",
                    "-qm",
                    "diverged"
                ]
            )
            .status
            .success()
        );
        let diverged = git_at(&root, &["rev-parse", "HEAD"]).stdout;
        fs::write(root.join("a"), "new-a\n").unwrap();
        let status_before = git_at(&root, &["status", "--porcelain=v1"]).stdout;
        assert!(prepare_main_sync(&root, target.trim()).is_err());
        assert_eq!(git_at(&root, &["rev-parse", "HEAD"]).stdout, diverged);
        assert_eq!(
            git_at(&root, &["status", "--porcelain=v1"]).stdout,
            status_before
        );
        assert_eq!(fs::read_to_string(root.join("a")).unwrap(), "new-a\n");
    }

    #[test]
    fn review_request_only_reuses_the_same_decision_and_risk() {
        assert!(review_request_is_idempotent(
            "low",
            "low",
            "autonomous",
            false
        ));
        assert!(review_request_is_idempotent(
            "high",
            "high",
            "human-approved",
            true
        ));
        assert!(!review_request_is_idempotent(
            "low",
            "low",
            "autonomous",
            true
        ));
        assert!(!review_request_is_idempotent(
            "low",
            "high",
            "autonomous",
            false
        ));
    }

    #[test]
    fn receipt_v1_and_v2_are_normalized_without_relaxing_legacy_approval() {
        let common = [
            ("kind", string("review")),
            ("task_id", string("issue-24")),
            ("repository", string("owner/repo")),
            ("pr", Value::Number(24.into())),
            ("head_sha", string("b".repeat(40))),
            ("risk", string("low")),
            ("plan_id", string("CODEX-DELIVERY-TEST-v1")),
            ("actionable", Value::Number(0.into())),
            ("human_approved", Value::Bool(false)),
            ("tests_passed", Value::Bool(true)),
            ("neutral_review_passed", Value::Bool(true)),
            ("adversarial_review_passed", Value::Bool(true)),
            ("changed_files", Value::Array(vec![string("src/main.rs")])),
            ("created_at", string("now")),
        ];
        let mut v1 = Map::new();
        v1.insert("version".into(), Value::Number(1.into()));
        v1.extend(
            common
                .into_iter()
                .map(|(key, value)| (key.to_string(), value)),
        );
        let normalized = receipt(
            &Value::Object(v1),
            Path::new("/unused"),
            "issue-24",
            &"b".repeat(40),
            Some("owner/repo"),
        )
        .unwrap();
        assert_eq!(receipt_gate_mode(&normalized).unwrap(), STRICT_GATE_MODE);
        assert_eq!(receipt_decision(&normalized).unwrap(), "autonomous");
        assert!(normalized.get("human_approved").is_none());

        let mut v2 = normalized.as_object().unwrap().clone();
        v2.insert("version".into(), Value::Number(2.into()));
        v2.insert("human_approved".into(), Value::Bool(true));
        v2.remove("decision");
        v2.insert("gate_mode".into(), string(FREE_PRIVATE_GATE_MODE));
        v2.insert("risk".into(), string("high"));
        let normalized = receipt(
            &Value::Object(v2),
            Path::new("/unused"),
            "issue-24",
            &"b".repeat(40),
            Some("owner/repo"),
        )
        .unwrap();
        assert_eq!(receipt_decision(&normalized).unwrap(), "human-approved");

        let mut rejected = normalized.as_object().unwrap().clone();
        rejected.insert("human_approved".into(), Value::Bool(false));
        assert!(
            receipt(
                &Value::Object(rejected),
                Path::new("/unused"),
                "issue-24",
                &"b".repeat(40),
                Some("owner/repo"),
            )
            .is_err()
        );
    }

    #[test]
    fn receipt_schema_and_type_spoofing_fail_closed() {
        let base = receipt_v5("low", false);
        let mut bool_version = base.as_object().unwrap().clone();
        bool_version.insert("version".into(), Value::Bool(true));
        assert!(
            receipt(
                &Value::Object(bool_version),
                Path::new("/unused"),
                "issue-24",
                &"b".repeat(40),
                Some("owner/repo"),
            )
            .is_err()
        );
        let mut legacy_field = base.as_object().unwrap().clone();
        legacy_field.insert("neutral_review_passed".into(), Value::Bool(true));
        assert!(
            receipt(
                &Value::Object(legacy_field),
                Path::new("/unused"),
                "issue-24",
                &"b".repeat(40),
                Some("owner/repo"),
            )
            .is_err()
        );
        let mut actionable = base.as_object().unwrap().clone();
        actionable.insert("actionable".into(), Value::Number(1.into()));
        assert!(
            receipt(
                &Value::Object(actionable),
                Path::new("/unused"),
                "issue-24",
                &"b".repeat(40),
                Some("owner/repo"),
            )
            .is_err()
        );
        let missing_plan_version = legacy_receipt_v5("low", false);
        assert!(
            receipt(
                &missing_plan_version,
                Path::new("/unused"),
                "issue-24",
                &"b".repeat(40),
                Some("owner/repo"),
            )
            .is_ok()
        );
        assert!(validate_receipt_loop_ledger(Path::new("/unused"), &missing_plan_version).is_err());
    }

    #[test]
    fn risk_gate_and_safety_path_contracts_are_explicit() {
        assert!(risk("low").is_ok());
        assert!(risk("medium").is_ok());
        assert!(risk("high").is_ok());
        assert!(risk("critical").is_ok());
        assert!(risk("urgent").is_err());
        assert!(decision("autonomous").is_ok());
        assert!(decision("human-approved").is_ok());
        assert!(decision("approved").is_err());
        assert!(gate_mode(STRICT_GATE_MODE).is_ok());
        assert!(gate_mode(FREE_PRIVATE_GATE_MODE).is_ok());
        assert!(validate_gate_repository("owner/repo", STRICT_GATE_MODE).is_ok());
        assert!(validate_gate_repository("owner/repo", FREE_PRIVATE_GATE_MODE).is_ok());
        assert!(validate_gate_repository("owner/repo/extra", STRICT_GATE_MODE).is_err());
        assert!(safety_path(".github/workflows/ci.yml"));
        assert!(safety_path(".codex/config.base.toml"));
        assert!(safety_path("packages/cli/src/main.rs"));
        assert!(!safety_path("src/main.rs"));
    }

    #[test]
    fn required_ci_validation_rejects_duplicate_wrong_or_pending_runs() {
        let good_run = object([
            ("name", string("required-ci")),
            ("app", object([("id", Value::Number(15368.into()))])),
            ("head_sha", string("b".repeat(40))),
            ("status", string("completed")),
            ("conclusion", string("success")),
            ("completed_at", string("now")),
        ]);
        let valid = object([("check_runs", Value::Array(vec![good_run.clone()]))]);
        assert!(validate_check_runs(&Value::Array(vec![valid]), &"b".repeat(40)).is_ok());
        assert!(
            validate_check_runs(
                &Value::Array(vec![object([(
                    "check_runs",
                    Value::Array(vec![good_run.clone(), good_run.clone()]),
                )])]),
                &"b".repeat(40),
            )
            .is_err()
        );
        let mut queued = good_run.as_object().unwrap().clone();
        queued.insert("status".into(), string("queued"));
        assert!(
            validate_check_runs(
                &Value::Array(vec![object([(
                    "check_runs",
                    Value::Array(vec![Value::Object(queued)]),
                )])]),
                &"b".repeat(40),
            )
            .is_err()
        );
    }

    #[test]
    fn state_schema_rejects_bool_version_and_invalid_stage_and_supports_resume() {
        with_codex_home(|home| {
            let managed = home.join("worktrees").join(repo_key("owner/repo").unwrap());
            let path = managed.join(".state/issue-24.delivery.json");
            let receipt = receipt_v3();
            let valid = object([
                ("version", Value::Number(1.into())),
                ("kind", string("delivery")),
                ("task_id", string("issue-24")),
                ("repository", string("owner/repo")),
                ("pr", Value::Number(24.into())),
                ("head_sha", string("b".repeat(40))),
                ("branch", string("feat/issue-24")),
                ("stage", string("merge_started")),
                ("updated_at", string("now")),
                ("last_error", string("")),
            ]);
            write_private(&path, &valid);
            assert_eq!(
                str_value(
                    &load_state("owner/repo", "issue-24", &receipt, "feat/issue-24").unwrap(),
                    "stage"
                )
                .unwrap(),
                "merge_started"
            );
            let resumed = save_stage("owner/repo", "issue-24", &valid, "merged", "").unwrap();
            assert_eq!(str_value(&resumed, "stage").unwrap(), "merged");
            let mut invalid = valid.as_object().unwrap().clone();
            invalid.insert("version".into(), Value::Bool(true));
            write_private(&path, &Value::Object(invalid));
            assert!(load_state("owner/repo", "issue-24", &receipt, "feat/issue-24").is_err());
            let mut invalid = valid.as_object().unwrap().clone();
            invalid.insert("stage".into(), string("unknown"));
            write_private(&path, &Value::Object(invalid));
            assert!(load_state("owner/repo", "issue-24", &receipt, "feat/issue-24").is_err());
        });
    }

    #[test]
    fn state_and_manifest_symlinks_are_not_trusted() {
        with_codex_home(|home| {
            let managed = home.join("worktrees").join(repo_key("owner/repo").unwrap());
            let state = managed.join(".state/issue-24.delivery.json");
            let target = managed.join(".state/state-target.json");
            write_private(&target, &receipt_v3());
            #[cfg(unix)]
            std::os::unix::fs::symlink(&target, &state).unwrap();
            #[cfg(unix)]
            assert!(open_private_regular(&state, "managed file").is_err());
        });
    }

    #[test]
    fn parser_rejects_explicit_strict_mode_and_accepts_free_private_only_explicitly() {
        let common = vec![
            OsString::from("record-review"),
            OsString::from("--task-id"),
            OsString::from("issue-24"),
            OsString::from("--pr"),
            OsString::from("24"),
            OsString::from("--head"),
            OsString::from("b".repeat(40)),
            OsString::from("--risk"),
            OsString::from("high"),
            OsString::from("--plan-id"),
            OsString::from("CODEX-DELIVERY-TEST-v1"),
            OsString::from("--plan-version"),
            OsString::from("2"),
            OsString::from("--tests-passed"),
            OsString::from("--independent-review-passed"),
            OsString::from("--specialist-review-passed"),
        ];
        let mut strict = common.clone();
        strict.extend([
            OsString::from("--gate-mode"),
            OsString::from(STRICT_GATE_MODE),
        ]);
        assert!(parse_args(strict).is_err());
        let mut free = common;
        free.extend([
            OsString::from("--gate-mode"),
            OsString::from(FREE_PRIVATE_GATE_MODE),
        ]);
        assert_eq!(parse_args(free).unwrap().gate, FREE_PRIVATE_GATE_MODE);
    }

    #[test]
    fn parser_enforces_the_risk_based_review_profile() {
        let args = |risk_value: &str, specialist: bool| {
            let mut values = vec![
                OsString::from("record-review"),
                OsString::from("--task-id"),
                OsString::from("issue-24"),
                OsString::from("--pr"),
                OsString::from("24"),
                OsString::from("--head"),
                OsString::from("b".repeat(40)),
                OsString::from("--risk"),
                OsString::from(risk_value),
                OsString::from("--plan-id"),
                OsString::from("CODEX-DELIVERY-TEST-v1"),
                OsString::from("--plan-version"),
                OsString::from("2"),
                OsString::from("--tests-passed"),
                OsString::from("--independent-review-passed"),
            ];
            if specialist {
                values.push(OsString::from("--specialist-review-passed"));
            }
            values
        };
        assert!(parse_args(args("low", false)).is_ok());
        assert!(parse_args(args("low", true)).is_err());
        assert!(parse_args(args("high", false)).is_err());
        assert!(parse_args(args("high", true)).is_ok());
    }

    #[test]
    fn parser_rejects_unknown_legacy_options_and_duplicate_flags() {
        let mut legacy = vec![
            OsString::from("record-review"),
            OsString::from("--task-id"),
            OsString::from("issue-24"),
            OsString::from("--pr"),
            OsString::from("24"),
            OsString::from("--head"),
            OsString::from("b".repeat(40)),
            OsString::from("--risk"),
            OsString::from("low"),
            OsString::from("--plan-id"),
            OsString::from("CODEX-DELIVERY-TEST-v1"),
            OsString::from("--plan-version"),
            OsString::from("2"),
            OsString::from("--tests-passed"),
            OsString::from("--independent-review-passed"),
        ];
        legacy.extend([
            OsString::from("--neutral-review-passed"),
            OsString::from("true"),
        ]);
        assert!(parse_args(legacy).is_err());

        let mut duplicate = vec![
            OsString::from("record-review"),
            OsString::from("--task-id"),
            OsString::from("issue-24"),
            OsString::from("--pr"),
            OsString::from("24"),
            OsString::from("--head"),
            OsString::from("b".repeat(40)),
            OsString::from("--risk"),
            OsString::from("low"),
            OsString::from("--plan-id"),
            OsString::from("CODEX-DELIVERY-TEST-v1"),
            OsString::from("--plan-version"),
            OsString::from("2"),
            OsString::from("--tests-passed"),
            OsString::from("--independent-review-passed"),
        ];
        duplicate.push(OsString::from("--independent-review-passed"));
        assert!(parse_args(duplicate).is_err());
    }

    #[test]
    fn sandbox_retry_is_finish_only_and_bound_to_a_single_ready_state() {
        let arguments = |command: &str| {
            vec![
                OsString::from(command),
                OsString::from("--task-id"),
                OsString::from("issue-24"),
                OsString::from("--pr"),
                OsString::from("24"),
                OsString::from("--head"),
                OsString::from("b".repeat(40)),
                OsString::from("--plan-id"),
                OsString::from("CODEX-DELIVERY-TEST-v1"),
                OsString::from("--plan-version"),
                OsString::from("2"),
                OsString::from("--sandbox-retry"),
            ]
        };
        assert!(parse_args(arguments("finish")).unwrap().sandbox_retry);
        assert!(parse_args(arguments("deliver")).is_err());
        assert!(validate_main_sync_retry("merged", MAIN_SYNC_SANDBOX_RETRY_READY, true).is_ok());
        assert!(
            validate_main_sync_retry("merged", MAIN_SYNC_SANDBOX_RETRY_CONSUMED, true).is_err()
        );
        assert!(validate_main_sync_retry("merged", MAIN_SYNC_SANDBOX_RETRY_READY, false).is_err());
        assert!(validate_main_sync_retry("main_synced", "", false).is_ok());
    }

    #[test]
    fn sandbox_retry_requires_a_readonly_error_on_a_changed_runtime_path() {
        let changed = vec![".codex/AGENTS.md".to_string(), "README.md".to_string()];
        assert!(sandbox_readonly_sync_failure(
            "error: unable to unlink old '.codex/AGENTS.md': Permission denied",
            &changed,
        ));
        assert!(!sandbox_readonly_sync_failure(
            "fatal: Not possible to fast-forward, aborting.",
            &changed,
        ));
        assert!(!sandbox_readonly_sync_failure(
            "error: unable to create file README.md: Permission denied",
            &changed,
        ));
        assert!(!sandbox_readonly_sync_failure(
            "error: .codex/AGENTS.md changed\nerror: index.lock: Permission denied",
            &changed,
        ));
    }

    #[test]
    fn canonical_invocation_resolves_a_bare_path_command_before_comparing_paths() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let directory = env::temp_dir().join(format!("codex-delivery-path-{suffix}"));
        fs::create_dir_all(&directory).unwrap();
        let binary = directory.join("codex-delivery");
        fs::write(&binary, b"binary").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let path_value =
            std::env::join_paths([PathBuf::from("/does-not-exist"), directory.clone()]).unwrap();
        let resolved =
            resolve_invocation_path(Path::new("codex-delivery"), Some(path_value.as_os_str()))
                .unwrap();
        assert_eq!(resolved, binary);
        assert_eq!(
            fs::canonicalize(&resolved).unwrap(),
            fs::canonicalize(&binary).unwrap()
        );
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn bounded_runner_rejects_output_overflow_and_timeout() {
        let overflow = run(
            &["/bin/sh".into(), "-c".into(), "printf '%*s' 64 x".into()],
            Path::new("/tmp"),
            Duration::from_secs(2),
            32,
        );
        assert!(overflow.is_err());
        let timeout = run(
            &["/bin/sh".into(), "-c".into(), "sleep 1".into()],
            Path::new("/tmp"),
            Duration::from_millis(20),
            MAX_OUTPUT_BYTES,
        );
        assert!(timeout.is_err());
    }

    #[test]
    fn receipt_timestamp_is_rfc3339_utc() {
        let timestamp = now();
        assert_eq!(timestamp.len(), 30);
        assert_eq!(&timestamp[4..5], "-");
        assert_eq!(&timestamp[7..8], "-");
        assert_eq!(&timestamp[10..11], "T");
        assert_eq!(&timestamp[19..20], ".");
        assert!(timestamp.ends_with('Z'));
    }

    #[test]
    fn cli_parser_requires_fixed_delivery_identity() {
        let args = vec![
            "deliver",
            "--task-id",
            "issue-24",
            "--pr",
            "24",
            "--head",
            &"b".repeat(40),
            "--plan-id",
            "CODEX-DELIVERY-TEST-v1",
            "--plan-version",
            "2",
        ]
        .into_iter()
        .map(OsString::from)
        .collect::<Vec<_>>();
        let parsed = parse_args(args).expect("parser should accept complete delivery args");
        assert_eq!(parsed.pr, 24);
        assert_eq!(parsed.plan_version, 2);
        assert_eq!(parsed.gate, STRICT_GATE_MODE);
        assert!(parse_args(vec![OsString::from("deliver")]).is_err());
        assert!(
            match_cli_receipt(
                &receipt_v5("low", false),
                Some(24),
                Some(&"b".repeat(40)),
                Some("CODEX-DELIVERY-TEST-v1"),
                Some(1),
                STRICT_GATE_MODE,
            )
            .is_err()
        );
    }
    #[test]
    fn bounded_capture_has_a_fixed_limit() {
        assert_eq!(MAX_OUTPUT_BYTES, 4 * 1024 * 1024);
        assert_eq!(COMMAND_TIMEOUT, 45);
        assert_eq!(OPERATION_TIMEOUT, 300);
    }

    #[test]
    fn local_git_execution_and_transport_keys_fail_closed() {
        for key in [
            "include.path",
            "includeIf.gitdir:path",
            "core.sshCommand",
            "credential.helper",
            "credential.https://github.com/owner/repo.helper",
            "submodule.recurse",
            "fetch.recurseSubmodules",
            "push.recurseSubmodules",
            "core.fsmonitor",
            "http.https://github.com/.sslVerify",
            "http.https://github.com/.extraHeader",
            "diff.external",
            "filter.lfs.process",
            "remote.origin.uploadpack",
            "remote.origin.pushurl",
            "url.ssh://evil/.insteadOf",
            "protocol.ext.allow",
        ] {
            assert!(dangerous_local_git_key(key), "{key} must be rejected");
        }
        assert!(!dangerous_local_git_key("remote.origin.url"));

        let command = git_command(&["fetch".into(), "origin".into()]).unwrap();
        for expected in [
            "credential.helper=",
            "credential.https://github.com.helper=!/usr/bin/gh auth git-credential",
            "submodule.recurse=false",
            "fetch.recurseSubmodules=false",
            "push.recurseSubmodules=no",
        ] {
            assert!(
                command.iter().any(|argument| argument == expected),
                "{expected}"
            );
        }
    }

    #[test]
    fn external_environment_isolated_from_proxy_and_user_github_config() {
        let mut command = Command::new("/bin/true");
        command
            .env("GH_CONFIG_DIR", "/tmp/user-config")
            .env("GH_TOKEN", "untrusted-token")
            .env("GITHUB_TOKEN", "untrusted-token")
            .env("GITHUB_ENTERPRISE_TOKEN", "untrusted-token")
            .env("GH_DEBUG", "api")
            .env("GH_FORCE_TTY", "1")
            .env("HTTPS_PROXY", "http://proxy.invalid")
            .env("GIT_SSH_COMMAND", "/tmp/ssh");
        let private = Path::new("/tmp/codex-delivery-private-gh");
        safe_environment(&mut command, Some(private));
        let values: HashMap<OsString, Option<OsString>> = command
            .get_envs()
            .map(|(key, value)| (key.to_os_string(), value.map(OsString::from)))
            .collect();
        assert_eq!(
            values.get(OsStr::new("GH_CONFIG_DIR")),
            Some(&Some(private.into()))
        );
        assert_eq!(
            values.get(OsStr::new("GH_HOST")),
            Some(&Some("github.com".into()))
        );
        for key in [
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "GITHUB_ENTERPRISE_TOKEN",
            "GH_DEBUG",
            "GH_FORCE_TTY",
        ] {
            assert_eq!(values.get(OsStr::new(key)), Some(&None), "{key}");
        }
        assert_eq!(values.get(OsStr::new("HTTPS_PROXY")), Some(&None));
        assert_eq!(values.get(OsStr::new("GIT_SSH_COMMAND")), Some(&None));
    }

    #[test]
    fn atomic_json_reopens_after_rename_and_leaves_no_temporary_file() {
        with_codex_home(|home| {
            let state = home
                .join("worktrees")
                .join(repo_key("owner/repo").unwrap())
                .join(".state/durability.json");
            let value = object([
                ("version", Value::Number(1.into())),
                ("kind", string("delivery")),
            ]);
            atomic_json(&state, &value).unwrap();
            assert_eq!(read_json_file(&state, "durability").unwrap(), value);
            let temporary = fs::read_dir(state.parent().unwrap())
                .unwrap()
                .filter_map(|entry| entry.ok())
                .any(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .contains("durability.json.tmp")
                });
            assert!(!temporary, "atomic temporary file must not remain");
        });
    }

    #[test]
    fn gh_snapshots_keep_auth_writable_and_body_immutable() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let root = env::temp_dir().join(format!("codex-delivery-body-{suffix}"));
        fs::create_dir(&root).unwrap();
        #[cfg(unix)]
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let source = root.join("body.md");
        fs::write(&source, b"safe body").unwrap();
        #[cfg(unix)]
        fs::set_permissions(&source, fs::Permissions::from_mode(0o600)).unwrap();
        let sandbox_path = root.join("sandbox");
        fs::create_dir(&sandbox_path).unwrap();
        #[cfg(unix)]
        fs::set_permissions(&sandbox_path, fs::Permissions::from_mode(0o700)).unwrap();
        let sandbox = GhSandbox { path: sandbox_path };
        let hosts = sandbox.write_auth_hosts(b"github.com:\n").unwrap();
        let args = vec![
            "pr".into(),
            "create".into(),
            "--body-file".into(),
            source.display().to_string(),
        ];
        let rewritten = sandbox.snapshot_args(&args).unwrap();
        fs::write(&source, b"secret replacement").unwrap();
        let snapshot = rewritten.last().unwrap();
        assert_eq!(fs::read(snapshot).unwrap(), b"safe body");
        #[cfg(unix)]
        {
            assert_eq!(
                fs::metadata(snapshot).unwrap().permissions().mode() & 0o777,
                0o400
            );
            assert_eq!(
                fs::metadata(&hosts).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(&sandbox.path).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        drop(sandbox);
        fs::remove_dir_all(&root).unwrap();
        assert!(!root.exists());
    }
}

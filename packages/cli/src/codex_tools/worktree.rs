//! `codex-worktree` の Rust 実装。
//!
//! このモジュールは通常の CLI としても、親 CLI から呼び出すライブラリとしても
//! 使えるように、プロセス終了を行わず `entrypoint` から終了コードを返す。Git の
//! 実行は常に安全な環境、30 秒の deadline、4 MiB の bounded capture で行う。

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserializer;
use serde::de::{self, MapAccess, Visitor};
use serde_json::{Map, Value};

use super::trust;

#[cfg(test)]
use std::io::Read;
#[cfg(test)]
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
#[cfg(unix)]
use std::os::unix::io::AsRawFd;

const MANIFEST_VERSION: i64 = 1;
const MAX_MANIFEST_BYTES: u64 = 256 * 1024;
#[cfg(test)]
const MAX_CAPTURE_BYTES: usize = 4 * 1024 * 1024;
const GIT_TIMEOUT: Duration = Duration::from_secs(30);

#[cfg(unix)]
const TRUSTED_GIT_COMMAND: &str = "/usr/bin/git";
#[cfg(not(unix))]
const TRUSTED_GIT_COMMAND: &str = "git";
#[cfg(unix)]
const TRUSTED_SSH_COMMAND: &str = "/usr/bin/ssh";
#[cfg(not(unix))]
const TRUSTED_SSH_COMMAND: &str = "ssh";
const SYSTEM_PATH: &str = "/usr/bin:/bin";

const BRANCH_PREFIXES: &[&str] = &[
    "feat", "feature", "fix", "refactor", "docs", "test", "chore", "ci", "build", "perf", "style",
    "hotfix", "update",
];
const PROTECTED_BRANCHES: &[&str] = &["main", "master", "develop", "development", "trunk"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeError(pub String);

impl std::fmt::Display for WorktreeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for WorktreeError {}

fn error(message: impl Into<String>) -> WorktreeError {
    WorktreeError(message.into())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Repository {
    root: PathBuf,
    common_git_dir: PathBuf,
    github_name: String,
    default_branch: String,
    default_oid: String,
}

impl Repository {
    fn key(&self) -> Result<String, WorktreeError> {
        repository_key(&self.github_name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Manifest {
    version: i64,
    status: String,
    task_id: String,
    repository: String,
    common_git_dir: String,
    github_name: String,
    branch: String,
    base: String,
    base_oid: String,
    worktree: String,
    created_at: String,
    detail: String,
}

impl Manifest {
    fn json(&self) -> Result<String, WorktreeError> {
        // Python の json.dump(..., ensure_ascii=False, indent=2, sort_keys=True) と同じ
        // key 順・改行を保つ。serde_jsonはUTF-8とsurrogate pairを正しく処理する。
        let mut fields = BTreeMap::new();
        fields.insert("base", Value::String(self.base.clone()));
        fields.insert("base_oid", Value::String(self.base_oid.clone()));
        fields.insert("branch", Value::String(self.branch.clone()));
        fields.insert("common_git_dir", Value::String(self.common_git_dir.clone()));
        fields.insert("created_at", Value::String(self.created_at.clone()));
        fields.insert("detail", Value::String(self.detail.clone()));
        fields.insert("github_name", Value::String(self.github_name.clone()));
        fields.insert("repository", Value::String(self.repository.clone()));
        fields.insert("status", Value::String(self.status.clone()));
        fields.insert("task_id", Value::String(self.task_id.clone()));
        fields.insert("version", Value::Number(self.version.into()));
        fields.insert("worktree", Value::String(self.worktree.clone()));
        let fields: Map<String, Value> = fields
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect();
        let mut output = serde_json::to_string_pretty(&Value::Object(fields))
            .map_err(|cause| error(format!("manifest JSONを生成できません: {cause}")))?;
        output.push('\n');
        Ok(output)
    }
}

#[derive(Debug, Clone, PartialEq)]
enum JsonValue {
    Object(BTreeMap<String, JsonValue>),
    String(String),
    Number(i64),
    Bool(bool),
    Null,
}

#[cfg(test)]
fn json_escape(value: &str) -> String {
    let mut output = String::with_capacity(value.len() + 2);
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{08}' => output.push_str("\\b"),
            '\u{0c}' => output.push_str("\\f"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character <= '\u{1f}' => {
                output.push_str(&format!("\\u{:04x}", character as u32));
            }
            character => output.push(character),
        }
    }
    output.push('"');
    output
}

#[cfg(test)]
fn json_pretty(value: &JsonValue, indent: usize) -> String {
    match value {
        JsonValue::String(value) => json_escape(value),
        JsonValue::Number(value) => value.to_string(),
        JsonValue::Bool(value) => value.to_string(),
        JsonValue::Null => "null".to_string(),
        JsonValue::Object(values) => {
            if values.is_empty() {
                return "{}".to_string();
            }
            let mut output = String::from("{");
            for (index, (key, value)) in values.iter().enumerate() {
                output.push('\n');
                output.push_str(&" ".repeat(indent + 2));
                output.push_str(&json_escape(key));
                output.push_str(": ");
                output.push_str(&json_pretty(value, indent + 2));
                if index + 1 != values.len() {
                    output.push(',');
                }
            }
            output.push('\n');
            output.push_str(&" ".repeat(indent));
            output.push('}');
            output
        }
    }
}

#[cfg(test)]
fn json_pretty_object(values: &BTreeMap<&str, JsonValue>) -> String {
    let owned = values
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect();
    format!("{}\n", json_pretty(&JsonValue::Object(owned), 0))
}

#[cfg(test)]
#[allow(dead_code)]
struct JsonParser<'a> {
    bytes: &'a [u8],
    position: usize,
}

#[cfg(test)]
#[allow(dead_code)]
impl<'a> JsonParser<'a> {
    fn new(value: &'a str) -> Self {
        Self {
            bytes: value.as_bytes(),
            position: 0,
        }
    }

    fn parse(mut self) -> Result<JsonValue, WorktreeError> {
        let value = self.value()?;
        self.space();
        if self.position != self.bytes.len() {
            return Err(error("manifest JSON末尾を解析できません"));
        }
        Ok(value)
    }

    fn space(&mut self) {
        while self
            .bytes
            .get(self.position)
            .is_some_and(|byte| byte.is_ascii_whitespace())
        {
            self.position += 1;
        }
    }

    fn value(&mut self) -> Result<JsonValue, WorktreeError> {
        self.space();
        match self.bytes.get(self.position).copied() {
            Some(b'{') => self.object(),
            Some(b'"') => Ok(JsonValue::String(self.string()?)),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(b't') if self.take(b"true") => Ok(JsonValue::Bool(true)),
            Some(b'f') if self.take(b"false") => Ok(JsonValue::Bool(false)),
            Some(b'n') if self.take(b"null") => Ok(JsonValue::Null),
            _ => Err(error("manifest JSON valueを解析できません")),
        }
    }

    fn take(&mut self, expected: &[u8]) -> bool {
        if self
            .bytes
            .get(self.position..self.position + expected.len())
            == Some(expected)
        {
            self.position += expected.len();
            true
        } else {
            false
        }
    }

    fn object(&mut self) -> Result<JsonValue, WorktreeError> {
        self.position += 1;
        let mut values = BTreeMap::new();
        self.space();
        if self.bytes.get(self.position) == Some(&b'}') {
            self.position += 1;
            return Ok(JsonValue::Object(values));
        }
        loop {
            self.space();
            if self.bytes.get(self.position) != Some(&b'"') {
                return Err(error("manifest object keyを解析できません"));
            }
            let key = self.string()?;
            self.space();
            if self.bytes.get(self.position) != Some(&b':') {
                return Err(error("manifest object separatorを解析できません"));
            }
            self.position += 1;
            let value = self.value()?;
            if values.insert(key, value).is_some() {
                return Err(error("manifestに重複keyがあります"));
            }
            self.space();
            match self.bytes.get(self.position) {
                Some(b',') => self.position += 1,
                Some(b'}') => {
                    self.position += 1;
                    return Ok(JsonValue::Object(values));
                }
                _ => return Err(error("manifest object末尾を解析できません")),
            }
        }
    }

    fn string(&mut self) -> Result<String, WorktreeError> {
        if self.bytes.get(self.position) != Some(&b'"') {
            return Err(error("manifest stringを解析できません"));
        }
        self.position += 1;
        let mut output = String::new();
        while let Some(byte) = self.bytes.get(self.position).copied() {
            self.position += 1;
            match byte {
                b'"' => return Ok(output),
                b'\\' => {
                    let escaped = self
                        .bytes
                        .get(self.position)
                        .copied()
                        .ok_or_else(|| error("manifest escapeが不完全です"))?;
                    self.position += 1;
                    match escaped {
                        b'"' => output.push('"'),
                        b'\\' => output.push('\\'),
                        b'/' => output.push('/'),
                        b'b' => output.push('\u{08}'),
                        b'f' => output.push('\u{0c}'),
                        b'n' => output.push('\n'),
                        b'r' => output.push('\r'),
                        b't' => output.push('\t'),
                        b'u' => {
                            let digits = self
                                .bytes
                                .get(self.position..self.position + 4)
                                .ok_or_else(|| error("manifest unicode escapeが不完全です"))?;
                            let text = std::str::from_utf8(digits)
                                .map_err(|_| error("manifest unicode escapeが不正です"))?;
                            let code = u16::from_str_radix(text, 16)
                                .map_err(|_| error("manifest unicode escapeが不正です"))?;
                            self.position += 4;
                            let character = char::from_u32(code as u32)
                                .ok_or_else(|| error("manifest unicode escapeが不正です"))?;
                            output.push(character);
                        }
                        _ => return Err(error("manifest escapeが不正です")),
                    }
                }
                byte if byte < 0x20 => return Err(error("manifest stringに制御文字があります")),
                byte => {
                    let start = self.position - 1;
                    let width =
                        utf8_width(byte).ok_or_else(|| error("manifest UTF-8が不正です"))?;
                    let end = start + width;
                    if end > self.bytes.len() {
                        return Err(error("manifest UTF-8が不完全です"));
                    }
                    let text = std::str::from_utf8(&self.bytes[start..end])
                        .map_err(|_| error("manifest UTF-8が不正です"))?;
                    output.push_str(text);
                    self.position = end;
                }
            }
        }
        Err(error("manifest stringが閉じられていません"))
    }

    fn number(&mut self) -> Result<JsonValue, WorktreeError> {
        let start = self.position;
        if self.bytes.get(self.position) == Some(&b'-') {
            self.position += 1;
        }
        while self
            .bytes
            .get(self.position)
            .is_some_and(|byte| byte.is_ascii_digit())
        {
            self.position += 1;
        }
        let text = std::str::from_utf8(&self.bytes[start..self.position])
            .map_err(|_| error("manifest numberが不正です"))?;
        let number = text
            .parse::<i64>()
            .map_err(|_| error("manifest numberが不正です"))?;
        Ok(JsonValue::Number(number))
    }
}

#[cfg(test)]
#[allow(dead_code)]
fn utf8_width(byte: u8) -> Option<usize> {
    match byte {
        0..=0x7f => Some(1),
        0xc2..=0xdf => Some(2),
        0xe0..=0xef => Some(3),
        0xf0..=0xf4 => Some(4),
        _ => None,
    }
}

struct StrictManifestMapVisitor;

impl<'de> Visitor<'de> for StrictManifestMapVisitor {
    type Value = BTreeMap<String, Value>;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("manifest JSON object")
    }

    fn visit_map<A>(self, mut access: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut values = BTreeMap::new();
        while let Some(key) = access.next_key::<String>()? {
            if values.contains_key(&key) {
                return Err(de::Error::custom("manifestに重複keyがあります"));
            }
            values.insert(key, access.next_value::<Value>()?);
        }
        Ok(values)
    }
}

fn parse_manifest(contents: &str) -> Result<BTreeMap<String, JsonValue>, WorktreeError> {
    let mut deserializer = serde_json::Deserializer::from_str(contents);
    let values: BTreeMap<String, Value> = deserializer
        .deserialize_map(StrictManifestMapVisitor)
        .map_err(|cause| error(format!("manifest JSONを解析できません: {cause}")))?;
    deserializer
        .end()
        .map_err(|cause| error(format!("manifest JSON末尾を解析できません: {cause}")))?;
    Ok(values
        .into_iter()
        .map(|(key, value)| (key, serde_value_to_legacy(value)))
        .collect())
}

fn serde_value_to_legacy(value: Value) -> JsonValue {
    match value {
        Value::Object(values) => JsonValue::Object(
            values
                .into_iter()
                .map(|(key, value)| (key, serde_value_to_legacy(value)))
                .collect(),
        ),
        Value::String(value) => JsonValue::String(value),
        Value::Number(value) => value
            .as_i64()
            .map(JsonValue::Number)
            .unwrap_or(JsonValue::Null),
        Value::Bool(value) => JsonValue::Bool(value),
        Value::Array(_) | Value::Null => JsonValue::Null,
    }
}

fn json_string(values: &BTreeMap<String, JsonValue>, key: &str) -> Result<String, WorktreeError> {
    match values.get(key) {
        Some(JsonValue::String(value)) => Ok(value.clone()),
        _ => Err(error("manifest schema mismatch")),
    }
}

fn json_number(values: &BTreeMap<String, JsonValue>, key: &str) -> Result<i64, WorktreeError> {
    match values.get(key) {
        Some(JsonValue::Number(value)) => Ok(*value),
        _ => Err(error("manifest schema mismatch")),
    }
}

fn json_manifest(values: BTreeMap<String, JsonValue>) -> Result<Manifest, WorktreeError> {
    const FIELDS: &[&str] = &[
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
    ];
    if values.len() != FIELDS.len() || FIELDS.iter().any(|field| !values.contains_key(*field)) {
        return Err(error("manifest schema mismatch"));
    }
    Ok(Manifest {
        version: json_number(&values, "version")?,
        status: json_string(&values, "status")?,
        task_id: json_string(&values, "task_id")?,
        repository: json_string(&values, "repository")?,
        common_git_dir: json_string(&values, "common_git_dir")?,
        github_name: json_string(&values, "github_name")?,
        branch: json_string(&values, "branch")?,
        base: json_string(&values, "base")?,
        base_oid: json_string(&values, "base_oid")?,
        worktree: json_string(&values, "worktree")?,
        created_at: json_string(&values, "created_at")?,
        detail: json_string(&values, "detail")?,
    })
}

#[cfg(test)]
#[derive(Debug)]
struct Capture {
    bytes: Vec<u8>,
    oversized: bool,
}

#[cfg(test)]
fn capture_pipe<R: Read>(mut reader: R, exceeded: Arc<AtomicBool>) -> Capture {
    let mut bytes = Vec::with_capacity(4096.min(MAX_CAPTURE_BYTES));
    let mut buffer = [0_u8; 8192];
    let mut oversized = false;
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(size) => {
                if bytes.len() < MAX_CAPTURE_BYTES {
                    let room = MAX_CAPTURE_BYTES - bytes.len();
                    bytes.extend_from_slice(&buffer[..size.min(room)]);
                }
                if bytes.len() >= MAX_CAPTURE_BYTES
                    && size > MAX_CAPTURE_BYTES.saturating_sub(bytes.len())
                {
                    oversized = true;
                    exceeded.store(true, Ordering::Release);
                }
            }
            Err(_) => break,
        }
    }
    Capture { bytes, oversized }
}

#[derive(Debug)]
struct GitOutput {
    stdout: String,
    stderr: String,
}

fn safe_environment(command: &mut Command) {
    command.env_remove("SSH_ASKPASS");
    // `env_clear` would drop PATH and locale, so explicitly remove only Git's
    // environment controls. GIT_CONFIG_NOSYSTEM and an empty global config
    // keep system/global configuration out of every internal Git invocation.
    for (key, _) in std::env::vars_os() {
        let text = key.to_string_lossy();
        if text.starts_with("GIT_")
            || text.eq_ignore_ascii_case("HTTP_PROXY")
            || text.eq_ignore_ascii_case("HTTPS_PROXY")
            || text.eq_ignore_ascii_case("ALL_PROXY")
            || text.eq_ignore_ascii_case("NO_PROXY")
            || text.eq_ignore_ascii_case("SSL_CERT_FILE")
            || text.eq_ignore_ascii_case("SSL_CERT_DIR")
        {
            command.env_remove(key);
        }
    }
    let overridden_keys: Vec<OsString> = command
        .get_envs()
        .filter(|(key, value)| {
            let text = key.to_string_lossy();
            value.is_some()
                && (text.starts_with("GIT_")
                    || text.eq_ignore_ascii_case("HTTP_PROXY")
                    || text.eq_ignore_ascii_case("HTTPS_PROXY")
                    || text.eq_ignore_ascii_case("ALL_PROXY")
                    || text.eq_ignore_ascii_case("NO_PROXY")
                    || text.eq_ignore_ascii_case("SSL_CERT_FILE")
                    || text.eq_ignore_ascii_case("SSL_CERT_DIR"))
        })
        .map(|(key, _)| key.to_os_string())
        .collect();
    for key in overridden_keys {
        command.env_remove(key);
    }
    command.env("GIT_CONFIG_NOSYSTEM", "1");
    #[cfg(unix)]
    command.env("GIT_CONFIG_GLOBAL", "/dev/null");
    #[cfg(windows)]
    command.env("GIT_CONFIG_GLOBAL", "NUL");
    command.env("GIT_TERMINAL_PROMPT", "0");
    command.env("GIT_SSH_VARIANT", "ssh");
    command.env("GIT_PAGER", "cat");
    command.env("PATH", SYSTEM_PATH);
}

fn isolated_git_command(cwd: &Path, arguments: &[&str]) -> Result<Command, WorktreeError> {
    let git = trust::trusted_system_binary(TRUSTED_GIT_COMMAND, "Git").map_err(error)?;
    let ssh = trust::trusted_system_binary(TRUSTED_SSH_COMMAND, "SSH").map_err(error)?;
    let mut command = Command::new(git);
    // Command-line config has higher precedence than repository-local config.
    // Values that can select or execute another process are therefore pinned
    // for every Git subcommand, including network operations.
    command
        .current_dir(cwd)
        .arg("--no-pager")
        .arg("-c")
        .arg("core.hooksPath=/dev/null")
        .arg("-c")
        .arg(format!("core.sshCommand={ssh} -F /dev/null"))
        .arg("-c")
        .arg("core.gitProxy=")
        .arg("-c")
        .arg("core.fsmonitor=")
        .arg("-c")
        .arg("credential.helper=")
        .arg("-c")
        .arg("http.proxy=")
        .arg("-c")
        .arg("https.proxy=")
        .arg("-c")
        .arg("protocol.ext.allow=never")
        .arg("-c")
        .arg(if cfg!(test) {
            "protocol.file.allow=always"
        } else {
            "protocol.file.allow=never"
        })
        .arg("-c")
        .arg("protocol.git.allow=never")
        .arg("-c")
        .arg("http.sslVerify=true")
        .arg("-c")
        .arg("remote.origin.uploadpack=git-upload-pack")
        .arg("-c")
        .arg("remote.origin.receivepack=git-receive-pack")
        .arg("-c")
        .arg("core.bare=false")
        .arg("--work-tree")
        .arg(cwd)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    safe_environment(&mut command);
    Ok(command)
}

fn local_config_keys(cwd: &Path) -> Result<Vec<String>, WorktreeError> {
    let mut command = isolated_git_command(
        cwd,
        &[
            "config",
            "--local",
            "--includes",
            "--name-only",
            "--null",
            "--list",
        ],
    )?;
    let output = crate::codex_tools::process::run(&mut command, GIT_TIMEOUT)
        .map_err(|cause| error(format!("repository configを確認できません: {cause}")))?;
    if !output.status.success() {
        return Err(error("repository configを安全に確認できません"));
    }
    let text = String::from_utf8(output.stdout)
        .map_err(|_| error("repository configのkeyがUTF-8ではありません"))?;
    Ok(text
        .split('\0')
        .filter(|key| !key.is_empty())
        .map(str::to_ascii_lowercase)
        .collect())
}

fn unsafe_local_config_key(key: &str) -> bool {
    key == "include.path"
        || key.starts_with("includeif.")
        || key == "core.sshcommand"
        || key == "core.gitproxy"
        || key == "core.fsmonitor"
        || key == "credential.helper"
        || key == "http.proxy"
        || key == "https.proxy"
        || key == "protocol.ext.allow"
        || key.starts_with("protocol.") && key.ends_with(".allow")
        || key == "http.sslverify"
        || key == "http.sslcainfo"
        || key.starts_with("http.")
            && (key.ends_with(".proxy")
                || key.ends_with(".extraheader")
                || key.ends_with(".proxycommand")
                || key.ends_with(".sslcainfo")
                || key.ends_with(".sslverify")
                || key.ends_with(".sslcert")
                || key.ends_with(".sslkey"))
        || key.starts_with("url.")
            && (key.ends_with(".insteadof") || key.ends_with(".pushinsteadof"))
        || key.starts_with("http.")
            && (key.ends_with(".proxy")
                || key.ends_with(".extraheader")
                || key.ends_with(".proxycommand"))
        || key.starts_with("remote.")
            && (key.ends_with(".uploadpack")
                || key.ends_with(".receivepack")
                || key.ends_with(".proxy"))
        || key.starts_with("filter.")
            && (key.ends_with(".process") || key.ends_with(".clean") || key.ends_with(".smudge"))
        || key == "diff.external"
        || key.starts_with("diff.") && (key.ends_with(".command") || key.ends_with(".textconv"))
        || key.starts_with("merge.") && key.ends_with(".driver")
        || key == "interactive.difffilter"
        || key == "gc.recentobjectshook"
        || key == "core.alternaterefscommand"
}

fn validate_local_config(cwd: &Path) -> Result<(), WorktreeError> {
    if let Some(key) = local_config_keys(cwd)?
        .into_iter()
        .find(|key| unsafe_local_config_key(key))
    {
        return Err(error(format!(
            "repository configの実行・transport設定を安全に隔離できません: {key}"
        )));
    }
    Ok(())
}

trait EmptyFallback {
    fn if_empty(self, fallback: &str) -> String;
}
impl EmptyFallback for String {
    fn if_empty(self, fallback: &str) -> String {
        if self.is_empty() {
            fallback.to_string()
        } else {
            self
        }
    }
}

fn git(cwd: &Path, arguments: &[&str]) -> Result<GitOutput, WorktreeError> {
    validate_local_config(cwd)?;
    let mut command = isolated_git_command(cwd, arguments)?;
    let output = crate::codex_tools::process::run(&mut command, GIT_TIMEOUT)
        .map_err(|cause| error(format!("gitを実行できません: {cause}")))?;
    let stdout =
        String::from_utf8(output.stdout).map_err(|_| error("git stdoutがUTF-8ではありません"))?;
    let stderr =
        String::from_utf8(output.stderr).map_err(|_| error("git stderrがUTF-8ではありません"))?;
    if !output.status.success() {
        return Err(error(
            stderr.trim().to_string().if_empty("Git operation failed"),
        ));
    }
    Ok(GitOutput { stdout, stderr })
}

fn git_allow_failure(cwd: &Path, arguments: &[&str]) -> Result<(i32, GitOutput), WorktreeError> {
    // Branch existence checks need the exit status. The same bounded runner is
    // used, but non-zero output is not converted into WorktreeError here.
    validate_local_config(cwd)?;
    let mut command = isolated_git_command(cwd, arguments)?;
    let output = crate::codex_tools::process::run(&mut command, GIT_TIMEOUT)
        .map_err(|cause| error(format!("gitを実行できません: {cause}")))?;
    let stdout =
        String::from_utf8(output.stdout).map_err(|_| error("git stdoutがUTF-8ではありません"))?;
    let stderr =
        String::from_utf8(output.stderr).map_err(|_| error("git stderrがUTF-8ではありません"))?;
    Ok((
        output.status.code().unwrap_or(1),
        GitOutput { stdout, stderr },
    ))
}

fn git_stdout(cwd: &Path, arguments: &[&str]) -> Result<String, WorktreeError> {
    Ok(git(cwd, arguments)?.stdout)
}

fn local_config_values(cwd: &Path, key: &str) -> Result<Vec<String>, WorktreeError> {
    let mut command = isolated_git_command(
        cwd,
        &[
            "config",
            "--local",
            "--includes",
            "--null",
            "--get-all",
            key,
        ],
    )?;
    let output = crate::codex_tools::process::run(&mut command, GIT_TIMEOUT)
        .map_err(|cause| error(format!("repository configを確認できません: {cause}")))?;
    if !output.status.success() {
        if output.status.code() == Some(1) {
            return Ok(Vec::new());
        }
        return Err(error(format!(
            "repository configの{key}を安全に確認できません"
        )));
    }
    let text = String::from_utf8(output.stdout)
        .map_err(|_| error(format!("repository configの{key}がUTF-8ではありません")))?;
    Ok(text
        .split('\0')
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect())
}

fn safe_local_origin(url: &str) -> bool {
    let value = url.trim();
    if value.is_empty()
        || value.starts_with('-')
        || value.bytes().any(|byte| byte.is_ascii_control())
    {
        return false;
    }
    let path = Path::new(value);
    path.is_absolute()
        || (!value.contains("://") && !value.starts_with("git@") && !value.contains(':'))
}

fn origin_urls(root: &Path) -> Result<(String, String), WorktreeError> {
    validate_local_config(root)?;
    let fetch_urls = local_config_values(root, "remote.origin.url")?;
    let push_urls = local_config_values(root, "remote.origin.pushurl")?;
    if fetch_urls.len() != 1 || push_urls.len() > 1 {
        return Err(error(
            "originのfetch/push URLはそれぞれ1件に限定してください",
        ));
    }
    let fetch = fetch_urls
        .into_iter()
        .next()
        .ok_or_else(|| error("originのfetch URLを安全に確認できません"))?
        .trim()
        .to_string();
    let push = push_urls
        .into_iter()
        .next()
        .map(|value| value.trim().to_string())
        .unwrap_or_else(|| fetch.clone());
    Ok((fetch, push))
}

fn absolute_git_path(cwd: &Path, argument: &str) -> Result<PathBuf, WorktreeError> {
    let output = git_stdout(cwd, &["rev-parse", "--path-format=absolute", argument])?;
    let path = PathBuf::from(output.trim());
    if path.as_os_str().is_empty() {
        return Err(error(format!("Git pathを確認できません: {argument}")));
    }
    fs::canonicalize(&path).map_err(|cause| error(format!("Git pathを解決できません: {cause}")))
}

fn github_repository(remote: &str) -> Option<String> {
    let value = remote.trim().trim_end_matches('/');
    let value = [
        "https://github.com/",
        "ssh://git@github.com/",
        "git@github.com:",
    ]
    .iter()
    .find_map(|prefix| value.strip_prefix(prefix))?;
    let repository = value.strip_suffix(".git").unwrap_or(value);
    let valid = repository.split_once('/').is_some_and(|(owner, name)| {
        !owner.is_empty()
            && !name.is_empty()
            && owner.bytes().all(valid_github_byte)
            && name.bytes().all(valid_github_byte)
    });
    valid.then(|| repository.to_string())
}

fn valid_github_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-' | b'/')
}

fn origin_identity(root: &Path, allow_local_origin: bool) -> Result<String, WorktreeError> {
    let (fetch, push) = origin_urls(root)?;
    let fetch_repository = github_repository(&fetch);
    let push_repository = github_repository(&push);
    if fetch_repository.is_none() || push_repository != fetch_repository {
        if allow_local_origin && fetch == push && safe_local_origin(&fetch) {
            return Ok("test/local".to_string());
        }
        return Err(error(
            "originのfetch/push先は同一GitHub repositoryにしてください",
        ));
    }
    fetch_repository.ok_or_else(|| error("origin repositoryを確認できません"))
}

fn is_protected(branch: &str) -> bool {
    PROTECTED_BRANCHES.contains(&branch)
        || branch
            .strip_prefix("release")
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
        || branch
            .strip_prefix("production")
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

fn valid_oid(value: &str) -> bool {
    (value.len() == 40 || value.len() == 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn remote_default(root: &Path) -> Result<(String, String), WorktreeError> {
    let (origin, _) = origin_urls(root)?;
    let arguments = [
        "ls-remote",
        "--upload-pack=git-upload-pack",
        "--symref",
        origin.as_str(),
        "HEAD",
    ];
    let output = git_stdout(root, &arguments)?;
    let lines: Vec<&str> = output.lines().collect();
    if lines.len() != 2 {
        return Err(error("originのdefault branchを一意に確認できません"));
    }
    let (ref_part, head_part) = lines[0]
        .split_once('\t')
        .ok_or_else(|| error("origin HEADの応答を安全に解析できません"))?;
    let branch = ref_part
        .strip_prefix("ref: refs/heads/")
        .filter(|_| head_part == "HEAD")
        .ok_or_else(|| error("origin HEADの応答を安全に解析できません"))?;
    let (oid, head) = lines[1]
        .split_once('\t')
        .ok_or_else(|| error("origin HEADの応答を安全に解析できません"))?;
    if head != "HEAD" || !valid_oid(oid) || !is_protected(branch) {
        return Err(error("origin HEADの応答を安全に解析できません"));
    }
    Ok((branch.to_string(), oid.to_ascii_lowercase()))
}

fn inspect_repository(
    cwd: &Path,
    allow_local_origin: bool,
    require_remote: bool,
) -> Result<Repository, WorktreeError> {
    let root = absolute_git_path(cwd, "--show-toplevel")?;
    let common_git_dir = absolute_git_path(cwd, "--git-common-dir")?;
    if root
        != common_git_dir
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default()
    {
        return Err(error("worktree作成はmain checkoutから実行してください"));
    }
    let github_name = origin_identity(&root, allow_local_origin)?;
    let (default_branch, default_oid) = if require_remote {
        remote_default(&root)?
    } else {
        (String::new(), String::new())
    };
    Ok(Repository {
        root,
        common_git_dir,
        github_name,
        default_branch,
        default_oid,
    })
}

fn repository_key(github_name: &str) -> Result<String, WorktreeError> {
    let (owner, name) = github_name
        .split_once('/')
        .ok_or_else(|| error("GitHub repository名が不正です"))?;
    Ok(format!(
        "{}-{}--{}-{}",
        owner.len(),
        owner.to_ascii_lowercase(),
        name.len(),
        name.to_ascii_lowercase()
    ))
}

fn generate_task_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let seconds = nanos / 1_000_000_000;
    let remainder = nanos % 1_000_000_000;
    // UTC formatting without pulling a date crate. This is enough to preserve
    // the documented task-id shape; the timestamp remains monotonic enough for
    // IDs generated by one process.
    let days = seconds / 86_400;
    let day_seconds = seconds % 86_400;
    let (year, month, day) = civil_from_days(days as i64);
    let hour = day_seconds / 3600;
    let minute = (day_seconds % 3600) / 60;
    let second = day_seconds % 60;
    let entropy = (nanos ^ ((std::process::id() as u128) << 32) ^ remainder) as u64;
    format!("task-{year:04}{month:02}{day:02}t{hour:02}{minute:02}{second:02}z-{entropy:08x}")
}

// Howard Hinnant の civil_from_days を整数演算にしたもの。
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    (y + if m <= 2 { 1 } else { 0 }, m, d)
}

fn normalize_task_id(issue: Option<i64>, task_id: Option<&str>) -> Result<String, WorktreeError> {
    let value = match (issue, task_id) {
        (Some(issue), None) if issue >= 1 => format!("issue-{issue}"),
        (Some(_), None) => return Err(error("Issue番号は1以上にしてください")),
        (None, Some(value)) => value.to_string(),
        (None, None) => generate_task_id(),
        (Some(_), Some(_)) => return Err(error("Issue番号とtask IDは同時に指定できません")),
    };
    if !valid_task_id(&value) {
        return Err(error(
            "task IDはissue-<番号>またはtask-<安全なID>にしてください",
        ));
    }
    Ok(value)
}

fn valid_task_id(value: &str) -> bool {
    if let Some(number) = value.strip_prefix("issue-") {
        !number.is_empty()
            && number.bytes().all(|byte| byte.is_ascii_digit())
            && number.parse::<u64>().is_ok_and(|n| n >= 1)
    } else if let Some(rest) = value.strip_prefix("task-") {
        !rest.is_empty()
            && rest.len() <= 64
            && rest
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            && !rest.starts_with('-')
            && !rest.ends_with('-')
    } else {
        false
    }
}

fn valid_branch(branch: &str) -> bool {
    if is_protected(branch) || branch.is_empty() {
        return false;
    }
    let (prefix, rest) = match branch.split_once('/') {
        Some(value) => value,
        None => return false,
    };
    if !BRANCH_PREFIXES.contains(&prefix)
        || rest.is_empty()
        || !rest
            .as_bytes()
            .first()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    {
        return false;
    }
    if rest.contains("..")
        || rest.ends_with('.')
        || rest.ends_with(".lock")
        || rest.contains('@')
        || rest.contains(' ')
        || rest.contains('~')
        || rest.contains('^')
        || rest.contains(':')
        || rest.contains('?')
        || rest.contains('*')
        || rest.contains('[')
        || rest.contains('\\')
    {
        return false;
    }
    rest.bytes().all(|byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'.')
    })
}

fn validate_branch(root: &Path, branch: &str) -> Result<(), WorktreeError> {
    if !valid_branch(branch) {
        return Err(error(
            "一般的なprefixを持つ非保護作業branchを指定してください",
        ));
    }
    let (code, output) = git_allow_failure(root, &["check-ref-format", "--branch", branch])?;
    if code != 0 {
        return Err(error(
            output
                .stderr
                .trim()
                .to_string()
                .if_empty("Gitで無効なbranch名です"),
        ));
    }
    Ok(())
}

fn codex_home() -> Result<PathBuf, WorktreeError> {
    let candidate = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| OsString::from("~")))
                .join(".codex")
        });
    if !candidate.is_absolute()
        || candidate
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(error("CODEX_HOMEは安全な絶対pathにしてください"));
    }
    reject_symlink_components(&candidate)?;
    Ok(candidate)
}

#[cfg(unix)]
fn safe_directory(path: &Path) -> Result<(), WorktreeError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|cause| error(format!("管理directoryを検査できません: {cause}")))?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != unsafe { libc::geteuid() }
    {
        return Err(error(format!(
            "安全な管理directoryではありません: {}",
            path.display()
        )));
    }
    if metadata.mode() & 0o077 != 0 {
        return Err(error(format!(
            "管理directoryのmodeが安全ではありません: {}",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(not(unix))]
fn safe_directory(path: &Path) -> Result<(), WorktreeError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|cause| error(format!("管理directoryを検査できません: {cause}")))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(error(format!(
            "安全な管理directoryではありません: {}",
            path.display()
        )));
    }
    Ok(())
}

fn reject_symlink_components(path: &Path) -> Result<(), WorktreeError> {
    let mut current = PathBuf::from(
        path.components()
            .next()
            .map(|component| component.as_os_str())
            .unwrap_or(OsStr::new("/")),
    );
    for component in path.components().skip(1) {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(error(format!(
                    "symlink componentを拒否しました: {}",
                    current.display()
                )));
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(error(format!(
                    "管理pathのcomponentがdirectoryではありません: {}",
                    current.display()
                )));
            }
            Ok(_) => {}
            Err(cause) if cause.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(cause) => return Err(error(format!("管理pathを安全に検査できません: {cause}"))),
        }
    }
    Ok(())
}

fn ensure_directory(path: &Path, parent: Option<&Path>) -> Result<(), WorktreeError> {
    if let Some(parent) = parent
        && (path.parent() != Some(parent) || path.file_name().is_none())
    {
        return Err(error("管理root外のpathを拒否しました"));
    }
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(error(format!(
                "安全な管理directoryではありません: {}",
                path.display()
            )));
        }
    } else {
        fs::create_dir(path)
            .map_err(|cause| error(format!("管理directoryを作成できません: {cause}")))?;
    }
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|cause| error(format!("管理directory modeを設定できません: {cause}")))?;
    safe_directory(path)
}

fn managed_paths(
    repository: &Repository,
    task_id: &str,
    create: bool,
) -> Result<(PathBuf, PathBuf, PathBuf), WorktreeError> {
    let home = codex_home()?;
    if home.exists() {
        safe_directory(&home)?;
    } else if create {
        fs::create_dir(&home)
            .map_err(|cause| error(format!("CODEX_HOMEを作成できません: {cause}")))?;
        #[cfg(unix)]
        fs::set_permissions(&home, fs::Permissions::from_mode(0o700))
            .map_err(|cause| error(format!("CODEX_HOME modeを設定できません: {cause}")))?;
    }
    let root = home.join("worktrees");
    let key = repository.key()?;
    let repository_root = root.join(key);
    let state_root = repository_root.join(".state");
    let lock_root = repository_root.join(".locks");
    reject_symlink_components(&lock_root)?;
    if create {
        ensure_directory(&root, Some(&home))?;
        ensure_directory(&repository_root, Some(&root))?;
        ensure_directory(&state_root, Some(&repository_root))?;
        ensure_directory(&lock_root, Some(&repository_root))?;
    }
    let target = repository_root.join(task_id);
    if target.parent() != Some(repository_root.as_path()) {
        return Err(error("worktree pathが管理root外です"));
    }
    Ok((
        target,
        state_root.join(format!("{task_id}.json")),
        lock_root.join("lifecycle.lock"),
    ))
}

fn snapshot(root: &Path) -> Result<(String, String, String, String, String), WorktreeError> {
    Ok((
        git_stdout(root, &["rev-parse", "--abbrev-ref", "HEAD"])?,
        git_stdout(root, &["rev-parse", "HEAD"])?,
        git_stdout(root, &["status", "--porcelain=v1", "--untracked-files=all"])?,
        git_stdout(root, &["diff", "--cached", "--binary", "--"])?,
        git_stdout(root, &["diff", "--binary", "--"])?,
    ))
}

fn atomic_manifest(path: &Path, manifest: &Manifest) -> Result<(), WorktreeError> {
    let file_name = path
        .file_name()
        .ok_or_else(|| error("manifest pathが不正です"))?
        .to_string_lossy();
    let temporary = path.with_file_name(format!(
        ".{file_name}.tmp.{}.{}",
        std::process::id(),
        task_entropy()
    ));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut output = options
            .open(&temporary)
            .map_err(|cause| error(format!("manifestを作成できません: {cause}")))?;
        output
            .write_all(manifest.json()?.as_bytes())
            .map_err(|cause| error(format!("manifestを書き込めません: {cause}")))?;
        output
            .sync_all()
            .map_err(|cause| error(format!("manifestをsyncできません: {cause}")))?;
        #[cfg(unix)]
        output
            .set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|cause| error(format!("manifest modeを設定できません: {cause}")))?;
        fs::rename(&temporary, path)
            .map_err(|cause| error(format!("manifestをatomic renameできません: {cause}")))?;
        // Directory fsync closes the rename durability window on Unix.
        #[cfg(unix)]
        {
            let directory = File::open(
                path.parent()
                    .ok_or_else(|| error("manifest parentがありません"))?,
            )
            .map_err(|cause| error(format!("manifest parentを開けません: {cause}")))?;
            directory
                .sync_all()
                .map_err(|cause| error(format!("manifest parentをsyncできません: {cause}")))?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn task_entropy() -> String {
    let value = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        ^ std::process::id() as u128;
    format!("{:08x}", value as u64)
}

struct ExclusiveLock {
    #[allow(dead_code)]
    file: File,
}

impl ExclusiveLock {
    fn acquire(path: &Path) -> Result<Self, WorktreeError> {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        let file = options
            .open(path)
            .map_err(|cause| error(format!("lifecycle lockを取得できません: {cause}")))?;
        #[cfg(unix)]
        {
            if let Ok(metadata) = file.metadata()
                && (metadata.file_type().is_symlink()
                    || metadata.uid() != unsafe { libc::geteuid() }
                    || metadata.mode() & 0o077 != 0)
            {
                return Err(error(format!(
                    "lifecycle lockが安全ではありません: {}",
                    path.display()
                )));
            }
            // flock is advisory and scoped to the open descriptor. Holding the
            // File in this guard keeps the lock until the operation returns.
            let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
            if result != 0 {
                return Err(error(format!(
                    "lifecycle lockを取得できません: {}",
                    io::Error::last_os_error()
                )));
            }
            file.set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(|cause| error(format!("lifecycle lock modeを設定できません: {cause}")))?;
        }
        Ok(Self { file })
    }
}

fn worktree_records(root: &Path) -> Result<Vec<BTreeMap<String, String>>, WorktreeError> {
    let output = git_stdout(root, &["worktree", "list", "--porcelain", "-z"])?;
    let mut records = Vec::new();
    let mut record = BTreeMap::new();
    for item in output.split('\0') {
        if item.is_empty() {
            if !record.is_empty() {
                records.push(record);
                record = BTreeMap::new();
            }
            continue;
        }
        if let Some((key, value)) = item.split_once(' ') {
            record.insert(key.to_string(), value.to_string());
        } else {
            record.insert(item.to_string(), String::new());
        }
    }
    if !record.is_empty() {
        records.push(record);
    }
    Ok(records)
}

fn branch_exists(repository: &Repository, branch: &str) -> Result<bool, WorktreeError> {
    let (local_code, _) = git_allow_failure(
        &repository.root,
        &["rev-parse", "--verify", &format!("refs/heads/{branch}")],
    )?;
    if local_code == 0 {
        return Ok(true);
    }
    let (origin, _) = origin_urls(&repository.root)?;
    let remote_ref = format!("refs/heads/{branch}");
    let arguments = [
        "ls-remote",
        "--exit-code",
        "--upload-pack=git-upload-pack",
        "--heads",
        origin.as_str(),
        remote_ref.as_str(),
    ];
    let (remote_code, output) = git_allow_failure(&repository.root, &arguments)?;
    if remote_code != 0 && remote_code != 2 {
        return Err(error(
            output
                .stderr
                .trim()
                .to_string()
                .if_empty("originのbranch衝突を確認できません"),
        ));
    }
    Ok(remote_code == 0)
}

fn create_worktree(
    cwd: &Path,
    branch: &str,
    task_id: &str,
    allow_local_origin: bool,
) -> Result<PathBuf, WorktreeError> {
    let repository = inspect_repository(cwd, allow_local_origin, true)?;
    validate_branch(&repository.root, branch)?;
    let (target, manifest_path, lock_path) = managed_paths(&repository, task_id, true)?;
    let before = snapshot(&repository.root)?;
    let created_at = format_timestamp();
    let base = format!("origin/{}", repository.default_branch);
    let _lock = ExclusiveLock::acquire(&lock_path)?;
    if target.exists()
        || target.is_symlink()
        || manifest_path.exists()
        || manifest_path.is_symlink()
    {
        return Err(error("task IDまたはworktree pathが既に使用されています"));
    }
    if worktree_records(&repository.root)?
        .iter()
        .any(|record| record.get("worktree") == Some(&target.to_string_lossy().to_string()))
    {
        return Err(error("worktree pathがGit metadataに既に登録されています"));
    }
    if branch_exists(&repository, branch)? {
        return Err(error("localまたはremote branchが既に存在します"));
    }
    let (origin, _) = origin_urls(&repository.root)?;
    let refspec = format!(
        "+refs/heads/{}:refs/remotes/origin/{}",
        repository.default_branch, repository.default_branch
    );
    let fetch_arguments = [
        "fetch",
        "--no-tags",
        "--upload-pack=git-upload-pack",
        origin.as_str(),
        refspec.as_str(),
    ];
    git_stdout(&repository.root, &fetch_arguments)?;
    let fetched_oid = git_stdout(
        &repository.root,
        &["rev-parse", &format!("refs/remotes/{base}")],
    )?
    .trim()
    .to_ascii_lowercase();
    let (remote_branch, remote_oid) = remote_default(&repository.root)?;
    if remote_branch != repository.default_branch || fetched_oid != remote_oid {
        return Err(error(
            "fetch後のorigin default branchがremote HEADと一致しません",
        ));
    }
    let creating = Manifest {
        version: MANIFEST_VERSION,
        status: "creating".to_string(),
        task_id: task_id.to_string(),
        repository: repository.root.to_string_lossy().into_owned(),
        common_git_dir: repository.common_git_dir.to_string_lossy().into_owned(),
        github_name: repository.github_name.clone(),
        branch: branch.to_string(),
        base: base.clone(),
        base_oid: fetched_oid.clone(),
        worktree: target.to_string_lossy().into_owned(),
        created_at,
        detail: String::new(),
    };
    atomic_manifest(&manifest_path, &creating)?;
    let target_string = target.to_string_lossy().into_owned();
    let operation = (|| {
        git_stdout(
            &repository.root,
            &[
                "worktree",
                "add",
                "--lock",
                "--reason",
                &format!("codex-task:{task_id}"),
                "-b",
                branch,
                &target_string,
                &base,
            ],
        )?;
        if absolute_git_path(&target, "--git-common-dir")? != repository.common_git_dir {
            return Err(error("作成したworktreeが対象repositoryに属していません"));
        }
        if git_stdout(&target, &["rev-parse", "--abbrev-ref", "HEAD"])?.trim() != branch {
            return Err(error("作成したworktreeのbranchが一致しません"));
        }
        if git_stdout(&target, &["rev-parse", "HEAD"])?
            .trim()
            .to_ascii_lowercase()
            != fetched_oid
        {
            return Err(error("作成したworktreeのHEADが取得済みbaseと一致しません"));
        }
        if !git_stdout(
            &target,
            &["status", "--porcelain=v1", "--untracked-files=all"],
        )?
        .is_empty()
        {
            return Err(error("作成したworktreeがcleanではありません"));
        }
        if snapshot(&repository.root)? != before {
            return Err(error(
                "main checkoutのbranch、index、working treeが変化しました",
            ));
        }
        Ok(())
    })();
    if let Err(cause) = operation {
        let failed = Manifest {
            status: "failed".to_string(),
            detail: cause.to_string(),
            ..creating.clone()
        };
        let _ = atomic_manifest(&manifest_path, &failed);
        return Err(cause);
    }
    let ready = Manifest {
        status: "ready".to_string(),
        ..creating
    };
    atomic_manifest(&manifest_path, &ready)?;
    Ok(target)
}

fn format_timestamp() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let days = now.as_secs() / 86_400;
    let seconds = now.as_secs() % 86_400;
    let (year, month, day) = civil_from_days(days as i64);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:09}Z",
        seconds / 3600,
        (seconds % 3600) / 60,
        seconds % 60,
        now.subsec_nanos()
    )
}

fn invalid_manifest(repository: &Repository, path: &Path, detail: &str) -> Manifest {
    Manifest {
        version: MANIFEST_VERSION,
        status: "invalid".to_string(),
        task_id: path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        repository: repository.root.to_string_lossy().into_owned(),
        common_git_dir: repository.common_git_dir.to_string_lossy().into_owned(),
        github_name: repository.github_name.clone(),
        branch: String::new(),
        base: String::new(),
        base_oid: String::new(),
        worktree: String::new(),
        created_at: String::new(),
        detail: detail.to_string(),
    }
}

fn validated_manifest(
    repository: &Repository,
    path: &Path,
    values: BTreeMap<String, JsonValue>,
) -> Result<Manifest, WorktreeError> {
    let manifest = json_manifest(values)?;
    let (target, expected_path, _) = managed_paths(repository, &manifest.task_id, false)?;
    if manifest.version != MANIFEST_VERSION
        || !matches!(manifest.status.as_str(), "creating" | "ready" | "failed")
        || path != expected_path
        || path.file_stem().and_then(OsStr::to_str) != Some(manifest.task_id.as_str())
        || manifest.repository != repository.root.to_string_lossy()
        || manifest.common_git_dir != repository.common_git_dir.to_string_lossy()
        || !manifest
            .github_name
            .eq_ignore_ascii_case(&repository.github_name)
        || manifest.worktree != target.to_string_lossy()
        || !valid_branch(&manifest.branch)
        || !manifest
            .base
            .strip_prefix("origin/")
            .is_some_and(is_protected)
        || !valid_oid(&manifest.base_oid)
        || manifest.created_at.is_empty()
    {
        return Err(error("manifest values mismatch"));
    }
    Ok(manifest)
}

fn load_manifests(repository: &Repository) -> Result<Vec<(PathBuf, Manifest)>, WorktreeError> {
    let (_, sample, _) = managed_paths(repository, "task-placeholder", false)?;
    let parent = sample
        .parent()
        .ok_or_else(|| error("manifest parentがありません"))?;
    if !parent.is_dir() || parent.is_symlink() {
        return Ok(Vec::new());
    }
    let mut paths = Vec::new();
    for entry in fs::read_dir(parent)
        .map_err(|cause| error(format!("manifest directoryを読めません: {cause}")))?
    {
        let path = entry
            .map_err(|cause| error(format!("manifest entryを読めません: {cause}")))?
            .path();
        if path.extension().and_then(OsStr::to_str) == Some("json") {
            paths.push(path);
        }
    }
    paths.sort();
    let mut manifests = Vec::new();
    for path in paths {
        let result = (|| {
            let metadata = fs::symlink_metadata(&path)
                .map_err(|cause| error(format!("manifestを検査できません: {cause}")))?;
            #[cfg(unix)]
            let safe_owner = metadata.uid() == unsafe { libc::geteuid() };
            #[cfg(not(unix))]
            let safe_owner = true;
            #[cfg(unix)]
            let safe_mode = metadata.mode() & 0o077 == 0;
            #[cfg(not(unix))]
            let safe_mode = true;
            if path.is_symlink()
                || !metadata.is_file()
                || !safe_owner
                || !safe_mode
                || metadata.len() > MAX_MANIFEST_BYTES
            {
                return Err(error("manifest is not a safe regular file"));
            }
            let contents = fs::read_to_string(&path)
                .map_err(|cause| error(format!("manifestを読めません: {cause}")))?;
            validated_manifest(repository, &path, parse_manifest(&contents)?)
        })();
        match result {
            Ok(manifest) => manifests.push((path, manifest)),
            Err(_) => manifests.push((
                path.clone(),
                invalid_manifest(repository, &path, "manifestを安全に解析・検証できません"),
            )),
        }
    }
    Ok(manifests)
}

fn diagnose(
    cwd: &Path,
    task_id: Option<&str>,
    allow_local_origin: bool,
) -> Result<Vec<(String, String, String)>, WorktreeError> {
    let repository = inspect_repository(cwd, allow_local_origin, false)?;
    let registered: BTreeMap<String, BTreeMap<String, String>> =
        worktree_records(&repository.root)?
            .into_iter()
            .filter_map(|record| record.get("worktree").cloned().map(|path| (path, record)))
            .collect();
    let mut results = Vec::new();
    let mut found = false;
    for (_, manifest) in load_manifests(&repository)? {
        if task_id.is_some_and(|task| manifest.task_id != task) {
            continue;
        }
        found = true;
        let mut status = manifest.status.clone();
        let mut detail = manifest.detail.clone();
        let path = (!manifest.worktree.is_empty()).then(|| PathBuf::from(&manifest.worktree));
        let record = registered.get(&manifest.worktree);
        if manifest.status != "invalid" {
            if manifest.version != MANIFEST_VERSION
                || !manifest
                    .github_name
                    .eq_ignore_ascii_case(&repository.github_name)
            {
                status = "invalid".to_string();
                detail = "manifestのrepository情報が一致しません".to_string();
            } else if path
                .as_ref()
                .is_none_or(|path| path.is_symlink() || !path.exists())
            {
                status = "missing".to_string();
                detail = "worktree directoryがありません".to_string();
            } else if record.is_none() {
                status = "unregistered".to_string();
                detail = "Git worktreeとして登録されていません".to_string();
            } else if let Some(path) = path {
                let checked = (|| {
                    let metadata = fs::symlink_metadata(&path)
                        .map_err(|cause| error(format!("worktreeを検査できません: {cause}")))?;
                    #[cfg(unix)]
                    if !metadata.is_dir() || metadata.uid() != unsafe { libc::geteuid() } {
                        return Err(error("worktree pathが安全なdirectoryではありません"));
                    }
                    #[cfg(not(unix))]
                    if !metadata.is_dir() {
                        return Err(error("worktree pathが安全なdirectoryではありません"));
                    }
                    if absolute_git_path(&path, "--git-common-dir")? != repository.common_git_dir {
                        return Err(error("worktreeのcommon Git dirが一致しません"));
                    }
                    let current = git_stdout(&path, &["rev-parse", "--abbrev-ref", "HEAD"])?
                        .trim()
                        .to_string();
                    let head = git_stdout(&path, &["rev-parse", "HEAD"])?
                        .trim()
                        .to_ascii_lowercase();
                    let dirty = git_stdout(
                        &path,
                        &["status", "--porcelain=v1", "--untracked-files=all"],
                    )?;
                    if current != manifest.branch {
                        status = "branch-mismatch".to_string();
                        detail = format!("current branch: {current}");
                    } else if manifest.status == "failed" { /* Keep failed detail. */
                    } else if manifest.status == "creating"
                        && head != manifest.base_oid.to_ascii_lowercase()
                    {
                        status = "diverged".to_string();
                        detail = "中断後にworktree HEADがbaseから変化しました".to_string();
                    } else if manifest.status == "creating" {
                        status = "interrupted".to_string();
                        detail = "作成完了前に中断しました。recoverで再検証できます".to_string();
                    } else if !dirty.is_empty() {
                        status = "dirty".to_string();
                        detail = "未commitまたは未追跡の変更があります".to_string();
                    } else if manifest.status == "ready" {
                        status = "ready".to_string();
                        detail = "再開可能です".to_string();
                    }
                    Ok::<(), WorktreeError>(())
                })();
                if checked.is_err() {
                    status = "invalid".to_string();
                    detail = "worktree実体を安全に検証できません".to_string();
                }
            }
        }
        results.push((manifest.task_id, status, detail));
    }
    if task_id.is_some() && !found {
        return Err(error("指定taskのmanifestがありません"));
    }
    Ok(results)
}

fn resume(cwd: &Path, task_id: &str, allow_local_origin: bool) -> Result<PathBuf, WorktreeError> {
    let repository = inspect_repository(cwd, allow_local_origin, false)?;
    for (_, manifest) in load_manifests(&repository)? {
        if manifest.task_id != task_id {
            continue;
        }
        let results = diagnose(cwd, Some(task_id), allow_local_origin)?;
        if !matches!(
            results.first().map(|row| row.1.as_str()),
            Some("ready" | "dirty")
        ) {
            return Err(error(format!(
                "taskを再開できません: {}",
                results
                    .first()
                    .map(|row| row.1.as_str())
                    .unwrap_or("unknown")
            )));
        }
        return Ok(PathBuf::from(manifest.worktree));
    }
    Err(error("指定taskのmanifestがありません"))
}

fn recover(cwd: &Path, task_id: &str, allow_local_origin: bool) -> Result<PathBuf, WorktreeError> {
    let repository = inspect_repository(cwd, allow_local_origin, false)?;
    let (_, manifest_path, lock_path) = managed_paths(&repository, task_id, false)?;
    if !lock_path
        .parent()
        .is_some_and(|parent| parent.is_dir() && !parent.is_symlink())
    {
        return Err(error("lifecycle lockを安全に確認できません"));
    }
    let _lock = ExclusiveLock::acquire(&lock_path)?;
    let matches: Vec<Manifest> = load_manifests(&repository)?
        .into_iter()
        .filter_map(|(_, manifest)| (manifest.task_id == task_id).then_some(manifest))
        .collect();
    if matches.len() != 1 {
        return Err(error("指定taskのmanifestを一意に確認できません"));
    }
    let manifest = matches
        .into_iter()
        .next()
        .ok_or_else(|| error("指定taskのmanifestがありません"))?;
    let results = diagnose(cwd, Some(task_id), allow_local_origin)?;
    let status = results
        .first()
        .map(|row| row.1.as_str())
        .unwrap_or("unknown");
    if manifest.status != "creating" || status != "interrupted" {
        return Err(error(format!("taskを安全にrecoverできません: {status}")));
    }
    let recovered = Manifest {
        status: "ready".to_string(),
        detail: String::new(),
        ..manifest.clone()
    };
    atomic_manifest(&manifest_path, &recovered)?;
    Ok(PathBuf::from(manifest.worktree))
}

/// `codex-worktree` の process entrypoint。引数エラーは2、操作エラーは1、成功は0。
/// `args` は実行ファイル名を先頭に含む `std::env::args_os()` 形式を受け取る。
pub fn entrypoint<I>(args: I) -> i32
where
    I: IntoIterator<Item = OsString>,
{
    match run_entrypoint(args.into_iter().collect()) {
        Ok(code) => code,
        Err((code, message)) => {
            eprintln!("codex-worktree: {message}");
            code
        }
    }
}

fn run_entrypoint(args: Vec<OsString>) -> Result<i32, (i32, String)> {
    let parsed = parse_args(&args).map_err(|message| (2, message))?;
    let cwd = std::env::current_dir()
        .map_err(|cause| (1, format!("current directoryを取得できません: {cause}")))?;
    let operation = match parsed.command.as_str() {
        "create" => {
            if parsed.issue.is_some() && parsed.task_id.is_some() {
                return Err((2, "Issue番号とtask IDは同時に指定できません".to_string()));
            }
            let task_id = normalize_task_id(parsed.issue, parsed.task_id.as_deref())
                .map_err(|cause| (2, cause.to_string()))?;
            let branch = parsed.branch.unwrap_or_else(|| format!("feat/{task_id}"));
            create_worktree(&cwd, &branch, &task_id, false)
                .map(|path| path.to_string_lossy().into_owned())
        }
        "resume" => {
            let task_id = normalize_task_id(None, parsed.task_id.as_deref())
                .map_err(|cause| (2, cause.to_string()))?;
            resume(&cwd, &task_id, false).map(|path| path.to_string_lossy().into_owned())
        }
        "recover" => {
            let task_id = normalize_task_id(None, parsed.task_id.as_deref())
                .map_err(|cause| (2, cause.to_string()))?;
            recover(&cwd, &task_id, false).map(|path| path.to_string_lossy().into_owned())
        }
        "list" | "doctor" => {
            let task_id = parsed.task_id.as_deref();
            diagnose(&cwd, task_id, false).map(|rows| {
                rows.into_iter()
                    .map(|(task, status, detail)| format!("{task}\t{status}\t{detail}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
        }
        _ => Err(error("未対応のcommand")),
    };
    match operation {
        Ok(output) => {
            if !output.is_empty() {
                println!("{output}");
            }
            if parsed.command == "doctor" {
                let rows = diagnose(&cwd, parsed.task_id.as_deref(), false)
                    .map_err(|cause| (1, cause.to_string()))?;
                return Ok(if rows.iter().all(|row| row.1 == "ready") {
                    0
                } else {
                    1
                });
            }
            Ok(0)
        }
        Err(cause) => Err((1, cause.to_string())),
    }
}

#[derive(Debug, Default)]
struct ParsedArgs {
    command: String,
    branch: Option<String>,
    issue: Option<i64>,
    task_id: Option<String>,
}

fn parse_args(args: &[OsString]) -> Result<ParsedArgs, String> {
    let start = args
        .first()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value == "codex-worktree" || value.ends_with("/codex-worktree"));
    let values: Vec<String> = args
        .iter()
        .skip(usize::from(start))
        .map(|value| {
            value
                .to_str()
                .map(str::to_string)
                .ok_or_else(|| "引数がUTF-8ではありません".to_string())
        })
        .collect::<Result<_, _>>()?;
    let command = values
        .first()
        .cloned()
        .ok_or_else(|| "commandを指定してください".to_string())?;
    if matches!(command.as_str(), "-h" | "--help") {
        return Err(
            "usage: codex-worktree <create|list|doctor|resume|recover> [options]".to_string(),
        );
    }
    if !matches!(
        command.as_str(),
        "create" | "list" | "doctor" | "resume" | "recover"
    ) {
        return Err(format!("unknown command: {command}"));
    }
    let mut parsed = ParsedArgs {
        command: command.clone(),
        ..ParsedArgs::default()
    };
    let mut index = 1;
    while index < values.len() {
        let value = &values[index];
        match value.as_str() {
            "-h" | "--help" => return Err(format!("usage: codex-worktree {command} [options]")),
            "--branch" if command == "create" => {
                index += 1;
                parsed.branch = Some(
                    values
                        .get(index)
                        .cloned()
                        .ok_or_else(|| "--branchには値が必要です".to_string())?,
                );
            }
            "--issue" if command == "create" => {
                index += 1;
                let value = values
                    .get(index)
                    .ok_or_else(|| "--issueには値が必要です".to_string())?;
                parsed.issue = Some(
                    value
                        .parse::<i64>()
                        .map_err(|_| "--issueは整数で指定してください".to_string())?,
                );
            }
            "--task-id"
                if matches!(command.as_str(), "create" | "doctor" | "resume" | "recover") =>
            {
                index += 1;
                parsed.task_id = Some(
                    values
                        .get(index)
                        .cloned()
                        .ok_or_else(|| "--task-idには値が必要です".to_string())?,
                );
            }
            _ => return Err(format!("unknown or misplaced argument: {value}")),
        }
        index += 1;
    }
    if matches!(command.as_str(), "resume" | "recover") && parsed.task_id.is_none() {
        return Err("--task-idが必要です".to_string());
    }
    if command != "create" && parsed.branch.is_some() {
        return Err("--branchはcreateでのみ使用できます".to_string());
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use std::process::Command;
    use std::sync::{Mutex, MutexGuard};
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    struct TemporaryRepository {
        directory: PathBuf,
        repository: PathBuf,
        codex_home: PathBuf,
        previous_codex_home: Option<OsString>,
        _lock: MutexGuard<'static, ()>,
    }

    impl TemporaryRepository {
        fn new() -> Self {
            let lock = TEST_LOCK.lock().unwrap();
            let entropy = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let directory = std::env::temp_dir().join(format!(
                "codex-worktree-rust-test-{}-{entropy:x}",
                std::process::id()
            ));
            fs::create_dir_all(&directory).unwrap();
            let remote = directory.join("remote.git");
            let repository = directory.join("main");
            let codex_home = directory.join("codex-home");
            run_git(&directory, &["init", "--bare", remote.to_str().unwrap()]);
            run_git(
                &directory,
                &[
                    "init",
                    "--initial-branch=main",
                    repository.to_str().unwrap(),
                ],
            );
            run_git(&repository, &["config", "user.name", "Test User"]);
            run_git(
                &repository,
                &["config", "user.email", "test@example.invalid"],
            );
            fs::write(repository.join("README.md"), "test\n").unwrap();
            run_git(&repository, &["add", "--", "README.md"]);
            run_git(&repository, &["commit", "-m", "initial"]);
            run_git(
                &repository,
                &["remote", "add", "origin", remote.to_str().unwrap()],
            );
            run_git(&repository, &["push", "-u", "origin", "main"]);
            run_git(&remote, &["symbolic-ref", "HEAD", "refs/heads/main"]);
            let previous_codex_home = std::env::var_os("CODEX_HOME");
            // Tests intentionally scope this process-global variable under TEST_LOCK.
            unsafe { std::env::set_var("CODEX_HOME", &codex_home) };
            Self {
                directory,
                repository,
                codex_home,
                previous_codex_home,
                _lock: lock,
            }
        }

        fn managed_repository(&self) -> PathBuf {
            self.codex_home
                .join("worktrees")
                .join(repository_key("test/local").unwrap())
        }

        fn manifest(&self, task_id: &str) -> PathBuf {
            self.managed_repository()
                .join(".state")
                .join(format!("{task_id}.json"))
        }
    }

    impl Drop for TemporaryRepository {
        fn drop(&mut self) {
            if let Some(value) = self.previous_codex_home.take() {
                unsafe { std::env::set_var("CODEX_HOME", value) };
            } else {
                unsafe { std::env::remove_var("CODEX_HOME") };
            }
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    fn run_git(cwd: &Path, args: &[&str]) {
        let status = Command::new("git")
            .current_dir(cwd)
            .args(args)
            .status()
            .expect("test git");
        assert!(status.success(), "git {:?} failed", args);
    }

    fn run_git_output(cwd: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(cwd)
            .args(args)
            .output()
            .expect("test git output");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("test git UTF-8")
    }

    #[test]
    fn task_ids_are_strict_and_issue_ids_are_supported() {
        assert_eq!(normalize_task_id(Some(22), None).unwrap(), "issue-22");
        assert!(normalize_task_id(Some(0), None).is_err());
        assert!(normalize_task_id(None, Some("../task")).is_err());
        assert!(normalize_task_id(None, Some("task-UPPER")).is_err());
        assert!(normalize_task_id(None, Some("task-safe-id")).is_ok());
        assert!(valid_task_id(&generate_task_id()));
    }

    #[test]
    fn branch_and_remote_parsing_are_strict() {
        assert!(valid_branch("feat/safe-worktree"));
        for branch in [
            "main",
            "release/2026-08",
            "codex/example",
            "feat/a..b",
            "feat/x.lock",
            "Feat/x",
        ] {
            assert!(!valid_branch(branch));
        }
        assert_eq!(
            github_repository("https://github.com/owner/repo.git"),
            Some("owner/repo".to_string())
        );
        assert_eq!(
            github_repository("git@github.com:owner/repo.git"),
            Some("owner/repo".to_string())
        );
        assert!(github_repository("https://example.com/owner/repo.git").is_none());
        assert!(github_repository("http://github.com/owner/repo.git").is_none());
        assert_ne!(
            repository_key("a--b/c").unwrap(),
            repository_key("a/b--c").unwrap()
        );
    }

    #[test]
    fn manifest_json_is_sorted_and_round_trips() {
        let manifest = Manifest {
            version: 1,
            status: "ready".into(),
            task_id: "issue-22".into(),
            repository: "/repo".into(),
            common_git_dir: "/repo/.git".into(),
            github_name: "owner/repo".into(),
            branch: "feat/example".into(),
            base: "origin/main".into(),
            base_oid: "a".repeat(40),
            worktree: "/worktree".into(),
            created_at: "2026-08-20T00:00:00Z".into(),
            detail: "日本語".into(),
        };
        let json = manifest.json().unwrap();
        assert!(json.starts_with("{\n  \"base\": "));
        assert!(json.ends_with("}\n"));
        assert_eq!(
            json_manifest(parse_manifest(&json).unwrap()).unwrap(),
            manifest
        );
        let escaped = json.replace("日本語", "\\ud83d\\ude00");
        let parsed = json_manifest(parse_manifest(&escaped).unwrap()).unwrap();
        assert_eq!(parsed.detail, "😀");
    }

    #[test]
    fn malformed_manifest_is_rejected_without_panic() {
        assert!(parse_manifest("[]").is_err());
        assert!(json_manifest(parse_manifest("{\"version\":true}").unwrap()).is_err());
        assert!(parse_manifest("{\"version\":1,}").is_err());
    }

    #[test]
    fn generated_manifest_is_bounded() {
        let manifest = Manifest {
            version: 1,
            status: "ready".into(),
            task_id: "task-safe".into(),
            repository: "x".into(),
            common_git_dir: "x".into(),
            github_name: "owner/repo".into(),
            branch: "feat/x".into(),
            base: "origin/main".into(),
            base_oid: "a".repeat(40),
            worktree: "x".into(),
            created_at: "now".into(),
            detail: "".into(),
        };
        assert!(manifest.json().unwrap().len() < MAX_MANIFEST_BYTES as usize);
    }

    #[test]
    fn atomic_manifest_replaces_bytes_and_leaves_no_temporary_file() {
        let fixture = TemporaryRepository::new();
        let _target =
            create_worktree(&fixture.repository, "feat/atomic", "task-atomic", true).unwrap();
        let path = fixture.manifest("task-atomic");
        let mut manifest =
            json_manifest(parse_manifest(&fs::read_to_string(&path).unwrap()).unwrap()).unwrap();
        manifest.detail = "atomic update".into();
        atomic_manifest(&path, &manifest).unwrap();
        let loaded =
            json_manifest(parse_manifest(&fs::read_to_string(&path).unwrap()).unwrap()).unwrap();
        assert_eq!(loaded.detail, "atomic update");
        let temporary_count = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp."))
            .count();
        assert_eq!(temporary_count, 0);
    }

    #[test]
    fn safe_environment_removes_git_controls() {
        let mut command = Command::new("env");
        command
            .env("GIT_SSH_COMMAND", "unsafe")
            .env("HTTPS_PROXY", "http://proxy.invalid")
            .env("SSL_CERT_FILE", "/tmp/untrusted.pem");
        safe_environment(&mut command);
        let values: BTreeMap<OsString, Option<OsString>> = command
            .get_envs()
            .map(|(key, value)| (key.to_os_string(), value.map(OsString::from)))
            .collect();
        assert_eq!(values.get(OsStr::new("GIT_SSH_COMMAND")), Some(&None));
        assert_eq!(values.get(OsStr::new("HTTPS_PROXY")), Some(&None));
        assert_eq!(values.get(OsStr::new("SSL_CERT_FILE")), Some(&None));
        assert_eq!(
            values.get(OsStr::new("PATH")),
            Some(&Some(OsString::from(SYSTEM_PATH)))
        );
    }

    #[test]
    fn unsafe_repository_config_keys_are_fail_closed() {
        for key in [
            "include.path",
            "core.sshCommand",
            "http.example.proxy",
            "http.https://github.com/.sslVerify",
            "http.https://github.com/.extraHeader",
            "url.safe.insteadOf",
            "protocol.file.allow",
            "filter.lfs.process",
            "diff.external",
            "merge.custom.driver",
        ] {
            assert!(unsafe_local_config_key(&key.to_ascii_lowercase()), "{key}");
        }
        assert!(!unsafe_local_config_key("remote.origin.url"));
        assert!(!unsafe_local_config_key("user.name"));
    }

    #[cfg(unix)]
    #[test]
    fn malicious_repository_ssh_command_is_rejected_without_execution() {
        let fixture = TemporaryRepository::new();
        let marker = fixture.directory.join("ssh-command-ran");
        let command = format!("sh -c 'touch {}'", marker.display());
        run_git(
            &fixture.repository,
            &["config", "core.sshCommand", command.as_str()],
        );

        assert!(create_worktree(&fixture.repository, "feat/ssh", "task-ssh", true).is_err());
        assert!(!marker.exists());
    }

    #[test]
    fn bounded_capture_stops_at_limit() {
        let exceeded = Arc::new(AtomicBool::new(false));
        let capture = capture_pipe(
            io::Cursor::new(vec![b'x'; MAX_CAPTURE_BYTES + 1]),
            Arc::clone(&exceeded),
        );
        assert_eq!(capture.bytes.len(), MAX_CAPTURE_BYTES);
        assert!(capture.oversized && exceeded.load(Ordering::Acquire));
    }

    #[test]
    fn parse_args_requires_safe_shapes() {
        let args = vec![
            OsString::from("codex-worktree"),
            OsString::from("create"),
            OsString::from("--issue"),
            OsString::from("22"),
        ];
        let parsed = parse_args(&args).unwrap();
        assert_eq!(parsed.issue, Some(22));
        assert!(parse_args(&[OsString::from("codex-worktree"), OsString::from("resume")]).is_err());
    }

    #[test]
    fn entrypoint_returns_argument_exit_code_without_process_exit() {
        assert_eq!(
            entrypoint([OsString::from("codex-worktree"), OsString::from("resume"),]),
            2
        );
        assert_eq!(
            entrypoint([
                OsString::from("codex-worktree"),
                OsString::from("create"),
                OsString::from("--issue"),
                OsString::from("0"),
            ]),
            2
        );
    }

    #[test]
    fn civil_timestamp_shape_is_utc_compatible() {
        assert!(generate_task_id().starts_with("task-20"));
        assert!(format_timestamp().ends_with('Z'));
    }

    #[test]
    fn create_preserves_dirty_main_and_writes_ready_manifest() {
        let fixture = TemporaryRepository::new();
        fs::write(fixture.repository.join("local.txt"), "untracked\n").unwrap();
        let before = snapshot(&fixture.repository).unwrap();
        let target = create_worktree(&fixture.repository, "feat/first", "issue-22", true).unwrap();
        assert_eq!(snapshot(&fixture.repository).unwrap(), before);
        assert_eq!(
            run_git_output(&target, &["rev-parse", "--abbrev-ref", "HEAD"]).trim(),
            "feat/first"
        );
        assert_eq!(run_git_output(&target, &["status", "--porcelain=v1"]), "");
        let manifest_path = fixture.manifest("issue-22");
        let metadata = fs::symlink_metadata(&manifest_path).unwrap();
        assert!(metadata.is_file());
        #[cfg(unix)]
        assert_eq!(metadata.mode() & 0o077, 0);
        let manifest =
            json_manifest(parse_manifest(&fs::read_to_string(manifest_path).unwrap()).unwrap())
                .unwrap();
        assert_eq!(manifest.status, "ready");
        assert_eq!(manifest.worktree, target.to_string_lossy());
    }

    #[test]
    fn duplicate_task_and_branch_are_rejected_without_cleanup() {
        let fixture = TemporaryRepository::new();
        let target = create_worktree(&fixture.repository, "feat/first", "issue-22", true).unwrap();
        assert!(create_worktree(&fixture.repository, "feat/second", "issue-22", true).is_err());
        assert!(create_worktree(&fixture.repository, "feat/first", "task-another", true).is_err());
        assert!(target.is_dir());
    }

    #[test]
    fn doctor_resume_reports_ready_dirty_and_missing() {
        let fixture = TemporaryRepository::new();
        let target =
            create_worktree(&fixture.repository, "feat/doctor", "task-doctor", true).unwrap();
        assert_eq!(
            diagnose(&fixture.repository, Some("task-doctor"), true).unwrap()[0].1,
            "ready"
        );
        assert_eq!(
            resume(&fixture.repository, "task-doctor", true).unwrap(),
            target
        );
        fs::write(target.join("dirty.txt"), "dirty\n").unwrap();
        assert_eq!(
            diagnose(&fixture.repository, Some("task-doctor"), true).unwrap()[0].1,
            "dirty"
        );
        let missing = target.with_file_name("missing-preserved");
        fs::rename(&target, &missing).unwrap();
        assert_eq!(
            diagnose(&fixture.repository, Some("task-doctor"), true).unwrap()[0].1,
            "missing"
        );
    }

    #[test]
    fn recover_promotes_only_an_interrupted_worktree() {
        let fixture = TemporaryRepository::new();
        let target =
            create_worktree(&fixture.repository, "feat/recover", "task-recover", true).unwrap();
        let path = fixture.manifest("task-recover");
        let mut manifest =
            json_manifest(parse_manifest(&fs::read_to_string(&path).unwrap()).unwrap()).unwrap();
        manifest.status = "creating".into();
        atomic_manifest(&path, &manifest).unwrap();
        assert_eq!(
            diagnose(&fixture.repository, Some("task-recover"), true).unwrap()[0].1,
            "interrupted"
        );
        assert!(resume(&fixture.repository, "task-recover", true).is_err());
        assert_eq!(
            recover(&fixture.repository, "task-recover", true).unwrap(),
            target
        );
        assert_eq!(
            diagnose(&fixture.repository, Some("task-recover"), true).unwrap()[0].1,
            "ready"
        );
        manifest.status = "failed".into();
        manifest.detail = "simulated failure".into();
        atomic_manifest(&path, &manifest).unwrap();
        fs::write(target.join("dirty.txt"), "preserve\n").unwrap();
        assert_eq!(
            diagnose(&fixture.repository, Some("task-recover"), true).unwrap()[0].1,
            "failed"
        );
        assert!(resume(&fixture.repository, "task-recover", true).is_err());
    }

    #[test]
    fn recover_rejects_interrupted_worktree_with_changed_head() {
        let fixture = TemporaryRepository::new();
        let target =
            create_worktree(&fixture.repository, "feat/diverged", "task-diverged", true).unwrap();
        let path = fixture.manifest("task-diverged");
        let mut manifest =
            json_manifest(parse_manifest(&fs::read_to_string(&path).unwrap()).unwrap()).unwrap();
        manifest.status = "creating".into();
        atomic_manifest(&path, &manifest).unwrap();
        fs::write(target.join("changed.txt"), "changed\n").unwrap();
        run_git(&target, &["add", "--", "changed.txt"]);
        run_git(&target, &["commit", "-m", "changed head"]);
        assert_eq!(
            diagnose(&fixture.repository, Some("task-diverged"), true).unwrap()[0].1,
            "diverged"
        );
        assert!(recover(&fixture.repository, "task-diverged", true).is_err());
    }

    #[test]
    fn invalid_manifest_is_isolated_from_managed_root() {
        let fixture = TemporaryRepository::new();
        let _target =
            create_worktree(&fixture.repository, "feat/invalid", "task-invalid", true).unwrap();
        let path = fixture.manifest("task-invalid");
        let mut manifest =
            json_manifest(parse_manifest(&fs::read_to_string(&path).unwrap()).unwrap()).unwrap();
        manifest.worktree = fixture.repository.to_string_lossy().into_owned();
        manifest.branch = "main".into();
        atomic_manifest(&path, &manifest).unwrap();
        assert_eq!(
            diagnose(&fixture.repository, Some("task-invalid"), true).unwrap()[0].1,
            "invalid"
        );
        assert!(resume(&fixture.repository, "task-invalid", true).is_err());
        // Type/schema corruption is reported as invalid, never followed as a path.
        let mut raw = parse_manifest(&fs::read_to_string(&path).unwrap()).unwrap();
        raw.insert("worktree".into(), JsonValue::Number(1));
        let corrupt = json_pretty_object(
            &raw.iter()
                .map(|(key, value)| (key.as_str(), value.clone()))
                .collect(),
        );
        fs::write(&path, corrupt).unwrap();
        assert_eq!(
            diagnose(&fixture.repository, Some("task-invalid"), true).unwrap()[0].1,
            "invalid"
        );
    }

    #[test]
    fn symlinked_codex_home_and_parent_are_rejected() {
        let fixture = TemporaryRepository::new();
        let external = fixture.directory.join("external-home");
        fs::create_dir(&external).unwrap();
        // Existing CODEX_HOME is replaced by a symlink, then restored by Drop.
        unsafe {
            std::env::remove_var("CODEX_HOME");
            std::os::unix::fs::symlink(&external, &fixture.codex_home).unwrap();
            std::env::set_var("CODEX_HOME", &fixture.codex_home);
        }
        assert!(
            create_worktree(&fixture.repository, "feat/symlink", "task-symlink", true).is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn lifecycle_lock_symlink_is_rejected_before_recovery() {
        let fixture = TemporaryRepository::new();
        let _target = create_worktree(&fixture.repository, "feat/lock", "task-lock", true).unwrap();
        let repository = inspect_repository(&fixture.repository, true, false).unwrap();
        let (_, _, lock_path) = managed_paths(&repository, "task-lock", false).unwrap();
        let external = fixture.directory.join("external-lock");
        fs::write(&external, "not-the-lock").unwrap();
        fs::remove_file(&lock_path).unwrap();
        std::os::unix::fs::symlink(&external, &lock_path).unwrap();
        assert!(recover(&fixture.repository, "task-lock", true).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn create_disables_repository_git_hooks() {
        let fixture = TemporaryRepository::new();
        let hooks = fixture.directory.join("hooks");
        fs::create_dir(&hooks).unwrap();
        let marker = fixture.directory.join("post-checkout-ran");
        let hook = hooks.join("post-checkout");
        fs::write(&hook, format!("#!/bin/sh\ntouch '{}'\n", marker.display())).unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
        run_git(
            &fixture.repository,
            &["config", "core.hooksPath", hooks.to_str().unwrap()],
        );
        create_worktree(&fixture.repository, "feat/no-hooks", "task-no-hooks", true).unwrap();
        assert!(!marker.exists());
    }
}

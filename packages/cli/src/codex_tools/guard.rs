//! Codex `PreToolUse` guard.
//!
//! The hook keeps the hot path allocation-light: syntactic checks run before
//! starting any process, while Git/GitHub read-backs are bounded and fail
//! closed.  Process lifecycle is delegated to the shared process runner.

use std::collections::{BTreeMap, HashMap};
use std::ffi::CStr;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
#[cfg(unix)]
use std::os::unix::io::AsRawFd;

use crate::codex_tools::{process, trust};
use serde::Deserializer;
use serde::de::{self, MapAccess, Visitor};
use serde_json::Value as StrictJsonValue;

const MAX_BODY_FILE_BYTES: u64 = 256 * 1024;
const MAX_GIT_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
const MAX_HOOK_INPUT_BYTES: usize = 1024 * 1024;
const GIT_COMMAND_TIMEOUT: Duration = Duration::from_secs(2);
const GH_COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_MANIFEST_BYTES: u64 = 256 * 1024;
const BODY_SNAPSHOT_DIR_PREFIX: &str = ".codex-hook-body-";
const BODY_SNAPSHOT_MAX_RETAINED: usize = 128;
const BODY_SNAPSHOT_TTL: Duration = Duration::from_secs(60 * 60);
const SYSTEM_GIT: &str = "/usr/bin/git";
const SYSTEM_GH: &str = "/usr/bin/gh";
const SYSTEM_PATH: &str = "/usr/bin:/bin";

static BODY_SNAPSHOT_COUNTER: AtomicU64 = AtomicU64::new(0);

const GIT_READ_ONLY: &[&str] = &[
    "status",
    "log",
    "diff",
    "show",
    "rev-parse",
    "blame",
    "shortlog",
    "describe",
    "reflog",
    "ls-files",
    "ls-tree",
    "cat-file",
    "rev-list",
    "merge-base",
    "name-rev",
    "grep",
];
const GIT_SAFE_WRITE: &[&str] = &["add", "commit", "fetch", "pull", "push", "switch"];
const GIT_VALUE_OPTIONS: &[&str] = &["-C", "--git-dir", "--work-tree", "--namespace"];
const GIT_FORBIDDEN_OPTIONS: &[&str] = &["-c", "--config", "--config-env", "--exec-path"];
const GIT_READ_FORBIDDEN_ARGS: &[&str] = &[
    "--ext-diff",
    "--textconv",
    "--filters",
    "--paginate",
    "-p",
    "--output",
    "-O",
];
const BRANCH_VALUE_OPTIONS: &[&str] = &[
    "--contains",
    "--no-contains",
    "--merged",
    "--no-merged",
    "--points-at",
    "--sort",
    "--format",
    "--column",
    "--color",
    "--abbrev",
];
const BRANCH_WRITE_ARGS: &[&str] = &[
    "-d",
    "-D",
    "-m",
    "-M",
    "-c",
    "-C",
    "--delete",
    "--move",
    "--copy",
    "--edit-description",
    "--set-upstream-to",
    "--unset-upstream",
    "--create-reflog",
];
const PROTECTED_BRANCHES: &[&str] = &["main", "master", "develop", "development", "trunk"];
const BRANCH_PREFIXES: &[&str] = &[
    "feat", "feature", "fix", "refactor", "docs", "test", "chore", "ci", "build", "perf", "style",
    "hotfix", "update",
];
const GH_GLOBAL_VALUE_OPTIONS: &[&str] = &["-R", "--repo", "--hostname"];
const DESTRUCTIVE_COMMANDS: &[&str] = &["rm", "rmdir", "unlink", "shred"];
const SHELLS: &[&str] = &["bash", "dash", "sh", "zsh"];
const WRAPPERS: &[&str] = &[
    "command", "env", "exec", "nice", "nohup", "sudo", "timeout", "xargs",
];
const RESTRICTED_COMMANDS: &[&str] = &["git", "gh", "codex-worktree", "codex-delivery"];
const SHELL_COMPOUND_PREFIXES: &[&str] = &[
    "!",
    "-",
    "{",
    "case",
    "coproc",
    "do",
    "elif",
    "else",
    "end",
    "for",
    "foreach",
    "function",
    "if",
    "nocorrect",
    "noglob",
    "repeat",
    "select",
    "then",
    "time",
    "until",
    "while",
];
const SHELL_COMMAND_PREFIXES: &[&str] = &[
    "!",
    "-",
    "{",
    "do",
    "elif",
    "else",
    "if",
    "nocorrect",
    "noglob",
    "then",
    "until",
    "while",
];
const SHELL_CONTEXT_MUTATIONS: &[&str] = &[
    "alias", "autoload", "builtin", "cd", "chdir", "declare", "emulate", "enable", "eval",
    "export", "float", "function", "hash", "integer", "local", "popd", "pushd", "readonly",
    "rehash", "set", "setopt", "typeset", "unalias", "unset", "unsetopt",
];
const COMMAND_RESOLUTION_MUTATIONS: &[&str] = &[
    ".", "alias", "autoload", "builtin", "emulate", "enable", "eval", "function", "hash", "rehash",
    "source", "unalias",
];
const SENSITIVE_NAMES: &[&str] = &[
    ".env",
    "auth.json",
    "credentials.json",
    ".credentials.json",
    "id_rsa",
    "id_ed25519",
];
const SENSITIVE_SUFFIXES: &[&str] = &[".pem", ".p12", ".pfx", ".jks", ".keystore"];

fn basename(value: &str) -> &str {
    value.rsplit('/').next().unwrap_or(value)
}

fn starts_with_option(value: &str, option: &str) -> bool {
    value == option
        || value
            .strip_prefix(option)
            .is_some_and(|rest| rest.starts_with('='))
}

fn protected_branch(value: &str) -> bool {
    let branch = value.strip_prefix("refs/heads/").unwrap_or(value);
    PROTECTED_BRANCHES.contains(&branch)
        || branch == "release"
        || branch.starts_with("release/")
        || branch == "production"
        || branch.starts_with("production/")
}

fn valid_work_branch(value: &str) -> bool {
    let branch = value.strip_prefix("refs/heads/").unwrap_or(value);
    if protected_branch(branch) {
        return false;
    }
    let mut parts = branch.splitn(2, '/');
    let prefix = parts.next().unwrap_or("");
    let name = parts.next().unwrap_or("");
    !name.is_empty()
        && prefix != "codex"
        && BRANCH_PREFIXES.contains(&prefix)
        && prefix
            .as_bytes()
            .first()
            .is_some_and(|c| c.is_ascii_lowercase())
        && name
            .as_bytes()
            .first()
            .is_some_and(|c| c.is_ascii_lowercase())
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
        && prefix
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-')
}

fn valid_remote_branch(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._/-".contains(&c))
        && !value.contains("..")
        && !value.contains("//")
        && !value.ends_with('.')
        && !value.ends_with(".lock")
        && !value.contains("@{")
}

fn first_command(args: &[String], value_options: &[&str]) -> (Option<String>, Vec<String>) {
    let mut skip = false;
    for (i, token) in args.iter().enumerate() {
        if skip {
            skip = false;
            continue;
        }
        if value_options.iter().any(|o| token == *o) {
            skip = true;
            continue;
        }
        if value_options
            .iter()
            .any(|o| starts_with_option(token, o) && *o != "-C")
        {
            continue;
        }
        if token.starts_with('-') {
            continue;
        }
        return (
            Some(token.clone()),
            args.get(i + 1..).unwrap_or(&[]).to_vec(),
        );
    }
    (None, Vec::new())
}

fn wrapper_value_options(name: &str) -> &'static [&'static str] {
    match name {
        "env" => &[
            "-u",
            "--unset",
            "-C",
            "--chdir",
            "-S",
            "--split-string",
            "-a",
            "--argv0",
        ],
        "exec" => &["-a"],
        "nice" => &["-n", "--adjustment"],
        "sudo" => &[
            "-u",
            "--user",
            "-g",
            "--group",
            "-h",
            "--host",
            "-p",
            "--prompt",
            "-C",
            "--close-from",
            "-D",
            "--chdir",
            "-R",
            "--chroot",
            "-T",
            "--command-timeout",
            "-r",
            "--role",
            "-t",
            "--type",
            "-U",
            "--other-user",
        ],
        "timeout" => &["-k", "--kill-after", "-s", "--signal"],
        "xargs" => &[
            "-a",
            "--arg-file",
            "-d",
            "--delimiter",
            "-E",
            "--eof",
            "-I",
            "--replace",
            "-L",
            "--max-lines",
            "-n",
            "--max-args",
            "-P",
            "--max-procs",
            "-s",
            "--max-chars",
            "--process-slot-var",
        ],
        _ => &[],
    }
}

fn command_start(tokens: &[String]) -> Option<usize> {
    let mut i = 0;
    while i < tokens.len() {
        let name = basename(&tokens[i]);
        if name == "command" && tokens.get(i + 1).is_some_and(|v| v == "-v" || v == "-V") {
            return Some(i);
        }
        if is_assignment(&tokens[i]) {
            i += 1;
            continue;
        }
        if !WRAPPERS.contains(&name) {
            return Some(i);
        }
        i += 1;
        let options = wrapper_value_options(name);
        while i < tokens.len() {
            let option = &tokens[i];
            if is_assignment(option) {
                i += 1;
                continue;
            }
            if option == "--" {
                i += 1;
                break;
            }
            if options.contains(&option.as_str()) {
                i = i.saturating_add(2);
                continue;
            }
            if options
                .iter()
                .any(|o| starts_with_option(option, o) && o.starts_with("--"))
                || option.starts_with('-')
            {
                i += 1;
                continue;
            }
            break;
        }
        if name == "timeout" && i < tokens.len() {
            i += 1;
        }
    }
    None
}

fn is_assignment(value: &str) -> bool {
    let Some((key, _)) = value.split_once('=') else {
        return false;
    };
    !key.is_empty()
        && key.bytes().enumerate().all(|(i, c)| {
            (i == 0 && (c.is_ascii_alphabetic() || c == b'_'))
                || (i > 0 && (c.is_ascii_alphanumeric() || c == b'_'))
        })
}

fn ambiguous_wrapper_options(tokens: &[String]) -> bool {
    let mut i = 0;
    while i < tokens.len() {
        let name = basename(&tokens[i]);
        if !WRAPPERS.contains(&name) {
            return false;
        }
        if name == "command" && tokens.get(i + 1).is_some_and(|v| v == "-v" || v == "-V") {
            return false;
        }
        i += 1;
        let options = wrapper_value_options(name);
        while i < tokens.len() {
            let option = &tokens[i];
            if is_assignment(option) {
                i += 1;
                continue;
            }
            if option == "--" {
                i += 1;
                break;
            }
            if options.contains(&option.as_str()) {
                if i + 1 >= tokens.len() {
                    return true;
                }
                i += 2;
                continue;
            }
            if options
                .iter()
                .any(|o| starts_with_option(option, o) && o.starts_with("--"))
            {
                i += 1;
                continue;
            }
            if option.starts_with('-') {
                return true;
            }
            break;
        }
        if name == "timeout" && i < tokens.len() {
            i += 1;
        }
    }
    false
}

fn env_split_string(tokens: &[String]) -> bool {
    let mut i = 0;
    while i < tokens.len() && is_assignment(&tokens[i]) {
        i += 1;
    }
    if tokens.get(i).map(|v| basename(v)) != Some("env") {
        return false;
    }
    i += 1;
    while i < tokens.len() {
        let token = &tokens[i];
        if is_assignment(token) {
            i += 1;
            continue;
        }
        if token == "-S"
            || token == "--split-string"
            || token.starts_with("-S")
            || token.starts_with("--split-string=")
        {
            return true;
        }
        if token == "--" {
            return false;
        }
        let options = wrapper_value_options("env");
        if options.contains(&token.as_str()) {
            i += 2;
            continue;
        }
        if options
            .iter()
            .any(|o| starts_with_option(token, o) && o.starts_with("--"))
        {
            i += 1;
            continue;
        }
        if token.starts_with('-') {
            i += 1;
            continue;
        }
        return false;
    }
    false
}

fn wrapper_chain_uses_split(tokens: &[String]) -> bool {
    let mut i = 0;
    while i < tokens.len() {
        while i < tokens.len() && is_assignment(&tokens[i]) {
            i += 1;
        }
        let Some(token) = tokens.get(i) else {
            return false;
        };
        let name = basename(token);
        if !WRAPPERS.contains(&name) {
            return false;
        }
        if name == "env" && env_split_string(&tokens[i..]) {
            return true;
        }
        if name == "command" && tokens.get(i + 1).is_some_and(|v| v == "-v" || v == "-V") {
            return false;
        }
        i += 1;
        let options = wrapper_value_options(name);
        while i < tokens.len() {
            if is_assignment(&tokens[i]) {
                i += 1;
                continue;
            }
            if tokens[i] == "--" {
                i += 1;
                break;
            }
            if options.contains(&tokens[i].as_str()) {
                i += 2;
                continue;
            }
            if options
                .iter()
                .any(|o| starts_with_option(&tokens[i], o) && o.starts_with("--"))
                || tokens[i].starts_with('-')
            {
                i += 1;
                continue;
            }
            break;
        }
        if name == "timeout" && i < tokens.len() {
            i += 1;
        }
    }
    false
}

/// Split a shell command without invoking a shell.  Punctuation is kept as a
/// token so callers can reject control operators and redirections.
fn command_segments(command: &str) -> Vec<Vec<String>> {
    let mut segments = Vec::new();
    let mut segment = Vec::new();
    let mut token = String::new();
    let mut quote: Option<u8> = None;
    let mut escaped = false;
    let mut comment = false;
    for c in command.bytes() {
        if comment {
            if c == b'\n' {
                comment = false;
            } else {
                continue;
            }
        }
        if escaped {
            token.push(c as char);
            escaped = false;
            continue;
        }
        if quote == Some(b'\'') {
            if c == b'\'' {
                quote = None;
            } else {
                token.push(c as char);
            }
            continue;
        }
        if quote == Some(b'"') {
            if c == b'"' {
                quote = None;
            } else if c == b'\\' {
                escaped = true;
            } else {
                token.push(c as char);
            }
            continue;
        }
        match c {
            b'\'' | b'"' => quote = Some(c),
            b'\\' => escaped = true,
            b'#' if token.is_empty() && segment.is_empty() => comment = true,
            b' ' | b'\t' | b'\r' => {
                if !token.is_empty() {
                    segment.push(std::mem::take(&mut token));
                }
            }
            b';' | b'&' | b'|' | b'(' | b')' | b'<' | b'>' | b'\n' => {
                if !token.is_empty() {
                    segment.push(std::mem::take(&mut token));
                }
                if !segment.is_empty() {
                    segments.push(std::mem::take(&mut segment));
                }
            }
            _ => token.push(c as char),
        }
    }
    if !token.is_empty() {
        segment.push(token);
    }
    if !segment.is_empty() {
        segments.push(segment);
    }
    segments
}

fn guarded_executable_token(token: &str) -> bool {
    let name = basename(token);
    RESTRICTED_COMMANDS.contains(&name)
        || DESTRUCTIVE_COMMANDS.contains(&name)
        || SHELLS.contains(&name)
        || [".", "eval", "find", "source"].contains(&name)
        || git_subcommand_executable(token)
}

fn has_command_resolution_mutation(tokens: &[String]) -> bool {
    let Some(start) = command_start(tokens) else {
        return false;
    };
    COMMAND_RESOLUTION_MUTATIONS.contains(&basename(&tokens[start]))
}

fn has_shell_context_mutation(tokens: &[String]) -> bool {
    if tokens.is_empty() {
        return false;
    }
    if tokens.iter().all(|token| is_assignment(token)) {
        return true;
    }
    command_start(tokens)
        .and_then(|start| tokens.get(start))
        .is_some_and(|token| SHELL_CONTEXT_MUTATIONS.contains(&basename(token)))
}

fn has_unparsed_guarded_command(tokens: &[String]) -> bool {
    if tokens.is_empty() || !SHELL_COMPOUND_PREFIXES.contains(&tokens[0].as_str()) {
        return false;
    }
    let mut arguments = tokens.to_vec();
    let mut ambiguous_compound = false;
    loop {
        while arguments.first().is_some_and(|token| is_assignment(token)) {
            arguments.remove(0);
        }
        let Some(prefix) = arguments.first().map(String::as_str) else {
            return false;
        };
        if ["coproc", "function"].contains(&prefix) {
            ambiguous_compound = true;
            arguments.remove(0);
            break;
        }
        if SHELL_COMMAND_PREFIXES.contains(&prefix) {
            arguments.remove(0);
            continue;
        }
        if prefix == "time" {
            arguments.remove(0);
            while arguments
                .first()
                .is_some_and(|token| ["-p", "--"].contains(&token.as_str()))
            {
                arguments.remove(0);
            }
            continue;
        }
        if prefix == "repeat" {
            if arguments.len() < 2 {
                return false;
            }
            arguments.drain(..2);
            continue;
        }
        break;
    }

    if ambiguous_compound {
        return arguments.iter().enumerate().any(|(index, token)| {
            basename(token) == "env" && env_split_string(&arguments[index..])
        }) || arguments
            .iter()
            .any(|token| guarded_executable_token(token) || command_word_expansion(token));
    }
    if has_command_resolution_mutation(&arguments) || wrapper_chain_uses_split(&arguments) {
        return true;
    }
    if arguments
        .iter()
        .any(|token| guarded_executable_token(token))
    {
        return true;
    }
    let Some(start) = command_start(&arguments) else {
        return false;
    };
    if ambiguous_wrapper_options(&arguments)
        && (arguments
            .iter()
            .any(|token| guarded_executable_token(token))
            || arguments.iter().any(|token| command_word_expansion(token)))
    {
        return true;
    }
    command_word_expansion(&arguments[start])
        || guarded_executable_token(&arguments[start])
        || python_helper_invocation_reason(&arguments).is_some()
}

fn has_unquoted(command: &str, chars: &[u8]) -> bool {
    let mut quote = 0u8;
    let mut escaped = false;
    let bytes = command.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if quote == b'\'' {
            if c == b'\'' {
                quote = 0;
            }
            i += 1;
            continue;
        }
        if quote == b'"' {
            if c == b'\\' {
                i += 2;
                continue;
            }
            if c == b'"' {
                quote = 0;
            }
            i += 1;
            continue;
        }
        if escaped {
            escaped = false;
            i += 1;
            continue;
        }
        if c == b'\\' {
            escaped = true;
            i += 1;
            continue;
        }
        if c == b'\'' || c == b'"' {
            quote = c;
            i += 1;
            continue;
        }
        if c == b'#' && (i == 0 || bytes[i - 1].is_ascii_whitespace()) {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if chars.contains(&c) {
            return true;
        }
        i += 1;
    }
    false
}

fn shell_expansion(command: &str) -> bool {
    has_unquoted(command, b"${*?[{`")
}

fn line_continuation(command: &str) -> bool {
    let bytes = command.as_bytes();
    let mut quote = 0u8;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if quote == b'\'' {
            if c == b'\'' {
                quote = 0;
            }
            i += 1;
            continue;
        }
        if c == b'\'' || c == b'"' {
            quote = if quote == 0 { c } else { 0 };
            i += 1;
            continue;
        }
        if c == b'\\' && i + 1 < bytes.len() && bytes[i + 1] == b'\n' && quote != b'\'' {
            return true;
        }
        i += if c == b'\\' { 2 } else { 1 };
    }
    false
}

fn command_word_expansion(token: &str) -> bool {
    token.starts_with('=')
        || token.starts_with('~')
        || token.chars().any(|c| "$`*?[".contains(c))
        || (token.contains('{') || token.contains('}')) && token != "{" && token != "}"
}

fn git_subcommand_executable(token: &str) -> bool {
    let value = basename(token);
    value.strip_prefix("git-").is_some_and(|rest| {
        !rest.is_empty()
            && rest
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
    })
}

fn option_values(args: &[String], short: &str, long: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == short || args[i] == long {
            let value = args.get(i + 1)?;
            out.push(value.clone());
            i += 2;
            continue;
        }
        if args[i].starts_with(&format!("{long}=")) {
            out.push(
                args[i]
                    .split_once('=')
                    .map(|(_, v)| v.to_string())
                    .unwrap_or_default(),
            );
        } else if !short.is_empty() && args[i].starts_with(short) && args[i] != short {
            out.push(args[i][short.len()..].to_string());
        }
        i += 1;
    }
    Some(out)
}

fn git_add_reason(args: &[String]) -> Option<String> {
    let Some(separator) = args.iter().position(|v| v == "--") else {
        return Some("git add は -- の後に対象パスを明示してください".into());
    };
    if separator != 0 || args.len() == separator + 1 {
        return Some("git add のオプション指定または空の対象は許可されていません".into());
    }
    for path in &args[separator + 1..] {
        let p = Path::new(path);
        if p.is_absolute()
            || [".", ".."].contains(&path.as_str())
            || path.starts_with('-')
            || path.starts_with(':')
            || p.components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Some("git add は個別のファイルまたはディレクトリだけを指定してください".into());
        }
        if path.chars().any(|c| "*?[]".contains(c)) {
            return Some("git add でglobは使用できません".into());
        }
    }
    None
}

fn git_commit_reason(args: &[String]) -> Option<String> {
    let forbidden = [
        "--amend",
        "--no-verify",
        "--signoff",
        "-s",
        "--author",
        "--date",
        "--reset-author",
        "--fixup",
        "--squash",
        "--reuse-message",
        "-C",
        "--reedit-message",
        "-c",
        "--no-gpg-sign",
    ];
    if args.iter().any(|a| {
        forbidden.contains(&a.as_str())
            || forbidden
                .iter()
                .any(|f| f.starts_with("--") && starts_with_option(a, f))
    }) {
        return Some("履歴改変、検証回避、author・帰属の上書きは許可されていません".into());
    }
    let Some(messages) = option_values(args, "-m", "--message") else {
        return Some("commit messageは-mで1件だけ明示してください".into());
    };
    if messages.len() != 1 {
        return Some("commit messageは-mで1件だけ明示してください".into());
    }
    let mut index = 0;
    while index < args.len() {
        let token = &args[index];
        if token == "-m" || token == "--message" {
            index += 2;
            continue;
        }
        if token.starts_with("--message=") || token.starts_with("-m") && token != "-m" {
            index += 1;
            continue;
        }
        if token == "-S" || token == "--gpg-sign" || token.starts_with("--gpg-sign=") {
            index += 1;
            continue;
        }
        return Some("git commit ではmessageとDaikiの署名設定以外の引数を使用できません".into());
    }
    let message = &messages[0];
    if message.contains('\n') || !valid_commit_subject(message) {
        return Some("commit messageは':gitmoji: 短い要約'の1行形式にしてください".into());
    }
    if ai_attribution(message) {
        return Some("CodexまたはOpenAIのAI帰属は記録できません".into());
    }
    if contains_secret(message) {
        return Some("commit messageに秘密情報らしい値が含まれています".into());
    }
    None
}

fn valid_commit_subject(message: &str) -> bool {
    let Some(rest) = message.strip_prefix(':') else {
        return false;
    };
    let Some((tag, subject)) = rest.split_once(':') else {
        return false;
    };
    !tag.is_empty()
        && tag.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_+-".contains(&byte)
        })
        && subject
            .strip_prefix(' ')
            .and_then(|value| value.chars().next())
            .is_some_and(|character| !character.is_whitespace())
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
        || key == "credential.helper"
        || key == "http.proxy"
        || key == "http.sslcainfo"
        || key == "http.sslverify"
        || key.starts_with("http.")
            && (key.ends_with(".proxy")
                || key.ends_with(".extraheader")
                || key.ends_with(".proxycommand")
                || key.ends_with(".sslcainfo")
                || key.ends_with(".sslverify"))
        || key.starts_with("pager.")
        || key.starts_with("filter.")
            && (key.ends_with(".process")
                || key.ends_with(".clean")
                || key.ends_with(".smudge")
                || key.ends_with(".required"))
        || key.starts_with("diff.") && (key.ends_with(".command") || key.ends_with(".textconv"))
        || key.starts_with("merge.") && key.ends_with(".driver")
        || key.starts_with("remote.")
            && (key.ends_with(".proxy")
                || key.ends_with(".uploadpack")
                || key.ends_with(".receivepack")
                || key.ends_with(".pushurl"))
        || key.starts_with("url.")
            && (key.ends_with(".insteadof") || key.ends_with(".pushinsteadof"))
        || key.starts_with("protocol.") && key.ends_with(".allow")
        || key == "interactive.difffilter"
        || key == "gc.recentobjectshook"
        || key == "core.alternaterefscommand"
}

fn isolate_git_environment(command: &mut Command) {
    command
        .env_clear()
        .envs(std::env::vars_os().filter(|(key, _)| {
            let key = key.to_string_lossy();
            !key.starts_with("GIT_")
                && key != "SSH_ASKPASS"
                && !key.eq_ignore_ascii_case("HTTP_PROXY")
                && !key.eq_ignore_ascii_case("HTTPS_PROXY")
                && !key.eq_ignore_ascii_case("ALL_PROXY")
                && !key.eq_ignore_ascii_case("NO_PROXY")
                && !key.eq_ignore_ascii_case("SSL_CERT_FILE")
                && !key.eq_ignore_ascii_case("SSL_CERT_DIR")
        }));
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_PAGER", "cat")
        .env("PATH", SYSTEM_PATH);
}

fn local_git_config_is_safe(cwd: &str) -> bool {
    let Ok(git) = trust::trusted_system_binary(SYSTEM_GIT, "Git") else {
        return false;
    };
    let mut command = Command::new(git);
    command.current_dir(cwd).args([
        "config",
        "--local",
        "--null",
        "--name-only",
        "--get-regexp",
        ".*",
    ]);
    isolate_git_environment(&mut command);
    let Ok(output) = process::run_with_limit(
        &mut command,
        GIT_COMMAND_TIMEOUT,
        MAX_MANIFEST_BYTES as usize,
    ) else {
        return false;
    };
    if !output.status.success() && output.status.code() != Some(1) {
        return false;
    }
    String::from_utf8(output.stdout).is_ok_and(|keys| {
        !keys
            .split('\0')
            .filter(|key| !key.is_empty())
            .any(dangerous_local_git_key)
    })
}

fn run_git(cwd: &str, args: &[&str]) -> Option<String> {
    if cwd.is_empty() || !local_git_config_is_safe(cwd) {
        return None;
    }
    let git = trust::trusted_system_binary(SYSTEM_GIT, "Git").ok()?;
    let mut command = Command::new(git);
    command
        .args([
            "--no-pager",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.pager=cat",
            "-c",
            "diff.external=",
        ])
        .args(args)
        .current_dir(cwd);
    isolate_git_environment(&mut command);
    let output =
        process::run_with_limit(&mut command, GIT_COMMAND_TIMEOUT, MAX_GIT_OUTPUT_BYTES).ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

struct GuardGhSandbox {
    path: PathBuf,
}

impl GuardGhSandbox {
    #[cfg(unix)]
    fn create() -> Option<Self> {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

        let root = fs::canonicalize("/tmp").ok()?;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_nanos();
        let mut path = None;
        for _ in 0..32 {
            let counter = BODY_SNAPSHOT_COUNTER.fetch_add(1, Ordering::Relaxed);
            let candidate = root.join(format!(
                ".codex-hook-gh-{}-{timestamp}-{counter}",
                std::process::id()
            ));
            let mut builder = fs::DirBuilder::new();
            builder.mode(0o700);
            match builder.create(&candidate) {
                Ok(()) => {
                    path = Some(candidate);
                    break;
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(_) => return None,
            }
        }
        let sandbox = Self { path: path? };

        let account_home = unsafe {
            let entry = libc::getpwuid(libc::getuid());
            if entry.is_null() {
                None
            } else {
                CStr::from_ptr((*entry).pw_dir)
                    .to_str()
                    .ok()
                    .map(PathBuf::from)
            }
        }?;
        let source = account_home.join(".config/gh/hosts.yml");
        if source.exists() {
            let mut options = fs::OpenOptions::new();
            options
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
            let mut source_file = options.open(source).ok()?;
            let metadata = source_file.metadata().ok()?;
            if !metadata.is_file()
                || metadata.uid() != unsafe { libc::getuid() }
                || metadata.mode() & 0o077 != 0
                || metadata.len() > MAX_MANIFEST_BYTES
            {
                return None;
            }
            let mut contents = Vec::with_capacity(metadata.len() as usize);
            source_file.read_to_end(&mut contents).ok()?;
            if contents.len() as u64 != metadata.len() {
                return None;
            }
            let destination = sandbox.path.join("hosts.yml");
            let mut destination_file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o400)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(destination)
                .ok()?;
            destination_file.write_all(&contents).ok()?;
            destination_file.sync_all().ok()?;
        }
        sync_body_snapshot_directory(&sandbox.path).ok()?;
        fs::set_permissions(&sandbox.path, fs::Permissions::from_mode(0o500)).ok()?;
        Some(sandbox)
    }

    #[cfg(not(unix))]
    fn create() -> Option<Self> {
        None
    }
}

impl Drop for GuardGhSandbox {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            let _ = fs::set_permissions(&self.path, fs::Permissions::from_mode(0o700));
        }
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn run_gh(cwd: &str, args: &[&str]) -> Option<(i32, Vec<u8>)> {
    if cwd.is_empty() {
        return None;
    }
    let sandbox = GuardGhSandbox::create()?;
    let gh = trust::trusted_system_binary(SYSTEM_GH, "GitHub CLI").ok()?;
    let mut command = Command::new(gh);
    command.args(args).current_dir(cwd);
    command
        .env_clear()
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_HOST", "github.com")
        .env("GH_CONFIG_DIR", &sandbox.path)
        .env("PATH", SYSTEM_PATH);
    let output = process::run_with_limit(
        &mut command,
        GH_COMMAND_TIMEOUT,
        MAX_BODY_FILE_BYTES as usize,
    )
    .ok()?;
    Some((output.status.code().unwrap_or(1), output.stdout))
}

fn resolved_git_path(cwd: &str, argument: &str) -> Option<PathBuf> {
    let output = run_git(cwd, &["rev-parse", "--path-format=absolute", argument])?;
    fs::canonicalize(output.trim()).ok()
}

fn github_repository_from_url(remote: &str) -> Option<String> {
    let mut url = remote.trim().trim_end_matches('/');
    let prefixes = [
        "https://github.com/",
        "http://github.com/",
        "ssh://git@github.com/",
        "git@github.com:",
    ];
    for prefix in prefixes {
        if let Some(rest) = url.strip_prefix(prefix) {
            url = rest;
            let repository = url.strip_suffix(".git").unwrap_or(url);
            let mut parts = repository.split('/');
            let owner = parts.next()?;
            let name = parts.next()?;
            if parts.next().is_none()
                && !owner.is_empty()
                && !name.is_empty()
                && owner.bytes().all(valid_repo_char)
                && name.bytes().all(valid_repo_char)
            {
                return Some(format!("{owner}/{name}"));
            }
        }
    }
    None
}

fn valid_repo_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || b"_.-".contains(&c)
}

fn origin_repository(cwd: &str) -> Option<String> {
    github_repository_from_url(run_git(cwd, &["remote", "get-url", "origin"])?.trim())
}

fn current_work_branch_reason(cwd: &str) -> Option<String> {
    let Some(current) = run_git(cwd, &["rev-parse", "--abbrev-ref", "HEAD"]) else {
        return Some("current work branchを確認できないためGit書き込みを拒否しました".into());
    };
    if valid_work_branch(current.trim()) {
        None
    } else {
        Some("git add/commitは非保護の作業branch上だけで実行できます".into())
    }
}

fn clean_worktree_reason(cwd: &str, operation: &str) -> Option<String> {
    let Some(status) = run_git(cwd, &["status", "--porcelain=v1", "--untracked-files=all"]) else {
        return Some(format!(
            "worktree状態を確認できないためgit {operation}を拒否しました"
        ));
    };
    if status.trim().is_empty() {
        None
    } else {
        Some(format!(
            "worktreeがcleanではないためgit {operation}を拒否しました"
        ))
    }
}

fn git_fetch_reason(args: &[String]) -> Option<String> {
    if args.len() != 2 || args.first().map(String::as_str) != Some("origin") {
        return Some("git fetch はoriginと単一のbase branchを明示してください".into());
    }
    let base = args[1].strip_prefix("refs/heads/").unwrap_or(&args[1]);
    if protected_branch(base) {
        None
    } else {
        Some("git fetch のbase branchが許可範囲外です".into())
    }
}

fn git_worktree_read_reason(args: &[String]) -> Option<String> {
    if args == ["list", "--porcelain"] || args == ["list", "--porcelain", "-z"] {
        None
    } else {
        Some("git worktreeは安定形式のlist照会だけ許可されます".into())
    }
}

fn git_ls_remote_read_reason(args: &[String]) -> Option<String> {
    if args.len() != 3
        || !["--branches", "--heads"].contains(&args[0].as_str())
        || args[1] != "origin"
    {
        return Some(
            "git ls-remoteはoriginの単一作業branchを照会する正規形だけ許可されます".into(),
        );
    }
    let branch = args[2].strip_prefix("refs/heads/").unwrap_or(&args[2]);
    if valid_work_branch(branch)
        && valid_remote_branch(branch)
        && !args[2].chars().any(|c| "*?[]".contains(c))
    {
        None
    } else {
        Some("git ls-remoteは安全な作業branchのrefを1件だけ指定してください".into())
    }
}

fn git_read_reason(command: &str, args: &[String], global_option: bool) -> Option<String> {
    if global_option {
        return Some("読み取りGitではglobal optionを使用できません".into());
    }
    if args.iter().any(|arg| {
        GIT_READ_FORBIDDEN_ARGS.contains(&arg.as_str())
            || arg.starts_with("--output=")
            || arg.starts_with("--paginate=")
    }) {
        return Some("外部command実行またはfile書き込みを伴うGit optionは使用できません".into());
    }
    if command == "grep"
        && args
            .iter()
            .any(|arg| arg.starts_with("-O") || arg.starts_with("--open-files-in-pager"))
    {
        return Some("git grepの外部pager実行は許可されていません".into());
    }
    if command == "reflog" {
        let operation = args.iter().find(|arg| !arg.starts_with('-'));
        if operation.is_some_and(|v| !["show", "list", "exists"].contains(&v.as_str())) {
            return Some("git reflogはshow、list、existsだけ許可されます".into());
        }
    }
    None
}

fn repository_key(repository: &str) -> Option<String> {
    let (owner, name) = repository.split_once('/')?;
    Some(format!(
        "{}-{}--{}-{}",
        owner.len(),
        owner.to_ascii_lowercase(),
        name.len(),
        name.to_ascii_lowercase()
    ))
}

fn uid_is_current(metadata: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid only reads the effective UID of the current process.
        metadata.uid() == unsafe { libc::geteuid() } as u32
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        false
    }
}

fn no_symlink_components(path: &Path) -> bool {
    let Some(root) = path.ancestors().last() else {
        return false;
    };
    let mut current = PathBuf::from(root);
    for component in path
        .strip_prefix(root)
        .ok()
        .into_iter()
        .flat_map(Path::components)
    {
        current.push(component.as_os_str());
        let Ok(metadata) = fs::symlink_metadata(&current) else {
            return false;
        };
        if metadata.file_type().is_symlink() || (!current.as_path().eq(path) && !metadata.is_dir())
        {
            return false;
        }
    }
    true
}

struct StrictObjectVisitor;

impl<'de> Visitor<'de> for StrictObjectVisitor {
    type Value = BTreeMap<String, StrictJsonValue>;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("JSON object with unique keys")
    }

    fn visit_map<A>(self, mut access: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut values = BTreeMap::new();
        while let Some(key) = access.next_key::<String>()? {
            if values.contains_key(&key) {
                return Err(de::Error::custom("duplicate JSON key"));
            }
            values.insert(key, access.next_value::<StrictJsonValue>()?);
        }
        Ok(values)
    }
}

fn parse_strict_object(contents: &str) -> Option<BTreeMap<String, StrictJsonValue>> {
    let mut deserializer = serde_json::Deserializer::from_str(contents);
    let values = deserializer.deserialize_map(StrictObjectVisitor).ok()?;
    deserializer.end().ok()?;
    Some(values)
}

fn manifest_schema_matches(
    manifest: &BTreeMap<String, StrictJsonValue>,
    expected_strings: &[(&str, &str)],
) -> bool {
    let string_fields = [
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
    manifest.len() == string_fields.len() + 1
        && manifest.get("version").and_then(StrictJsonValue::as_i64) == Some(1)
        && string_fields.iter().all(|key| {
            manifest
                .get(*key)
                .and_then(StrictJsonValue::as_str)
                .is_some()
        })
        && expected_strings.iter().all(|(key, expected)| {
            manifest.get(*key).and_then(StrictJsonValue::as_str) == Some(*expected)
        })
}

fn managed_worktree_reason(
    repository_root: &Path,
    common_git_dir: &Path,
    worktree_root: &Path,
) -> Option<String> {
    let Some(repository_path) = repository_root.to_str() else {
        return Some("managed worktreeのrepository pathを確認できません".into());
    };
    let Some(repository) = origin_repository(repository_path) else {
        return Some("managed worktreeのoriginを確認できません".into());
    };
    let Some(worktree_path) = worktree_root.to_str() else {
        return Some("managed worktree pathを確認できません".into());
    };
    let Some(worktree_repository) = origin_repository(worktree_path) else {
        return Some("managed worktreeのoriginを確認できません".into());
    };
    if !worktree_repository.eq_ignore_ascii_case(&repository) {
        return Some("managed worktreeのoriginがsession repositoryと一致しません".into());
    }
    let codex_home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/"))
                .join(".codex")
        });
    if !codex_home.is_absolute() || !no_symlink_components(&codex_home) {
        return Some("CODEX_HOMEを安全に解決できません".into());
    }
    let Some(key) = repository_key(&repository) else {
        return Some("managed worktree repository名を確認できません".into());
    };
    let Some(worktree_name) = worktree_root.file_name() else {
        return Some("managed worktree pathを確認できません".into());
    };
    let expected = codex_home.join("worktrees").join(key).join(worktree_name);
    if worktree_root != expected {
        return Some("requested cwdが登録対象のmanaged worktreeではありません".into());
    }
    let Some(worktree_name) = worktree_name.to_str() else {
        return Some("managed worktree pathを確認できません".into());
    };
    if !no_symlink_components(&expected) || !valid_task_id(worktree_name) {
        return Some("managed worktree pathを安全に解決できません".into());
    }
    let Some(expected_parent) = expected.parent() else {
        return Some("managed worktree pathを確認できません".into());
    };
    let state_root = expected_parent.join(".state");
    let manifest_path = state_root.join(format!("{worktree_name}.json"));
    let Ok(state_meta) = fs::symlink_metadata(&state_root) else {
        return Some("managed worktree state directoryを安全に検査できません".into());
    };
    let Ok(manifest_meta) = fs::symlink_metadata(&manifest_path) else {
        return Some("managed worktree manifestを安全に検査できません".into());
    };
    if !state_meta.is_dir()
        || state_meta.file_type().is_symlink()
        || !uid_is_current(&state_meta)
        || mode_has_group_or_other(&state_meta)
        || !manifest_meta.is_file()
        || manifest_meta.file_type().is_symlink()
        || !uid_is_current(&manifest_meta)
        || mode_has_group_or_other(&manifest_meta)
        || manifest_meta.len() > MAX_MANIFEST_BYTES
    {
        return Some("managed worktree manifestを安全に検査できません".into());
    }
    let Ok(manifest) = fs::read_to_string(&manifest_path) else {
        return Some("managed worktree manifestを安全に検査できません".into());
    };
    let Some(manifest) = parse_strict_object(&manifest) else {
        return Some("managed worktree manifestを安全に検査できません".into());
    };
    let Some(branch) = run_git(worktree_path, &["rev-parse", "--abbrev-ref", "HEAD"]) else {
        return Some("managed worktreeのbranchを確認できません".into());
    };
    let expected_strings = [
        ("status", "ready"),
        ("task_id", worktree_name),
        ("repository", repository_path),
        ("common_git_dir", common_git_dir.to_str().unwrap_or("")),
        ("github_name", repository.as_str()),
        ("worktree", worktree_path),
        ("branch", branch.trim()),
    ];
    if !manifest_schema_matches(&manifest, &expected_strings) {
        return Some("managed worktree manifestがcurrent repository状態と一致しません".into());
    }
    let Some(registered) = run_git(repository_path, &["worktree", "list", "--porcelain", "-z"])
    else {
        return Some("Git worktree登録を確認できません".into());
    };
    if !registered.contains(&format!("worktree {}\0", worktree_root.display())) {
        return Some("requested cwdがGit worktreeとして登録されていません".into());
    }
    None
}

fn branch_worktree(cwd: &str, head: &str) -> Result<PathBuf, String> {
    let Some(output) = run_git(cwd, &["worktree", "list", "--porcelain", "-z"]) else {
        return Err("Draft PRの登録済みworktreeを確認できません".into());
    };
    let expected_branch = format!("refs/heads/{head}");
    let mut matches = Vec::new();
    for raw_record in output.split("\0\0").filter(|record| !record.is_empty()) {
        let fields = raw_record.split('\0').collect::<Vec<_>>();
        let paths = fields
            .iter()
            .filter_map(|field| field.strip_prefix("worktree "))
            .collect::<Vec<_>>();
        let branches = fields
            .iter()
            .filter_map(|field| field.strip_prefix("branch "))
            .collect::<Vec<_>>();
        if branches != [expected_branch.as_str()] {
            continue;
        }
        if paths.len() != 1
            || fields
                .iter()
                .any(|field| *field == "prunable" || field.starts_with("prunable "))
        {
            return Err("Draft PRのhead worktree登録が安全な状態ではありません".into());
        }
        matches.push(PathBuf::from(paths[0]));
    }
    if matches.len() != 1 {
        return Err("Draft PRのhead branchを所有するworktreeを一意に確認できません".into());
    }
    let candidate = &matches[0];
    if !candidate.is_absolute()
        || candidate
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err("Draft PRのhead worktree pathを安全に解決できません".into());
    }
    let resolved = fs::canonicalize(candidate)
        .map_err(|_| "Draft PRのhead worktree pathを安全に解決できません".to_string())?;
    if candidate != &resolved {
        return Err("Draft PRのhead worktreeにsymlinkまたは非正規pathは使用できません".into());
    }
    let Some(resolved_str) = resolved.to_str() else {
        return Err("Draft PRのhead worktree rootを確認できません".into());
    };
    if resolved_git_path(resolved_str, "--show-toplevel").as_ref() != Some(&resolved) {
        return Err("Draft PRのhead worktree rootを確認できません".into());
    }
    Ok(resolved)
}

fn remote_refs_snapshot(cwd: &str, head: Option<&str>) -> Option<(String, String, Option<String>)> {
    let mut owned_args = vec![
        "ls-remote".to_string(),
        "--symref".to_string(),
        "origin".to_string(),
        "HEAD".to_string(),
    ];
    if let Some(head) = head {
        owned_args.push(format!("refs/heads/{head}"));
    }
    let args = owned_args.iter().map(String::as_str).collect::<Vec<_>>();
    let output = run_git(cwd, &args)?;
    let lines = output.lines().collect::<Vec<_>>();
    if lines.len() != if head.is_some() { 3 } else { 2 } {
        return None;
    }
    let default_line = lines[0].strip_prefix("ref: refs/heads/")?;
    let (default_branch, marker) = default_line.split_once('\t')?;
    if marker != "HEAD"
        || !valid_remote_branch(default_branch)
        || !valid_oid(lines[1].split_once('\t')?.0)
    {
        return None;
    }
    let (default_oid, default_marker) = lines[1].split_once('\t')?;
    if default_marker != "HEAD" || !valid_oid(default_oid) {
        return None;
    }
    let remote_head = if let Some(head) = head {
        let (oid, reference) = lines[2].split_once('\t')?;
        if reference != format!("refs/heads/{head}") || !valid_oid(oid) {
            return None;
        }
        Some(oid.to_string())
    } else {
        None
    };
    Some((
        format!("origin/{default_branch}"),
        default_oid.to_string(),
        remote_head,
    ))
}

fn draft_pr_preflight_reason(cwd: &str, base: &str, head: &str) -> Option<String> {
    let head_cwd = match branch_worktree(cwd, head) {
        Ok(path) => path,
        Err(reason) => return Some(reason),
    };
    let head_cwd = match head_cwd.to_str() {
        Some(path) => path,
        None => return Some("Draft PRのhead worktree pathを確認できません".into()),
    };
    let session_common = resolved_git_path(cwd, "--git-common-dir");
    let head_common = resolved_git_path(head_cwd, "--git-common-dir");
    if session_common.is_none() || session_common != head_common {
        return Some("Draft PRのhead worktreeがsession repositoryと一致しません".into());
    }
    if origin_repository(cwd) != origin_repository(head_cwd) {
        return Some("Draft PRのhead worktreeのoriginがsession repositoryと一致しません".into());
    }
    let current = run_git(head_cwd, &["rev-parse", "--abbrev-ref", "HEAD"]);
    let local_head = run_git(head_cwd, &["rev-parse", "HEAD"]);
    if current.as_deref().map(str::trim) != Some(head) || local_head.is_none() {
        return Some("Draft PRのhead worktreeとbranchを確認できません".into());
    }
    let status = run_git(
        head_cwd,
        &["status", "--porcelain=v1", "--untracked-files=all"],
    );
    if status
        .as_deref()
        .is_none_or(|value| !value.trim().is_empty())
    {
        return Some("Draft PRのhead worktreeがcleanではありません".into());
    }
    let snapshot = match remote_refs_snapshot(head_cwd, Some(head)) {
        Some(value) => value,
        None => return Some("Draft PRのremote refを安全に確認できません".into()),
    };
    let (remote_default_ref, remote_default_oid, remote_head) = snapshot;
    if remote_default_ref != format!("origin/{base}") {
        return Some("Draft PRのbaseはoriginのdefault branchと一致させてください".into());
    }
    let local_default_oid = run_git(
        head_cwd,
        &[
            "rev-parse",
            "--verify",
            &format!("refs/remotes/{remote_default_ref}"),
        ],
    );
    if local_default_oid
        .as_deref()
        .map(str::trim)
        .is_none_or(|value| !value.eq_ignore_ascii_case(&remote_default_oid))
    {
        return Some("Draft PRのbase remote-tracking refがremoteと一致しません".into());
    }
    if remote_head.is_none_or(|value| {
        local_head
            .as_deref()
            .map(str::trim)
            .is_none_or(|local| !local.eq_ignore_ascii_case(&value))
    }) {
        return Some("Draft PRのheadはpush済みのworktree HEADと一致させてください".into());
    }
    None
}

fn mode_has_group_or_other(metadata: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o077 != 0
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        false
    }
}

fn valid_task_id(value: &str) -> bool {
    if let Some(rest) = value.strip_prefix("issue-") {
        return !rest.is_empty()
            && rest.bytes().all(|c| c.is_ascii_digit())
            && rest.as_bytes().first() != Some(&b'0');
    }
    if let Some(rest) = value.strip_prefix("task-") {
        return !rest.is_empty()
            && rest.len() <= 64
            && rest
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
            && rest
                .as_bytes()
                .first()
                .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    }
    false
}

fn valid_plan_id(value: &str) -> bool {
    let Some((prefix, version)) = value.rsplit_once("-v") else {
        return false;
    };
    prefix.len() >= 8
        && prefix.len() <= 128
        && prefix
            .as_bytes()
            .first()
            .is_some_and(|c| c.is_ascii_uppercase())
        && prefix
            .bytes()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'-')
        && !version.is_empty()
        && !version.starts_with('0')
        && version.bytes().all(|c| c.is_ascii_digit())
}

fn valid_oid(value: &str) -> bool {
    (value.len() == 40 || value.len() == 64) && value.bytes().all(|c| c.is_ascii_hexdigit())
}

fn git_write_target(session_cwd: Option<&str>, explicit: Option<&str>) -> Result<String, String> {
    let session = session_cwd
        .filter(|v| !v.is_empty())
        .ok_or_else(|| "Git書き込みのsession cwdを確認できません".to_string())?;
    if explicit.is_none()
        && std::env::var("CODEX_WORKTREE_MODE").ok().as_deref() == Some("single-checkout")
    {
        return Ok(session.to_string());
    }
    let session_root = resolved_git_path(session, "--show-toplevel")
        .ok_or_else(|| "Git書き込みのsession repositoryを確認できません".to_string())?;
    let common = resolved_git_path(session, "--git-common-dir")
        .ok_or_else(|| "Git書き込みのsession repositoryを確認できません".to_string())?;
    let main = common
        .parent()
        .ok_or_else(|| "Git repository rootを確認できません".to_string())?
        .to_path_buf();
    let target = if let Some(explicit) = explicit {
        let candidate = Path::new(explicit);
        if !candidate.is_absolute()
            || candidate
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err("git -Cにはmanaged worktreeの絶対pathを指定してください".into());
        }
        let resolved =
            fs::canonicalize(candidate).map_err(|_| "git -Cのpathを安全に解決できません")?;
        if resolved != candidate {
            return Err("git -Cではsymlinkまたは非正規pathを使用できません".into());
        }
        let target_root = resolved_git_path(explicit, "--show-toplevel")
            .ok_or_else(|| "git -Cのrepositoryを確認できません".to_string())?;
        let target_common = resolved_git_path(explicit, "--git-common-dir")
            .ok_or_else(|| "git -Cのrepositoryを確認できません".to_string())?;
        if target_root != resolved || target_common != common {
            return Err("git -CのworktreeがCodex sessionのrepositoryと一致しません".into());
        }
        if session_root == main {
            target_root
        } else {
            return Err("linked worktree sessionから別worktreeへ書き込むことはできません".into());
        }
    } else {
        session_root
    };
    if target == main {
        return Err("Git書き込みは専用managed worktreeで実行してください。例: env -u SSH_ASKPASS git -C <managed-worktree> ...".into());
    }
    managed_worktree_reason(&main, &common, &target)
        .map_or(Ok(target.to_string_lossy().into_owned()), Err)
}

fn clean_environment_for_git(removed_ssh_askpass: bool) -> Option<String> {
    if std::env::vars_os().any(|(key, _)| {
        let key = key.to_string_lossy();
        key.starts_with("GIT_") && key != "GIT_PAGER"
    }) {
        return Some("Git commandではGitのrepository状態、path、外部command、出力先を変更する環境変数を使用できません".into());
    }
    let _ = removed_ssh_askpass;
    None
}

fn git_invocation_reason(tokens: &[String], cwd: Option<&str>) -> Option<String> {
    let start = command_start(tokens)?;
    if basename(tokens.get(start)?) != "git" {
        return None;
    }
    if tokens[start] != "git" {
        return Some("Git commandはPATHからgitを直接実行してください".into());
    }
    let mut explicit: Option<String> = None;
    let mut global = false;
    let mut other_global = false;
    let mut removed_ssh = false;
    if start != 0 {
        if tokens[..start] == ["env", "-u", "SSH_ASKPASS"] {
            removed_ssh = true;
        } else {
            return Some("Git commandをwrapper、環境変数、cwd変更経由で実行できません".into());
        }
    }
    let args = &tokens[start + 1..];
    let mut i = 0;
    while i < args.len() {
        let token = &args[i];
        if GIT_FORBIDDEN_OPTIONS
            .iter()
            .any(|o| token == *o || starts_with_option(token, o))
            || token.starts_with("-c") && token != "-C"
        {
            return Some("gitの設定・aliasによるコマンド上書きは許可されていません".into());
        }
        if token == "-C" {
            if explicit.is_some() || i + 1 >= args.len() {
                return Some("git -Cはmanaged worktreeの絶対pathを1件だけ指定してください".into());
            }
            explicit = args.get(i + 1).cloned();
            global = true;
            i += 2;
            continue;
        }
        if GIT_VALUE_OPTIONS[1..].contains(&token.as_str()) {
            global = true;
            other_global = true;
            i += 2;
            continue;
        }
        if GIT_VALUE_OPTIONS[1..]
            .iter()
            .any(|o| starts_with_option(token, o))
        {
            global = true;
            other_global = true;
            i += 1;
            continue;
        }
        if token.starts_with('-') {
            global = true;
            other_global = true;
            i += 1;
            continue;
        }
        let command = token.as_str();
        let command_args = args.get(i + 1..).unwrap_or(&[]).to_vec();
        if let Some(reason) = clean_environment_for_git(removed_ssh) {
            return Some(reason);
        }
        if let Some(explicit) = explicit.as_deref()
            && (GIT_READ_ONLY.contains(&command)
                || ["branch", "remote", "worktree", "ls-remote"].contains(&command))
        {
            if other_global {
                return Some("read-only git -Cでは他のglobal optionを使用できません".into());
            }
            let candidate = Path::new(explicit);
            let Some(session) = cwd else {
                return Some("git -Cのsession cwdを確認できません".into());
            };
            let session_common = resolved_git_path(session, "--git-common-dir");
            let session_root = resolved_git_path(session, "--show-toplevel");
            let target_common = resolved_git_path(explicit, "--git-common-dir");
            let target_root = resolved_git_path(explicit, "--show-toplevel");
            if session_common.is_none()
                || session_root.is_none()
                || target_common.is_none()
                || target_common != session_common
                || target_root.as_deref() != Some(candidate)
            {
                return Some("git -Cのrepositoryを確認できません".into());
            }
            let Some(common_root) = session_common.as_ref().and_then(|value| value.parent()) else {
                return Some("git -Cのrepositoryを確認できません".into());
            };
            let Some(target_root) = target_root.as_ref() else {
                return Some("git -Cのrepositoryを確認できません".into());
            };
            let Some(session_root) = session_root.as_ref() else {
                return Some("git -Cのrepositoryを確認できません".into());
            };
            if target_root == common_root
                || (session_root != common_root && target_root != session_root)
            {
                return Some("git -Cのrepositoryをmanaged worktreeへ限定できません".into());
            }
            if let Some(reason) =
                managed_worktree_reason(common_root, session_common.as_ref()?, target_root)
            {
                return Some(reason);
            }
        }
        if command == "branch" {
            if command_args.iter().any(|arg| {
                BRANCH_WRITE_ARGS.contains(&arg.as_str())
                    || arg.starts_with("--delete=")
                    || (arg.len() > 2
                        && ["-d", "-D", "-m", "-M", "-c", "-C"]
                            .iter()
                            .any(|x| arg.starts_with(x)))
            }) {
                return Some("git branchの変更操作は許可されていません".into());
            }
            if command_args
                .iter()
                .any(|arg| arg == "--list" || arg == "-l")
            {
                return None;
            }
            let (branch, _) = first_command(&command_args, BRANCH_VALUE_OPTIONS);
            return if branch.is_none() {
                None
            } else {
                Some("git branchは一覧・照会だけ許可されます".into())
            };
        }
        if command == "remote" {
            return if command_args
                .iter()
                .all(|arg| arg == "-v" || arg == "--verbose")
            {
                None
            } else {
                Some("git remoteは照会だけ許可されます".into())
            };
        }
        if command == "worktree" {
            return git_worktree_read_reason(&command_args);
        }
        if command == "ls-remote" {
            return git_ls_remote_read_reason(&command_args);
        }
        if GIT_READ_ONLY.contains(&command) {
            return git_read_reason(
                command,
                &command_args,
                if explicit.is_some() { false } else { global },
            );
        }
        if !GIT_SAFE_WRITE.contains(&command) {
            return Some("許可されていないGit書き込み操作です".into());
        }
        if (command == "pull" || command == "switch")
            && std::env::var("CODEX_WORKTREE_MODE").ok().as_deref() != Some("single-checkout")
        {
            return Some(
                "git pull/switchは明示的なsingle-checkout rollback時だけ使用できます".into(),
            );
        }
        if other_global {
            return Some("Git書き込みではglobal optionや別repository指定を使用できません".into());
        }
        let effective = match git_write_target(cwd, explicit.as_deref()) {
            Ok(v) => v,
            Err(e) => return Some(e),
        };
        if !removed_ssh && std::env::var_os("SSH_ASKPASS").is_some() {
            return Some(
                "Git書き込みではrepository、config、外部commandを変更する環境変数を使用できません"
                    .into(),
            );
        }
        let reason = match command {
            "add" => git_add_reason(&command_args),
            "commit" => git_commit_reason(&command_args),
            "fetch" => git_fetch_reason(&command_args),
            "push" => git_push_reason(&command_args, &effective),
            "pull" => git_pull_reason(&command_args, &effective),
            "switch" => git_switch_reason(&command_args, &effective),
            _ => Some("許可されていないGit書き込み操作です".into()),
        };
        if reason.is_some() {
            return reason;
        }
        if (command == "add" || command == "commit")
            && let Some(reason) = current_work_branch_reason(&effective)
        {
            return Some(reason);
        }
        if command == "switch"
            && let Some(reason) = clean_worktree_reason(&effective, "switch")
        {
            return Some(reason);
        }
        if command == "commit" {
            return staged_secret_reason(&effective);
        }
        return None;
    }
    if explicit.is_some() {
        Some("git -Cでは検証可能なread-only subcommandを明示してください".into())
    } else {
        None
    }
}

fn git_pull_reason(args: &[String], cwd: &str) -> Option<String> {
    let expected = [
        "--ff-only",
        "--no-rebase",
        "--no-autostash",
        "--no-recurse-submodules",
        "origin",
    ];
    if args.len() != 6
        || args[..5] != expected
        || !protected_branch(
            args.get(5)?
                .strip_prefix("refs/heads/")
                .unwrap_or(args.get(5)?),
        )
    {
        return Some("git pullはfast-forward限定の正規形だけを使用してください".into());
    }
    let base = args[5].strip_prefix("refs/heads/").unwrap_or(&args[5]);
    let origin = run_git(cwd, &["remote", "get-url", "origin"])
        .and_then(|v| github_repository_from_url(v.trim()));
    if origin.is_none() {
        return Some("originのGitHub repositoryを確認できません".into());
    }
    let default_ref = run_git(
        cwd,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    );
    if default_ref.as_deref().map(str::trim) != Some(&format!("origin/{base}")) {
        return Some("git pullはoriginの既定保護branchだけを同期できます".into());
    }
    if run_git(cwd, &["rev-parse", "--abbrev-ref", "HEAD"])
        .as_deref()
        .map(str::trim)
        != Some(base)
        || run_git(
            cwd,
            &[
                "rev-parse",
                "--abbrev-ref",
                "--symbolic-full-name",
                "@{upstream}",
            ],
        )
        .as_deref()
        .map(str::trim)
            != Some(&format!("origin/{base}"))
    {
        return Some("git pullのcurrent branchと同期対象が一致しません".into());
    }
    if let Some(reason) = clean_worktree_reason(cwd, "pull") {
        return Some(reason);
    }
    if run_git(
        cwd,
        &[
            "merge-base",
            "--is-ancestor",
            "HEAD",
            &format!("origin/{base}"),
        ],
    )
    .is_none()
    {
        return Some(
            "local branchがoriginよりaheadまたはdivergedしているためgit pullを拒否しました".into(),
        );
    }
    let Some(git_dir) = run_git(cwd, &["rev-parse", "--git-dir"]) else {
        return Some("Gitの進行中操作を確認できないためgit pullを拒否しました".into());
    };
    let mut git_dir = PathBuf::from(git_dir.trim());
    if !git_dir.is_absolute() {
        git_dir = Path::new(cwd).join(git_dir);
    }
    git_operation_in_progress_reason(&git_dir)
}

fn git_operation_in_progress_reason(git_dir: &Path) -> Option<String> {
    for name in [
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "rebase-apply",
        "rebase-merge",
        "sequencer",
    ] {
        match fs::symlink_metadata(git_dir.join(name)) {
            Ok(_) => return Some("Gitの進行中操作があるためgit pullを拒否しました".into()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => {
                return Some("Gitの進行中操作を確認できないためgit pullを拒否しました".into());
            }
        }
    }
    None
}

fn git_switch_reason(args: &[String], cwd: &str) -> Option<String> {
    if args.len() == 1 && PROTECTED_BRANCHES.contains(&args[0].as_str()) {
        let default_ref = run_git(
            cwd,
            &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
        );
        if default_ref.as_deref().map(str::trim) != Some(&format!("origin/{}", args[0])) {
            return Some("git switchの対象はoriginの既定保護branchと一致させてください".into());
        }
        return if run_git(
            cwd,
            &["rev-parse", "--verify", &format!("refs/heads/{}", args[0])],
        )
        .is_some()
        {
            None
        } else {
            Some("git switchの対象local branchを確認できません".into())
        };
    }
    if args.len() != 3 || !["-c", "--create"].contains(&args[0].as_str()) {
        return Some(
            "git switchは既定保護branchへの切り替えか新規作業branch作成だけ許可されます".into(),
        );
    }
    if !valid_work_branch(&args[1]) {
        return Some("一般的なprefixを持つ非保護作業ブランチを指定してください".into());
    }
    if !args[2].starts_with("origin/") || !protected_branch(&args[2][7..]) {
        return Some("作業ブランチはoriginのbase branchから作成してください".into());
    }
    None
}

fn git_push_reason(args: &[String], cwd: &str) -> Option<String> {
    if args.iter().any(|arg| {
        [
            "--force",
            "-f",
            "--force-with-lease",
            "--delete",
            "-d",
            "--mirror",
            "--tags",
            "--all",
        ]
        .contains(&arg.as_str())
            || arg.starts_with("--force-with-lease=")
    }) {
        return Some("force、削除、mirror、tagまたは一括pushは許可されていません".into());
    }
    if args.len() != 3
        || !["-u", "--set-upstream"].contains(&args[0].as_str())
        || args[1] != "origin"
    {
        return Some("pushはoriginへの単一作業ブランチを明示してください".into());
    }
    let Some(branch) = args[2].strip_prefix("HEAD:refs/heads/") else {
        return Some("push元はHEAD、push先はrefs/heads/<branch>で明示してください".into());
    };
    if !valid_work_branch(branch) {
        return Some("保護ブランチまたは許可されていないprefixへのpushです".into());
    }
    push_preflight_reason(cwd, branch)
}

fn changes_secret_reason(names: &str, patch: &str, label: &str) -> Option<String> {
    if names.lines().any(sensitive_path) {
        return Some(format!(
            "{label}に秘密情報を保持し得るファイルが含まれています"
        ));
    }
    let added = patch
        .lines()
        .filter(|line| line.starts_with('+') && !line.starts_with("+++"))
        .map(|line| &line[1..])
        .collect::<Vec<_>>()
        .join("\n");
    if contains_secret(&added) {
        return Some(format!("{label}に秘密情報らしい値が含まれています"));
    }
    if ai_attribution(&added) {
        return Some(format!(
            "{label}にCodexまたはOpenAIのAI帰属が含まれています"
        ));
    }
    None
}

fn staged_secret_reason(cwd: &str) -> Option<String> {
    let Some(names) = run_git(
        cwd,
        &["diff", "--cached", "--name-only", "--diff-filter=ACMR"],
    ) else {
        return Some("staged changesを検査できないためcommitを拒否しました".into());
    };
    let Some(patch) = run_git(
        cwd,
        &[
            "diff",
            "--cached",
            "--no-ext-diff",
            "--no-textconv",
            "--unified=0",
            "--",
        ],
    ) else {
        return Some("staged changesを検査できないためcommitを拒否しました".into());
    };
    changes_secret_reason(&names, &patch, "staged changes")
}

fn push_preflight_reason(cwd: &str, branch: &str) -> Option<String> {
    let Some(fetch) = run_git(cwd, &["remote", "get-url", "--all", "origin"]) else {
        return Some("originのfetch/push先を確認できないためpushを拒否しました".into());
    };
    let Some(push) = run_git(cwd, &["remote", "get-url", "--push", "--all", "origin"]) else {
        return Some("originのfetch/push先を確認できないためpushを拒否しました".into());
    };
    let fetch_lines: Vec<_> = fetch.lines().collect();
    let push_lines: Vec<_> = push.lines().collect();
    if fetch_lines.len() != 1
        || push_lines.len() != 1
        || github_repository_from_url(fetch_lines[0]).is_none()
        || github_repository_from_url(push_lines[0]) != github_repository_from_url(fetch_lines[0])
    {
        return Some("originのpush先がcurrent GitHub repositoryと一致しません".into());
    }
    if run_git(cwd, &["rev-parse", "--abbrev-ref", "HEAD"])
        .as_deref()
        .map(str::trim)
        != Some(branch)
    {
        return Some("current branchとpush先branchが一致しません".into());
    }
    if let Some(reason) = clean_worktree_reason(cwd, "push") {
        return Some(reason);
    }
    let base = if run_git(cwd, &["rev-parse", "--verify", &format!("origin/{branch}")]).is_some() {
        format!("origin/{branch}")
    } else {
        let Some(default_ref) = run_git(
            cwd,
            &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
        ) else {
            return Some("未push commitのbaseを特定できません".into());
        };
        let Some(default_branch) = default_ref.trim().strip_prefix("origin/") else {
            return Some("未push commitのbaseを特定できません".into());
        };
        if !protected_branch(default_branch) {
            return Some("originのdefault branchが保護対象ではありません".into());
        }
        format!("origin/{default_branch}")
    };
    let Some(names) = run_git(
        cwd,
        &[
            "diff",
            "--name-only",
            "--diff-filter=ACMR",
            &format!("{base}..HEAD"),
        ],
    ) else {
        return Some("未push commitを検査できません".into());
    };
    let Some(patch) = run_git(
        cwd,
        &[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--unified=0",
            &format!("{base}..HEAD"),
            "--",
        ],
    ) else {
        return Some("未push commitを検査できません".into());
    };
    changes_secret_reason(&names, &patch, "未push commit")
}

fn contains_secret(value: &str) -> bool {
    let ascii_word = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_';
    let extended_word = |byte: u8| ascii_word(byte) || byte == b'-';
    has_prefixed_token(value, concat!("github", "_pat_"), 20, ascii_word)
        || [
            concat!("gh", "p_"),
            concat!("gh", "o_"),
            concat!("gh", "u_"),
            concat!("gh", "s_"),
            concat!("gh", "r_"),
        ]
        .iter()
        .any(|prefix| has_prefixed_token(value, prefix, 20, ascii_word))
        || has_prefixed_token(value, concat!("sk", "-"), 20, extended_word)
        || has_prefixed_token(value, concat!("AK", "IA"), 16, |byte| {
            byte.is_ascii_digit() || byte.is_ascii_uppercase()
        })
        || ["xoxb-", "xoxa-", "xoxp-", "xoxr-", "xoxs-"]
            .iter()
            .any(|prefix| {
                has_prefixed_token(value, prefix, 10, |byte| {
                    byte.is_ascii_alphanumeric() || byte == b'-'
                })
            })
        || has_prefixed_token(value, concat!("AI", "za"), 30, extended_word)
        || value.contains(concat!("-----BEGIN ", "PRIVATE KEY-----"))
        || value.contains(concat!("-----BEGIN RSA ", "PRIVATE KEY-----"))
        || value.contains(concat!("-----BEGIN OPENSSH ", "PRIVATE KEY-----"))
        || value.contains(concat!("-----BEGIN EC ", "PRIVATE KEY-----"))
}

fn has_prefixed_token(
    value: &str,
    prefix: &str,
    minimum_suffix: usize,
    allowed: impl Fn(u8) -> bool,
) -> bool {
    value.match_indices(prefix).any(|(index, _)| {
        value.as_bytes()[index + prefix.len()..]
            .iter()
            .copied()
            .take_while(|byte| allowed(*byte))
            .count()
            >= minimum_suffix
    })
}

fn ai_attribution(value: &str) -> bool {
    value.lines().any(|line| {
        let lower = line.to_ascii_lowercase();
        [
            "co-authored-by",
            "generated by",
            "generated-by",
            "signed-off-by",
        ]
        .iter()
        .any(|prefix| {
            lower.match_indices(prefix).any(|(start, _)| {
                let remainder = lower[start + prefix.len()..].trim_start();
                let Some(attribution) = remainder.strip_prefix(':') else {
                    return false;
                };
                contains_ai_identity(attribution.trim_start())
            })
        })
    })
}

fn contains_ai_identity(value: &str) -> bool {
    if ["codex", "openai", "chatgpt", "claude", "gemini", "copilot"]
        .iter()
        .any(|identity| value.contains(identity))
    {
        return true;
    }
    value.match_indices("ai").any(|(start, _)| {
        let prefix_boundary = value[..start]
            .chars()
            .next_back()
            .is_none_or(|character| !character.is_alphanumeric() && character != '_');
        let end = start + 2;
        let suffix_boundary = value[end..]
            .chars()
            .next()
            .is_none_or(|character| !character.is_alphanumeric() && character != '_');
        prefix_boundary && suffix_boundary
    })
}

fn sensitive_path(path: &str) -> bool {
    let name = basename(path).to_ascii_lowercase();
    SENSITIVE_NAMES.contains(&name.as_str())
        || SENSITIVE_SUFFIXES
            .iter()
            .any(|suffix| name.ends_with(suffix))
        || (name.starts_with(".env.")
            && ![".env.example", ".env.sample", ".env.template"].contains(&name.as_str()))
}

fn gh_api_is_write(args: &[String]) -> bool {
    let flags = ["-f", "--raw-field", "-F", "--field", "--input", "--cache"];
    let mut i = 0;
    while i < args.len() {
        let token = &args[i];
        if flags
            .iter()
            .any(|flag| token == *flag || starts_with_option(token, flag))
            || token.starts_with("-f") && token != "-f"
            || token.starts_with("-F") && token != "-F"
        {
            return true;
        }
        if token == "-X" || token == "--method" {
            if args.get(i + 1).map(|v| v.to_ascii_uppercase()) != Some("GET".into()) {
                return true;
            }
            i += 2;
            continue;
        }
        if token.starts_with("--method=")
            && token.split_once('=').map(|(_, v)| v.to_ascii_uppercase()) != Some("GET".into())
        {
            return true;
        }
        if token.starts_with("-X") && token != "-X" && !token[2..].eq_ignore_ascii_case("GET") {
            return true;
        }
        i += 1;
    }
    false
}

fn gh_api_endpoint(args: &[String]) -> Option<String> {
    let options = [
        "-X",
        "--method",
        "-H",
        "--header",
        "-f",
        "--raw-field",
        "-F",
        "--field",
        "--input",
        "-q",
        "--jq",
        "--cache",
        "--hostname",
        "-p",
        "--preview",
        "-t",
        "--template",
    ];
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--" {
            return args.get(i + 1).cloned();
        }
        if options.contains(&args[i].as_str()) {
            i += 2;
            continue;
        }
        if options
            .iter()
            .any(|o| starts_with_option(&args[i], o) && o.starts_with("--"))
            || args[i].starts_with('-')
        {
            i += 1;
            continue;
        }
        return Some(args[i].clone());
    }
    None
}

fn graphql_endpoint(endpoint: Option<&str>) -> bool {
    let Some(endpoint) = endpoint else {
        return false;
    };
    let mut value = endpoint.to_string();
    for _ in 0..8 {
        let mut decoded = String::new();
        let bytes = value.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' {
                if i + 2 >= bytes.len()
                    || !bytes[i + 1].is_ascii_hexdigit()
                    || !bytes[i + 2].is_ascii_hexdigit()
                {
                    return true;
                }
                let high = (bytes[i + 1] as char).to_digit(16).unwrap_or(0);
                let low = (bytes[i + 2] as char).to_digit(16).unwrap_or(0);
                decoded.push((high * 16 + low) as u8 as char);
                i += 3;
            } else {
                decoded.push(bytes[i] as char);
                i += 1;
            }
        }
        if decoded == value {
            break;
        }
        value = decoded;
    }
    let path = value.split(['#', '?']).next().unwrap_or("");
    let mut part = path
        .split('/')
        .filter(|v| !v.is_empty() && *v != ".")
        .collect::<Vec<_>>();
    while part.contains(&"..") {
        let mut next = Vec::new();
        for item in part {
            if item == ".." {
                let _ = next.pop();
            } else {
                next.push(item);
            }
        }
        part = next;
    }
    part.len() == 1 && part[0].eq_ignore_ascii_case("graphql")
}

fn required_option(args: &[String], short: &str, long: &str) -> Option<String> {
    let values = option_values(args, short, long)?;
    if values.len() == 1 {
        values.first().cloned()
    } else {
        None
    }
}

fn strict_gh_args(
    args: &[String],
    value_options: &[(&str, &str)],
    switches: &[&str],
) -> Option<Vec<String>> {
    let mut positional = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let token = &args[i];
        if switches.contains(&token.as_str()) {
            i += 1;
            continue;
        }
        if value_options
            .iter()
            .any(|(short, long)| token == short || token == long)
        {
            if i + 1 >= args.len() {
                return None;
            }
            i += 2;
            continue;
        }
        if value_options
            .iter()
            .any(|(_, long)| starts_with_option(token, long))
        {
            i += 1;
            continue;
        }
        if token.starts_with('-') {
            return None;
        }
        positional.push(token.clone());
        i += 1;
    }
    Some(positional)
}

fn github_target_reason(positional: &[String], label: &str) -> Option<String> {
    if positional.len() == 1
        && positional[0].bytes().all(|c| c.is_ascii_digit())
        && positional[0].as_bytes().first().is_some_and(|c| *c != b'0')
    {
        None
    } else {
        Some(format!("{label}対象は単一の数値IDで指定してください"))
    }
}

fn github_text_reason(values: &[String], label: &str) -> Option<String> {
    for value in values {
        if contains_secret(value) {
            return Some(format!("{label}に秘密情報らしい値を含めることはできません"));
        }
        if ai_attribution(value) {
            return Some(format!(
                "{label}にCodexまたはOpenAIのAI帰属を含めることはできません"
            ));
        }
        if contains_copilot_mention(value) {
            return Some(format!("{label}に@copilot mentionを含めることはできません"));
        }
    }
    None
}

fn body_content_reason(contents: &str, label: &str) -> Option<String> {
    if contains_secret(contents) {
        return Some(format!("{label}に秘密情報らしい値が含まれています"));
    }
    if ai_attribution(contents) {
        return Some(format!(
            "{label}にCodexまたはOpenAIのAI帰属が含まれています"
        ));
    }
    if contains_copilot_mention(contents) {
        return Some(format!("{label}に@copilot mentionを含めることはできません"));
    }
    None
}

#[cfg(target_os = "linux")]
fn opened_fd_path(file: &std::fs::File) -> Option<PathBuf> {
    let link = PathBuf::from("/proc/self/fd").join(file.as_raw_fd().to_string());
    let path = fs::read_link(link).ok()?;
    if path.to_string_lossy().ends_with(" (deleted)") {
        return None;
    }
    fs::canonicalize(path).ok()
}

#[cfg(all(unix, not(target_os = "linux")))]
fn opened_fd_path(_file: &std::fs::File) -> Option<PathBuf> {
    None
}

#[cfg(unix)]
fn safe_body_file_contents(path: &str, label: &str) -> Result<String, String> {
    if path == "-" {
        return Err(format!("{label}は検査可能なファイルで指定してください"));
    }
    let candidate = Path::new(path);
    let metadata = fs::symlink_metadata(candidate)
        .map_err(|_| format!("{label} fileを安全に検査できません"))?;
    if metadata.file_type().is_symlink() {
        return Err(format!("{label} fileにsymlinkは使用できません"));
    }
    let resolved =
        fs::canonicalize(candidate).map_err(|_| format!("{label} fileを安全に検査できません"))?;
    let tmp =
        fs::canonicalize("/tmp").map_err(|_| format!("{label} fileを安全に検査できません"))?;
    let resolved_name = resolved
        .to_str()
        .ok_or_else(|| format!("{label} fileを安全に検査できません"))?;
    if !resolved.starts_with(&tmp) || sensitive_path(resolved_name) {
        return Err(format!("{label} fileを安全に検査できません"));
    }

    let mut options = fs::OpenOptions::new();
    options.read(true).custom_flags(libc::O_NOFOLLOW);
    let file = options
        .open(&resolved)
        .map_err(|_| format!("{label} fileを安全に検査できません"))?;
    let opened =
        opened_fd_path(&file).ok_or_else(|| format!("{label} fileを安全に検査できません"))?;
    if opened != resolved || !opened.starts_with(&tmp) {
        return Err(format!("{label} fileを安全に検査できません"));
    }
    let opened_metadata = file
        .metadata()
        .map_err(|_| format!("{label} fileを安全に検査できません"))?;
    if !opened_metadata.is_file()
        || !uid_is_current(&opened_metadata)
        || opened_metadata.len() > MAX_BODY_FILE_BYTES
    {
        return Err(format!("{label} fileを安全に検査できません"));
    }
    let mut bytes = Vec::with_capacity(opened_metadata.len().min(64 * 1024) as usize);
    file.take(MAX_BODY_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| format!("{label} fileを安全に検査できません"))?;
    if bytes.len() as u64 > MAX_BODY_FILE_BYTES {
        return Err(format!("{label} fileを安全に検査できません"));
    }
    String::from_utf8(bytes).map_err(|_| format!("{label} fileを安全に検査できません"))
}

#[cfg(not(unix))]
fn safe_body_file_contents(_path: &str, label: &str) -> Result<String, String> {
    Err(format!("{label} fileを安全に検査できません"))
}

fn file_secret_reason(path: &str, label: &str) -> Option<String> {
    let contents = match safe_body_file_contents(path, label) {
        Ok(contents) => contents,
        Err(reason) => return Some(reason),
    };
    body_content_reason(&contents, label)
}

#[cfg(unix)]
fn body_snapshot_directory() -> Result<PathBuf, String> {
    let root = fs::canonicalize("/tmp")
        .map_err(|_| "body-file snapshot directoryを安全に作成できません".to_string())?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    cleanup_body_snapshots(&root, timestamp)?;
    for _ in 0..32 {
        let counter = BODY_SNAPSHOT_COUNTER.fetch_add(1, Ordering::Relaxed);
        let directory = root.join(format!(
            "{BODY_SNAPSHOT_DIR_PREFIX}{}-{timestamp}-{counter}",
            std::process::id()
        ));
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        match builder.create(&directory) {
            Ok(()) => {
                sync_body_snapshot_directory(&root)?;
                return Ok(directory);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(_) => {
                return Err("body-file snapshot directoryを安全に作成できません".into());
            }
        }
    }
    Err("body-file snapshot directoryを一意に作成できません".into())
}

#[cfg(unix)]
fn sync_body_snapshot_directory(path: &Path) -> Result<(), String> {
    let mut options = fs::OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
    options
        .open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| "body-file snapshot directoryをdurableに保存できません".to_string())
}

#[cfg(unix)]
fn snapshot_timestamp(name: &str) -> Option<u128> {
    let suffix = name.strip_prefix(BODY_SNAPSHOT_DIR_PREFIX)?;
    let mut parts = suffix.split('-');
    let pid = parts.next()?;
    let timestamp = parts.next()?;
    let counter = parts.next()?;
    if parts.next().is_some()
        || pid.is_empty()
        || counter.is_empty()
        || !pid.bytes().all(|byte| byte.is_ascii_digit())
        || !counter.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    timestamp.parse().ok()
}

#[cfg(unix)]
fn remove_expired_body_snapshot(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;

    let directory = fs::symlink_metadata(path)
        .map_err(|_| "expired body-file snapshotを検査できません".to_string())?;
    if !directory.is_dir()
        || directory.file_type().is_symlink()
        || directory.uid() != unsafe { libc::getuid() }
        || directory.mode() & 0o077 != 0
    {
        return Err("expired body-file snapshotを安全に検査できません".into());
    }
    let mut entries =
        fs::read_dir(path).map_err(|_| "expired body-file snapshotを検査できません".to_string())?;
    let entry = entries
        .next()
        .transpose()
        .map_err(|_| "expired body-file snapshotを検査できません".to_string())?
        .ok_or_else(|| "expired body-file snapshotが空です".to_string())?;
    if entry.file_name() != "body" || entries.next().is_some() {
        return Err("expired body-file snapshotの構造が不正です".into());
    }
    let body = entry.path();
    let metadata = fs::symlink_metadata(&body)
        .map_err(|_| "expired body-file snapshotを検査できません".to_string())?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.uid() != unsafe { libc::getuid() }
        || metadata.nlink() != 1
        || metadata.mode() & 0o777 != 0o400
        || metadata.len() > MAX_BODY_FILE_BYTES
    {
        return Err("expired body-file snapshotを安全に検査できません".into());
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|_| "expired body-file snapshotを安全に削除できません".to_string())?;
    let result = fs::remove_file(&body).and_then(|()| fs::remove_dir(path));
    if result.is_err() {
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o500));
    }
    result.map_err(|_| "expired body-file snapshotを安全に削除できません".to_string())
}

#[cfg(unix)]
fn cleanup_body_snapshots(root: &Path, now_nanos: u128) -> Result<(), String> {
    let ttl_nanos = BODY_SNAPSHOT_TTL.as_nanos();
    let mut retained = 0usize;
    let mut removed = false;
    let entries = fs::read_dir(root)
        .map_err(|_| "body-file snapshot directoryを検査できません".to_string())?;
    for entry in entries {
        let entry =
            entry.map_err(|_| "body-file snapshot directoryを検査できません".to_string())?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some(timestamp) = snapshot_timestamp(&name) else {
            continue;
        };
        if now_nanos.saturating_sub(timestamp) >= ttl_nanos
            && remove_expired_body_snapshot(&entry.path()).is_ok()
        {
            removed = true;
            continue;
        }
        retained = retained.saturating_add(1);
    }
    if removed {
        sync_body_snapshot_directory(root)?;
    }
    if retained >= BODY_SNAPSHOT_MAX_RETAINED {
        return Err("body-file snapshotの保持上限に達しました".into());
    }
    Ok(())
}

#[cfg(unix)]
fn snapshot_body_file(path: &str, label: &str) -> Result<PathBuf, String> {
    let contents = safe_body_file_contents(path, label)?;
    if let Some(reason) = body_content_reason(&contents, label) {
        return Err(reason);
    }
    let directory = body_snapshot_directory()?;
    let snapshot = directory.join("body");
    let mut options = fs::OpenOptions::new();
    options
        .write(true)
        .create_new(true)
        .custom_flags(libc::O_NOFOLLOW)
        .mode(0o600);
    let mut output = options
        .open(&snapshot)
        .map_err(|_| "body-file snapshotを安全に作成できません".to_string())?;
    output
        .write_all(contents.as_bytes())
        .and_then(|_| output.sync_all())
        .map_err(|_| "body-file snapshotを安全に作成できません".to_string())?;
    let metadata = output
        .metadata()
        .map_err(|_| "body-file snapshotを安全に検証できません".to_string())?;
    if !metadata.is_file() || !uid_is_current(&metadata) || metadata.len() != contents.len() as u64
    {
        return Err("body-file snapshotを安全に検証できません".into());
    }
    fs::set_permissions(&snapshot, fs::Permissions::from_mode(0o400))
        .map_err(|_| "body-file snapshotをimmutableにできません".to_string())?;
    sync_body_snapshot_directory(&directory)?;
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o500))
        .map_err(|_| "body-file snapshot directoryを固定できません".to_string())?;
    if let Some(root) = directory.parent() {
        sync_body_snapshot_directory(root)?;
    }
    Ok(snapshot)
}

#[cfg(not(unix))]
fn snapshot_body_file(_path: &str, label: &str) -> Result<PathBuf, String> {
    Err(format!("{label} fileを安全にsnapshot化できません"))
}

fn shell_quote_token(token: &str) -> String {
    format!("'{}'", token.replace('\'', "'\\''"))
}

fn rewrite_body_file_command(command: &str, cwd: Option<&str>) -> Result<Option<String>, String> {
    let segments = command_segments(command);
    if segments.len() != 1 {
        return Ok(None);
    }
    let mut tokens = segments.into_iter().next().unwrap_or_default();
    if tokens.first().map(String::as_str) != Some("gh") {
        return Ok(None);
    }
    let Some((top_level, top_args)) = ({
        let (command, args) = first_command(&tokens[1..], GH_GLOBAL_VALUE_OPTIONS);
        command.map(|command| (command, args))
    }) else {
        return Ok(None);
    };
    let Some((subcommand, _)) = ({
        let (command, args) = first_command(&top_args, GH_GLOBAL_VALUE_OPTIONS);
        command.map(|command| (command, args))
    }) else {
        return Ok(None);
    };
    let body_label = match (top_level.as_str(), subcommand.as_str()) {
        ("issue", "create" | "edit" | "comment") => "Issue body",
        ("pr", "create" | "edit" | "comment" | "review") => "PR body",
        _ => return Ok(None),
    };

    // Keep the option spelling and replace only its path value.  The command
    // has already passed the strict lifecycle parser, so a malformed option
    // here is treated as a deny rather than being silently ignored.
    let mut locations: Vec<(usize, Option<usize>, String)> = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        let token = &tokens[index];
        if token == "-F" || token == "--body-file" {
            let Some(value_index) = index.checked_add(1).filter(|value| *value < tokens.len())
            else {
                return Err(format!("{body_label} file optionの値を確認できません"));
            };
            locations.push((index, Some(value_index), String::new()));
            index += 2;
            continue;
        }
        if let Some(value) = token.strip_prefix("--body-file=") {
            locations.push((index, None, value.to_string()));
        } else if token.starts_with("-F") && token.len() > 2 {
            locations.push((index, None, token[2..].to_string()));
        }
        index += 1;
    }
    if locations.is_empty() {
        return Ok(None);
    }

    for (option_index, value_index, attached_value) in locations {
        let path = value_index
            .map(|value_index| tokens[value_index].clone())
            .unwrap_or(attached_value);
        let snapshot = snapshot_body_file(&path, body_label)?;
        let snapshot = snapshot
            .to_str()
            .ok_or_else(|| format!("{body_label} snapshot pathを確認できません"))?;
        if let Some(value_index) = value_index {
            tokens[value_index] = snapshot.to_string();
        } else if tokens[option_index].starts_with("--body-file=") {
            tokens[option_index] = format!("--body-file={snapshot}");
        } else {
            tokens[option_index] = format!("-F{snapshot}");
        }
    }
    let rewritten = tokens
        .iter()
        .map(|token| shell_quote_token(token))
        .collect::<Vec<_>>()
        .join(" ");
    if let Some(reason) = blocked_reason(&rewritten, cwd, 0) {
        return Err(format!(
            "body-file snapshot後のcommandを再検証できません: {reason}"
        ));
    }
    Ok(Some(rewritten))
}

fn contains_copilot_mention(value: &str) -> bool {
    const NEEDLE: &[u8] = b"@copilot";
    value
        .as_bytes()
        .windows(NEEDLE.len())
        .enumerate()
        .any(|(start, candidate)| {
            if !candidate.eq_ignore_ascii_case(NEEDLE) {
                return false;
            }
            let valid_prefix = start == 0
                || !value.as_bytes()[start - 1].is_ascii_alphanumeric()
                    && !b"_@".contains(&value.as_bytes()[start - 1]);
            let end = start + NEEDLE.len();
            let valid_suffix = value
                .get(end..)
                .and_then(|suffix| suffix.chars().next())
                .is_none_or(|character| !character.is_alphanumeric() && character != '_');
            valid_prefix && valid_suffix
        })
}

fn gh_json(cwd: &str, args: &[&str]) -> Option<serde_json::Map<String, StrictJsonValue>> {
    let (status, output) = run_gh(cwd, args)?;
    if status != 0 {
        return None;
    }
    match serde_json::from_slice::<StrictJsonValue>(&output).ok()? {
        StrictJsonValue::Object(value) => Some(value),
        _ => None,
    }
}

fn json_string_value(value: Option<&StrictJsonValue>) -> Option<&str> {
    match value {
        Some(StrictJsonValue::String(value)) => Some(value),
        _ => None,
    }
}

fn json_u64_value(value: Option<&StrictJsonValue>) -> Option<u64> {
    value.and_then(StrictJsonValue::as_u64)
}

fn json_bool_value(value: Option<&StrictJsonValue>) -> Option<bool> {
    value.and_then(StrictJsonValue::as_bool)
}

fn gh_run_cancel_reason(args: &[String], cwd: &str) -> Option<String> {
    if args.len() != 4
        || args.first().map(String::as_str) != Some("cancel")
        || args[1].is_empty()
        || !args[1].bytes().all(|c| c.is_ascii_digit())
        || args[1].starts_with('0')
        || args[2] != "--repo"
        || !args[3].contains('/')
    {
        return Some("gh run cancelはrun-idとrepoを指定する正規形だけ許可されます".into());
    }
    if std::env::var_os("GH_REPO").is_some() {
        return Some("gh run cancelではGH_REPO環境変数を使用できません".into());
    }
    if [
        "GH_CONFIG_DIR",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "no_proxy",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
    ]
    .iter()
    .any(|key| std::env::var_os(key).is_some())
    {
        return Some("gh run cancelではGitHub transportを変更する環境変数を使用できません".into());
    }
    match run_gh(
        cwd,
        &["config", "get", "http_unix_socket", "--host", "github.com"],
    ) {
        Some((0, output)) if output.is_empty() => {}
        _ => return Some("gh run cancelのGitHub transport設定を確認できません".into()),
    }
    let repository = &args[3];
    if let Some(reason) = repository_reason(repository, cwd) {
        return Some(reason);
    }
    let endpoint = format!("repos/{}/actions/runs/{}", repository, args[1]);
    let payload = gh_json(cwd, &["api", "--method", "GET", &endpoint]);
    let Some(payload) = payload else {
        return Some("gh run cancel対象のremote状態を確認できません".into());
    };
    let remote_repository = match payload.get("repository") {
        Some(StrictJsonValue::Object(value)) => json_string_value(value.get("full_name")),
        _ => None,
    };
    let expected_id = args[1].parse::<u64>().ok();
    if expected_id.is_none() || json_u64_value(payload.get("id")) != expected_id {
        return Some("gh run cancel対象のidがrun-idと一致しません".into());
    }
    if remote_repository.is_none_or(|value| !value.eq_ignore_ascii_case(repository)) {
        return Some("gh run cancel対象のrepository identityが一致しません".into());
    }
    if json_string_value(payload.get("status"))
        .is_none_or(|status| !["queued", "in_progress"].contains(&status))
        || !matches!(payload.get("conclusion"), Some(StrictJsonValue::Null))
    {
        return Some("gh run cancel対象は未完了runだけ許可されます".into());
    }
    let expected = format!(
        "https://api.github.com/repos/{}/actions/runs/{}/cancel",
        repository, args[1]
    );
    if json_string_value(payload.get("cancel_url"))
        .is_none_or(|value| !value.eq_ignore_ascii_case(&expected))
    {
        return Some("gh run cancel対象のcancel_urlがrepositoryまたはrun-idと一致しません".into());
    }
    None
}

fn repository_reason(repository: &str, cwd: &str) -> Option<String> {
    if !repository.contains('/')
        || origin_repository(cwd).is_none_or(|origin| !origin.eq_ignore_ascii_case(repository))
    {
        Some("GitHub書き込み先がcurrent repositoryのoriginと一致しません".into())
    } else {
        None
    }
}

fn issue_write_reason(command: &str, args: &[String], cwd: &str) -> Option<String> {
    if command == "comment"
        && args.iter().any(|arg| {
            arg == "-b" || arg == "--body" || arg.starts_with("--body=") || arg.starts_with("-b")
        })
    {
        return Some("Issue commentでは--body-fileが必須です（--bodyは使用できません）".into());
    }
    let repository = match required_option(args, "-R", "--repo") {
        Some(value) => value,
        None => return Some("Issue書き込みには対象repositoryを明示してください".into()),
    };
    if let Some(reason) = repository_reason(&repository, cwd) {
        return Some(reason);
    }
    let common = [("-R", "--repo")];
    match command {
        "create" => {
            let mut values = common.to_vec();
            values.extend_from_slice(&[
                ("-F", "--body-file"),
                ("-t", "--title"),
                ("-l", "--label"),
                ("-a", "--assignee"),
                ("", "--milestone"),
            ]);
            let positional = match strict_gh_args(args, &values, &[]) {
                Some(value) => value,
                None => {
                    return Some(
                        "Issue作成では許可された引数だけを正規形で指定してください".into(),
                    );
                }
            };
            if !positional.is_empty() {
                return Some("Issue作成では許可された引数だけを正規形で指定してください".into());
            }
            let title = match required_option(args, "-t", "--title") {
                Some(value) => value,
                None => return Some("Issue作成にはtitleとbody-fileを明示してください".into()),
            };
            let body = match required_option(args, "-F", "--body-file") {
                Some(value) => value,
                None => return Some("Issue作成にはtitleとbody-fileを明示してください".into()),
            };
            let mut metadata_values = Vec::new();
            for (short, long) in [("-l", "--label"), ("-a", "--assignee"), ("", "--milestone")] {
                let Some(values) = option_values(args, short, long) else {
                    return Some("Issue作成のmetadata引数に値を明示してください".into());
                };
                metadata_values.extend(values);
            }
            let mut text_values = vec![title];
            text_values.extend(metadata_values);
            if let Some(reason) = github_text_reason(&text_values, "Issue作成値") {
                return Some(reason);
            }
            file_secret_reason(&body, "Issue body")
        }
        "edit" => {
            let mut values = common.to_vec();
            values.extend_from_slice(&[
                ("-F", "--body-file"),
                ("-t", "--title"),
                ("", "--add-label"),
                ("", "--remove-label"),
                ("", "--add-assignee"),
                ("", "--remove-assignee"),
                ("", "--milestone"),
            ]);
            let positional = match strict_gh_args(args, &values, &["--remove-milestone"]) {
                Some(value) => value,
                None => {
                    return Some(
                        "Issue編集では許可された引数だけを正規形で指定してください".into(),
                    );
                }
            };
            if let Some(reason) = github_target_reason(&positional, "Issue編集") {
                return Some(reason);
            }
            let body_files = option_values(args, "-F", "--body-file");
            let titles = option_values(args, "-t", "--title");
            if body_files.as_ref().is_none_or(|values| values.len() > 1)
                || titles.as_ref().is_none_or(|values| values.len() > 1)
            {
                return Some("Issue編集のtitleとbody-fileはそれぞれ1件までです".into());
            }
            let body_file = body_files.as_ref().and_then(|values| values.first());
            let title = titles.as_ref().and_then(|values| values.first());
            let mut mutation = title.is_some()
                || body_file.is_some()
                || args.iter().any(|value| value == "--remove-milestone");
            let mut metadata_values = Vec::new();
            for (short, long) in [
                ("", "--add-label"),
                ("", "--remove-label"),
                ("", "--add-assignee"),
                ("", "--remove-assignee"),
                ("", "--milestone"),
            ] {
                let Some(values) = option_values(args, short, long) else {
                    return Some("Issue編集のmetadata引数に値を明示してください".into());
                };
                mutation |= !values.is_empty();
                metadata_values.extend(values);
            }
            if !mutation {
                return Some(
                    "Issue編集にはtitle、body-file、label、assignee、milestoneの変更を明示してください"
                        .into(),
                );
            }
            let mut text_values = Vec::new();
            if let Some(title) = title {
                text_values.push(title.clone());
            }
            text_values.extend(metadata_values);
            if let Some(reason) = github_text_reason(&text_values, "Issue編集値") {
                return Some(reason);
            }
            if let Some(body_file) = body_file {
                file_secret_reason(body_file, "Issue body")
            } else {
                None
            }
        }
        "comment" => {
            let positional = match strict_gh_args(args, &[common[0], ("-F", "--body-file")], &[]) {
                Some(value) => value,
                None => {
                    return Some(
                        "Issue commentでは許可された引数だけを正規形で指定してください".into(),
                    );
                }
            };
            if let Some(reason) = github_target_reason(&positional, "Issue comment") {
                return Some(reason);
            }
            let body = match required_option(args, "-F", "--body-file") {
                Some(value) => value,
                None => return Some("Issue commentにはbody-fileを明示してください".into()),
            };
            file_secret_reason(&body, "Issue body")
        }
        "close" | "reopen" => {
            let values = if command == "close" {
                [common[0], ("", "--reason")].to_vec()
            } else {
                common.to_vec()
            };
            let positional = match strict_gh_args(args, &values, &[]) {
                Some(value) => value,
                None => {
                    return Some(
                        "Issue lifecycleでは許可された引数だけを正規形で指定してください".into(),
                    );
                }
            };
            if let Some(reason) = github_target_reason(&positional, &format!("Issue {command}")) {
                return Some(reason);
            }
            if command == "close" {
                let reasons = option_values(args, "", "--reason");
                if reasons.as_ref().is_none_or(|values| values.len() > 1) {
                    return Some("Issue closeのreasonは1件だけ指定してください".into());
                }
                if reasons.as_ref().is_some_and(|values| {
                    values.first().is_some_and(|value| {
                        !["completed", "not planned"].contains(&value.as_str())
                    })
                }) {
                    return Some("Issue closeのreasonが許可範囲外です".into());
                }
            }
            None
        }
        _ => Some("許可されていないGitHub Issue操作です".into()),
    }
}

fn pr_write_reason(command: &str, args: &[String], cwd: &str) -> Option<String> {
    let repository = match required_option(args, "-R", "--repo") {
        Some(value) => value,
        None => return Some("PR書き込みには対象repositoryを明示してください".into()),
    };
    if let Some(reason) = repository_reason(&repository, cwd) {
        return Some(reason);
    }
    let common = [("-R", "--repo")];
    match command {
        "create" => {
            if !args.contains(&"--draft".to_string()) {
                return Some("PRはDraftとして作成してください".into());
            }
            let mut values = common.to_vec();
            values.extend_from_slice(&[
                ("-B", "--base"),
                ("-H", "--head"),
                ("-t", "--title"),
                ("-F", "--body-file"),
            ]);
            let positional = match strict_gh_args(args, &values, &["--draft"]) {
                Some(value) => value,
                None => {
                    return Some("Draft PRでは許可された引数だけを正規形で指定してください".into());
                }
            };
            if !positional.is_empty() {
                return Some("Draft PRでは許可された引数だけを正規形で指定してください".into());
            }
            let base = match required_option(args, "-B", "--base") {
                Some(value) => value,
                None => {
                    return Some(
                        "Draft PRにはrepo、base、head、title、body-fileを明示してください".into(),
                    );
                }
            };
            let head = match required_option(args, "-H", "--head") {
                Some(value) => value,
                None => {
                    return Some(
                        "Draft PRにはrepo、base、head、title、body-fileを明示してください".into(),
                    );
                }
            };
            let title = match required_option(args, "-t", "--title") {
                Some(value) => value,
                None => {
                    return Some(
                        "Draft PRにはrepo、base、head、title、body-fileを明示してください".into(),
                    );
                }
            };
            let body = match required_option(args, "-F", "--body-file") {
                Some(value) => value,
                None => {
                    return Some(
                        "Draft PRにはrepo、base、head、title、body-fileを明示してください".into(),
                    );
                }
            };
            if !protected_branch(&base) || !valid_work_branch(&head) {
                return Some("Draft PRのrepositoryまたはhead branchが許可範囲外です".into());
            }
            if let Some(reason) = draft_pr_preflight_reason(cwd, &base, &head) {
                return Some(reason);
            }
            if let Some(reason) = github_text_reason(&[title], "PR title") {
                return Some(reason);
            }
            file_secret_reason(&body, "PR body")
        }
        "edit" => {
            let mut values = common.to_vec();
            values.extend_from_slice(&[
                ("-F", "--body-file"),
                ("-t", "--title"),
                ("", "--add-label"),
                ("", "--remove-label"),
                ("", "--add-assignee"),
                ("", "--remove-assignee"),
                ("", "--milestone"),
                ("", "--add-reviewer"),
                ("", "--remove-reviewer"),
            ]);
            let positional = match strict_gh_args(args, &values, &["--remove-milestone"]) {
                Some(value) => value,
                None => {
                    return Some("PR編集では許可された引数だけを正規形で指定してください".into());
                }
            };
            if let Some(reason) = github_target_reason(&positional, "PR編集") {
                return Some(reason);
            }
            let titles = option_values(args, "-t", "--title");
            let body_files = option_values(args, "-F", "--body-file");
            if titles.as_ref().is_none_or(|values| values.len() > 1)
                || body_files.as_ref().is_none_or(|values| values.len() > 1)
            {
                return Some("PR編集のtitleとbody-fileはそれぞれ1件までです".into());
            }
            let title = titles.as_ref().and_then(|values| values.first());
            let body_file = body_files.as_ref().and_then(|values| values.first());
            let mut mutation = title.is_some()
                || body_file.is_some()
                || args.iter().any(|value| value == "--remove-milestone");
            let metadata_options = [
                ("", "--add-label"),
                ("", "--remove-label"),
                ("", "--add-assignee"),
                ("", "--remove-assignee"),
                ("", "--milestone"),
                ("", "--add-reviewer"),
                ("", "--remove-reviewer"),
            ];
            let mut metadata_values = Vec::new();
            for (short, long) in metadata_options {
                let values = option_values(args, short, long);
                if values.is_none() {
                    return Some("PR編集のmetadata引数に値を明示してください".into());
                }
                let values = values.unwrap_or_default();
                mutation |= !values.is_empty();
                metadata_values.extend(values);
            }
            if !mutation {
                return Some("PR編集にはtitle、body-file、label、assignee、milestone、reviewerの変更を明示してください".into());
            }
            let mut text_values = Vec::new();
            if let Some(title) = title {
                text_values.push(title.clone());
            }
            text_values.extend(metadata_values);
            if let Some(reason) = github_text_reason(&text_values, "PR編集値") {
                return Some(reason);
            }
            if let Some(body_file) = body_file {
                file_secret_reason(body_file, "PR body")
            } else {
                None
            }
        }
        "comment" => {
            let positional = match strict_gh_args(args, &[common[0], ("-F", "--body-file")], &[]) {
                Some(value) => value,
                None => {
                    return Some(
                        "PR commentでは許可された引数だけを正規形で指定してください".into(),
                    );
                }
            };
            if let Some(reason) = github_target_reason(&positional, "PR comment") {
                return Some(reason);
            }
            let body = match required_option(args, "-F", "--body-file") {
                Some(value) => value,
                None => return Some("PR commentにはbody-fileを明示してください".into()),
            };
            file_secret_reason(&body, "PR body")
        }
        "review" => {
            let switches = ["--approve", "--request-changes", "--comment"];
            let positional =
                match strict_gh_args(args, &[common[0], ("-F", "--body-file")], &switches) {
                    Some(value) => value,
                    None => {
                        return Some(
                            "PR reviewでは許可された引数だけを正規形で指定してください".into(),
                        );
                    }
                };
            if let Some(reason) = github_target_reason(&positional, "PR review") {
                return Some(reason);
            }
            let actions = switches
                .iter()
                .filter(|switch| args.iter().any(|value| value == **switch))
                .count();
            if actions != 1 {
                return Some("PR reviewのactionは1件だけ明示してください".into());
            }
            let body_files = option_values(args, "-F", "--body-file");
            if body_files.as_ref().is_none_or(|values| values.len() > 1) {
                return Some("PR reviewのbody-fileは1件までです".into());
            }
            if let Some(body) = body_files.as_ref().and_then(|values| values.first()) {
                file_secret_reason(body, "PR body")
            } else {
                None
            }
        }
        "close" | "reopen" | "update-branch" => {
            let positional = match strict_gh_args(args, &common, &[]) {
                Some(value) => value,
                None => {
                    return Some(
                        "PR lifecycleでは許可された引数だけを正規形で指定してください".into(),
                    );
                }
            };
            if let Some(reason) = github_target_reason(&positional, &format!("PR {command}")) {
                return Some(reason);
            }
            if command == "update-branch" {
                let number = positional.first()?;
                let payload = gh_json(
                    cwd,
                    &[
                        "pr",
                        "view",
                        number,
                        "--repo",
                        &repository,
                        "--json",
                        "number,state,isCrossRepository,headRepository,headRefName,headRefOid",
                    ],
                );
                let Some(payload) = payload else {
                    return Some("PR update-branchのremote対象を確認できません".into());
                };
                let head_repository = match payload.get("headRepository") {
                    Some(StrictJsonValue::Object(value)) => {
                        json_string_value(value.get("nameWithOwner"))
                    }
                    _ => None,
                };
                if json_string_value(payload.get("state")) != Some("OPEN")
                    || json_u64_value(payload.get("number")) != number.parse::<u64>().ok()
                    || json_bool_value(payload.get("isCrossRepository")) != Some(false)
                    || head_repository.is_none_or(|value| !value.eq_ignore_ascii_case(&repository))
                    || json_string_value(payload.get("headRefName"))
                        .is_none_or(|head| !valid_work_branch(head))
                    || json_string_value(payload.get("headRefOid"))
                        .is_none_or(|oid| !valid_oid(oid))
                {
                    return Some(
                        "PR update-branchはcurrent repository内のopen PRだけ許可されます".into(),
                    );
                }
            }
            None
        }
        "ready" => {
            if args.contains(&"--undo".to_string()) {
                let positional = match strict_gh_args(args, &common, &["--undo"]) {
                    Some(value) => value,
                    None => {
                        return Some(
                            "PR lifecycleでは許可された引数だけを正規形で指定してください".into(),
                        );
                    }
                };
                github_target_reason(&positional, "PR ready")
            } else {
                Some("PRのReady化はcodex-deliveryだけが実行できます".into())
            }
        }
        _ => Some("許可されていないGitHub PR操作です".into()),
    }
}

fn gh_read_only_subcommand(command: &str, subcommand: Option<&str>) -> bool {
    match command {
        "pr" => subcommand
            .is_none_or(|value| ["list", "view", "status", "checks", "diff"].contains(&value)),
        "run" => subcommand.is_none_or(|value| ["list", "view", "watch"].contains(&value)),
        "repo" => subcommand.is_none_or(|value| ["list", "view"].contains(&value)),
        "release" => subcommand
            .is_none_or(|value| ["list", "view", "verify", "verify-asset"].contains(&value)),
        "workflow" => subcommand.is_none_or(|value| ["list", "view"].contains(&value)),
        "label" | "cache" | "secret" => subcommand.is_none_or(|value| value == "list"),
        "variable" => subcommand.is_none_or(|value| ["list", "get"].contains(&value)),
        "ruleset" => subcommand.is_none_or(|value| ["list", "view", "check"].contains(&value)),
        _ => false,
    }
}

fn gh_invocation_reason(tokens: &[String], cwd: Option<&str>) -> Option<String> {
    let start = command_start(tokens)?;
    if basename(tokens.get(start)?) != "gh" {
        return None;
    }
    if tokens[start] != "gh" || start != 0 {
        return Some("GitHub commandをwrapper、環境変数、cwd変更経由で実行できません".into());
    }
    if tokens
        .iter()
        .any(|token| token.contains('$') || token.contains('`'))
    {
        return Some("GitHub commandでshell展開を使用できません".into());
    }
    if std::env::var("GH_HOST")
        .ok()
        .is_some_and(|host| !host.eq_ignore_ascii_case("github.com"))
        || tokens
            .iter()
            .any(|token| token == "--hostname" || token.starts_with("--hostname="))
    {
        return Some("GitHub接続先hostを変更できません".into());
    }
    if tokens.get(1).map(String::as_str) == Some("run")
        && tokens.get(2).map(String::as_str) == Some("cancel")
        && tokens[3..]
            .iter()
            .any(|token| token == "--help" || token == "-h")
    {
        return Some("gh run cancelはrun-idとrepoを指定する正規形だけ許可されます".into());
    }
    if tokens
        .iter()
        .any(|token| token == "--help" || token == "-h")
    {
        return None;
    }
    let (command, args) = first_command(&tokens[1..], GH_GLOBAL_VALUE_OPTIONS);
    let command = command?;
    let cwd = cwd.unwrap_or("");
    match command.as_str() {
        "issue" => {
            let (sub, subargs) = first_command(&args, GH_GLOBAL_VALUE_OPTIONS);
            match sub.as_deref() {
                None | Some("list") | Some("status") | Some("view") => None,
                Some("create") | Some("edit") | Some("comment") | Some("close")
                | Some("reopen") => issue_write_reason(sub.as_deref().unwrap_or(""), &subargs, cwd),
                Some("delete") => Some("GitHub Issueの削除は許可されていません".into()),
                Some("develop") => {
                    Some("gh issue developはブランチを作成するため実行できません".into())
                }
                _ => Some("許可されていないGitHub Issue操作です".into()),
            }
        }
        "api" => {
            if graphql_endpoint(gh_api_endpoint(&args).as_deref()) {
                return Some(
                    "gh api graphqlはquery、mutation、subscriptionを問わず直接実行できません"
                        .into(),
                );
            }
            if args.iter().any(|token| {
                token == "-H"
                    || token == "--header"
                    || token.starts_with("--header=")
                    || token.starts_with("http://")
                    || token.starts_with("https://")
            }) {
                return Some("gh apiではhostまたは認証headerを変更できません".into());
            }
            if gh_api_is_write(&args) {
                Some("gh apiはGETの読み取り専用利用に限られます".into())
            } else {
                None
            }
        }
        "pr" => {
            let (sub, subargs) = first_command(&args, GH_GLOBAL_VALUE_OPTIONS);
            match sub.as_deref() {
                None | Some("list") | Some("view") | Some("status") | Some("checks")
                | Some("diff") => None,
                Some("create")
                | Some("edit")
                | Some("comment")
                | Some("review")
                | Some("ready")
                | Some("close")
                | Some("reopen")
                | Some("update-branch") => {
                    pr_write_reason(sub.as_deref().unwrap_or(""), &subargs, cwd)
                }
                _ => Some("PRはDraft作成、通常lifecycle、読み取り専用操作だけ許可されます".into()),
            }
        }
        "run" => {
            let (sub, subargs) = first_command(&args, GH_GLOBAL_VALUE_OPTIONS);
            match sub.as_deref() {
                None | Some("list") | Some("view") | Some("watch") => None,
                Some("cancel") => {
                    let mut full = Vec::with_capacity(subargs.len() + 1);
                    full.push("cancel".to_string());
                    full.extend(subargs);
                    gh_run_cancel_reason(&full, cwd)
                }
                _ => Some("gh runはcancelまたは読み取り専用操作だけ許可されます".into()),
            }
        }
        "status" | "search" => None,
        "repo" | "release" | "workflow" | "label" | "cache" | "variable" | "secret" | "ruleset" => {
            let (subcommand, _) = first_command(&args, GH_GLOBAL_VALUE_OPTIONS);
            if gh_read_only_subcommand(&command, subcommand.as_deref()) {
                None
            } else {
                Some("許可されていないGitHub書き込み操作です".into())
            }
        }
        _ => Some("許可されていないGitHub書き込み操作です".into()),
    }
}

fn python_helper_invocation_reason(tokens: &[String]) -> Option<String> {
    let start = command_start(tokens)?;
    let name = basename(tokens.get(start)?);
    if !name.starts_with("python")
        || !name.strip_prefix("python").is_some_and(|v| {
            v.is_empty()
                || (v.starts_with('3') && v[1..].chars().all(|c| c == '.' || c.is_ascii_digit()))
        })
    {
        return None;
    }
    let mut i = start + 1;
    while i < tokens.len() {
        if tokens[i] == "-m" || (tokens[i].starts_with("-m") && tokens[i] != "-m") {
            let module = if tokens[i] == "-m" {
                let value = tokens.get(i + 1)?;
                i += 2;
                value.clone()
            } else {
                let value = tokens[i][2..].to_string();
                i += 1;
                value
            };
            if ["codex-worktree", "codex-delivery"].contains(&module.as_str())
                || tokens[i..]
                    .iter()
                    .any(|value| ["codex-worktree", "codex-delivery"].contains(&basename(value)))
            {
                return Some("helperはPython moduleまたはinterpreter経由で実行できません".into());
            }
            return None;
        }
        if ["-c", "-W", "-X", "--check-hash-based-pycs"].contains(&tokens[i].as_str()) {
            i += 2;
            continue;
        }
        if tokens[i].starts_with('-') {
            i += 1;
            continue;
        }
        if ["codex-worktree", "codex-delivery"].contains(&basename(&tokens[i])) {
            return Some("helperはPython interpreterやwrapper経由で実行できません".into());
        }
        break;
    }
    None
}

fn worktree_helper_invocation_reason(tokens: &[String]) -> Option<String> {
    let start = command_start(tokens)?;
    if basename(tokens.get(start)?) != "codex-worktree" {
        return None;
    }
    if tokens[start] != "codex-worktree" || start != 0 {
        return Some("worktree helperはPATHから直接実行してください".into());
    }
    let args = &tokens[1..];
    if args.is_empty() {
        return Some("worktree helperのsubcommandを指定してください".into());
    }
    if args == ["--help"] || args == ["-h"] {
        return None;
    }
    let command = &args[0];
    let rest = &args[1..];
    if rest.iter().any(|v| v == "--help" || v == "-h") {
        return Some("codex-worktreeのhelpはsubcommand直後に単独指定してください".into());
    }
    match command.as_str() {
        "list" => {
            if rest.is_empty() {
                None
            } else {
                Some("codex-worktree listに引数は指定できません".into())
            }
        }
        "doctor" => {
            if rest.is_empty() {
                None
            } else {
                helper_task_args("doctor", rest)
            }
        }
        "resume" | "recover" => helper_task_args(command, rest),
        "create" => {
            let mut seen = HashMap::new();
            let mut i = 0;
            while i < rest.len() {
                if !["--branch", "--issue", "--task-id"].contains(&rest[i].as_str())
                    || seen.contains_key(&rest[i])
                    || i + 1 >= rest.len()
                {
                    return Some(
                        "codex-worktree createでは許可された引数だけを1回ずつ指定してください"
                            .into(),
                    );
                }
                seen.insert(rest[i].clone(), rest[i + 1].clone());
                i += 2;
            }
            if seen.contains_key("--issue") && seen.contains_key("--task-id") {
                return Some("Issue番号とtask IDは同時に指定できません".into());
            }
            if seen.get("--issue").is_some_and(|v| {
                v.is_empty() || v.starts_with('0') || !v.bytes().all(|c| c.is_ascii_digit())
            }) || seen.get("--task-id").is_some_and(|v| !valid_task_id(v))
                || seen.get("--branch").is_some_and(|v| !valid_work_branch(v))
            {
                Some("worktree helperの引数が許可形式ではありません".into())
            } else {
                None
            }
        }
        _ => Some("許可されていないworktree helper操作です".into()),
    }
}

fn helper_task_args(command: &str, args: &[String]) -> Option<String> {
    if args.len() == 2 && args[0] == "--task-id" && valid_task_id(&args[1]) {
        None
    } else {
        Some(format!(
            "codex-worktree {command}のtask IDを正規形で指定してください"
        ))
    }
}

fn delivery_helper_invocation_reason(tokens: &[String]) -> Option<String> {
    let start = command_start(tokens)?;
    if basename(tokens.get(start)?) != "codex-delivery" {
        return None;
    }
    if tokens[start] != "codex-delivery" || start != 0 {
        return Some("delivery helperはPATHから直接実行してください".into());
    }
    let args = &tokens[1..];
    if args.is_empty() {
        return Some("delivery helperのsubcommandを指定してください".into());
    }
    if args == ["--help"] || args == ["-h"] {
        return None;
    }
    let command = &args[0];
    if !["record-review", "approve-review", "deliver", "finish"].contains(&command.as_str()) {
        return Some("許可されていないdelivery helper操作です".into());
    }
    let rest = &args[1..];
    if rest == ["--help"] || rest == ["-h"] {
        return None;
    }
    let allowed = [
        "--task-id",
        "--pr",
        "--head",
        "--plan-id",
        "--gate-mode",
        "--risk",
    ];
    let switches = [
        "--tests-passed",
        "--neutral-review-passed",
        "--adversarial-review-passed",
    ];
    let mut values = HashMap::new();
    let mut seen = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        if switches.contains(&rest[i].as_str()) {
            if seen.contains(&rest[i]) {
                return Some("delivery helperのevidence flagを重複指定できません".into());
            }
            seen.push(rest[i].clone());
            i += 1;
            continue;
        }
        if !allowed.contains(&rest[i].as_str())
            || values.contains_key(&rest[i])
            || i + 1 >= rest.len()
        {
            return Some(
                "delivery helperでは許可されたoptionを1回ずつ正規形で指定してください".into(),
            );
        }
        values.insert(rest[i].clone(), rest[i + 1].clone());
        i += 2;
    }
    for key in ["--task-id", "--pr", "--head", "--plan-id"] {
        if !values.contains_key(key) {
            return Some(
                "delivery helperのtask、PR、head、plan、review evidenceをすべて明示してください"
                    .into(),
            );
        }
    }
    if (command == "record-review" || command == "approve-review")
        && (!["low", "medium", "high", "critical"]
            .contains(&values.get("--risk").map(String::as_str).unwrap_or(""))
            || seen.len() != 3)
    {
        return Some("delivery helperのreview evidenceまたはriskが不正です".into());
    }
    if values
        .get("--gate-mode")
        .is_some_and(|v| v != "github-free-private")
        || !valid_task_id(values.get("--task-id")?)
        || !values.get("--pr")?.bytes().all(|c| c.is_ascii_digit())
        || !valid_oid(values.get("--head")?)
        || !valid_plan_id(values.get("--plan-id")?)
    {
        return Some("delivery helperの値が許可形式ではありません".into());
    }
    None
}

fn nested_shell_commands(tokens: &[String]) -> Vec<String> {
    let mut output = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        if !SHELLS.contains(&basename(token)) {
            continue;
        }
        for j in index + 1..tokens.len() {
            let option = &tokens[j];
            if option.starts_with('-') && !option.starts_with("--") && option[1..].contains('c') {
                if let Some(payload) = tokens.get(j + 1) {
                    output.push(payload.clone());
                }
                break;
            }
            if !option.starts_with('-') {
                break;
            }
        }
    }
    output
}

fn git_operation(tokens: &[String]) -> Option<String> {
    let start = command_start(tokens)?;
    if basename(tokens.get(start)?) != "git" {
        return None;
    }
    first_command(
        &tokens[start + 1..],
        &[
            "-C",
            "--git-dir",
            "--work-tree",
            "--namespace",
            "-c",
            "--config",
            "--config-env",
            "--exec-path",
        ],
    )
    .0
}

fn has_write_operation(tokens: &[String]) -> bool {
    if GIT_SAFE_WRITE.contains(&git_operation(tokens).as_deref().unwrap_or("")) {
        return true;
    }
    let start = command_start(tokens);
    if let Some(start) = start {
        match basename(tokens.get(start).unwrap_or(&String::new())) {
            "codex-worktree" => {
                return tokens
                    .get(start + 1)
                    .is_some_and(|v| ["create", "recover"].contains(&v.as_str()));
            }
            "codex-delivery" => {
                return tokens.get(start + 1).is_some_and(|v| {
                    ["record-review", "approve-review", "deliver", "finish"].contains(&v.as_str())
                });
            }
            "gh" => {
                let (command, args) = first_command(&tokens[start + 1..], GH_GLOBAL_VALUE_OPTIONS);
                let (sub, _) = first_command(&args, GH_GLOBAL_VALUE_OPTIONS);
                return match command.as_deref() {
                    Some("issue") => sub.is_some_and(|v| {
                        ["create", "edit", "comment", "close", "reopen"].contains(&v.as_str())
                    }),
                    Some("pr") => sub.is_some_and(|v| {
                        [
                            "create",
                            "edit",
                            "comment",
                            "review",
                            "ready",
                            "close",
                            "reopen",
                            "update-branch",
                        ]
                        .contains(&v.as_str())
                    }),
                    Some("run") => sub.as_deref() == Some("cancel"),
                    Some("api") => gh_api_is_write(&args),
                    _ => false,
                };
            }
            _ => {}
        }
    }
    false
}

fn shell_wraps_restricted(tokens: &[String]) -> bool {
    let Some(start) = command_start(tokens) else {
        return false;
    };
    if !SHELLS.contains(&basename(&tokens[start])) {
        return false;
    }
    tokens[start + 1..].iter().any(|token| {
        RESTRICTED_COMMANDS.contains(&basename(token))
            || DESTRUCTIVE_COMMANDS.contains(&basename(token))
    })
}

fn contains_restricted(tokens: &[String], depth: usize) -> bool {
    if depth > 3 {
        return false;
    }
    if command_start(tokens)
        .and_then(|i| tokens.get(i))
        .is_some_and(|token| {
            RESTRICTED_COMMANDS.contains(&basename(token)) || git_subcommand_executable(token)
        })
    {
        return true;
    }
    nested_shell_commands(tokens)
        .iter()
        .flat_map(|payload| command_segments(payload))
        .any(|nested| contains_restricted(&nested, depth + 1))
}

fn shell_tokens_with_punctuation(command: &str) -> Option<Vec<String>> {
    const PUNCTUATION: &[u8] = b";&|()<>\n";
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut quote = None;
    let mut escaped = false;
    let bytes = command.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if escaped {
            token.push(byte as char);
            escaped = false;
            index += 1;
            continue;
        }
        if quote == Some(b'\'') {
            if byte == b'\'' {
                quote = None;
            } else {
                token.push(byte as char);
            }
            index += 1;
            continue;
        }
        if quote == Some(b'"') {
            if byte == b'"' {
                quote = None;
            } else if byte == b'\\' {
                escaped = true;
            } else {
                token.push(byte as char);
            }
            index += 1;
            continue;
        }
        match byte {
            b'\'' | b'"' => quote = Some(byte),
            b'\\' => escaped = true,
            b' ' | b'\t' | b'\r' => {
                if !token.is_empty() {
                    tokens.push(std::mem::take(&mut token));
                }
            }
            byte if PUNCTUATION.contains(&byte) => {
                if !token.is_empty() {
                    tokens.push(std::mem::take(&mut token));
                }
                let start = index;
                index += 1;
                while index < bytes.len() && PUNCTUATION.contains(&bytes[index]) {
                    index += 1;
                }
                tokens.push(String::from_utf8_lossy(&bytes[start..index]).into_owned());
                continue;
            }
            _ => token.push(byte as char),
        }
        index += 1;
    }
    if escaped || quote.is_some() {
        return None;
    }
    if !token.is_empty() {
        tokens.push(token);
    }
    Some(tokens)
}

fn punctuation_is_redirection(token: &str) -> bool {
    !token.is_empty()
        && token.chars().all(|c| "<>&|".contains(c))
        && token.chars().any(|c| "<>".contains(c))
}

fn leading_redirection_hides_guarded(command: &str) -> bool {
    if !has_unquoted(command, b"<>") {
        return false;
    }
    let Some(tokens) = shell_tokens_with_punctuation(command) else {
        return true;
    };
    let mut chunk = Vec::new();
    let mut chunks = Vec::new();
    for token in tokens {
        if !token.is_empty() && token.chars().all(|c| ";&|()\n".contains(c)) {
            if !chunk.is_empty() {
                chunks.push(std::mem::take(&mut chunk));
            }
        } else {
            chunk.push(token);
        }
    }
    if !chunk.is_empty() {
        chunks.push(chunk);
    }
    for chunk in chunks {
        let mut arguments = Vec::new();
        let mut consumed = false;
        let mut i = 0;
        while i < chunk.len() {
            let is_fd = chunk[i].bytes().all(|byte| byte.is_ascii_digit());
            if (is_fd || punctuation_is_redirection(&chunk[i]))
                && (is_fd
                    && chunk
                        .get(i + 1)
                        .is_some_and(|token| punctuation_is_redirection(token))
                    || punctuation_is_redirection(&chunk[i]))
            {
                let operator_index = if is_fd { i + 1 } else { i };
                consumed = true;
                i = operator_index + 1;
                if i < chunk.len() && !punctuation_is_redirection(&chunk[i]) {
                    i += 1;
                }
                continue;
            }
            arguments.push(chunk[i].clone());
            i += 1;
        }
        if !consumed || arguments.is_empty() {
            continue;
        }
        while arguments.first().is_some_and(|token| {
            is_assignment(token) || SHELL_COMMAND_PREFIXES.contains(&token.as_str())
        }) {
            arguments.remove(0);
        }
        if arguments.first().map(String::as_str) == Some("time") {
            arguments.remove(0);
            while arguments
                .first()
                .is_some_and(|token| ["-p", "--"].contains(&token.as_str()))
            {
                arguments.remove(0);
            }
        }
        if arguments.first().map(String::as_str) == Some("repeat") {
            if arguments.len() >= 2 {
                arguments.drain(..2);
            } else {
                arguments.clear();
            }
        }
        let Some(start) = command_start(&arguments) else {
            continue;
        };
        let executable = &arguments[start];
        if guarded_executable_token(executable)
            || command_word_expansion(executable)
            || contains_restricted(&arguments, 0)
        {
            return true;
        }
    }
    false
}

fn blocked_reason(command: &str, cwd: Option<&str>, depth: usize) -> Option<String> {
    if depth > 3 {
        return Some("入れ子が深いshellコマンドは安全性を確認できません".into());
    }
    if line_continuation(command) {
        return Some("backslash-newlineによるshell token連結は安全に検査できません".into());
    }
    if command.contains("$(") || command.contains('`') {
        return Some("command substitutionを含むcommandは安全に検査できません".into());
    }
    if leading_redirection_hides_guarded(command) {
        return Some("先頭redirectionを伴うGit/GitHub/helper commandは実行できません".into());
    }
    let segments = command_segments(command);
    if segments.is_empty() {
        return Some("command is required".into());
    }
    if segments.len() > 1
        && segments
            .iter()
            .any(|tokens| has_command_resolution_mutation(tokens))
    {
        return Some("command解決を変更するshell builtinを他のsegmentと連結できません".into());
    }
    if segments
        .iter()
        .any(|tokens| has_unparsed_guarded_command(tokens))
    {
        return Some("shell予約語を介したGit/GitHub/helper commandは安全に検査できません".into());
    }
    let restricted = segments.iter().any(|tokens| contains_restricted(tokens, 0));
    let has_compound = segments.iter().any(|tokens| {
        tokens
            .first()
            .is_some_and(|value| SHELL_COMPOUND_PREFIXES.contains(&value.as_str()))
    });
    if restricted && has_compound {
        return Some("shell複合構文とGit/GitHub/helper commandを同じ入力で実行できません".into());
    }
    if restricted
        && segments.len() > 1
        && !segments.iter().all(|tokens| contains_restricted(tokens, 0))
    {
        return Some("Git/GitHub/helper commandを他のshell segmentと連結できません".into());
    }
    if restricted && shell_expansion(command) {
        return Some("Git/GitHub/helper commandでは未引用のshell展開を使用できません".into());
    }
    if restricted && has_unquoted(command, b"<>") {
        return Some("Git/GitHub/helper commandではshell redirectionを使用できません".into());
    }
    let has_write = segments.iter().any(|tokens| has_write_operation(tokens));
    if has_write && has_unquoted(command, b";&|()\n") {
        return Some(
            "Git/GitHub書き込みはshell control operatorなしの直接commandで実行してください".into(),
        );
    }
    if has_write && (depth > 0 || segments.len() != 1) {
        return Some("Git/GitHub書き込みは単一の直接commandで実行してください".into());
    }
    if restricted
        && segments.len() > 1
        && segments
            .iter()
            .any(|tokens| has_shell_context_mutation(tokens))
    {
        return Some("Git/GitHub/helper commandの前後でshell環境やcwdを変更できません".into());
    }
    for tokens in &segments {
        if ambiguous_wrapper_options(tokens) && (restricted || shell_expansion(command)) {
            return Some("wrapperの未知optionを含むguard対象commandは安全に検査できません".into());
        }
        let Some(start) = command_start(tokens) else {
            continue;
        };
        if tokens.get(start).is_some_and(|v| command_word_expansion(v)) {
            return Some("shell展開で実行commandを決定する操作は許可されていません".into());
        }
        if tokens
            .get(start)
            .is_some_and(|v| git_subcommand_executable(v))
        {
            return Some("Git内部commandはcanonicalなgit <subcommand>で実行してください".into());
        }
        if tokens
            .get(start)
            .is_some_and(|v| ["source", "."].contains(&basename(v)))
        {
            return Some("sourceによる未検査scriptの実行は許可されていません".into());
        }
        if tokens
            .get(start)
            .is_some_and(|v| SHELLS.contains(&basename(v)))
            && nested_shell_commands(tokens).is_empty()
        {
            return Some("shellは検査可能な-c inline commandだけを実行できます".into());
        }
        if wrapper_chain_uses_split(tokens) {
            return Some("envのsplit-stringは安全に解析できません".into());
        }
        if tokens.get(start).is_some_and(|v| basename(v) == "builtin") {
            return Some("builtin経由のshell builtin実行は安全に検査できません".into());
        }
        if shell_wraps_restricted(tokens) {
            return Some("shell経由のGit/GitHub/削除commandは実行できません".into());
        }
        if tokens
            .get(start)
            .is_some_and(|v| DESTRUCTIVE_COMMANDS.contains(&basename(v)))
        {
            return Some("削除コマンドは自動実行できません".into());
        }
        if tokens.get(start).is_some_and(|v| basename(v) == "find")
            && tokens[start + 1..].iter().any(|v| {
                ["-delete", "-exec", "-execdir", "-ok", "-okdir"].contains(&v.as_str())
                    || v.starts_with("-fprint")
                    || v.starts_with("-fprintf")
                    || v.starts_with("-fls")
            })
        {
            return Some(
                "findによる削除、外部command実行、file書き込みは許可されていません".into(),
            );
        }
        if tokens.get(start).is_some_and(|v| basename(v) == "eval") {
            let nested = tokens[start + 1..].join(" ");
            if nested.is_empty() {
                return Some("evalの実行内容を確認できません".into());
            }
            if let Some(reason) = blocked_reason(&nested, cwd, depth + 1) {
                return Some(reason);
            }
        }
        let is_git = command_start(tokens)
            .and_then(|index| tokens.get(index))
            .is_some_and(|value| basename(value) == "git");
        if is_git
            && cwd
                .filter(|value| !value.is_empty())
                .is_some_and(|value| !local_git_config_is_safe(value))
        {
            return Some("repository local Git configの安全性を確認できません".into());
        }
        if let Some(reason) = git_invocation_reason(tokens, cwd) {
            return Some(reason);
        }
        if let Some(reason) = gh_invocation_reason(tokens, cwd) {
            return Some(reason);
        }
        if let Some(reason) = python_helper_invocation_reason(tokens) {
            return Some(reason);
        }
        if let Some(reason) = worktree_helper_invocation_reason(tokens) {
            return Some(reason);
        }
        if let Some(reason) = delivery_helper_invocation_reason(tokens) {
            return Some(reason);
        }
        for nested in nested_shell_commands(tokens) {
            if let Some(reason) = blocked_reason(&nested, cwd, depth + 1) {
                return Some(reason);
            }
        }
    }
    None
}

fn write_context_reason(command: &str, cwd: Option<&str>) -> Option<String> {
    if !command_segments(command)
        .iter()
        .any(|tokens| has_write_operation(tokens))
    {
        return None;
    }
    let Some(session) = cwd.filter(|v| !v.is_empty()) else {
        return Some("Git/GitHub書き込みのsession cwdを確認できません".into());
    };
    let root = resolved_git_path(session, "--show-toplevel");
    let common = resolved_git_path(session, "--git-common-dir");
    let (Some(root), Some(common)) = (root, common) else {
        return Some("Git/GitHub書き込みのrepository rootを確認できません".into());
    };
    let Some(main) = common.parent() else {
        return Some("Git/GitHub書き込みのrepository rootを確認できません".into());
    };
    if root != main {
        managed_worktree_reason(main, &common, &root)
    } else {
        None
    }
}

#[cfg(test)]
#[derive(Debug, Clone)]
enum JsonValue {
    Object(HashMap<String, JsonValue>),
    String(String),
    Null,
    Bool,
    Number,
    Array,
}

#[cfg(test)]
struct JsonParser<'a> {
    input: &'a [u8],
    index: usize,
}

#[cfg(test)]
impl<'a> JsonParser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            input: input.as_bytes(),
            index: 0,
        }
    }
    fn parse(mut self) -> Result<JsonValue, ()> {
        let value = self.value()?;
        self.ws();
        if self.index == self.input.len() {
            Ok(value)
        } else {
            Err(())
        }
    }
    fn ws(&mut self) {
        while self
            .input
            .get(self.index)
            .is_some_and(|c| c.is_ascii_whitespace())
        {
            self.index += 1;
        }
    }
    fn value(&mut self) -> Result<JsonValue, ()> {
        self.ws();
        match self.input.get(self.index).copied() {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => Ok(JsonValue::String(self.string()?)),
            Some(b't') if self.take(b"true") => Ok(JsonValue::Bool),
            Some(b'f') if self.take(b"false") => Ok(JsonValue::Bool),
            Some(b'n') if self.take(b"null") => Ok(JsonValue::Null),
            Some(c) if c == b'-' || c.is_ascii_digit() => {
                while self
                    .input
                    .get(self.index)
                    .is_some_and(|c| c.is_ascii_digit() || b"+-.eE".contains(c))
                {
                    self.index += 1;
                }
                Ok(JsonValue::Number)
            }
            _ => Err(()),
        }
    }
    fn take(&mut self, expected: &[u8]) -> bool {
        if self.input.get(self.index..self.index + expected.len()) == Some(expected) {
            self.index += expected.len();
            true
        } else {
            false
        }
    }
    fn object(&mut self) -> Result<JsonValue, ()> {
        self.index += 1;
        let mut result = HashMap::new();
        self.ws();
        if self.input.get(self.index) == Some(&b'}') {
            self.index += 1;
            return Ok(JsonValue::Object(result));
        }
        loop {
            self.ws();
            let key = self.string()?;
            self.ws();
            if self.input.get(self.index) != Some(&b':') {
                return Err(());
            }
            self.index += 1;
            let value = self.value()?;
            result.insert(key, value);
            self.ws();
            match self.input.get(self.index) {
                Some(b',') => self.index += 1,
                Some(b'}') => {
                    self.index += 1;
                    return Ok(JsonValue::Object(result));
                }
                _ => return Err(()),
            }
        }
    }
    fn array(&mut self) -> Result<JsonValue, ()> {
        self.index += 1;
        self.ws();
        if self.input.get(self.index) == Some(&b']') {
            self.index += 1;
            return Ok(JsonValue::Array);
        }
        loop {
            let _ = self.value()?;
            self.ws();
            match self.input.get(self.index) {
                Some(b',') => self.index += 1,
                Some(b']') => {
                    self.index += 1;
                    return Ok(JsonValue::Array);
                }
                _ => return Err(()),
            }
        }
    }
    fn string(&mut self) -> Result<String, ()> {
        if self.input.get(self.index) != Some(&b'"') {
            return Err(());
        }
        self.index += 1;
        let mut result = String::new();
        while let Some(&c) = self.input.get(self.index) {
            self.index += 1;
            match c {
                b'"' => return Ok(result),
                b'\\' => {
                    let escape = *self.input.get(self.index).ok_or(())?;
                    self.index += 1;
                    match escape {
                        b'"' => result.push('"'),
                        b'\\' => result.push('\\'),
                        b'/' => result.push('/'),
                        b'b' => result.push('\x08'),
                        b'f' => result.push('\x0c'),
                        b'n' => result.push('\n'),
                        b'r' => result.push('\r'),
                        b't' => result.push('\t'),
                        b'u' => {
                            let digits = self.input.get(self.index..self.index + 4).ok_or(())?;
                            self.index += 4;
                            let text = std::str::from_utf8(digits).map_err(|_| ())?;
                            let code = u16::from_str_radix(text, 16).map_err(|_| ())?;
                            result.push(char::from_u32(code as u32).ok_or(())?);
                        }
                        _ => return Err(()),
                    }
                }
                c if c < 0x20 => return Err(()),
                c => result.push(c as char),
            }
        }
        Err(())
    }
}

fn json_string(value: &str) -> String {
    let mut output = String::from("\"");
    for c in value.chars() {
        match c {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            c if c.is_control() => output.push_str(&format!("\\u{:04x}", c as u32)),
            c => output.push(c),
        }
    }
    output.push('"');
    output
}

fn deny(reason: &str) {
    let full = format!("{reason}（PreToolUse hookが直接操作を拒否しました）。");
    let payload = format!(
        "{{\"hookSpecificOutput\":{{\"hookEventName\":\"PreToolUse\",\"permissionDecision\":\"deny\",\"permissionDecisionReason\":{}}}}}",
        json_string(&full)
    );
    let mut stdout = io::stdout().lock();
    let _ = stdout.write_all(payload.as_bytes());
    let _ = stdout.flush();
    let mut stderr = io::stderr().lock();
    let _ = writeln!(stderr, "{full}");
}

fn allow_updated_input(command: &str) {
    let payload = format!(
        "{{\"hookSpecificOutput\":{{\"hookEventName\":\"PreToolUse\",\"permissionDecision\":\"allow\",\"updatedInput\":{{\"command\":{}}}}}}}",
        json_string(command)
    );
    let mut stdout = io::stdout().lock();
    let _ = stdout.write_all(payload.as_bytes());
    let _ = stdout.flush();
}

/// Run the hook protocol.  `0` means allow, `2` means deny or malformed input.
pub fn entrypoint() -> i32 {
    let mut input = Vec::with_capacity(8 * 1024);
    let mut limited = io::stdin().lock().take((MAX_HOOK_INPUT_BYTES + 1) as u64);
    if limited.read_to_end(&mut input).is_err() || input.len() > MAX_HOOK_INPUT_BYTES {
        deny("hook inputを安全に解析・検査できません");
        return 2;
    }
    let input = match std::str::from_utf8(&input) {
        Ok(input) => input,
        Err(_) => {
            deny("hook inputを安全に解析・検査できません");
            return 2;
        }
    };
    let root = match serde_json::from_str::<StrictJsonValue>(input) {
        Ok(StrictJsonValue::Object(value)) => value,
        _ => {
            deny("hook inputを安全に解析・検査できません");
            return 2;
        }
    };
    let Some(tool_input) = root.get("tool_input").and_then(StrictJsonValue::as_object) else {
        deny("hook inputを安全に解析・検査できません");
        return 2;
    };
    let Some(command) = tool_input
        .get("command")
        .and_then(StrictJsonValue::as_str)
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            tool_input
                .get("cmd")
                .and_then(StrictJsonValue::as_str)
                .filter(|value| !value.trim().is_empty())
        })
    else {
        deny("hook inputを安全に解析・検査できません");
        return 2;
    };
    let cwd = root.get("cwd").and_then(StrictJsonValue::as_str);
    if let Some(reason) = write_context_reason(command, cwd) {
        deny(&reason);
        return 2;
    }
    if let Some(reason) = blocked_reason(command, cwd, 0) {
        deny(&reason);
        return 2;
    }
    match rewrite_body_file_command(command, cwd) {
        Ok(Some(updated_command)) => allow_updated_input(&updated_command),
        Ok(None) => {}
        Err(reason) => {
            deny(&reason);
            return 2;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_read_commands_do_not_start_processes() {
        assert!(blocked_reason("git status", None, 0).is_none());
        assert!(blocked_reason("git diff --stat", None, 0).is_none());
        assert!(blocked_reason("git branch --contains main", None, 0).is_none());
        assert!(blocked_reason("git remote -v", None, 0).is_none());
    }

    #[test]
    fn shell_safety_is_fail_closed() {
        assert!(blocked_reason("git status > /tmp/status", None, 0).is_some());
        assert!(blocked_reason("git status; git reset --hard HEAD", None, 0).is_some());
        assert!(blocked_reason("git status $(printf x)", None, 0).is_some());
        assert!(blocked_reason("rm -rf README.md", None, 0).is_some());
        assert!(blocked_reason("git-reset --hard HEAD", None, 0).is_some());
    }

    #[test]
    fn repository_config_execution_and_transport_keys_fail_closed() {
        for key in [
            "include.path",
            "core.sshCommand",
            "core.fsmonitor",
            "credential.helper",
            "http.example.proxy",
            "url.safe.insteadOf",
            "remote.origin.pushurl",
            "protocol.ext.allow",
            "filter.lfs.process",
            "diff.external",
            "merge.custom.driver",
        ] {
            assert!(dangerous_local_git_key(key), "{key}");
        }
        assert!(!dangerous_local_git_key("remote.origin.url"));
        assert!(!dangerous_local_git_key("user.name"));
    }

    #[test]
    fn commit_subject_and_options_match_python_contract() {
        assert!(git_commit_reason(&["-m".into(), ":bug: summary".into()]).is_none());
        assert!(git_commit_reason(&["-m".into(), ":bad emoji: summary".into()]).is_some());
        assert!(
            git_commit_reason(&["-m".into(), ":bug: summary".into(), "--quiet".into()]).is_some()
        );
    }

    #[test]
    fn pull_preflight_detects_in_progress_git_state() {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let git_dir = std::env::temp_dir().join(format!("codex-guard-git-state-{suffix}"));
        fs::create_dir(&git_dir).expect("create git state fixture");
        assert!(git_operation_in_progress_reason(&git_dir).is_none());
        fs::write(git_dir.join("MERGE_HEAD"), b"fixture").expect("write git state fixture");
        assert!(git_operation_in_progress_reason(&git_dir).is_some());
        fs::remove_file(git_dir.join("MERGE_HEAD")).expect("remove git state fixture");
        fs::remove_dir(git_dir).expect("remove git state directory");
    }

    #[test]
    fn managed_manifest_schema_is_strict_and_rejects_duplicate_keys() {
        let fixture = r#"{
            "version": 1,
            "status": "ready",
            "task_id": "issue-31",
            "repository": "/repo",
            "common_git_dir": "/repo/.git",
            "github_name": "owner/repo",
            "branch": "refactor/example",
            "base": "origin/main",
            "base_oid": "0123456789012345678901234567890123456789",
            "worktree": "/managed/issue-31",
            "created_at": "2026-08-20T00:00:00Z",
            "detail": ""
        }"#;
        let expected = [
            ("status", "ready"),
            ("task_id", "issue-31"),
            ("repository", "/repo"),
            ("branch", "refactor/example"),
        ];
        let manifest = parse_strict_object(fixture).expect("strict manifest");
        assert!(manifest_schema_matches(&manifest, &expected));

        let version_spoof = fixture.replacen("\"version\": 1", "\"version\": 10", 1);
        let version_spoof = parse_strict_object(&version_spoof).expect("version spoof object");
        assert!(!manifest_schema_matches(&version_spoof, &expected));
        assert!(parse_strict_object(r#"{"version":1,"version":1}"#).is_none());
    }

    #[test]
    fn shell_reserved_words_and_leading_redirections_cannot_hide_guarded_commands() {
        for command in [
            "if true; then git reset --hard HEAD; fi",
            "for value in one; do gh repo delete owner/repo --yes; done",
            "time git reset --hard HEAD",
            "! git reset --hard HEAD",
            "2>status git reset --hard HEAD",
            "> status git reset --hard HEAD",
            "2>&1 git reset --hard HEAD",
        ] {
            assert!(
                blocked_reason(command, None, 0).is_some(),
                "guarded command bypassed: {command}"
            );
        }
    }

    #[test]
    fn github_top_level_read_only_allowlist_is_closed() {
        for command in [
            "gh status",
            "gh search code pattern",
            "gh repo list",
            "gh repo view owner/repo",
            "gh release list",
            "gh release verify owner/repo",
            "gh workflow view build.yml",
            "gh label list",
            "gh cache list",
            "gh variable get NAME",
            "gh secret list",
            "gh ruleset check",
        ] {
            let tokens = command_segments(command)
                .into_iter()
                .next()
                .expect("tokens");
            assert!(
                gh_invocation_reason(&tokens, None).is_none(),
                "read-only command was rejected: {command}"
            );
        }
        for command in [
            "gh repo delete owner/repo --yes",
            "gh release delete v1.0.0",
            "gh workflow disable build.yml",
            "gh label create unsafe",
            "gh cache delete 123",
            "gh variable set NAME",
            "gh secret set NAME",
            "gh ruleset delete 1",
        ] {
            let tokens = command_segments(command)
                .into_iter()
                .next()
                .expect("tokens");
            assert!(
                gh_invocation_reason(&tokens, None).is_some(),
                "write command was allowed: {command}"
            );
        }
    }

    #[test]
    fn github_lifecycle_option_shapes_match_python_oracle() {
        let payload: StrictJsonValue =
            serde_json::from_str(r#"{"number":123,"isCrossRepository":false,"spoofed":true}"#)
                .expect("remote fixture");
        let payload = payload.as_object().expect("remote object");
        assert_eq!(json_u64_value(payload.get("number")), Some(123));
        assert_eq!(json_u64_value(payload.get("spoofed")), None);
        assert_eq!(
            json_bool_value(payload.get("isCrossRepository")),
            Some(false)
        );

        let issue_close = vec![
            "123".into(),
            "--repo".into(),
            "owner/repo".into(),
            "--reason".into(),
            "completed".into(),
        ];
        assert_eq!(
            strict_gh_args(&issue_close, &[("", "--repo"), ("", "--reason")], &[]),
            Some(vec!["123".into()])
        );
        assert_eq!(
            option_values(&issue_close, "", "--reason"),
            Some(vec!["completed".into()])
        );

        let review = vec![
            "123".into(),
            "--repo".into(),
            "owner/repo".into(),
            "--approve".into(),
            "--body-file".into(),
            "/tmp/review.md".into(),
        ];
        assert_eq!(
            strict_gh_args(
                &review,
                &[("", "--repo"), ("-F", "--body-file")],
                &["--approve", "--request-changes", "--comment"]
            ),
            Some(vec!["123".into()])
        );
        assert_eq!(
            option_values(&review, "-F", "--body-file"),
            Some(vec!["/tmp/review.md".into()])
        );
        assert!(
            strict_gh_args(
                &["123".into(), "--approve".into(), "--comment".into()],
                &[],
                &["--approve", "--comment"]
            )
            .is_some()
        );
        assert!(ai_attribution(concat!("Co-Authored", "-By : Open", "AI")));
        assert!(ai_attribution(concat!("generated", "-by: A", "I agent")));
        assert!(!ai_attribution("ordinary generated result about rust"));
        assert!(contains_copilot_mention("please ask @Copilot now"));
        assert!(!contains_copilot_mention("name@copilot"));
        assert!(!contains_copilot_mention("@copilot_helper"));
    }

    #[test]
    fn issue_edit_requires_a_safe_explicit_mutation() {
        let cwd = std::env::current_dir().expect("current repository");
        let cwd = cwd.to_str().expect("UTF-8 repository path");
        let repository = origin_repository(cwd).expect("GitHub origin");
        let base = vec!["123".into(), "--repo".into(), repository.clone()];
        assert!(issue_write_reason("edit", &base, cwd).is_some());

        let mut safe = base.clone();
        safe.extend(["--add-label".into(), "enhancement".into()]);
        assert!(issue_write_reason("edit", &safe, cwd).is_none());

        let mut unsafe_metadata = base;
        unsafe_metadata.extend(["--add-label".into(), "@Copilot".into()]);
        assert!(issue_write_reason("edit", &unsafe_metadata, cwd).is_some());
    }

    #[test]
    fn helper_syntax_is_restricted() {
        assert!(blocked_reason("codex-worktree list", None, 0).is_none());
        assert!(blocked_reason("codex-worktree create --task-id task-example", None, 0).is_none());
        assert!(blocked_reason("codex-worktree create --issue 0", None, 0).is_some());
        assert!(blocked_reason("codex-delivery deliver --task-id task-example --pr 1 --head 0123456789012345678901234567890123456789 --plan-id PLAN-TEST-v1", None, 0).is_none());
    }

    #[test]
    fn parser_handles_hook_shape() {
        let input = r#"{"tool_input":{"command":"git status"},"cwd":"/tmp"}"#;
        let parsed = JsonParser::new(input).parse().expect("parse JSON fixture");
        let JsonValue::Object(root) = parsed else {
            panic!("fixture root must be an object");
        };
        let Some(JsonValue::String(cwd)) = root.get("cwd") else {
            panic!("fixture cwd must be a string");
        };
        assert_eq!(cwd, "/tmp");
    }

    #[test]
    fn strict_hook_json_rejects_malformed_numbers() {
        assert_eq!(MAX_HOOK_INPUT_BYTES, 1024 * 1024);
        assert!(
            serde_json::from_str::<StrictJsonValue>(
                r#"{"tool_input":{"command":"git status"},"n":1+2}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<StrictJsonValue>(
                r#"{"tool_input":{"command":"git status"},"n":-}"#
            )
            .is_err()
        );
    }

    #[test]
    fn writes_require_a_session_cwd() {
        assert!(write_context_reason("git add -- README.md", None).is_some());
        assert!(
            write_context_reason("codex-worktree create --task-id task-example", None).is_some()
        );
        assert!(write_context_reason("codex-delivery deliver --task-id task-example --pr 1 --head 0123456789012345678901234567890123456789 --plan-id PLAN-TEST-v1", None).is_some());
    }

    #[test]
    fn preflight_and_secret_guards_fail_closed_without_repository_state() {
        assert!(contains_secret(concat!(
            "github",
            "_pat_abcdefghijklmnopqrstuvwxyz"
        )));
        assert!(contains_secret(concat!(
            "-----BEGIN OPENSSH ",
            "PRIVATE KEY-----"
        )));
        assert!(!contains_secret("task-example"));
        assert!(!contains_secret(concat!("sk", "-short")));
        assert!(!contains_secret("ordinary text"));
        assert!(sensitive_path("credentials.json"));
        assert!(sensitive_path("config/client.pem"));
        assert!(!sensitive_path(".env.example"));
        assert!(file_secret_reason("/tmp/does-not-exist-codex-guard", "PR body").is_some());
        assert!(git_add_reason(&[".".into()]).is_some());
        assert!(git_commit_reason(&["-m".into(), "Fix config".into()]).is_some());
        assert!(github_text_reason(&["@copilot please".into()], "PR title").is_some());
        assert!(github_text_reason(&["@Copilot please".into()], "PR title").is_some());
        assert!(github_text_reason(&["@COPILOT please".into()], "PR title").is_some());
        assert!(
            git_push_reason(
                &[
                    "-u".into(),
                    "origin".into(),
                    "HEAD:refs/heads/feature/example".into()
                ],
                ""
            )
            .is_some()
        );
        assert!(
            git_pull_reason(
                &[
                    "--ff-only".into(),
                    "--no-rebase".into(),
                    "--no-autostash".into(),
                    "--no-recurse-submodules".into(),
                    "origin".into(),
                    "main".into()
                ],
                ""
            )
            .is_some()
        );
        assert!(git_switch_reason(&["main".into()], "").is_some());
        assert!(
            gh_run_cancel_reason(
                &[
                    "cancel".into(),
                    "1".into(),
                    "--repo".into(),
                    "owner/repo".into()
                ],
                ""
            )
            .is_some()
        );
        assert!(
            managed_worktree_reason(
                Path::new("/tmp/missing-repository"),
                Path::new("/tmp/missing-common"),
                Path::new("/tmp/missing-worktree")
            )
            .is_some()
        );
    }

    #[cfg(unix)]
    #[test]
    fn body_file_snapshot_keeps_the_checked_bytes_and_private_modes() {
        use std::os::unix::fs::MetadataExt;

        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let source_root = std::env::temp_dir().join(format!("codex-guard-body-source-{suffix}"));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&source_root)
            .expect("create body source directory");
        let source = source_root.join("body.md");
        let mut source_file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&source)
            .expect("create body source");
        source_file
            .write_all(b"checked body")
            .expect("write body source");

        let snapshot = snapshot_body_file(source.to_str().expect("UTF-8 source"), "PR body")
            .expect("create checked snapshot");
        fs::write(&source, b"replacement after check").expect("replace source");
        assert_eq!(fs::read(&snapshot).expect("read snapshot"), b"checked body");
        let snapshot_metadata = fs::metadata(&snapshot).expect("snapshot metadata");
        assert_eq!(snapshot_metadata.mode() & 0o777, 0o400);
        let snapshot_root = snapshot.parent().expect("snapshot parent");
        assert_eq!(
            fs::metadata(snapshot_root)
                .expect("snapshot directory metadata")
                .mode()
                & 0o777,
            0o500
        );

        remove_expired_body_snapshot(snapshot_root).expect("remove owned snapshot");
        fs::remove_file(&source).expect("remove body source");
        fs::remove_dir(&source_root).expect("remove body source directory");
    }

    #[cfg(unix)]
    #[test]
    fn body_snapshot_gc_removes_only_expired_valid_snapshots() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("codex-guard-body-gc-{suffix}"));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .expect("create GC fixture root");
        let expired = root.join(format!("{BODY_SNAPSHOT_DIR_PREFIX}1-1-1"));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&expired)
            .expect("create expired snapshot");
        let body = expired.join("body");
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o400)
            .open(&body)
            .expect("create expired body");
        file.write_all(b"expired").expect("write expired body");
        file.sync_all().expect("sync expired body");
        fs::set_permissions(&expired, fs::Permissions::from_mode(0o500))
            .expect("protect expired snapshot");
        let unrelated = root.join(format!("{BODY_SNAPSHOT_DIR_PREFIX}not-managed"));
        fs::create_dir(&unrelated).expect("create unrelated directory");

        cleanup_body_snapshots(&root, BODY_SNAPSHOT_TTL.as_nanos() + 2)
            .expect("clean expired snapshots");
        assert!(!expired.exists());
        assert!(unrelated.exists());

        fs::remove_dir(&unrelated).expect("remove unrelated directory");
        fs::remove_dir(&root).expect("remove GC fixture root");
    }

    #[test]
    fn command_falls_back_to_cmd_when_command_is_empty() {
        let parsed: Result<StrictJsonValue, _> =
            serde_json::from_str(r#"{"tool_input":{"command":"","cmd":"git status"}}"#);
        assert!(parsed.is_ok());
        let Some(value) = parsed.ok() else {
            return;
        };
        let Some(object) = value
            .as_object()
            .and_then(|root| root.get("tool_input"))
            .and_then(StrictJsonValue::as_object)
        else {
            return;
        };
        assert_eq!(
            object
                .get("command")
                .and_then(StrictJsonValue::as_str)
                .filter(|value| !value.trim().is_empty())
                .or_else(|| object.get("cmd").and_then(StrictJsonValue::as_str)),
            Some("git status")
        );
    }

    #[test]
    fn migrated_syntactic_corpus() {
        let allowed = [
            "git status",
            "git diff --stat",
            "git branch",
            "git branch --contains main",
            "git branch --list 'feature/*'",
            "git remote -v",
            "git worktree list --porcelain",
            "git worktree list --porcelain -z",
            "git ls-remote --branches origin feature/example",
            "git ls-remote --heads origin refs/heads/feature/example",
            "git rev-parse --abbrev-ref --symbolic-full-name '@{upstream}'",
            "git --version",
            "gh issue list",
            "gh issue view 1 --repo owner/repo",
            "gh pr view 1 --repo owner/repo",
            "gh api repos/owner/repo",
            "codex-worktree list",
            "codex-worktree --help",
            "codex-delivery --help",
            "printf '%s' 'git status > /tmp/status'",
            "printf '%s' git > /tmp/status",
            "rg --files /tmp/git > /tmp/path",
        ];
        for command in allowed {
            assert!(
                blocked_reason(command, None, 0).is_none(),
                "allowed: {command}"
            );
        }

        let blocked = [
            "git worktree list",
            "git worktree list --porcelain --verbose",
            "git ls-remote origin feature/example",
            "git ls-remote --branches upstream feature/example",
            "git ls-remote --branches origin main",
            "git status > /tmp/status",
            "git status 2> /tmp/status",
            "> /tmp/status git reset --hard HEAD",
            "gh pr view 1 > /tmp/pr",
            "codex-delivery deliver",
            "codex-worktree create --issue 0",
            "git reset --hard HEAD",
            "git add .",
            "git commit -m 'Fix config'",
            "git fetch --prune origin main",
            "gh issue delete 1 --repo owner/repo",
            "gh api graphql",
            "gh api repos/owner/repo -f state=open",
            "gh api repos/owner/repo -H 'Authorization: Bearer secret'",
            "gh run cancel 1 --repo owner/repo",
            "python3 /tmp/codex-worktree create",
            "python3 -m codex-delivery deliver",
            "bash -c 'git reset --hard HEAD'",
            "eval 'git reset --hard HEAD'",
            "find . -delete",
            "git status $(printf x)",
            "git status $CMD",
            "git status; git reset --hard HEAD",
            "git-reset --hard HEAD",
        ];
        for command in blocked {
            assert!(
                blocked_reason(command, None, 0).is_some(),
                "blocked: {command}"
            );
        }
    }
}

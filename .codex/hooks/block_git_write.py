#!/usr/bin/env python3
"""Codex PreToolUse hook: 開発ワークフローの危険な直接操作を拒否する。

Git/GitHub CLI は安全な形だけを許可し、削除コマンド、保護ブランチへの
書き込み、履歴改変、AI帰属、秘密情報を含むcommit/PRを早期拒否する。
rules、workspace-write sandbox、AGENTS.mdと併用する多層防御であり、
任意のプログラム内部まで解析できる完全なセキュリティ境界ではない。
"""

import json
import os
from pathlib import Path
import re
import selectors
import shlex
import stat
import subprocess
import sys
import time


GIT_READ_ONLY = {
    "status", "log", "diff", "show", "rev-parse", "blame", "shortlog",
    "describe", "reflog", "ls-files", "ls-tree", "cat-file", "rev-list",
    "merge-base", "name-rev", "grep",
}
GIT_SAFE_WRITE = {"add", "commit", "fetch", "pull", "push", "switch"}
GIT_OPTS_WITH_VALUE = {"-C", "--git-dir", "--work-tree", "--namespace"}
GIT_FORBIDDEN_GLOBAL_OPTS = {"-c", "--config", "--config-env", "--exec-path"}
GIT_READ_FORBIDDEN_ARGS = {
    "--ext-diff", "--textconv", "--filters", "--paginate", "-p", "--output", "-O",
}

BRANCH_READ_OPTS_WITH_VALUE = {
    "--contains", "--no-contains", "--merged", "--no-merged", "--points-at",
    "--sort", "--format", "--column", "--color", "--abbrev",
}
BRANCH_WRITE_ARGS = {
    "-d", "-D", "-m", "-M", "-c", "-C", "--delete", "--move", "--copy",
    "--edit-description", "--set-upstream-to", "--unset-upstream",
    "--create-reflog",
}
REMOTE_READ_ARGS = {"-v", "--verbose"}

BRANCH_PREFIXES = {
    "feat", "feature", "fix", "refactor", "docs", "test", "chore", "ci",
    "build", "perf", "style", "hotfix", "update",
}
PROTECTED_BRANCHES = {"main", "master", "develop", "development", "trunk"}
PROTECTED_BRANCH_PATTERNS = (
    re.compile(r"^release(?:/|$)"),
    re.compile(r"^production(?:/|$)"),
)
BRANCH_RE = re.compile(r"^[a-z][a-z0-9-]*/[a-z0-9][a-z0-9._-]*$")

GH_ISSUE_ALLOWED = {
    "list", "status", "view", "create", "edit", "comment",
}
GH_READ_ONLY = {
    "pr": {"list", "view", "status", "checks", "diff"},
    "run": {"list", "view", "watch"},
    "repo": {"list", "view"},
    "release": {"list", "view", "verify", "verify-asset"},
    "workflow": {"list", "view"},
    "label": {"list"},
    "cache": {"list"},
    "variable": {"list", "get"},
    "secret": {"list"},
    "ruleset": {"list", "view", "check"},
}
GH_READ_ONLY_TOP_LEVEL = {"status", "search"}
GH_GLOBAL_OPTS_WITH_VALUE = {"-R", "--repo", "--hostname"}

DESTRUCTIVE_COMMANDS = {"rm", "rmdir", "unlink", "shred"}
SHELLS = {"bash", "dash", "sh", "zsh"}
COMMAND_WRAPPERS = {"command", "env", "exec", "nice", "nohup", "sudo", "timeout", "xargs"}
ENV_OPTS_WITH_VALUE = {"-u", "--unset", "-C", "--chdir", "-S", "--split-string"}
EXEC_OPTS_WITH_VALUE = {"-a"}
NICE_OPTS_WITH_VALUE = {"-n", "--adjustment"}
TIMEOUT_OPTS_WITH_VALUE = {"-k", "--kill-after", "-s", "--signal"}
XARGS_OPTS_WITH_VALUE = {
    "-a", "--arg-file", "-d", "--delimiter", "-E", "--eof", "-I", "--replace",
    "-L", "--max-lines", "-n", "--max-args", "-P", "--max-procs", "-s", "--max-chars",
}
SUDO_OPTS_WITH_VALUE = {
    "-u", "--user", "-g", "--group", "-h", "--host", "-p", "--prompt",
    "-C", "--chdir", "-R", "--chroot", "-T", "--command-timeout", "-r",
    "--role", "-t", "--type",
}
# 改行もshellのcommand separatorとして扱う。shlexの既定では改行が単なる
# whitespaceになり、前後のcommandが1 segmentへ結合されるため、読み取りの
# 後ろに置かれた書き込みを見落とす可能性がある。
SHELL_PUNCTUATION = ";&|()`\n"
MAX_BODY_FILE_BYTES = 256 * 1024
MAX_GIT_OUTPUT_BYTES = 4 * 1024 * 1024
GIT_COMMAND_TIMEOUT_SECONDS = 2
MAX_MANIFEST_BYTES = 256 * 1024
GIT_WRITE_ENVIRONMENT_KEYS = {
    "GIT_DIR", "GIT_WORK_TREE", "GIT_COMMON_DIR", "GIT_NAMESPACE",
    "GIT_CONFIG_PARAMETERS", "GIT_CONFIG_COUNT", "GIT_CONFIG_GLOBAL",
    "GIT_CONFIG_SYSTEM",
}
GIT_SANITIZED_ENVIRONMENT_KEYS = GIT_WRITE_ENVIRONMENT_KEYS | {
    "GIT_EXEC_PATH", "GIT_SSH", "GIT_SSH_COMMAND", "GIT_ASKPASS",
    "SSH_ASKPASS", "GIT_PROXY_COMMAND",
}
GIT_SANITIZED_WRAPPER = ("env", "-u", "SSH_ASKPASS")
AI_ATTRIBUTION_RE = re.compile(
    r"(?i)(?:co-authored-by|generated(?:-| )by|signed-off-by)\s*:\s*.*"
    r"(?:codex|openai|chatgpt|claude|gemini|copilot|\bai(?:\s+(?:assistant|agent|bot))?\b)"
)
COMMIT_SUBJECT_RE = re.compile(r"^:[a-z0-9_+-]+: \S.*$")
TASK_ID_RE = re.compile(r"^(?:issue-[1-9][0-9]*|task-[a-z0-9][a-z0-9-]{0,63})$")

SECRET_PATTERNS = (
    re.compile(r"github_pat_[A-Za-z0-9_]{20,}"),
    re.compile(r"gh[pousr]_[A-Za-z0-9_]{20,}"),
    re.compile(r"sk-[A-Za-z0-9_-]{20,}"),
    re.compile(r"AKIA[0-9A-Z]{16}"),
    re.compile(r"xox[baprs]-[A-Za-z0-9-]{10,}"),
    re.compile(r"AIza[0-9A-Za-z_-]{30,}"),
    re.compile(r"-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----"),
)
SENSITIVE_FILENAMES = {
    ".env", "auth.json", "credentials.json", ".credentials.json",
    "id_rsa", "id_ed25519",
}
SENSITIVE_SUFFIXES = {".pem", ".p12", ".pfx", ".jks", ".keystore"}


def _branch_arg_is_write(arg):
    """git branch の変更オプション（値を結合した形式を含む）ならTrue。"""
    if arg in BRANCH_WRITE_ARGS:
        return True
    if any(
        arg.startswith(f"{write_arg}=")
        for write_arg in BRANCH_WRITE_ARGS
        if write_arg.startswith("--")
    ):
        return True
    return len(arg) > 2 and arg[:2] in {"-d", "-D", "-m", "-M", "-c", "-C"}


def _first_command(args, options_with_value=frozenset()):
    """オプションを読み飛ばし、最初のコマンドと残りの引数を返す。"""
    skip_next = False
    for index, token in enumerate(args):
        if skip_next:
            skip_next = False
            continue
        if token in options_with_value:
            skip_next = True
            continue
        if any(token.startswith(f"{option}=") for option in options_with_value):
            continue
        if token.startswith("-"):
            continue
        return token, args[index + 1:]
    return None, []


def _command_start(tokens):
    """envやsudoなどを読み飛ばし、実際の実行コマンド位置を返す。"""
    index = 0
    while index < len(tokens):
        token = tokens[index]
        basename = os.path.basename(token)
        if re.match(r"^[A-Za-z_][A-Za-z0-9_]*=", token):
            index += 1
            continue
        if basename not in COMMAND_WRAPPERS:
            return index
        index += 1
        options_with_value = frozenset()
        if basename == "env":
            options_with_value = ENV_OPTS_WITH_VALUE
        elif basename == "exec":
            options_with_value = EXEC_OPTS_WITH_VALUE
        elif basename == "nice":
            options_with_value = NICE_OPTS_WITH_VALUE
        elif basename == "sudo":
            options_with_value = SUDO_OPTS_WITH_VALUE
        elif basename == "timeout":
            options_with_value = TIMEOUT_OPTS_WITH_VALUE
        elif basename == "xargs":
            options_with_value = XARGS_OPTS_WITH_VALUE
        while index < len(tokens):
            option = tokens[index]
            if re.match(r"^[A-Za-z_][A-Za-z0-9_]*=", option):
                index += 1
                continue
            if option == "--":
                index += 1
                break
            if option in options_with_value:
                index += 2
                continue
            if any(option.startswith(f"{item}=") for item in options_with_value if item.startswith("--")):
                index += 1
                continue
            if option.startswith("-"):
                index += 1
                continue
            break
        if basename == "timeout" and index < len(tokens):
            # timeoutのdurationは実行commandではないため1 token読み飛ばす。
            index += 1
    return None


def _is_protected_branch(branch):
    branch = branch.removeprefix("refs/heads/")
    return branch in PROTECTED_BRANCHES or any(
        pattern.match(branch) for pattern in PROTECTED_BRANCH_PATTERNS
    )


def _valid_work_branch(branch):
    branch = branch.removeprefix("refs/heads/")
    if _is_protected_branch(branch) or not BRANCH_RE.fullmatch(branch):
        return False
    prefix = branch.split("/", 1)[0]
    return prefix in BRANCH_PREFIXES and prefix != "codex"


def _option_values(args, short, long):
    """短形式・長形式の指定値を列挙する。"""
    values = []
    index = 0
    while index < len(args):
        token = args[index]
        if token in {short, long}:
            if index + 1 >= len(args):
                return None
            values.append(args[index + 1])
            index += 2
            continue
        if token.startswith(f"{long}="):
            values.append(token.split("=", 1)[1])
        elif short and token.startswith(short) and token != short:
            values.append(token[len(short):])
        index += 1
    return values


def _git_add_reason(args):
    if "--" not in args:
        return "git add は -- の後に対象パスを明示してください"
    separator = args.index("--")
    options = args[:separator]
    paths = args[separator + 1:]
    if options or not paths:
        return "git add のオプション指定または空の対象は許可されていません"
    for path in paths:
        candidate = Path(path)
        if (
            candidate.is_absolute()
            or path in {".", ".."}
            or path.startswith(("-", ":"))
            or ".." in candidate.parts
        ):
            return "git add は個別のファイルまたはディレクトリだけを指定してください"
        if any(char in path for char in "*?[]"):
            return "git add でglobは使用できません"
    return None


def _git_read_reason(command, args, global_option_used):
    if global_option_used:
        return "読み取りGitではglobal optionを使用できません"
    for arg in args:
        if arg in GIT_READ_FORBIDDEN_ARGS or any(
            arg.startswith(f"{option}=") for option in GIT_READ_FORBIDDEN_ARGS if option.startswith("--")
        ):
            return "外部command実行またはfile書き込みを伴うGit optionは使用できません"
        if command == "grep" and arg.startswith("-O"):
            return "git grepの外部pager実行は許可されていません"
        if command == "grep" and arg.startswith("--open-files-in-pager"):
            return "git grepの外部pager実行は許可されていません"
    if command == "reflog" and args:
        operation = next((arg for arg in args if not arg.startswith("-")), None)
        if operation not in {"show", "list", "exists"}:
            return "git reflogはshow、list、existsだけ許可されます"
    return None


def _git_commit_reason(args):
    forbidden = {
        "--amend", "--no-verify", "--signoff", "-s", "--author", "--date",
        "--reset-author", "--fixup", "--squash", "--reuse-message", "-C",
        "--reedit-message", "-c", "--no-gpg-sign",
    }
    for arg in args:
        if arg in forbidden or any(
            arg.startswith(f"{option}=") for option in forbidden if option.startswith("--")
        ):
            return "履歴改変、検証回避、author・帰属の上書きは許可されていません"
    messages = _option_values(args, "-m", "--message")
    if messages is None or len(messages) != 1:
        return "commit messageは-mで1件だけ明示してください"
    message = messages[0]
    allowed_tokens = {"-m", "--message", "-S", "--gpg-sign"}
    index = 0
    while index < len(args):
        token = args[index]
        if token in {"-m", "--message"}:
            index += 2
            continue
        if token.startswith("--message=") or (token.startswith("-m") and token != "-m"):
            index += 1
            continue
        if token in allowed_tokens or token.startswith("--gpg-sign="):
            index += 1
            continue
        return "git commit ではmessageとDaikiの署名設定以外の引数を使用できません"
    if "\n" in message or not COMMIT_SUBJECT_RE.fullmatch(message):
        return "commit messageは':gitmoji: 短い要約'の1行形式にしてください"
    if AI_ATTRIBUTION_RE.search(message):
        return "CodexまたはOpenAIのAI帰属は記録できません"
    if _contains_secret(message):
        return "commit messageに秘密情報らしい値が含まれています"
    return None


def _current_work_branch_reason(cwd):
    current = _run_git(cwd, "rev-parse", "--abbrev-ref", "HEAD")
    if current is None or not _valid_work_branch(current.strip()):
        return "git add/commitは非保護の作業branch上だけで実行できます"
    return None


def _clean_worktree_reason(cwd, operation):
    status = _run_git(cwd, "status", "--porcelain=v1", "--untracked-files=all")
    if status is None or status.strip():
        return f"worktreeがcleanではないためgit {operation}を拒否しました"
    return None


def _git_fetch_reason(args):
    if len(args) != 2 or args[0] != "origin":
        return "git fetch はoriginと単一のbase branchを明示してください"
    base = args[1].removeprefix("refs/heads/")
    if base not in PROTECTED_BRANCHES and not any(
        pattern.match(base) for pattern in PROTECTED_BRANCH_PATTERNS
    ):
        return "git fetch のbase branchが許可範囲外です"
    return None


def _pull_preflight_reason(cwd, base):
    """既定保護branchのfast-forward同期に必要なlocal状態を検査する。"""
    if _origin_repository(cwd) is None:
        return "originのGitHub repositoryを確認できません"
    default_ref = _run_git(cwd, "symbolic-ref", "--short", "refs/remotes/origin/HEAD")
    expected_ref = f"origin/{base}"
    if default_ref is None or default_ref.strip() != expected_ref:
        return "git pullはoriginの既定保護branchだけを同期できます"
    current = _run_git(cwd, "rev-parse", "--abbrev-ref", "HEAD")
    if current is None or current.strip() != base:
        return "git pullのcurrent branchと同期対象が一致しません"
    upstream = _run_git(cwd, "rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}")
    if upstream is None or upstream.strip() != expected_ref:
        return "git pullのupstreamがoriginの既定保護branchと一致しません"
    clean_reason = _clean_worktree_reason(cwd, "pull")
    if clean_reason:
        return clean_reason
    if _run_git(cwd, "merge-base", "--is-ancestor", "HEAD", expected_ref) is None:
        return "local branchがoriginよりaheadまたはdivergedしているためgit pullを拒否しました"
    git_dir = _run_git(cwd, "rev-parse", "--git-dir")
    if git_dir is None:
        return "Gitの進行中操作を確認できないためgit pullを拒否しました"
    try:
        path = Path(git_dir.strip())
        if not path.is_absolute():
            path = Path(cwd) / path
        if any((path / name).exists() for name in (
            "MERGE_HEAD", "CHERRY_PICK_HEAD", "REVERT_HEAD", "rebase-apply",
            "rebase-merge", "sequencer",
        )):
            return "Gitの進行中操作があるためgit pullを拒否しました"
    except OSError:
        return "Gitの進行中操作を確認できないためgit pullを拒否しました"
    return None


def _git_pull_reason(args, cwd):
    required_options = [
        "--ff-only", "--no-rebase", "--no-autostash", "--no-recurse-submodules",
    ]
    if len(args) != len(required_options) + 2 or args[:4] != required_options or args[4] != "origin":
        return "git pullはfast-forward限定の正規形だけを使用してください"
    base = args[5].removeprefix("refs/heads/")
    if not _is_protected_branch(base):
        return "git pullの同期対象は保護branch許可リストに含まれる既定branchだけにしてください"
    return _pull_preflight_reason(cwd, base)


def _default_branch_switch_reason(args, cwd):
    """既定保護branchへの安全な切り替えに必要なlocal状態を検査する。"""
    base = args[0]
    default_ref = _run_git(cwd, "symbolic-ref", "--short", "refs/remotes/origin/HEAD")
    if default_ref is None or default_ref.strip() != f"origin/{base}":
        return "git switchの対象はoriginの既定保護branchと一致させてください"
    if _run_git(cwd, "rev-parse", "--verify", f"refs/heads/{base}") is None:
        return "git switchの対象local branchを確認できません"
    return None


def _git_switch_reason(args, cwd):
    if len(args) == 1 and args[0] in PROTECTED_BRANCHES:
        return _default_branch_switch_reason(args, cwd)
    if len(args) != 3 or args[0] not in {"-c", "--create"}:
        return "git switchは既定保護branchへの切り替えか新規作業branch作成だけ許可されます"
    branch, start = args[1], args[2]
    if not _valid_work_branch(branch):
        return "一般的なprefixを持つ非保護作業ブランチを指定してください"
    if not start.startswith("origin/") or not _is_protected_branch(start[7:]):
        return "作業ブランチはoriginのbase branchから作成してください"
    return None


def _git_worktree_read_reason(args):
    if args == ["list", "--porcelain", "-z"]:
        return None
    return "git worktreeは安定形式のlist照会だけ許可されます"


def _git_push_reason(args, cwd):
    if any(
        arg in {"--force", "-f", "--force-with-lease", "--delete", "-d", "--mirror", "--tags", "--all"}
        or arg.startswith("--force-with-lease=")
        for arg in args
    ):
        return "force、削除、mirror、tagまたは一括pushは許可されていません"
    if len(args) != 3 or args[0] not in {"-u", "--set-upstream"} or args[1] != "origin":
        return "pushはoriginへの単一作業ブランチを明示してください"
    refspec = args[2]
    if not refspec.startswith("HEAD:refs/heads/"):
        return "push元はHEAD、push先はrefs/heads/<branch>で明示してください"
    branch = refspec.removeprefix("HEAD:refs/heads/")
    if not _valid_work_branch(branch):
        return "保護ブランチまたは許可されていないprefixへのpushです"
    return _push_preflight_reason(cwd, branch)


def _git_write_target(session_cwd, explicit_cwd):
    """git -Cで明示されたmanaged worktreeを検証し、実行rootを返す。"""
    if explicit_cwd is None:
        if os.environ.get("CODEX_WORKTREE_MODE") == "single-checkout":
            return session_cwd, None
        session_root = _resolved_git_path(session_cwd, "--show-toplevel")
        session_common = _resolved_git_path(session_cwd, "--git-common-dir")
        if session_root is None or session_common is None:
            return None, "Git書き込みのsession repositoryを確認できません"
        main_root = session_common.parent
        if session_root == main_root:
            return None, "Git書き込みは専用managed worktreeで実行してください"
        reason = _managed_worktree_reason(main_root, session_common, session_root)
        if reason:
            return None, reason
        return str(session_root), None
    if not isinstance(session_cwd, str) or not session_cwd:
        return None, "Git書き込みのsession cwdを確認できません"
    candidate = Path(explicit_cwd)
    if not candidate.is_absolute() or ".." in candidate.parts:
        return None, "git -Cにはmanaged worktreeの絶対pathを指定してください"
    try:
        resolved = candidate.resolve(strict=True)
    except OSError:
        return None, "git -Cのpathを安全に解決できません"
    if candidate != resolved:
        return None, "git -Cではsymlinkまたは非正規pathを使用できません"

    session_root = _resolved_git_path(session_cwd, "--show-toplevel")
    session_common = _resolved_git_path(session_cwd, "--git-common-dir")
    target_root = _resolved_git_path(resolved, "--show-toplevel")
    target_common = _resolved_git_path(resolved, "--git-common-dir")
    if None in {session_root, session_common, target_root, target_common}:
        return None, "git -Cのrepositoryを確認できません"
    if resolved != target_root:
        return None, "git -Cにはmanaged worktree rootを指定してください"
    if session_common != target_common:
        return None, "git -CのworktreeがCodex sessionのrepositoryと一致しません"

    main_root = session_common.parent
    if session_root != main_root:
        return None, "linked worktree sessionから別worktreeへ書き込むことはできません"
    if target_root == main_root:
        return None, "git -Cは別のmanaged worktreeを明示するときだけ使用できます"
    reason = _managed_worktree_reason(main_root, session_common, target_root)
    if reason:
        return None, reason
    return str(target_root), None


def _git_invocation_reason(tokens, cwd=None):
    start = _command_start(tokens)
    if start is None or os.path.basename(tokens[start]) != "git":
        return None
    if tokens[start] != "git":
        return "Git commandはPATHからgitを直接実行してください"
    removed_environment = set()
    if start != 0 and tuple(tokens[:start]) == GIT_SANITIZED_WRAPPER:
        removed_environment.add("SSH_ASKPASS")
    elif start != 0:
        return "Git commandをwrapper、環境変数、cwd変更経由で実行できません"
    git_args = tokens[start + 1:]
    skip_next = False
    global_option_used = False
    other_global_option_used = False
    explicit_cwd = None
    for index, token in enumerate(git_args):
        if skip_next:
            skip_next = False
            continue
        if token in GIT_FORBIDDEN_GLOBAL_OPTS or any(
            token.startswith(f"{option}=") for option in GIT_FORBIDDEN_GLOBAL_OPTS
        ):
            return "gitの設定・aliasによるコマンド上書きは許可されていません"
        if token.startswith("-c") and token != "-C":
            return "git -cによる設定・alias上書きは許可されていません"
        if token == "-C":
            if explicit_cwd is not None or index + 1 >= len(git_args):
                return "git -Cはmanaged worktreeの絶対pathを1件だけ指定してください"
            explicit_cwd = git_args[index + 1]
            global_option_used = True
            skip_next = True
            continue
        if token in GIT_OPTS_WITH_VALUE:
            global_option_used = True
            other_global_option_used = True
            skip_next = True
            continue
        if any(token.startswith(f"{option}=") for option in GIT_OPTS_WITH_VALUE):
            global_option_used = True
            other_global_option_used = True
            continue
        if token.startswith("-"):
            global_option_used = True
            other_global_option_used = True
            continue

        args = git_args[index + 1:]
        if token == "branch":
            if any(_branch_arg_is_write(arg) for arg in args):
                return "git branchの変更操作は許可されていません"
            if "--list" in args or "-l" in args:
                return None
            branch_arg, _ = _first_command(args, BRANCH_READ_OPTS_WITH_VALUE)
            return None if branch_arg is None else "git branchは一覧・照会だけ許可されます"
        if token == "remote":
            return None if all(arg in REMOTE_READ_ARGS for arg in args) else "git remoteは照会だけ許可されます"
        if token == "worktree":
            return _git_worktree_read_reason(args)
        if token in GIT_READ_ONLY:
            return _git_read_reason(token, args, global_option_used)
        if token not in GIT_SAFE_WRITE:
            return "許可されていないGit書き込み操作です"
        if token in {"pull", "switch"} and os.environ.get("CODEX_WORKTREE_MODE") != "single-checkout":
            return "git pull/switchは明示的なsingle-checkout rollback時だけ使用できます"
        if other_global_option_used:
            return "Git書き込みではglobal optionや別repository指定を使用できません"
        effective_cwd, target_reason = _git_write_target(cwd, explicit_cwd)
        if target_reason:
            return target_reason
        if any(
            key in GIT_SANITIZED_ENVIRONMENT_KEYS - removed_environment
            or key.startswith(("GIT_CONFIG_KEY_", "GIT_CONFIG_VALUE_"))
            for key in os.environ
        ):
            return "Git書き込みではrepository、config、外部commandを変更する環境変数を使用できません"

        if token == "push":
            reason = _git_push_reason(args, effective_cwd)
        elif token == "pull":
            reason = _git_pull_reason(args, effective_cwd)
        elif token == "switch":
            reason = _git_switch_reason(args, effective_cwd)
        else:
            reason = {
                "add": _git_add_reason,
                "commit": _git_commit_reason,
                "fetch": _git_fetch_reason,
            }[token](args)
        if reason:
            return reason
        if token in {"add", "commit"}:
            branch_reason = _current_work_branch_reason(effective_cwd)
            if branch_reason:
                return branch_reason
        if token == "switch":
            clean_reason = _clean_worktree_reason(effective_cwd, "switch")
            if clean_reason:
                return clean_reason
        if token == "commit":
            return _staged_secret_reason(effective_cwd)
        return None
    return None


def _gh_api_is_write(args):
    mutation_flags = {"-f", "--raw-field", "-F", "--field", "--input"}
    index = 0
    while index < len(args):
        token = args[index]
        if token in mutation_flags or any(token.startswith(f"{flag}=") for flag in mutation_flags):
            return True
        if token.startswith("-f") and token != "-f" or token.startswith("-F") and token != "-F":
            return True
        if token in {"-X", "--method"}:
            if index + 1 >= len(args) or args[index + 1].upper() != "GET":
                return True
            index += 2
            continue
        if token.startswith("--method=") and token.split("=", 1)[1].upper() != "GET":
            return True
        if token.startswith("-X") and token != "-X" and token[2:].upper() != "GET":
            return True
        index += 1
    return False


def _required_option(args, short, long):
    values = _option_values(args, short, long)
    return values[0] if values is not None and len(values) == 1 else None


def _strict_gh_args(args, value_options, switch_options=frozenset()):
    """GitHub writeの引数を正規形へ限定し、positional引数を返す。"""
    short_options = {short for short, _ in value_options if short}
    long_options = {long for _, long in value_options}
    positional = []
    index = 0
    while index < len(args):
        token = args[index]
        if token in switch_options:
            index += 1
            continue
        if token in short_options or token in long_options:
            if index + 1 >= len(args):
                return None
            index += 2
            continue
        if any(token.startswith(f"{long}=") for long in long_options):
            index += 1
            continue
        if token.startswith("-"):
            return None
        positional.append(token)
        index += 1
    return positional


def _github_repository_from_url(remote):
    url = remote.strip().rstrip("/")
    for prefix in (
        "https://github.com/",
        "http://github.com/",
        "ssh://git@github.com/",
        "git@github.com:",
    ):
        if not url.startswith(prefix):
            continue
        repository = url.removeprefix(prefix)
        if repository.endswith(".git"):
            repository = repository[:-4]
        if re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository):
            return repository
    return None


def _origin_repository(cwd):
    remote = _run_git(cwd, "remote", "get-url", "origin")
    return None if remote is None else _github_repository_from_url(remote)


def _valid_remote_branch_name(branch):
    """ls-remoteの応答から安全に扱えるbranch名か確認する。"""
    return bool(
        branch
        and re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._/-]*", branch)
        and ".." not in branch
        and "//" not in branch
        and not branch.endswith((".", ".lock"))
        and "@{" not in branch
    )


def _remote_refs_snapshot(cwd, head=None):
    """originのdefault branchと任意headを1回のread-only照会で取得する。"""
    args = ["ls-remote", "--symref", "origin", "HEAD"]
    if head is not None:
        args.append(f"refs/heads/{head}")
    output = _run_git(cwd, *args)
    if output is None:
        return None

    lines = output.splitlines()
    expected_lines = 2 if head is None else 3
    if len(lines) != expected_lines:
        return None
    default_match = re.fullmatch(r"ref: refs/heads/([^\t]+)\tHEAD", lines[0])
    default_oid_match = re.fullmatch(r"([0-9a-fA-F]{40}|[0-9a-fA-F]{64})\tHEAD", lines[1])
    if (
        default_match is None
        or default_oid_match is None
        or not _valid_remote_branch_name(default_match.group(1))
    ):
        return None

    remote_head = None
    if head is not None:
        head_match = re.fullmatch(
            rf"([0-9a-fA-F]{{40}}|[0-9a-fA-F]{{64}})\trefs/heads/{re.escape(head)}",
            lines[2],
        )
        if head_match is None:
            return None
        remote_head = head_match.group(1)
    return f"origin/{default_match.group(1)}", default_oid_match.group(1), remote_head


def _repository_reason(repository, cwd):
    origin_repository = _origin_repository(cwd)
    if origin_repository is None:
        return "originのGitHub repositoryを確認できません"
    if repository.casefold() != origin_repository.casefold():
        return "GitHub書き込み先がcurrent repositoryのoriginと一致しません"
    return None


def _pr_create_reason(args, cwd):
    positional = _strict_gh_args(
        args,
        {
            ("-R", "--repo"),
            ("-B", "--base"),
            ("-H", "--head"),
            ("-t", "--title"),
            ("-F", "--body-file"),
        },
        {"--draft"},
    )
    if positional is None or positional:
        return "Draft PRでは許可された引数だけを正規形で指定してください"
    if "--draft" not in args:
        return "PRはDraftとして作成してください"
    if any(option in args for option in {"--fill", "--fill-first", "--fill-verbose", "--web", "--recover"}):
        return "PRのtitleとbodyは明示してください"
    repository = _required_option(args, "-R", "--repo")
    base = _required_option(args, "-B", "--base")
    head = _required_option(args, "-H", "--head")
    title = _required_option(args, "-t", "--title")
    body_file = _required_option(args, "-F", "--body-file")
    if not all((repository, base, head, title, body_file)):
        return "Draft PRにはrepo、base、head、title、body-fileを明示してください"
    if "/" not in repository or _is_protected_branch(head) or not _valid_work_branch(head):
        return "Draft PRのrepositoryまたはhead branchが許可範囲外です"
    repository_reason = _repository_reason(repository, cwd)
    if repository_reason:
        return repository_reason
    if not _is_protected_branch(base):
        return "Draft PRのbase branchが許可範囲外です"
    preflight_reason = _draft_pr_preflight_reason(cwd, base, head)
    if preflight_reason:
        return preflight_reason
    if AI_ATTRIBUTION_RE.search(title):
        return "PR titleにAI帰属を含めることはできません"
    if _contains_secret(title):
        return "PR titleに秘密情報らしい値が含まれています"
    return _file_secret_reason(body_file)


def _branch_worktree(cwd, head):
    """明示head branchを所有する登録済みworktreeを一意に解決する。"""
    output = _run_git(cwd, "worktree", "list", "--porcelain", "-z")
    if output is None:
        return None, "Draft PRの登録済みworktreeを確認できません"
    matches = []
    expected_branch = f"refs/heads/{head}"
    for raw_record in output.split("\0\0"):
        if not raw_record:
            continue
        fields = raw_record.split("\0")
        paths = [field[9:] for field in fields if field.startswith("worktree ")]
        branches = [field[7:] for field in fields if field.startswith("branch ")]
        if branches != [expected_branch]:
            continue
        if len(paths) != 1 or any(
            field == "prunable" or field.startswith("prunable ") for field in fields
        ):
            return None, "Draft PRのhead worktree登録が安全な状態ではありません"
        matches.append(paths[0])
    if len(matches) != 1:
        return None, "Draft PRのhead branchを所有するworktreeを一意に確認できません"

    candidate = Path(matches[0])
    if not candidate.is_absolute() or ".." in candidate.parts:
        return None, "Draft PRのhead worktree pathを安全に解決できません"
    try:
        resolved = candidate.resolve(strict=True)
    except OSError:
        return None, "Draft PRのhead worktree pathを安全に解決できません"
    if candidate != resolved:
        return None, "Draft PRのhead worktreeにsymlinkまたは非正規pathは使用できません"
    target_root = _resolved_git_path(resolved, "--show-toplevel")
    if target_root != resolved:
        return None, "Draft PRのhead worktree rootを確認できません"
    return str(resolved), None


def _draft_pr_preflight_reason(cwd, base, head):
    head_cwd, worktree_reason = _branch_worktree(cwd, head)
    if worktree_reason:
        return worktree_reason
    session_common = _resolved_git_path(cwd, "--git-common-dir")
    head_common = _resolved_git_path(head_cwd, "--git-common-dir")
    if session_common is None or session_common != head_common:
        return "Draft PRのhead worktreeがsession repositoryと一致しません"
    if _origin_repository(cwd) != _origin_repository(head_cwd):
        return "Draft PRのhead worktreeのoriginがsession repositoryと一致しません"

    current = _run_git(head_cwd, "rev-parse", "--abbrev-ref", "HEAD")
    local_head = _run_git(head_cwd, "rev-parse", "HEAD")
    if current is None or current.strip() != head or local_head is None:
        return "Draft PRのhead worktreeとbranchを確認できません"
    status = _run_git(head_cwd, "status", "--porcelain=v1", "--untracked-files=all")
    if status is None or status.strip():
        return "Draft PRのhead worktreeがcleanではありません"

    snapshot = _remote_refs_snapshot(head_cwd, head)
    if snapshot is None:
        return "Draft PRのremote refを安全に確認できません"
    remote_default_ref, remote_default_oid, remote_head = snapshot
    if remote_default_ref != f"origin/{base}":
        return "Draft PRのbaseはoriginのdefault branchと一致させてください"
    local_default_oid = _run_git(
        head_cwd, "rev-parse", "--verify", f"refs/remotes/{remote_default_ref}"
    )
    if (
        local_default_oid is None
        or local_default_oid.strip().casefold() != remote_default_oid.casefold()
    ):
        return "Draft PRのbase remote-tracking refがremoteと一致しません"
    if remote_head is None or local_head.strip().casefold() != remote_head.casefold():
        return "Draft PRのheadはpush済みのworktree HEADと一致させてください"
    return None


def _issue_write_reason(issue_command, issue_args, cwd):
    repository = _required_option(issue_args, "-R", "--repo")
    if not repository or "/" not in repository:
        return "Issue書き込みには対象repositoryを明示してください"
    repository_reason = _repository_reason(repository, cwd)
    if repository_reason:
        return repository_reason

    value_options = {("-R", "--repo"), ("-F", "--body-file")}
    if issue_command in {"create", "edit"}:
        value_options.add(("-t", "--title"))
    positional = _strict_gh_args(issue_args, value_options)
    if positional is None:
        return "Issue書き込みでは許可された引数だけを正規形で指定してください"
    if issue_command == "create" and positional:
        return "Issue作成ではpositional引数を使用できません"
    if issue_command in {"edit", "comment"} and (
        len(positional) != 1 or not positional[0].isdigit()
    ):
        return "Issueの編集・comment対象は単一の数値IDで指定してください"

    title = _required_option(issue_args, "-t", "--title")
    body_file = _required_option(issue_args, "-F", "--body-file")
    if issue_command == "create" and not (title and body_file):
        return "Issue作成にはtitleとbody-fileを明示してください"
    if issue_command == "comment" and not body_file:
        return "Issue commentにはbody-fileを明示してください"
    if issue_command == "edit" and not (title or body_file):
        return "Issue編集にはtitleまたはbody-fileを明示してください"
    if title and (_contains_secret(title) or AI_ATTRIBUTION_RE.search(title)):
        return "Issueへ秘密情報またはAI帰属を送信できません"
    if body_file:
        reason = _file_secret_reason(body_file)
        if reason:
            return reason.replace("PR body", "Issue body")
    return None


def _gh_invocation_reason(tokens, cwd=None):
    start = _command_start(tokens)
    if start is None or os.path.basename(tokens[start]) != "gh":
        return None
    if tokens[start] != "gh":
        return "GitHub commandはPATHからghを直接実行してください"
    if start != 0:
        return "GitHub commandをwrapper、環境変数、cwd変更経由で実行できません"
    if "--help" in tokens[start + 1:] or "-h" in tokens[start + 1:]:
        return None
    command, args = _first_command(tokens[start + 1:], GH_GLOBAL_OPTS_WITH_VALUE)
    if command is None:
        return None
    if command == "issue":
        issue_command, issue_args = _first_command(args, GH_GLOBAL_OPTS_WITH_VALUE)
        if issue_command is None or issue_command in {"list", "status", "view"}:
            return None
        if issue_command in {"create", "edit", "comment"}:
            if os.environ.get("GH_HOST", "github.com").casefold() != "github.com":
                return "GitHub書き込み先hostを環境変数で変更できません"
            if "--hostname" in tokens or any(
                token.startswith("--hostname=") for token in tokens
            ):
                return "GitHub書き込み先hostを変更できません"
            return _issue_write_reason(issue_command, issue_args, cwd)
        if issue_command == "develop":
            return "gh issue developはブランチを作成するため実行できません"
        if issue_command == "delete":
            return "GitHub Issueの削除は許可されていません"
        return "許可されていないGitHub Issue操作です"
    if command == "api":
        return "gh apiはGETの読み取り専用利用に限られます" if _gh_api_is_write(args) else None
    if command == "pr":
        subcommand, pr_args = _first_command(args, GH_GLOBAL_OPTS_WITH_VALUE)
        if subcommand == "create":
            if os.environ.get("GH_HOST", "github.com").casefold() != "github.com":
                return "GitHub書き込み先hostを環境変数で変更できません"
            if "--hostname" in tokens or any(
                token.startswith("--hostname=") for token in tokens
            ):
                return "GitHub書き込み先hostを変更できません"
            return _pr_create_reason(pr_args, cwd)
        if subcommand is None or subcommand in GH_READ_ONLY["pr"]:
            return None
        return "PRはDraft作成と読み取り専用操作だけ許可されます"
    if command in GH_READ_ONLY_TOP_LEVEL:
        return None
    if command in GH_READ_ONLY:
        subcommand, _ = _first_command(args, GH_GLOBAL_OPTS_WITH_VALUE)
        if subcommand is None or subcommand in GH_READ_ONLY[command]:
            return None
    return "許可されていないGitHub書き込み操作です"


def _worktree_helper_invocation_reason(tokens):
    start = _command_start(tokens)
    if start is None or os.path.basename(tokens[start]) != "codex-worktree":
        return None
    if tokens[start] != "codex-worktree" or start != 0:
        return "worktree helperはPATHから直接実行してください"
    args = tokens[start + 1:]
    if args in (["--help"], ["-h"]):
        return None
    if not args:
        return "worktree helperのsubcommandを指定してください"
    command, arguments = args[0], args[1:]
    if command == "list":
        return None if not arguments else "codex-worktree listに引数は指定できません"
    if command in {"doctor", "resume", "recover"}:
        if not arguments and command == "doctor":
            return None
        if len(arguments) != 2 or arguments[0] != "--task-id" or not TASK_ID_RE.fullmatch(arguments[1]):
            return f"codex-worktree {command}のtask IDを正規形で指定してください"
        return None
    if command != "create":
        return "許可されていないworktree helper操作です"

    values = {}
    index = 0
    while index < len(arguments):
        option = arguments[index]
        if option not in {"--branch", "--issue", "--task-id"} or option in values:
            return "codex-worktree createでは許可された引数だけを1回ずつ指定してください"
        if index + 1 >= len(arguments):
            return "codex-worktree createの引数が不足しています"
        values[option] = arguments[index + 1]
        index += 2
    if "--issue" in values and "--task-id" in values:
        return "Issue番号とtask IDは同時に指定できません"
    if "--issue" in values and (
        not values["--issue"].isdigit() or int(values["--issue"]) < 1
    ):
        return "Issue番号は1以上の整数にしてください"
    if "--task-id" in values and not TASK_ID_RE.fullmatch(values["--task-id"]):
        return "task IDが許可形式ではありません"
    if "--branch" in values and not _valid_work_branch(values["--branch"]):
        return "一般的なprefixを持つ非保護作業branchを指定してください"
    return None


def _contains_secret(text):
    return any(pattern.search(text) for pattern in SECRET_PATTERNS)


def _sensitive_path(path):
    candidate = Path(path)
    name = candidate.name.lower()
    if name in SENSITIVE_FILENAMES or any(name.endswith(suffix) for suffix in SENSITIVE_SUFFIXES):
        return True
    return name.startswith(".env.") and name not in {".env.example", ".env.sample", ".env.template"}


def _file_secret_reason(path):
    if path == "-":
        return "PR bodyは検査可能なファイルで指定してください"
    candidate = Path(path)
    try:
        if candidate.is_symlink():
            return "PR body fileにsymlinkは使用できません"
        resolved = candidate.resolve(strict=True)
        tmp_root = Path("/tmp").resolve(strict=True)
        metadata = resolved.stat()
        if not resolved.is_relative_to(tmp_root):
            return "PR body fileは/tmp配下の通常ファイルで指定してください"
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != os.getuid():
            return "PR body fileの種類または所有者を安全に確認できません"
        if metadata.st_size > MAX_BODY_FILE_BYTES:
            return "PR body fileが許容サイズを超えています"
        if _sensitive_path(resolved):
            return "PR body fileに機密ファイル名は使用できません"
        contents = resolved.read_text(encoding="utf-8")
    except (OSError, UnicodeError):
        return "PR body fileを安全に検査できません"
    if _contains_secret(contents):
        return "PR bodyに秘密情報らしい値が含まれています"
    if AI_ATTRIBUTION_RE.search(contents):
        return "PR bodyにCodexまたはOpenAIのAI帰属が含まれています"
    return None


def _run_git(cwd, *args):
    if not cwd:
        return None
    environment = os.environ.copy()
    for key in list(environment):
        if (
            key in GIT_SANITIZED_ENVIRONMENT_KEYS
            or key.startswith(("GIT_CONFIG_KEY_", "GIT_CONFIG_VALUE_"))
        ):
            environment.pop(key, None)
    process = None
    selector = None
    try:
        process = subprocess.Popen(
            ["git", *args],
            cwd=cwd,
            env=environment,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
        )
        if process.stdout is None:
            return None
        selector = selectors.DefaultSelector()
        selector.register(process.stdout, selectors.EVENT_READ)
        deadline = time.monotonic() + GIT_COMMAND_TIMEOUT_SECONDS
        chunks = []
        total = 0
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise subprocess.TimeoutExpired(["git", *args], GIT_COMMAND_TIMEOUT_SECONDS)
            if not selector.select(remaining):
                if process.poll() is None:
                    raise subprocess.TimeoutExpired(
                        ["git", *args], GIT_COMMAND_TIMEOUT_SECONDS
                    )
                break
            chunk = os.read(process.stdout.fileno(), 64 * 1024)
            if not chunk:
                break
            total += len(chunk)
            if total > MAX_GIT_OUTPUT_BYTES:
                return None
            chunks.append(chunk)
        remaining = max(0.01, deadline - time.monotonic())
        if process.wait(timeout=remaining) != 0:
            return None
        return b"".join(chunks).decode("utf-8")
    except (OSError, subprocess.SubprocessError):
        return None
    except UnicodeError:
        return None
    finally:
        if selector is not None:
            selector.close()
        if process is not None and process.poll() is None:
            process.kill()
            process.wait()


def _staged_secret_reason(cwd):
    names = _run_git(cwd, "diff", "--cached", "--name-only", "--diff-filter=ACMR")
    patch = _run_git(
        cwd, "diff", "--cached", "--no-ext-diff", "--no-textconv", "--unified=0", "--"
    )
    if names is None or patch is None:
        return "staged changesを検査できないためcommitを拒否しました"
    return _changes_secret_reason(names, patch, "staged changes")


def _changes_secret_reason(names, patch, label):
    if any(_sensitive_path(name) for name in names.splitlines()):
        return f"{label}に秘密情報を保持し得るファイルが含まれています"
    added_lines = "\n".join(
        line[1:] for line in patch.splitlines()
        if line.startswith("+") and not line.startswith("+++")
    )
    if _contains_secret(added_lines):
        return f"{label}に秘密情報らしい値が含まれています"
    if AI_ATTRIBUTION_RE.search(added_lines):
        return f"{label}にCodexまたはOpenAIのAI帰属が含まれています"
    return None


def _push_preflight_reason(cwd, branch):
    fetch_urls = _run_git(cwd, "remote", "get-url", "--all", "origin")
    push_urls = _run_git(cwd, "remote", "get-url", "--push", "--all", "origin")
    fetch_lines = [] if fetch_urls is None else fetch_urls.splitlines()
    push_lines = [] if push_urls is None else push_urls.splitlines()
    if len(fetch_lines) != 1 or len(push_lines) != 1:
        return "originのfetch/push先はそれぞれ1件だけ指定してください"
    fetch_repository = _github_repository_from_url(fetch_lines[0])
    push_repository = _github_repository_from_url(push_lines[0])
    if fetch_repository is None or push_repository != fetch_repository:
        return "originのpush先がcurrent GitHub repositoryと一致しません"

    current = _run_git(cwd, "rev-parse", "--abbrev-ref", "HEAD")
    if current is None or current.strip() != branch:
        return "current branchとpush先branchが一致しません"
    status = _run_git(cwd, "status", "--porcelain=v1", "--untracked-files=all")
    if status is None or status.strip():
        return "worktreeがcleanではないためpushを拒否しました"

    remote_target = _run_git(cwd, "rev-parse", "--verify", f"origin/{branch}")
    if remote_target is not None:
        base = f"origin/{branch}"
    else:
        default_ref = _run_git(cwd, "symbolic-ref", "--short", "refs/remotes/origin/HEAD")
        if default_ref is None:
            snapshot = _remote_refs_snapshot(cwd)
            if snapshot is None:
                return "未push commitのbaseを特定できません"
            default_ref, remote_default_oid, _ = snapshot
        else:
            remote_default_oid = None
        if not default_ref.strip().startswith("origin/"):
            return "未push commitのbaseを特定できません"
        default_branch = default_ref.strip().removeprefix("origin/")
        if not _is_protected_branch(default_branch):
            return "originのdefault branchが保護対象ではありません"
        local_default_oid = _run_git(
            cwd, "rev-parse", "--verify", f"refs/remotes/{default_ref.strip()}"
        )
        if local_default_oid is None:
            return "未push commitのbaseを特定できません"
        if (
            remote_default_oid is not None
            and local_default_oid.strip().casefold() != remote_default_oid.casefold()
        ):
            return "未push commitのbaseを特定できません"
        merge_base = _run_git(cwd, "merge-base", "HEAD", default_ref.strip())
        if merge_base is None:
            return "未push commitのbaseを特定できません"
        base = merge_base.strip()

    names = _run_git(cwd, "diff", "--name-only", "--diff-filter=ACMR", f"{base}..HEAD")
    patch = _run_git(
        cwd,
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--unified=0",
        f"{base}..HEAD",
        "--",
    )
    if names is None or patch is None:
        return "未push commitを検査できません"
    return _changes_secret_reason(names, patch, "未push commit")


def _nested_shell_commands(tokens):
    for index, token in enumerate(tokens):
        if os.path.basename(token) not in SHELLS:
            continue
        for option_index in range(index + 1, len(tokens)):
            option = tokens[option_index]
            if (
                option.startswith("-")
                and not option.startswith("--")
                and "c" in option[1:]
            ):
                if option_index + 1 < len(tokens):
                    yield tokens[option_index + 1]
                break
            if not option.startswith("-"):
                break


def _git_operation(tokens):
    start = _command_start(tokens)
    if start is None or os.path.basename(tokens[start]) != "git":
        return None
    command, _ = _first_command(
        tokens[start + 1:],
        GIT_OPTS_WITH_VALUE | GIT_FORBIDDEN_GLOBAL_OPTS,
    )
    return command


def _has_write_operation(tokens):
    git_operation = _git_operation(tokens)
    if git_operation in GIT_SAFE_WRITE:
        return True

    start = _command_start(tokens)
    if start is not None and os.path.basename(tokens[start]) == "codex-worktree":
        return len(tokens) > start + 1 and tokens[start + 1] in {"create", "recover"}

    if start is None or os.path.basename(tokens[start]) != "gh":
        return False
    command, args = _first_command(tokens[start + 1:], GH_GLOBAL_OPTS_WITH_VALUE)
    if command == "issue":
        subcommand, _ = _first_command(args, GH_GLOBAL_OPTS_WITH_VALUE)
        return subcommand in {"create", "edit", "comment"}
    if command == "pr":
        subcommand, _ = _first_command(args, GH_GLOBAL_OPTS_WITH_VALUE)
        return subcommand == "create"
    return False


def _shell_wraps_restricted_command(tokens):
    start = _command_start(tokens)
    if start is None or os.path.basename(tokens[start]) not in SHELLS:
        return False
    payload = " ".join(tokens[start + 1:])
    return re.search(
        r"(?:^|[\s;&|()])(?:git|gh|rm|rmdir|unlink|shred)(?:\s|$)",
        payload,
    ) is not None


def _command_segments(command):
    try:
        lexer = shlex.shlex(command, posix=True, punctuation_chars=SHELL_PUNCTUATION)
        # 改行をpunctuation tokenとして残し、quoted string内の改行だけは
        # 元のtoken内に保持する。
        lexer.whitespace = " \t\r"
        lexer.whitespace_split = True
        lexer.commenters = ""
        tokens = list(lexer)
    except ValueError:
        tokens = command.split()
    segment = []
    for token in tokens:
        if token and all(char in SHELL_PUNCTUATION for char in token):
            if segment:
                yield segment
                segment = []
            continue
        segment.append(token)
    if segment:
        yield segment


def blocked_reason(command, cwd=None, depth=0):
    """連結コマンドと入れ子shellを調べ、禁止操作の理由を返す。"""
    if depth > 3:
        return "入れ子が深いshellコマンドは安全性を確認できません"
    if "$(" in command or "`" in command:
        return "command substitutionを含むcommandは安全に検査できません"
    segments = list(_command_segments(command))
    has_write = any(_has_write_operation(tokens) for tokens in segments)
    if has_write and (depth > 0 or len(segments) != 1):
        return "Git/GitHub書き込みは単一の直接commandで実行してください"
    if has_write and ("$" in command or "`" in command):
        return "Git/GitHub書き込みでshell展開は使用できません"

    for tokens in segments:
        start = _command_start(tokens)
        if start is not None and any(char in tokens[start] for char in "$`"):
            return "shell展開で実行commandを決定する操作は許可されていません"
        if start is not None and os.path.basename(tokens[start]) in {"source", "."}:
            return "sourceによる未検査scriptの実行は許可されていません"
        if (
            start is not None
            and os.path.basename(tokens[start]) in SHELLS
            and not any(_nested_shell_commands(tokens))
        ):
            return "shellは検査可能な-c inline commandだけを実行できます"
        if (
            tokens
            and os.path.basename(tokens[0]) == "env"
            and any(
                token in {"-S", "--split-string"}
                or token.startswith("--split-string=")
                for token in tokens[1:]
            )
        ):
            return "envのsplit-stringは安全に解析できません"
        if _shell_wraps_restricted_command(tokens):
            return "shell経由のGit/GitHub/削除commandは実行できません"
        if start is not None and os.path.basename(tokens[start]) in DESTRUCTIVE_COMMANDS:
            return "削除コマンドは自動実行できません"
        if start is not None and os.path.basename(tokens[start]) == "find" and any(
            token in {"-delete", "-exec", "-execdir", "-ok", "-okdir"}
            or token.startswith(("-fprint", "-fprintf", "-fls"))
            for token in tokens[start + 1:]
        ):
            return "findによる削除、外部command実行、file書き込みは許可されていません"
        if start is not None and os.path.basename(tokens[start]) == "eval":
            nested = " ".join(tokens[start + 1:])
            if not nested:
                return "evalの実行内容を確認できません"
            reason = blocked_reason(nested, cwd, depth + 1)
            if reason:
                return reason
        reason = _git_invocation_reason(tokens, cwd)
        if reason:
            return reason
        reason = _gh_invocation_reason(tokens, cwd)
        if reason:
            return reason
        reason = _worktree_helper_invocation_reason(tokens)
        if reason:
            return reason
        for nested_command in _nested_shell_commands(tokens):
            reason = blocked_reason(nested_command, cwd, depth + 1)
            if reason:
                return reason
    return None


def _resolved_git_path(cwd, argument):
    value = _run_git(cwd, "rev-parse", "--path-format=absolute", argument)
    if value is None:
        return None
    try:
        return Path(value.strip()).resolve(strict=True)
    except OSError:
        return None


def _repository_key(repository):
    normalized = repository.casefold()
    owner, name = normalized.split("/", 1)
    return f"{len(owner)}-{owner}--{len(name)}-{name}"


def _managed_worktree_reason(repository_root, common_git_dir, worktree_root):
    repository = _origin_repository(repository_root)
    if repository is None or _origin_repository(worktree_root) != repository:
        return "managed worktreeのoriginがsession repositoryと一致しません"
    codex_home = Path(os.environ.get("CODEX_HOME", str(Path.home() / ".codex")))
    if not codex_home.is_absolute() or ".." in codex_home.parts:
        return "CODEX_HOMEを安全に解決できません"
    current = Path(codex_home.anchor)
    for part in codex_home.parts[1:]:
        current /= part
        try:
            metadata = current.lstat()
        except FileNotFoundError:
            break
        except OSError:
            return "CODEX_HOMEを安全に解決できません"
        if stat.S_ISLNK(metadata.st_mode):
            return "CODEX_HOMEのsymlink componentを許可できません"
        if current != codex_home and not stat.S_ISDIR(metadata.st_mode):
            return "CODEX_HOMEの親がdirectoryではありません"
    repository_key = _repository_key(repository)
    expected_worktree = codex_home / "worktrees" / repository_key / worktree_root.name
    if worktree_root != expected_worktree:
        return "requested cwdが登録対象のmanaged worktreeではありません"
    current = Path(expected_worktree.anchor)
    for part in expected_worktree.parts[1:]:
        current /= part
        try:
            metadata = current.lstat()
        except OSError:
            return "managed worktree pathを安全に解決できません"
        if stat.S_ISLNK(metadata.st_mode):
            return "managed worktree pathのsymlink componentを許可できません"
        if not stat.S_ISDIR(metadata.st_mode):
            return "managed worktree pathのcomponentがdirectoryではありません"
    try:
        managed_repository = (codex_home / "worktrees" / repository_key).resolve(strict=True)
        worktree_root = worktree_root.resolve(strict=True)
    except OSError:
        return "managed worktree pathを安全に解決できません"
    if worktree_root.parent != managed_repository or not TASK_ID_RE.fullmatch(worktree_root.name):
        return "requested cwdが登録対象のmanaged worktreeではありません"

    manifest_path = managed_repository / ".state" / f"{worktree_root.name}.json"
    try:
        metadata = manifest_path.lstat()
        if (
            manifest_path.is_symlink()
            or not stat.S_ISREG(metadata.st_mode)
            or metadata.st_uid != os.getuid()
            or metadata.st_mode & 0o077
            or metadata.st_size > MAX_MANIFEST_BYTES
        ):
            return "managed worktree manifestを安全に検査できません"
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, ValueError):
        return "managed worktree manifestを安全に検査できません"
    current = _run_git(worktree_root, "rev-parse", "--abbrev-ref", "HEAD")
    expected = {
        "version": 1,
        "status": "ready",
        "task_id": worktree_root.name,
        "repository": str(repository_root),
        "common_git_dir": str(common_git_dir),
        "github_name": repository,
        "worktree": str(worktree_root),
    }
    if (
        not isinstance(manifest, dict)
        or any(manifest.get(key) != value for key, value in expected.items())
        or current is None
        or manifest.get("branch") != current.strip()
    ):
        return "managed worktree manifestがcurrent repository状態と一致しません"
    registered = _run_git(repository_root, "worktree", "list", "--porcelain", "-z")
    if registered is None or f"worktree {worktree_root}\0" not in registered:
        return "requested cwdがGit worktreeとして登録されていません"
    return None


def _write_context_reason(command, session_cwd):
    segments = list(_command_segments(command))
    if not any(_has_write_operation(tokens) for tokens in segments):
        return None
    if not isinstance(session_cwd, str):
        return "Git/GitHub書き込みのsession cwdを確認できません"
    session_root = _resolved_git_path(session_cwd, "--show-toplevel")
    session_common = _resolved_git_path(session_cwd, "--git-common-dir")
    if None in {session_root, session_common}:
        return "Git/GitHub書き込みのrepository rootを確認できません"
    main_root = session_common.parent
    if session_root != main_root:
        reason = _managed_worktree_reason(main_root, session_common, session_root)
        if reason:
            return reason
    return None


def _deny(reason):
    reason = f"{reason}（PreToolUse hookが直接操作を拒否しました）。"
    json.dump({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": reason,
        }
    }, sys.stdout)
    print(reason, file=sys.stderr)


def main():
    try:
        data = json.load(sys.stdin)
        if not isinstance(data, dict):
            raise ValueError("hook input must be an object")
        tool_input = data.get("tool_input") or {}
        if not isinstance(tool_input, dict):
            raise ValueError("tool_input must be an object")
        command = tool_input.get("command") or tool_input.get("cmd") or ""
        session_cwd = data.get("cwd")
        if not isinstance(command, str) or not command.strip():
            raise ValueError("command is required")
        reason = _write_context_reason(command, session_cwd)
        if reason is None:
            reason = blocked_reason(command, session_cwd)
    except Exception:
        _deny("hook inputを安全に解析・検査できません")
        sys.exit(2)
    if reason:
        _deny(reason)
        sys.exit(2)
    sys.exit(0)


if __name__ == "__main__":
    main()

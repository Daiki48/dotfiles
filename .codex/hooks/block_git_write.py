#!/usr/bin/env python3
"""Codex PreToolUse hook: Git と GitHub CLI の直接呼び出しを制限する。

検出できる Git 呼び出しは許可済みの読み取り操作だけを通す。GitHub CLI は
Issue の管理操作と読み取り専用操作だけを許可し、それ以外を拒否する。

rules の forbidden は approval=never（exec 等）では評価されないため、
直接コマンドを早期拒否する多層防御として本 hook を用いる。任意のscriptや
外部programの内部処理までは解析できないため、sandboxやAGENTS.mdと併用する。
"""
import json
import os
import shlex
import sys

# Git は既知の書き込み操作を列挙せず、許可した読み取り専用操作だけを通す。
# これにより update-ref や alias など未知の変更経路も安全側で拒否できる。
GIT_READ_ONLY = {
    "status", "log", "diff", "show", "rev-parse", "blame", "shortlog",
    "describe", "reflog", "ls-files", "ls-tree", "cat-file", "rev-list",
    "merge-base", "name-rev", "grep",
}

# 対象リポジトリを指定するだけのグローバルオプションは読み取り時も許可する。
GIT_OPTS_WITH_VALUE = {"-C", "--git-dir", "--work-tree", "--namespace"}
GIT_FORBIDDEN_GLOBAL_OPTS = {"-c", "--config", "--config-env", "--exec-path"}

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

# Issue は外部記憶として Codex から更新できる。ブランチを作る develop と
# 破壊的な delete は含めない。
GH_ISSUE_ALLOWED = {
    "list", "status", "view", "create", "edit", "comment", "close",
    "reopen", "pin", "unpin", "lock", "unlock", "transfer",
}

# GitHub 上の状態を変えないサブコマンドだけを列挙する。未知のコマンドや
# extension は安全側に倒して拒否する。
GH_READ_ONLY = {
    "pr": {"list", "view", "status", "checks", "diff"},
    "run": {"list", "view", "watch", "download"},
    "repo": {"list", "view"},
    "release": {"list", "view", "download", "verify", "verify-asset"},
    "workflow": {"list", "view"},
    "label": {"list"},
    "cache": {"list"},
    "variable": {"list", "get"},
    "secret": {"list"},
    "ruleset": {"list", "view", "check"},
}
GH_READ_ONLY_TOP_LEVEL = {"status", "search"}
GH_GLOBAL_OPTS_WITH_VALUE = {"-R", "--repo", "--hostname"}
SHELLS = {"bash", "dash", "sh", "zsh"}
SHELL_PUNCTUATION = ";&|()`"


def _branch_arg_is_write(arg):
    """git branch の変更オプション（値を結合した形式を含む）なら True。"""
    if arg in BRANCH_WRITE_ARGS:
        return True
    if any(
        arg.startswith(f"{write_arg}=")
        for write_arg in BRANCH_WRITE_ARGS
        if write_arg.startswith("--")
    ):
        return True
    return len(arg) > 2 and arg[:2] in {"-d", "-D", "-m", "-M", "-c", "-C"}


def first_git_subcommand_is_write(tokens):
    """git 呼び出しが許可済みの読み取り用途でなければ True。"""
    for i, tok in enumerate(tokens):
        if tok == "git" or tok.endswith("/git"):
            skip_next = False
            git_args = tokens[i + 1:]
            for j, t in enumerate(git_args):
                if skip_next:
                    skip_next = False
                    continue
                if t in GIT_FORBIDDEN_GLOBAL_OPTS or any(
                    t.startswith(f"{option}=")
                    for option in GIT_FORBIDDEN_GLOBAL_OPTS
                ):
                    return True
                if t.startswith("-c") and t != "-C":
                    return True
                if t in GIT_OPTS_WITH_VALUE:
                    skip_next = True
                    continue
                if any(
                    t.startswith(f"{option}=") for option in GIT_OPTS_WITH_VALUE
                ):
                    continue
                if t.startswith("-"):
                    continue  # その他のグローバルオプションは読み飛ばす
                rest = git_args[j + 1:]
                if t == "branch":
                    if any(_branch_arg_is_write(arg) for arg in rest):
                        return True
                    if "--list" in rest or "-l" in rest:
                        return False
                    branch_arg, _ = _first_command(
                        rest, BRANCH_READ_OPTS_WITH_VALUE
                    )
                    return branch_arg is not None
                if t == "remote":
                    return any(arg not in REMOTE_READ_ARGS for arg in rest)
                return t not in GIT_READ_ONLY
    return False


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


def _gh_api_is_write(args):
    """gh api が GET 以外、または入力付きリクエストなら True。"""
    mutation_flags = {"-f", "--raw-field", "-F", "--field", "--input"}
    index = 0
    while index < len(args):
        token = args[index]
        if token in mutation_flags:
            return True
        if any(token.startswith(f"{flag}=") for flag in mutation_flags):
            return True
        if token.startswith("-f") and token != "-f":
            return True
        if token.startswith("-F") and token != "-F":
            return True
        if token in {"-X", "--method"}:
            if index + 1 >= len(args) or args[index + 1].upper() != "GET":
                return True
            index += 2
            continue
        if token.startswith("--method="):
            if token.split("=", 1)[1].upper() != "GET":
                return True
        elif token.startswith("-X") and token != "-X":
            if token[2:].upper() != "GET":
                return True
        index += 1
    return False


def gh_blocked_reason(tokens):
    """gh 呼び出しが許可範囲外なら拒否理由を返す。"""
    for index, token in enumerate(tokens):
        if token != "gh" and not token.endswith("/gh"):
            continue

        command, args = _first_command(
            tokens[index + 1:], GH_GLOBAL_OPTS_WITH_VALUE
        )
        if command is None:
            return None
        if command == "issue":
            issue_command, _ = _first_command(args, GH_GLOBAL_OPTS_WITH_VALUE)
            if issue_command is None or issue_command in GH_ISSUE_ALLOWED:
                return None
            if issue_command == "develop":
                return "gh issue develop はブランチを作成するため実行できません"
            if issue_command == "delete":
                return "GitHub Issue の削除は許可されていません"
            return "許可されていない GitHub Issue 操作です"
        if command == "api":
            if _gh_api_is_write(args):
                return "gh api は GET の読み取り専用利用に限られます"
            return None
        if command in GH_READ_ONLY_TOP_LEVEL:
            return None
        if command in GH_READ_ONLY:
            subcommand, _ = _first_command(args, GH_GLOBAL_OPTS_WITH_VALUE)
            if subcommand is None or subcommand in GH_READ_ONLY[command]:
                return None
        return "GitHub の書き込み操作は Issue 関連だけに制限されています"
    return None


def _nested_shell_commands(tokens):
    """bash -lc '...' などに埋め込まれたコマンド文字列を列挙する。"""
    for index, token in enumerate(tokens):
        if os.path.basename(token) not in SHELLS:
            continue
        for option_index in range(index + 1, len(tokens)):
            option = tokens[option_index]
            if option.startswith("-") and "c" in option[1:]:
                if option_index + 1 < len(tokens):
                    yield tokens[option_index + 1]
                break
            if not option.startswith("-"):
                break


def _command_segments(command):
    """shellの引用を保ったまま、連結されたコマンドをトークン列へ分ける。"""
    try:
        lexer = shlex.shlex(command, posix=True, punctuation_chars=SHELL_PUNCTUATION)
        lexer.whitespace_split = True
        lexer.commenters = ""
        tokens = list(lexer)
    except ValueError:
        # 引用符が不正なコマンドは通常shellでも失敗する。可能な範囲で検査を続ける。
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


def blocked_reason(command, depth=0):
    """連結コマンドと入れ子のshellを調べ、禁止操作の理由を返す。"""
    if depth > 3:
        return "入れ子が深いshellコマンドは安全性を確認できません"
    for tokens in _command_segments(command):
        if first_git_subcommand_is_write(tokens):
            return "Git の書き込み系操作は Daiki が手動で実施します"
        reason = gh_blocked_reason(tokens)
        if reason:
            return reason
        for nested_command in _nested_shell_commands(tokens):
            reason = blocked_reason(nested_command, depth + 1)
            if reason:
                return reason
    return None


def main():
    try:
        data = json.load(sys.stdin)
    except Exception:
        # 入力を解釈できない場合は素通し（git は rules / AGENTS.md でも二重防御）
        sys.exit(0)

    tool_input = data.get("tool_input") or {}
    command = tool_input.get("command") or tool_input.get("cmd") or ""

    reason = blocked_reason(command)
    if reason:
        reason = f"{reason}（PreToolUse hook が検出した直接操作を拒否しました）。"
        # deny を二重方式で表現する:
        #   - stdout の JSON（permissionDecision: deny） … Codex の JSON レスポンス方式
        #   - stderr の理由 + exit code 2               … Codex の exit code 方式（公式仕様: exit 2 = block）
        # exit 0 だと JSON が読まれなかった場合に素通り（fail-open）になるため、
        # どちらが優先評価されても、検出済み操作を拒否できるよう exit 2 にする。
        json.dump({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": reason,
            }
        }, sys.stdout)
        print(reason, file=sys.stderr)
        sys.exit(2)

    sys.exit(0)


if __name__ == "__main__":
    main()

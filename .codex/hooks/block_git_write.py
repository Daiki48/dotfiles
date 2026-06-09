#!/usr/bin/env python3
"""Codex PreToolUse hook: 危険コマンドをブロックするガードレール。

approval_policy=never でも評価される PreToolUse hook で、git の書き込み系
サブコマンド（push/pull/commit/...）と削除系コマンド（rm/rmdir）を deny する。
読み取り系 git（status/log/diff/...）と削除以外の一般コマンドは素通しする。

rules の forbidden は approval=never（exec 等）では評価されないため、
承認設定に依存しない確実なガードとして本 hook を用いる。
"""
import json
import re
import shlex
import sys

# 読み取り用法を持たない git 書き込み系サブコマンド
GIT_WRITE = {
    "push", "pull", "fetch", "commit", "merge", "rebase", "reset",
    "stash", "cherry-pick", "checkout", "switch", "restore", "clean",
    "revert", "add", "rm", "mv", "apply", "am", "tag", "gc", "prune",
    "config", "worktree", "submodule",
}

# 削除は全面禁止。退避が必要な場合は mv でリネームする。
DELETE_COMMANDS = {"rm", "rmdir"}

# `-c key=val` のように直後のトークンが値になるグローバルオプション
OPTS_WITH_VALUE = {"-c", "--config", "-C", "--git-dir", "--work-tree", "--namespace"}

BRANCH_READ_ARGS = {
    "-a", "--all", "-r", "--remotes", "-v", "-vv", "--verbose",
    "--list", "-l", "--show-current", "--contains", "--merged", "--no-merged",
}

REMOTE_READ_ARGS = {"-v", "--verbose"}


def first_git_subcommand_is_write(tokens):
    """トークン列に git 呼び出しがあり、最初のサブコマンドが書き込み系なら True。"""
    for i, tok in enumerate(tokens):
        if tok == "git" or tok.endswith("/git"):
            skip_next = False
            git_args = tokens[i + 1:]
            for j, t in enumerate(git_args):
                if skip_next:
                    skip_next = False
                    continue
                if t in OPTS_WITH_VALUE:
                    skip_next = True
                    continue
                if t.startswith("-"):
                    continue  # その他のグローバルオプションは読み飛ばす
                rest = git_args[j + 1:]
                if t == "branch":
                    return any(arg not in BRANCH_READ_ARGS for arg in rest)
                if t == "remote":
                    return any(arg not in REMOTE_READ_ARGS for arg in rest)
                return t in GIT_WRITE  # 最初の非オプション = サブコマンド
    return False


def first_command_is_delete(tokens):
    """最初の実行コマンドが削除系なら True。"""
    if not tokens:
        return False
    cmd = tokens[0]
    return cmd in DELETE_COMMANDS or cmd.endswith("/rm") or cmd.endswith("/rmdir")


def blocked_reason(command):
    """パイプ・連結・サブシェルで分割し、危険コマンドがあれば理由を返す。"""
    segments = re.split(r"[;\n|&()`]+|&&|\|\|", command)
    for seg in segments:
        seg = seg.strip()
        if not seg:
            continue
        try:
            tokens = shlex.split(seg)
        except ValueError:
            tokens = seg.split()
        if first_git_subcommand_is_write(tokens):
            return "Git の書き込み系操作は Daiki が手動で実施します"
        if first_command_is_delete(tokens):
            return "削除系コマンドは使用禁止です。必要なら mv でリネームして退避します"
    return None


def main():
    try:
        data = json.load(sys.stdin)
    except Exception:
        # 入力を解釈できない場合は素通し（git は rules / AGENTS.md でも二重防御）
        sys.exit(0)

    command = ((data.get("tool_input") or {}).get("command")) or ""

    reason = blocked_reason(command)
    if reason:
        reason = f"{reason}（PreToolUse hook によるガード。approval 設定に依存せず常時有効）。"
        # deny を二重方式で表現する:
        #   - stdout の JSON（permissionDecision: deny） … Codex の JSON レスポンス方式
        #   - stderr の理由 + exit code 2               … Codex の exit code 方式（公式仕様: exit 2 = block）
        # exit 0 だと JSON が読まれなかった場合に素通り（fail-open）になるため、
        # どちらが優先評価されても確実に拒否できるよう exit 2 で fail-close にする。
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

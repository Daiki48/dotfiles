#!/usr/bin/env python3
"""Codex PreToolUse hook: Git 書き込み系コマンドをブロックするガードレール。

approval_policy=never でも評価される PreToolUse hook で、git の書き込み系
サブコマンド（push/pull/commit/...）を含む Bash コマンドを deny する。
読み取り系 git（status/log/diff/...）と git 以外のコマンドは素通しする。

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
}

# `-c key=val` のように直後のトークンが値になるグローバルオプション
OPTS_WITH_VALUE = {"-c", "--config", "-C", "--git-dir", "--work-tree", "--namespace"}


def first_git_subcommand_is_write(tokens):
    """トークン列に git 呼び出しがあり、最初のサブコマンドが書き込み系なら True。"""
    for i, tok in enumerate(tokens):
        if tok == "git" or tok.endswith("/git"):
            skip_next = False
            for t in tokens[i + 1:]:
                if skip_next:
                    skip_next = False
                    continue
                if t in OPTS_WITH_VALUE:
                    skip_next = True
                    continue
                if t.startswith("-"):
                    continue  # その他のグローバルオプションは読み飛ばす
                return t in GIT_WRITE  # 最初の非オプション = サブコマンド
    return False


def is_blocked(command):
    """パイプ・連結・サブシェルで分割し、いずれかが git 書き込みなら True。"""
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
            return True
    return False


def main():
    try:
        data = json.load(sys.stdin)
    except Exception:
        # 入力を解釈できない場合は素通し（git は rules / AGENTS.md でも二重防御）
        sys.exit(0)

    command = ((data.get("tool_input") or {}).get("command")) or ""

    if is_blocked(command):
        reason = (
            "Git の書き込み系操作は Daiki が手動で実施します"
            "（PreToolUse hook によるガード。approval 設定に依存せず常時有効）。"
        )
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

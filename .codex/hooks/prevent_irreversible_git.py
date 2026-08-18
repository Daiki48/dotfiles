#!/usr/bin/env python3
"""Codexの不可逆な削除操作だけを拒否するPreToolUse hook。"""

import json
import re
import shlex
import sys


DELETION_COMMANDS = {"rm", "rmdir", "unlink", "shred"}
SHELL_SEPARATORS = {";", "&&", "||", "|", "\n"}
PROTECTED_PUSH_OPTIONS = {"--force", "-f", "--force-with-lease", "--delete", "-d", "--mirror", "--all", "--tags"}
GIT_OPTIONS_WITH_VALUE = {"-C", "--git-dir", "--work-tree", "--namespace"}


def _segments(command):
    lexer = shlex.shlex(command, posix=True, punctuation_chars=";&|\n")
    lexer.whitespace_split = True
    lexer.commenters = ""
    current = []
    for token in lexer:
        if token in SHELL_SEPARATORS:
            if current:
                yield current
                current = []
        else:
            current.append(token)
    if current:
        yield current


def _git_subcommand(tokens):
    index = 1
    while index < len(tokens):
        token = tokens[index]
        if token in GIT_OPTIONS_WITH_VALUE:
            index += 2
            continue
        if token.startswith("-"):
            index += 1
            continue
        return token, tokens[index + 1 :]
    return None, []


def blocked_reason(command):
    try:
        segments = list(_segments(command))
    except ValueError:
        return "commandを安全に解析できません"

    for tokens in segments:
        if not tokens:
            continue
        executable = tokens[0].rsplit("/", 1)[-1]
        if executable in DELETION_COMMANDS:
            return "ファイル削除操作は許可されていません"
        if executable != "git":
            continue

        subcommand, args = _git_subcommand(tokens)
        if subcommand in {"branch", "tag"} and any(
            arg in {"-d", "-D", "--delete"} for arg in args
        ):
            return f"git {subcommand}による削除は許可されていません"
        if subcommand == "push":
            if any(arg in PROTECTED_PUSH_OPTIONS for arg in args):
                return "force/delete/mirror/tag pushは許可されていません"
            if any(arg.startswith(":") for arg in args):
                return "削除refspecを使うpushは許可されていません"
    return None


def main():
    try:
        data = json.load(sys.stdin)
        tool_input = data.get("tool_input") or {}
        command = tool_input.get("command") or tool_input.get("cmd")
        if not isinstance(command, str) or not command.strip():
            raise ValueError("command is required")
        reason = blocked_reason(command)
    except Exception:
        reason = "hook inputを安全に解析できません"

    if reason:
        json.dump(
            {
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "deny",
                    "permissionDecisionReason": reason,
                }
            },
            sys.stdout,
        )
        sys.exit(2)


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Codexの不可逆な削除操作だけを拒否するPreToolUse hook。"""

import json
import shlex
import sys


DELETION_COMMANDS = {"rm", "rmdir", "unlink", "shred"}
COMMAND_WRAPPERS = {"command", "env", "exec", "sudo"}
SHELL_WRAPPERS = {"sh", "bash", "zsh", "dash", "ksh"}
SCHEDULE_WRAPPERS = {"nice", "timeout"}
WRAPPER_OPTIONS_WITH_VALUE = {
    "sudo": {"-u", "-g", "-h", "-p", "-C"},
    "env": {"-u", "-C"},
    "exec": {"-a", "--argv0"},
}
SHELL_SEPARATORS = {";", "&&", "||", "|", "\n"}
PROTECTED_PUSH_OPTIONS = {"--force", "-f", "--force-with-lease", "--delete", "-d", "--mirror", "--all", "--tags", "--prune"}
GIT_OPTIONS_WITH_VALUE = {"-C", "-c", "--config", "--git-dir", "--work-tree", "--namespace"}


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


def _has_temporary_git_config(tokens):
    index = 1
    while index < len(tokens):
        token = tokens[index]
        if token in {"-c", "--config", "--config-env"}:
            return True
        if token.startswith("-c") or token.startswith("--config=") or token.startswith("--config-env="):
            return True
        if token in GIT_OPTIONS_WITH_VALUE:
            index += 2
            continue
        if token.startswith("-"):
            index += 1
            continue
        return False
    return False


def _unwrap(tokens):
    index = 0
    while index < len(tokens):
        executable = tokens[index].rsplit("/", 1)[-1]
        if executable in SHELL_WRAPPERS:
            return executable, tokens[index:]
        if executable == "env" and "-S" in tokens[index:]:
            return executable, tokens[index:]
        if executable not in COMMAND_WRAPPERS:
            return executable, tokens[index:]
        index += 1
        options_with_value = WRAPPER_OPTIONS_WITH_VALUE.get(executable, set())
        while index < len(tokens):
            token = tokens[index]
            if token in options_with_value:
                index += 2
            elif token.startswith("-") or (executable == "env" and "=" in token):
                index += 1
            else:
                break
    return None, []


def _wrapped_command_reason(executable, tokens):
    if executable in SHELL_WRAPPERS:
        for command_index, token in enumerate(tokens[1:], start=1):
            if token == "--command" or (token.startswith("-") and not token.startswith("--") and "c" in token[1:]):
                if command_index + 1 >= len(tokens):
                    return "shell commandを安全に解析できません"
                return blocked_reason(tokens[command_index + 1])
        return None

    if executable == "env" and "-S" in tokens:
        command_index = tokens.index("-S")
        if command_index + 1 >= len(tokens):
            return "env -Sを安全に解析できません"
        return blocked_reason(tokens[command_index + 1])

    if executable == "nice":
        index = 1
        while index < len(tokens) and tokens[index].startswith("-"):
            index += 2 if tokens[index] in {"-n", "--adjustment"} else 1
        return blocked_reason(" ".join(tokens[index:])) if index < len(tokens) else None

    if executable == "timeout":
        index = 1
        while index < len(tokens) and tokens[index].startswith("-"):
            index += 2 if tokens[index] in {"-k", "--kill-after", "-s", "--signal"} else 1
        index += 1  # duration
        return blocked_reason(" ".join(tokens[index:])) if index < len(tokens) else None

    return None


def _is_deleting_refspec(argument):
    if argument.startswith("+"):
        return True
    if ":" not in argument:
        return False
    source, destination = argument.split(":", 1)
    return not source or not destination


def blocked_reason(command):
    try:
        segments = list(_segments(command))
    except ValueError:
        return "commandを安全に解析できません"

    for tokens in segments:
        if not tokens:
            continue
        if any(token.startswith("GIT_CONFIG_") and "=" in token for token in tokens):
            return "一時Git設定を伴う操作は許可されていません"
        executable, tokens = _unwrap(tokens)
        wrapped_reason = _wrapped_command_reason(executable, tokens)
        if wrapped_reason:
            return wrapped_reason
        if executable in DELETION_COMMANDS:
            return "ファイル削除操作は許可されていません"
        if executable != "git":
            continue

        if _has_temporary_git_config(tokens):
            return "一時Git設定を伴う操作は許可されていません"
        subcommand, args = _git_subcommand(tokens)
        if subcommand == "rm":
            return "git rmによるファイル削除は許可されていません"
        if subcommand in {"branch", "tag"} and any(
            arg in {"-d", "-D", "--delete"} or arg.startswith("--delete=")
            for arg in args
        ):
            return f"git {subcommand}による削除は許可されていません"
        if subcommand == "checkout" and "--" in args:
            return "作業中の変更を破棄するcheckoutは許可されていません"
        if subcommand == "stash" and any(arg in {"clear", "drop"} for arg in args):
            return "stash削除は許可されていません"
        if subcommand == "push":
            if any(
                arg in PROTECTED_PUSH_OPTIONS
                or any(
                    option.startswith("--") and arg.startswith(f"{option}=")
                    for option in PROTECTED_PUSH_OPTIONS
                )
                for arg in args
            ):
                return "force/delete/mirror/tag pushは許可されていません"
            if any(_is_deleting_refspec(arg) for arg in args):
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

#!/usr/bin/env python3
"""限定した1ファイルの抜粋をローカルgpt-ossへ渡す。探索・実行は委譲しない。"""

import argparse
from contextlib import contextmanager
import fcntl
import json
import os
from pathlib import Path
import re
import signal
import stat
import sys
import time
import tomllib
import urllib.error
import urllib.request

ENDPOINT = "http://127.0.0.1:11434/api/chat"
LINE_IDS = {"type": "array", "minItems": 1, "maxItems": 12,
            "items": {"type": "integer", "minimum": 1}}
SCHEMA = {
    "type": "object",
    "properties": {
        "abstain": {"type": "boolean"},
        "items": {
            "type": "array", "maxItems": 1,
            "items": {
                "type": "object",
                "properties": {"text": {"type": "string"}, "line_ids": LINE_IDS},
                "required": ["text", "line_ids"], "additionalProperties": False,
            },
        },
    },
    "required": ["abstain", "items"], "additionalProperties": False,
}
EXTRACT_SCHEMA = {
    "type": "object",
    "properties": {"abstain": {"type": "boolean"},
                   "line_ids": {**LINE_IDS, "minItems": 0}},
    "required": ["abstain", "line_ids"], "additionalProperties": False,
}
INSPECT_SCHEMA = {
    "type": "object", "properties": {
        "decisions": {"type": "array", "items": {"type": "boolean"}},
        "explanation": {"type": "string"},
        "evidence": {"type": "array", "items": {
            "type": "object", "properties": {"file": {"type": "string"}, "line": {"type": "integer"}},
            "required": ["file", "line"], "additionalProperties": False}},
    }, "required": ["decisions", "explanation", "evidence"], "additionalProperties": False,
}


class Fallback(Exception):
    pass


def load_settings():
    directory = Path(__file__).resolve().parent
    settings = tomllib.loads((directory / "settings.config").read_text())
    # 検証した小さい入力範囲を設定編集だけで無制限に広げない。
    limits = {"max_source_bytes": 8000, "max_lines": 160, "max_question_bytes": 1600}
    for name, maximum in limits.items():
        if type(settings.get(name)) is not int or not 1 <= settings[name] <= maximum:
            raise Fallback("invalid_settings")
    if settings.get("model") != "gpt-oss:20b" or settings.get("num_ctx") != 16384:
        raise Fallback("unsupported_model_or_context")
    for effort, (tokens, seconds) in {"low": (2048, 60), "medium": (4096, 90), "high": (8192, 180)}.items():
        budget = settings.get("efforts", {}).get(effort, {})
        for key, maximum in {"num_predict": tokens, "timeout_seconds": seconds}.items():
            if type(budget.get(key)) is not int or not 1 <= budget[key] <= maximum:
                raise Fallback("invalid_effort_budget")
    if settings.get("default_effort") not in {"low", "medium", "high"}:
        raise Fallback("invalid_default_effort")
    roles = json.loads((directory / "roles.json").read_text())
    return settings, roles


def select_effort(settings, role, requested=None):
    effort = requested or ("low" if role == "extract" else settings["default_effort"])
    if effort not in settings["efforts"]:
        raise Fallback("invalid_effort")
    return {**settings, **settings["efforts"][effort], "effort": effort}


def read_excerpt(cwd, filename, start, end, settings):
    if start < 1 or end < start or end - start + 1 > settings["max_lines"]:
        raise Fallback("range_too_large_or_invalid")
    root = cwd.resolve(strict=True)
    path = (root / filename).resolve(strict=True)
    if not path.is_relative_to(root) or not path.is_file():
        raise Fallback("file_outside_repository_or_not_regular")
    relative = path.relative_to(root)
    # 明示指定された資料だけを読む。秘密情報の最終選別は呼び出し元も行う。
    if any(part in {".git", ".ssh", ".aws", ".gnupg"} for part in relative.parts) or \
       path.name.startswith(".env") or path.name in {"auth.json", "credentials", "id_rsa", "id_ed25519"}:
        raise Fallback("sensitive_path")
    if path.stat().st_size > 8 * 1024 * 1024:
        raise Fallback("file_too_large")
    with path.open(encoding="utf-8") as source:
        selected = []
        for number, line in enumerate(source, 1):
            if number >= start:
                selected.append(line)
                if sum(len(item.encode()) for item in selected) > settings["max_source_bytes"]:
                    raise Fallback("excerpt_too_large")
            if number == end:
                break
    if len(selected) != end - start + 1:
        raise Fallback("range_not_present")
    excerpt = "".join(selected)
    if not excerpt.strip() or "\0" in excerpt:
        raise Fallback("empty_or_binary_excerpt")
    return str(relative), excerpt


def validate_question(role, question, excerpt, settings):
    if not question.strip() or len(question.encode()) > settings["max_question_bytes"]:
        raise Fallback("question_too_large_or_empty")
    # 実測で不安定だった構文解釈は推論前にLunaへ送る。安全境界ではなく品質上の保守的制限。
    if role in {"inspect", "summarize"} and re.search(r"toml|DocumentMut|引用符|クォート", question + "\n" + excerpt, re.IGNORECASE):
        raise Fallback("syntax_semantics_requires_luna")
    if role == "extract":
        identifiers = re.findall(r"[A-Za-z_][A-Za-z_0-9]*", question)
        if not identifiers:
            raise Fallback("identifier_required")
        if not any(re.search(r"\b" + re.escape(item) + r"\b", excerpt) for item in identifiers):
            raise Fallback("identifier_not_in_excerpt")


def acquire_lock():
    path = f"/tmp/codex-local-model-{os.getuid()}.lock"
    fd = os.open(path, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)
    try:
        info = os.fstat(fd)
        if info.st_uid != os.getuid() or not stat.S_ISREG(info.st_mode):
            raise Fallback("unsafe_lock")
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        return fd
    except BaseException:
        os.close(fd)
        raise


@contextmanager
def deadline(seconds):
    def stop(signum, _frame):
        raise Fallback("timeout" if signum == signal.SIGALRM else "cancelled")

    watched = (signal.SIGALRM, signal.SIGTERM, signal.SIGHUP)
    previous = {item: signal.getsignal(item) for item in watched}
    if signal.getitimer(signal.ITIMER_REAL) != (0.0, 0.0):
        raise Fallback("existing_timer")
    try:
        for item in watched:
            signal.signal(item, stop)
        signal.setitimer(signal.ITIMER_REAL, seconds)
        yield
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
        for item, handler in previous.items():
            signal.signal(item, handler)


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise Fallback("redirect_refused")


def source_lines(excerpt):
    # read_excerptの物理改行と一致させ、Unicode区切り文字を新しい行にしない。
    return excerpt.removesuffix("\n").split("\n")


def infer(role, question, excerpt, settings, roles, path="source", start=1):
    payload = {
        "model": settings["model"], "stream": False, "think": settings["effort"], "keep_alive": "10m",
        "messages": [] if role == "inspect" else [
            {"role": "system", "content": roles["system"] + "\n" + roles[role]},
            *roles[role + "_examples"],
            {"role": "user", "content": json.dumps(
                {"question": question,
                 "source": {str(i): line for i, line in enumerate(source_lines(excerpt), 1)}},
                ensure_ascii=False)},
        ],
        "format": EXTRACT_SCHEMA if role == "extract" else (INSPECT_SCHEMA if role == "inspect" else SCHEMA),
        "options": {"num_ctx": settings["num_ctx"], "num_predict": settings["num_predict"],
                    "temperature": 0, "seed": 0},
    }
    if role == "inspect":
        # 比較評価と同じ、行番号付きコード・真偽判定・説明の形式を維持する。
        payload["messages"] = [
            {"role": "system", "content": roles["inspect_system"]},
            {"role": "user", "content": json.dumps({"question": question, "sources": [{
                "file": path, "start": start, "end": start + len(source_lines(excerpt)) - 1,
                "code": "\n".join(f"{start + i}: {line}" for i, line in enumerate(source_lines(excerpt)))
            }]}, ensure_ascii=False)},
        ]
    # proxy環境変数とredirectを使わず、loopbackの固定endpointへだけ送る。
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
    request = urllib.request.Request(ENDPOINT, data=json.dumps(payload).encode(),
                                     headers={"Content-Type": "application/json"}, method="POST")
    with opener.open(request, timeout=settings["timeout_seconds"]) as response:
        raw = response.read(262145)
    if len(raw) > 262144:
        raise Fallback("response_too_large")
    result = json.loads(raw)
    if not isinstance(result, dict):
        raise Fallback("invalid_response_object")
    if result.get("done") is not True or result.get("done_reason") == "length":
        raise Fallback("incomplete_response")
    message = result.get("message", {})
    if not isinstance(message, dict):
        raise Fallback("invalid_message_object")
    if message.get("tool_calls"):
        raise Fallback("unexpected_tool_call")
    return json.loads(message["content"]), {
        "input_tokens": result.get("prompt_eval_count"), "output_tokens": result.get("eval_count")
    }


def validate_inspection(answer, excerpt, path, start, checks):
    if not isinstance(answer, dict) or set(answer) != {"decisions", "explanation", "evidence"}:
        raise Fallback("invalid_inspection_schema")
    decisions, explanation, refs = answer["decisions"], answer["explanation"], answer["evidence"]
    if not isinstance(decisions, list) or len(decisions) != checks or any(type(v) is not bool for v in decisions):
        raise Fallback("invalid_decisions")
    if not isinstance(explanation, str) or not explanation.strip() or len(explanation) > 1200:
        raise Fallback("invalid_explanation")
    if not isinstance(refs, list) or not 1 <= len(refs) <= 12:
        raise Fallback("invalid_evidence_count")
    lines = source_lines(excerpt)
    evidence = []
    for ref in refs:
        if not isinstance(ref, dict) or set(ref) != {"file", "line"} or ref["file"] != path or \
           type(ref["line"]) is not int or not start <= ref["line"] < start + len(lines):
            raise Fallback("invalid_evidence_reference")
        evidence.append({"source": f"{path}:{ref['line']}", "quote": lines[ref["line"] - start]})
    return [{"decisions": decisions, "text": explanation, "evidence": evidence}]


def validate_answer(answer, role, excerpt, path, start):
    field = "line_ids" if role == "extract" else "items"
    if not isinstance(answer, dict) or set(answer) != {"abstain", field}:
        raise Fallback("invalid_schema")
    if type(answer["abstain"]) is not bool or not isinstance(answer[field], list):
        raise Fallback("invalid_schema")
    if answer["abstain"]:
        raise Fallback("insufficient_evidence")
    items = [{"text": "", "line_ids": answer[field]}] if role == "extract" else answer[field]
    if len(items) != 1:
        raise Fallback("invalid_item_count")
    lines = source_lines(excerpt)
    findings = []
    for item in items:
        if not isinstance(item, dict) or set(item) != {"text", "line_ids"}:
            raise Fallback("invalid_item")
        text, ids = item["text"], item["line_ids"]
        if not isinstance(text, str) or len(text) > 600 or (role != "extract" and not text.strip()):
            raise Fallback("invalid_text")
        if role == "summarize":
            sentence = text.strip()
            endings = list(re.finditer(r"[。！？!?]|\.(?=\s|$)", sentence))
            if any(c in text for c in "\n\r\v\f\u2028\u2029") or len(endings) > 1 or \
               (endings and endings[0].end() != len(sentence)):
                raise Fallback("summary_not_one_sentence")
        if not isinstance(ids, list) or not 1 <= len(ids) <= 12 or \
           any(type(i) is not int or not 1 <= i <= len(lines) for i in ids) or len(set(ids)) != len(ids):
            raise Fallback("invalid_line_ids")
        evidence = [{"source": f"{path}:{start + i - 1}", "quote": lines[i - 1]} for i in sorted(ids)]
        findings.append({"text": text if role != "extract" else "\n".join(e["quote"] for e in evidence),
                         "evidence": evidence})
    return findings


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cwd", required=True, type=Path)
    parser.add_argument("--file", required=True)
    parser.add_argument("--start", required=True, type=int)
    parser.add_argument("--end", required=True, type=int)
    parser.add_argument("--role", required=True, choices=("extract", "summarize", "inspect"))
    parser.add_argument("--effort", choices=("low", "medium", "high"), help="省略時はextract=low、他は設定値medium")
    parser.add_argument("--checks", type=int, help="inspectで質問に列挙した真偽判定の数（1〜12）")
    parser.add_argument("--question", required=True)
    args = parser.parse_args()
    settings, roles = load_settings()
    settings = select_effort(settings, args.role, args.effort)
    if args.role == "inspect" and (args.checks is None or not 1 <= args.checks <= 12):
        raise Fallback("inspection_requires_1_to_12_checks")
    if not args.question.strip() or len(args.question.encode()) > settings["max_question_bytes"]:
        raise Fallback("question_too_large_or_empty")
    path, excerpt = read_excerpt(args.cwd, args.file, args.start, args.end, settings)
    validate_question(args.role, args.question, excerpt, settings)
    lock = acquire_lock()
    started = time.monotonic()
    try:
        with deadline(settings["timeout_seconds"]):
            answer, usage = infer(args.role, args.question, excerpt, settings, roles, path, args.start)
            findings = (validate_inspection(answer, excerpt, path, args.start, args.checks) if args.role == "inspect"
                        else validate_answer(answer, args.role, excerpt, path, args.start))
        print(json.dumps({"status": "ok", "model": settings["model"], "effort": settings["effort"], "role": args.role, "findings": findings,
                          "usage": usage, "elapsed_seconds": round(time.monotonic() - started, 2)},
                         ensure_ascii=False))
        return 0
    finally:
        os.close(lock)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (Fallback, OSError, ValueError, KeyError, TypeError) as error:
        # 生のHTTP応答や資料をエラーへ転載しない。
        reason = str(error) if isinstance(error, Fallback) else type(error).__name__
        print(json.dumps({"status": "fallback", "reason": reason,
                          "next": "Luna xhighのexplorerへ引き継ぐ。同じローカル依頼を再試行しない。"},
                         ensure_ascii=False))
        sys.exit(2)

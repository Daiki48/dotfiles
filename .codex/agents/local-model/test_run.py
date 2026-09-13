"""入力境界、原文の機械的再構成、推論前の振り分け、停止処理を検証する。"""

import json
import os
from pathlib import Path
import signal
import sys
import tempfile
import unittest
from unittest.mock import patch
import run
import activate


class LocalModelTests(unittest.TestCase):
    def test_unreliable_syntax_questions_route_before_inference(self):
        settings, _ = run.load_settings()
        for role in ('inspect', 'summarize'):
            with self.assertRaisesRegex(run.Fallback, 'syntax_semantics_requires_luna'):
                run.validate_question(role, 'キーの意味', 'parse::<DocumentMut>()', settings)
        run.validate_question('extract', 'PORTの行', 'TOML PORT = 3000', settings)

    def test_activation_preserves_checkout_and_original_link(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            home = root / 'codex-home'
            home.mkdir()
            original = root / 'original.md'
            original.write_text('original policy')
            source = root / 'task/.codex/agents/local-model'
            source.mkdir(parents=True)
            (source.parents[1] / 'AGENTS.md').write_text('new policy')
            for name in ['run.py', 'settings.config', 'roles.json']:
                (source / name).write_text('fixture')
            (home / 'AGENTS.md').symlink_to(original)
            self.assertEqual(activate.install(home, original, source)['status'], 'ready')
            result = activate.install(home, original, source, True)
            self.assertEqual((home / 'AGENTS.md').read_text(), 'new policy')
            self.assertEqual(original.read_text(), 'original policy')
            self.assertEqual(os.readlink(Path(result['backup']) / 'AGENTS.md'), str(original))
            self.assertEqual(activate.install(home, original, source, True)['status'], 'already_active')
            (home / 'AGENTS.override.md').write_text('override')
            with self.assertRaisesRegex(RuntimeError, 'global_override'):
                activate.install(home, original, source, True)

    def test_effort_defaults_and_explicit_override(self):
        settings, _ = run.load_settings()
        self.assertEqual(run.select_effort(settings, "extract")["effort"], "low")
        for role in ("summarize", "inspect"):
            self.assertEqual(run.select_effort(settings, role)["effort"], "medium")
        for effort, tokens, seconds in [("low", 2048, 60), ("medium", 4096, 90), ("high", 8192, 180)]:
            chosen = run.select_effort(settings, "inspect", effort)
            self.assertEqual((chosen["effort"], chosen["num_predict"], chosen["timeout_seconds"]),
                             (effort, tokens, seconds))
        with self.assertRaises(run.Fallback):
            run.select_effort(settings, "inspect", "xhigh")

    def test_inspect_validates_each_claim_and_reconstructs_evidence(self):
        answer = {"decisions": [True, False], "explanation": "戻り値は0。",
                  "evidence": [{"file": "x.py", "line": 21}]}
        findings = run.validate_inspection(answer, "if not items:\n    return 0\n", "x.py", 20, 2)
        self.assertEqual(findings[0]["evidence"][0]["source"], "x.py:21")
        for change in [{"decisions": [True]}, {"decisions": [True, 0]},
                       {"explanation": "a" * 1201}, {"evidence": [{"file": "other", "line": 21}]},
                       {"evidence": [{"file": "x.py", "line": 19}]}, {"evidence": []}]:
            with self.assertRaises(run.Fallback):
                run.validate_inspection({**answer, **change}, "if not items:\n    return 0\n", "x.py", 20, 2)

    def test_incomplete_response_and_tool_calls_are_rejected(self):
        settings, roles = run.load_settings()
        for effort in ("low", "medium", "high"):
            chosen = run.select_effort(settings, "inspect", effort)
            for payload in [{"done": True, "done_reason": "length"},
                            {"done": False}, [], {"done": True, "message": None},
                            {"done": True, "message": {"tool_calls": [{"name": "shell"}]}}]:
                with patch.object(run.urllib.request, "build_opener") as factory:
                    response = factory.return_value.open.return_value.__enter__.return_value
                    response.read.return_value = json.dumps(payload).encode()
                    with self.assertRaises(run.Fallback):
                        run.infer("inspect", "挙動", "return 0", chosen, roles)
                    body = json.loads(factory.return_value.open.call_args.args[0].data)
                    self.assertEqual(body["think"], effort)
                    self.assertEqual(body["model"], "gpt-oss:20b")
                    self.assertNotIn("tools", body)

    def test_extract_reconstructs_original_lines_and_numbers(self):
        result = run.validate_answer({"abstain": False, "line_ids": [2]}, "extract",
                                     "header\n  port = 3000\n", "config.toml", 40)
        self.assertEqual(result[0]["text"], "  port = 3000")
        self.assertEqual(result[0]["evidence"], [{"source": "config.toml:41", "quote": "  port = 3000"}])

    def test_summary_keeps_verbatim_evidence(self):
        result = run.validate_answer(
            {"abstain": False, "items": [{"text": "上限は100。", "line_ids": [1]}]},
            "summarize", "return min(x, 100)\n", "app.py", 12)
        self.assertEqual(result[0]["evidence"][0]["source"], "app.py:12")

    def test_unicode_separator_does_not_invent_source_lines(self):
        excerpt = "foo\u2028bar\vqux\nnext\n"
        self.assertEqual(run.source_lines(excerpt), ["foo\u2028bar\vqux", "next"])
        result = run.validate_answer({"abstain": False, "line_ids": [2]}, "extract", excerpt, "x", 7)
        self.assertEqual(result[0]["evidence"], [{"source": "x:8", "quote": "next"}])
        with self.assertRaises(run.Fallback):
            run.validate_answer({"abstain": False, "line_ids": [2]}, "extract", "foo\u2028bar\n", "x", 1)

    def test_summary_requires_one_sentence(self):
        for text in ["A。B。", "A\nB", "A. B.", "A！B", "A\u2028B"]:
            with self.subTest(text=text), self.assertRaisesRegex(run.Fallback, "summary_not_one_sentence"):
                run.validate_answer({"abstain": False, "items": [{"text": text, "line_ids": [1]}]},
                                    "summarize", "line\n", "f", 1)
        run.validate_answer({"abstain": False, "items": [{"text": "0.5が返る。", "line_ids": [1]}]},
                            "summarize", "return 0.5\n", "f", 1)

    def test_invalid_ids_fail_closed(self):
        for ids in [[], [0], [2], [1, 1], [True], ["1"]]:
            with self.subTest(ids=ids), self.assertRaises(run.Fallback):
                run.validate_answer({"abstain": False, "line_ids": ids}, "extract", "line\n", "f", 1)

    def test_no_evidence_is_not_success(self):
        with self.assertRaisesRegex(run.Fallback, "insufficient_evidence"):
            run.validate_answer({"abstain": True, "line_ids": []}, "extract", "line\n", "f", 1)

    def test_model_cannot_inject_quote_or_command(self):
        with self.assertRaises(run.Fallback):
            run.validate_answer({"abstain": False, "line_ids": [1], "command": "touch file"},
                                "extract", "line\n", "f", 1)

    def test_oversize_and_outside_paths_never_reach_inference(self):
        settings, _ = run.load_settings()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "large.txt").write_text("x" * 8001)
            (root / ".env").write_text("EXAMPLE=placeholder")
            for filename, start, end in [("large.txt", 1, 1), (".env", 1, 1),
                                         ("large.txt", 1, 161), ("../outside", 1, 1)]:
                argv = ["run.py", "--cwd", directory, "--file", filename,
                        "--start", str(start), "--end", str(end), "--role", "extract",
                        "--question", "設定値の行"]
                with self.subTest(filename=filename, end=end), patch.object(sys, "argv", argv), \
                     patch.object(run, "infer") as infer:
                    with self.assertRaises((run.Fallback, OSError)):
                        run.main()
                    infer.assert_not_called()

    def test_excerpt_is_not_silently_truncated(self):
        settings, _ = run.load_settings()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "small.txt").write_text("one\ntwo\n")
            with self.assertRaisesRegex(run.Fallback, "range_not_present"):
                run.read_excerpt(root, "small.txt", 1, 3, settings)
            path, excerpt = run.read_excerpt(root, "small.txt", 2, 2, settings)
            self.assertEqual((path, excerpt), ("small.txt", "two\n"))

    def test_unanchored_extraction_routes_before_inference(self):
        settings, _ = run.load_settings()
        with self.assertRaisesRegex(run.Fallback, "identifier_required"):
            run.validate_question("extract", "待受ポートの設定行", "PORT=8080", settings)
        with self.assertRaisesRegex(run.Fallback, "identifier_not_in_excerpt"):
            run.validate_question("extract", "DATABASE_URLの設定行", "PORT=8080", settings)
        run.validate_question("extract", "待受ポート(PORT)の設定行", "PORT=8080", settings)

    def test_request_has_no_tools_proxy_or_history(self):
        settings, roles = run.load_settings()
        settings = run.select_effort(settings, "extract")
        with patch.object(run.urllib.request, "build_opener") as factory:
            response = factory.return_value.open.return_value.__enter__.return_value
            response.read.return_value = json.dumps(
                {"done": True, "message": {"content": '{"abstain":false,"line_ids":[1]}'}}).encode()
            run.infer("extract", "portの行", "port=3000", settings, roles)
            request = factory.return_value.open.call_args.args[0]
            body = json.loads(request.data)
            self.assertEqual(request.full_url, "http://127.0.0.1:11434/api/chat")
            self.assertNotIn("tools", body)
            self.assertEqual(len(body["messages"]), len(roles["extract_examples"]) + 2)
            self.assertEqual(body["think"], "low")
            self.assertEqual(body["options"]["num_ctx"], 16384)
            self.assertEqual(factory.call_args.args[0].proxies, {})

    def test_redirect_is_refused(self):
        with self.assertRaises(run.Fallback):
            run.NoRedirect().redirect_request(None, None, 302, "", {}, "https://example.com")

    def test_timeout_and_termination_restore_handlers(self):
        for sig in (signal.SIGALRM, signal.SIGTERM, signal.SIGHUP):
            before = signal.getsignal(sig)
            with self.subTest(sig=sig), self.assertRaises(run.Fallback):
                with run.deadline(5):
                    os.kill(os.getpid(), sig)
            self.assertEqual(signal.getsignal(sig), before)
            self.assertEqual(signal.getitimer(signal.ITIMER_REAL), (0.0, 0.0))


if __name__ == "__main__":
    unittest.main()

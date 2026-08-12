#!/usr/bin/env python3
"""Codex開発ワークフローガードの単体テスト。"""

import importlib.util
from pathlib import Path
from unittest import mock
import tempfile
import unittest


MODULE_PATH = Path(__file__).with_name("block_git_write.py")
SPEC = importlib.util.spec_from_file_location("block_git_write", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
GUARD = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GUARD)


class GuardTest(unittest.TestCase):
    def assert_allowed(self, command, cwd=None):
        self.assertIsNone(GUARD.blocked_reason(command, cwd), command)

    def assert_blocked(self, command, cwd=None):
        self.assertIsNotNone(GUARD.blocked_reason(command, cwd), command)

    def test_git_read_only_is_allowed(self):
        for command in (
            "git status",
            "git diff --stat",
            "git branch",
            "git branch --contains main",
            "git branch --list 'feature/*'",
            "git remote -v",
            "git --version",
        ):
            with self.subTest(command=command):
                self.assert_allowed(command)

    def test_safe_git_writes_are_allowed(self):
        with (
            mock.patch.object(GUARD, "_staged_secret_reason", return_value=None),
            mock.patch.object(GUARD, "_push_preflight_reason", return_value=None),
        ):
            for command in (
                "git fetch origin main",
                "git switch -c feature/example origin/main",
                "git switch --create fix/example origin/master",
                "git add -- src/main.rs README.md",
                "git commit -m ':wrench: 設定を更新'",
                "git commit -S -m ':bug: Fix startup failure'",
                "git push -u origin HEAD:refs/heads/feature/example",
                "git push --set-upstream origin HEAD:refs/heads/refactor/example",
            ):
                with self.subTest(command=command):
                    self.assert_allowed(command, "/workspace")

    def test_unsafe_git_writes_are_blocked(self):
        for command in (
            "git add .",
            "git add -A",
            "git add -- .",
            "git add -- 'src/*'",
            "git commit --amend -m ':bug: 修正'",
            "git commit --no-verify -m ':bug: 修正'",
            "git commit --author 'Codex <bot@example.com>' -m ':bug: 修正'",
            "git commit -m 'Fix config'",
            "git commit -m ':bug: 修正\n\nCo-authored-by: Codex <bot@example.com>'",
            "git fetch --prune origin main",
            "git switch main",
            "git switch -c codex/example origin/main",
            "git switch -c feature/example origin/feature/base",
            "git push origin main",
            "git push -u origin HEAD:refs/heads/main",
            "git push --force-with-lease origin feature/example",
            "git push --delete origin feature/example",
            "git push --tags",
            "git merge origin/main",
            "git rebase origin/main",
            "git reset --hard HEAD~1",
            "git branch -D feature/example",
            "git update-ref refs/heads/main HEAD",
            "git -c alias.save=commit save -m test",
            "git -C /workspace add -- README.md",
            "env -u TOKEN git reset --hard HEAD",
            "sudo -u root git push -u origin HEAD:refs/heads/main",
        ):
            with self.subTest(command=command):
                self.assert_blocked(command, "/workspace")

    def test_command_position_avoids_git_path_false_positive(self):
        for command in (
            "rg --files /tmp/example/git",
            "printf '%s' /usr/bin/git",
            "python3 tool.py --path /usr/bin/git",
        ):
            with self.subTest(command=command):
                self.assert_allowed(command)

    def test_nested_shell_and_chain_are_checked(self):
        for command in (
            "zsh -lc 'git reset --hard HEAD'",
            "zsh -lc 'git status && git push -u origin HEAD:refs/heads/main'",
            "echo checked; git merge origin/main",
            "bash -lc 'rm -rf build'",
            "env -u TOKEN rm -rf build",
        ):
            with self.subTest(command=command):
                self.assert_blocked(command)

    def test_destructive_commands_are_blocked(self):
        for command in (
            "rm file.txt",
            "rm -rf build",
            "rmdir empty",
            "unlink link",
            "shred secret.txt",
            "sudo rm file.txt",
        ):
            with self.subTest(command=command):
                self.assert_blocked(command)

    def test_issue_management_and_github_reads_are_allowed(self):
        for command in (
            "gh issue list",
            "gh issue view 123",
            "gh issue create --repo owner/repo --title test --body body",
            "gh issue comment 123 --repo owner/repo --body progress",
            "gh -R owner/repo issue view 123",
            "gh pr view 123",
            "gh pr --repo owner/repo diff 123",
            "gh run view 456 --log",
            "gh api repos/owner/repo/issues/123 -X GET",
            "gh pr create --help",
        ):
            with self.subTest(command=command):
                self.assert_allowed(command)

    def test_draft_pr_with_explicit_fields_is_allowed(self):
        with tempfile.NamedTemporaryFile(mode="w", encoding="utf-8") as body:
            body.write("## 概要\n\n設定を更新します。\n")
            body.flush()
            command = (
                "gh pr create --draft --repo owner/repo --base main "
                "--head feature/example --title ':wrench: 設定を更新' "
                f"--body-file {body.name}"
            )
            self.assert_allowed(command)

    def test_unsafe_pr_and_github_writes_are_blocked(self):
        with tempfile.NamedTemporaryFile(mode="w", encoding="utf-8") as body:
            body.write("## 概要\n")
            body.flush()
            for command in (
                f"gh pr create --repo owner/repo --base main --head feature/example --title test --body-file {body.name}",
                f"gh pr create --draft --repo owner/repo --base main --head main --title test --body-file {body.name}",
                "gh pr create --draft --fill",
                "gh pr merge 123 --squash",
                "gh pr ready 123",
                "gh repo create owner/repo",
                "gh workflow run deploy.yml",
                "gh release create v1.0.0",
                "gh api repos/owner/repo -X PATCH -f name=test",
                "gh issue develop 123 --name feature/test",
                "gh issue delete 123 --yes",
                "gh issue create --title test --body body",
                "gh issue close 123 --repo owner/repo",
            ):
                with self.subTest(command=command):
                    self.assert_blocked(command)

    def test_secret_and_ai_attribution_detection(self):
        github_token = "github_" + "pat_" + "A" * 24
        openai_key = "sk-" + "B" * 24
        self.assertTrue(GUARD._contains_secret(github_token))
        self.assertTrue(GUARD._contains_secret(openai_key))
        self.assertFalse(GUARD._contains_secret("sk-example-placeholder"))
        self.assertIsNotNone(
            GUARD.AI_ATTRIBUTION_RE.search(
                "Co-authored-by: Codex <bot@example.com>"
            )
        )

    def test_sensitive_paths_are_detected_without_blocking_templates(self):
        for path in (".env", ".env.local", "auth.json", "id_ed25519", "cert.pem"):
            with self.subTest(path=path):
                self.assertTrue(GUARD._sensitive_path(path))
        for path in (".env.example", ".env.sample", "docs/auth.json.example"):
            with self.subTest(path=path):
                self.assertFalse(GUARD._sensitive_path(path))

    def test_staged_scan_fails_closed_and_detects_added_secrets(self):
        github_token = "github_" + "pat_" + "C" * 24
        with mock.patch.object(GUARD, "_run_git", return_value=None):
            self.assertIsNotNone(GUARD._staged_secret_reason("/workspace"))
        with mock.patch.object(
            GUARD,
            "_run_git",
            side_effect=["src/main.rs\n", f"diff --git a/x b/x\n+TOKEN={github_token}\n"],
        ):
            self.assertIsNotNone(GUARD._staged_secret_reason("/workspace"))
        with mock.patch.object(
            GUARD,
            "_run_git",
            side_effect=["src/main.rs\n", "diff --git a/x b/x\n+safe = true\n"],
        ):
            self.assertIsNone(GUARD._staged_secret_reason("/workspace"))

    def test_push_preflight_checks_current_branch_and_new_commits(self):
        with mock.patch.object(GUARD, "_run_git", return_value="main\n"):
            self.assertIsNotNone(
                GUARD._push_preflight_reason("/workspace", "feature/example")
            )

        def clean_git(_cwd, *args):
            if args[:3] == ("rev-parse", "--abbrev-ref", "HEAD"):
                return "feature/example\n"
            if args[:3] == ("rev-parse", "--verify", "origin/feature/example"):
                return None
            if args[:2] == ("merge-base", "HEAD"):
                return "base123\n" if args[2] == "origin/main" else None
            if args[:2] == ("rev-list", "--count"):
                return "2\n"
            if args[:3] == ("diff", "--name-only", "--diff-filter=ACMR"):
                return "src/main.rs\n"
            if args[:3] == ("diff", "--no-ext-diff", "--unified=0"):
                return "diff --git a/x b/x\n+safe = true\n"
            return None

        with mock.patch.object(GUARD, "_run_git", side_effect=clean_git):
            self.assertIsNone(
                GUARD._push_preflight_reason("/workspace", "feature/example")
            )

        github_token = "github_" + "pat_" + "D" * 24

        def secret_git(cwd, *args):
            result = clean_git(cwd, *args)
            if args[:3] == ("diff", "--no-ext-diff", "--unified=0"):
                return f"diff --git a/x b/x\n+TOKEN={github_token}\n"
            return result

        with mock.patch.object(GUARD, "_run_git", side_effect=secret_git):
            self.assertIsNotNone(
                GUARD._push_preflight_reason("/workspace", "feature/example")
            )


if __name__ == "__main__":
    unittest.main()

#!/usr/bin/env python3
"""Git / GitHub CLI ガードレールの単体テスト。"""

import importlib.util
from pathlib import Path
import unittest


MODULE_PATH = Path(__file__).with_name("block_git_write.py")
SPEC = importlib.util.spec_from_file_location("block_git_write", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
GUARD = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GUARD)


class GuardTest(unittest.TestCase):
    def assert_allowed(self, command):
        self.assertIsNone(GUARD.blocked_reason(command), command)

    def assert_blocked(self, command):
        self.assertIsNotNone(GUARD.blocked_reason(command), command)

    def test_git_read_only_is_allowed(self):
        for command in (
            "git status",
            "git diff --stat",
            "git branch",
            "git branch --contains main",
            "git branch --list 'feature/*'",
            "git remote -v",
        ):
            with self.subTest(command=command):
                self.assert_allowed(command)

    def test_git_write_is_blocked(self):
        for command in (
            "git commit -m test",
            "git switch -c feature/test",
            "git branch feature/test",
            "git branch -D feature/test",
            "git branch --set-upstream-to=origin/main",
            "git branch -mrenamed",
            "git update-ref refs/heads/main HEAD",
            "git -c alias.save=commit save -m test",
            "git push origin main",
            "zsh -lc 'git commit -m test'",
            "zsh -lc 'git status && git commit -m test'",
            "echo checked;git commit -m test",
        ):
            with self.subTest(command=command):
                self.assert_blocked(command)

    def test_issue_management_is_allowed(self):
        for command in (
            "gh issue list",
            "gh issue view 123",
            "gh issue create --title test --body body",
            "gh issue edit 123 --add-label bug",
            "gh issue comment 123 --body progress",
            "gh issue close 123",
            "gh -R owner/repo issue reopen 123",
            "gh issue --repo owner/repo transfer 123 target/repo",
        ):
            with self.subTest(command=command):
                self.assert_allowed(command)

    def test_issue_branch_and_delete_are_blocked(self):
        self.assert_blocked("gh issue develop 123 --name feature/test")
        self.assert_blocked("gh issue delete 123 --yes")

    def test_github_read_only_is_allowed(self):
        for command in (
            "gh pr view 123",
            "gh pr --repo owner/repo diff 123",
            "gh run view 456 --log",
            "gh release view v1.0.0",
            "gh api repos/owner/repo/issues/123",
            "gh api repos/owner/repo -X GET",
        ):
            with self.subTest(command=command):
                self.assert_allowed(command)

    def test_non_issue_github_write_is_blocked(self):
        for command in (
            "gh pr create --title test",
            "gh pr merge 123 --squash",
            "gh repo create owner/repo",
            "gh workflow run deploy.yml",
            "gh release create v1.0.0",
            "gh api repos/owner/repo -X PATCH -f name=test",
            "gh api repos/owner/repo/issues -f title=test",
            "bash -lc 'gh pr merge 123'",
            "bash -lc 'gh pr view 123 && gh pr merge 123'",
            "echo checked;gh pr merge 123",
        ):
            with self.subTest(command=command):
                self.assert_blocked(command)


if __name__ == "__main__":
    unittest.main()

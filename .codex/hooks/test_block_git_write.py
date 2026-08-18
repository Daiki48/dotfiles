#!/usr/bin/env python3
"""Codex開発ワークフローガードの単体テスト。"""

import importlib.util
import io
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
            "git rev-parse --abbrev-ref --symbolic-full-name '@{upstream}'",
            "git --version",
        ):
            with self.subTest(command=command):
                self.assert_allowed(command)

    def test_safe_git_writes_are_allowed(self):
        with (
            mock.patch.object(GUARD, "_staged_secret_reason", return_value=None),
            mock.patch.object(GUARD, "_push_preflight_reason", return_value=None),
            mock.patch.object(GUARD, "_pull_preflight_reason", return_value=None),
            mock.patch.object(GUARD, "_default_branch_switch_reason", return_value=None),
            mock.patch.object(GUARD, "_current_work_branch_reason", return_value=None),
            mock.patch.object(GUARD, "_clean_worktree_reason", return_value=None),
        ):
            for command in (
                "git fetch origin main",
                "git pull --ff-only --no-rebase --no-autostash --no-recurse-submodules origin main",
                "git switch main",
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

    def test_git_add_rejects_repository_wide_pathspecs(self):
        for command in (
            "git add -- :/",
            "git add -- :(glob)**",
            "git add -- src/..",
            "git add -- /workspace/file.txt",
        ):
            with self.subTest(command=command):
                self.assert_blocked(command, "/workspace")

        with mock.patch.dict(GUARD.os.environ, {"GIT_DIR": "/tmp/other/.git"}):
            self.assert_blocked("git add -- README.md", "/workspace")

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
            "git pull origin main",
            "git pull --rebase origin main",
            "git pull --ff-only origin main",
            "git pull --ff-only --no-rebase --no-autostash --no-recurse-submodules origin feature/example",
            "git pull --ff-only --no-rebase --no-autostash --no-recurse-submodules origin stable",
            "git switch -c codex/example origin/main",
            "git switch -c feature/example origin/feature/base",
            "git switch feature/example",
            "git switch release/2026-08",
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
            "git symbolic-ref --delete refs/remotes/origin/HEAD",
            "git symbolic-ref refs/remotes/origin/HEAD",
            "git -c alias.save=commit save -m test",
            "git -C /workspace add -- README.md",
            "env -u TOKEN git reset --hard HEAD",
            "sudo -u root git push -u origin HEAD:refs/heads/main",
            "exec rm README.md",
            "nice rm README.md",
            "timeout 1 rm README.md",
            "xargs rm",
            "find . -delete",
            "find . -exec rm README.md ;",
            "x=rm; \"$x\" README.md",
            "sh -c 'x=rm; \"$x\" README.md'",
            "x=gh; \"$x\" issue comment 9 --repo attacker/repo",
            "eval 'rm README.md'",
            "printf 'rm README.md\\n' | bash",
            "bash /tmp/payload.sh",
            "bash --norc /tmp/payload.sh",
            "bash --restricted /tmp/payload.sh",
            "bash --rcfile /tmp/payload.sh -i",
            "source /tmp/payload.sh",
            ". /tmp/payload.sh",
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
            "zsh -lc 'git status\ngit commit -m \":bug: 修正\"'",
            "echo checked; git merge origin/main",
            "git status\ngit commit -m ':bug: 修正'",
            "git status\r\ngit commit -m ':bug: 修正'",
            "bash -lc 'rm -rf build'",
            "bash -O extglob -c 'git reset --hard HEAD'",
            "env -S 'git reset --hard HEAD'",
            "env --split-string='rm -rf build'",
            "cd /tmp/other && git push -u origin HEAD:refs/heads/feature/example",
            "env -C /tmp/other git push -u origin HEAD:refs/heads/feature/example",
            "GIT_DIR=/tmp/other/.git git push -u origin HEAD:refs/heads/feature/example",
            "env -u TOKEN rm -rf build",
        ):
            with self.subTest(command=command):
                self.assert_blocked(command)

    def test_newline_separated_read_commands_are_allowed(self):
        for command in (
            "git branch -a\ngit log --oneline -3",
            "git status\r\ngit diff --stat",
            "printf '%s' 'git status\ngit commit -m unsafe'",
        ):
            with self.subTest(command=command):
                self.assert_allowed(command)

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
            "gh -R owner/repo issue view 123",
            "gh pr view 123",
            "gh pr --repo owner/repo diff 123",
            "gh run view 456 --log",
            "gh api repos/owner/repo/issues/123 -X GET",
            "gh pr create --help",
        ):
            with self.subTest(command=command):
                self.assert_allowed(command)

        with (
            tempfile.NamedTemporaryFile(mode="w", encoding="utf-8") as body,
            mock.patch.object(GUARD, "_origin_repository", return_value="owner/repo"),
        ):
            body.write("## 計画\n")
            body.flush()
            for command in (
                f"gh issue create --repo owner/repo --title test --body-file {body.name}",
                f"gh issue comment 123 --repo owner/repo --body-file {body.name}",
            ):
                with self.subTest(command=command):
                    self.assert_allowed(command, "/workspace")

    def test_draft_pr_with_explicit_fields_is_allowed(self):
        with tempfile.NamedTemporaryFile(mode="w", encoding="utf-8") as body:
            body.write("## 概要\n\n設定を更新します。\n")
            body.flush()
            command = (
                "gh pr create --draft --repo owner/repo --base main "
                "--head feature/example --title ':wrench: 設定を更新' "
                f"--body-file {body.name}"
            )
            with (
                mock.patch.object(GUARD, "_origin_repository", return_value="owner/repo"),
                mock.patch.object(GUARD, "_draft_pr_preflight_reason", return_value=None),
            ):
                self.assert_allowed(command, "/workspace")

    def test_github_writes_must_target_current_origin(self):
        with (
            tempfile.NamedTemporaryFile(mode="w", encoding="utf-8") as body,
            mock.patch.object(GUARD, "_origin_repository", return_value="owner/repo"),
        ):
            body.write("## 概要\n")
            body.flush()
            self.assert_blocked(
                "gh issue create --repo attacker/repo --title test --body safe",
                "/workspace",
            )
            self.assert_blocked(
                "gh pr create --draft --repo attacker/repo --base main "
                "--head feature/example --title test "
                f"--body-file {body.name}",
                "/workspace",
            )

    def test_github_write_bypasses_are_blocked(self):
        with (
            tempfile.NamedTemporaryFile(mode="w", encoding="utf-8") as body,
            mock.patch.object(GUARD, "_origin_repository", return_value="owner/repo"),
        ):
            body.write("## 本文\n")
            body.flush()
            for command in (
                "gh issue comment 9 --repo owner/repo --delete-last --yes",
                "gh issue comment 9 --repo owner/repo --edit-last --body-file " + body.name,
                "gh issue comment https://github.com/attacker/repo/issues/1 --repo owner/repo --body-file " + body.name,
                "gh issue create --repo owner/repo -ttest -F" + body.name,
                "gh -R owner/repo issue comment 9 --body-file " + body.name,
                "GH_HOST=example.com gh issue comment 9 --repo owner/repo --body-file " + body.name,
                "gh issue create --repo owner/repo --title safe --body $(cat /etc/hostname)",
                "gh issue create --repo owner/repo --title safe --body-file /etc/hostname",
                "gh pr create --draft --repo owner/repo --base main --head feature/example --title safe --body-file /etc/hostname",
            ):
                with self.subTest(command=command):
                    self.assert_blocked(command, "/workspace")
            with mock.patch.dict(GUARD.os.environ, {"GH_HOST": "example.com"}):
                self.assert_blocked(
                    "gh issue comment 9 --repo owner/repo --body-file " + body.name,
                    "/workspace",
                )

    def test_push_preflight_rejects_pushurl_and_dirty_worktree(self):
        with mock.patch.object(
            GUARD,
            "_run_git",
            side_effect=[
                "https://github.com/owner/repo.git\n",
                "https://github.com/attacker/repo.git\n",
            ],
        ):
            self.assertIsNotNone(
                GUARD._push_preflight_reason("/workspace", "feature/example")
            )

        with mock.patch.object(
            GUARD,
            "_run_git",
            side_effect=[
                "https://github.com/owner/repo.git\n",
                "git@github.com:owner/repo.git\n",
                "feature/example\n",
                " M README.md\n",
            ],
        ):
            self.assertIsNotNone(
                GUARD._push_preflight_reason("/workspace", "feature/example")
            )

    def test_pull_preflight_requires_clean_default_branch_without_local_commits(self):
        def safe_git(_cwd, *args):
            values = {
                ("remote", "get-url", "origin"): "https://github.com/owner/repo.git\n",
                ("symbolic-ref", "--short", "refs/remotes/origin/HEAD"): "origin/main\n",
                ("rev-parse", "--abbrev-ref", "HEAD"): "main\n",
                ("rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"): "origin/main\n",
                ("status", "--porcelain=v1", "--untracked-files=all"): "",
                ("merge-base", "--is-ancestor", "HEAD", "origin/main"): "",
                ("rev-parse", "--git-dir"): ".git\n",
            }
            return values.get(args)

        with mock.patch.object(GUARD, "_run_git", side_effect=safe_git):
            self.assertIsNone(GUARD._pull_preflight_reason("/workspace", "main"))

        def dirty_git(cwd, *args):
            if args == ("status", "--porcelain=v1", "--untracked-files=all"):
                return "?? local.txt\n"
            return safe_git(cwd, *args)

        with mock.patch.object(GUARD, "_run_git", side_effect=dirty_git):
            self.assertIsNotNone(GUARD._pull_preflight_reason("/workspace", "main"))

        def ahead_git(cwd, *args):
            if args == ("merge-base", "--is-ancestor", "HEAD", "origin/main"):
                return None
            return safe_git(cwd, *args)

        with mock.patch.object(GUARD, "_run_git", side_effect=ahead_git):
            self.assertIsNotNone(GUARD._pull_preflight_reason("/workspace", "main"))

        def wrong_default_git(cwd, *args):
            if args == ("symbolic-ref", "--short", "refs/remotes/origin/HEAD"):
                return "origin/develop\n"
            return safe_git(cwd, *args)

        with mock.patch.object(GUARD, "_run_git", side_effect=wrong_default_git):
            self.assertIsNotNone(GUARD._pull_preflight_reason("/workspace", "main"))

        def wrong_upstream_git(cwd, *args):
            if args == ("rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"):
                return "origin/develop\n"
            return safe_git(cwd, *args)

        with mock.patch.object(GUARD, "_run_git", side_effect=wrong_upstream_git):
            self.assertIsNotNone(GUARD._pull_preflight_reason("/workspace", "main"))

        with (
            mock.patch.object(GUARD, "_run_git", side_effect=safe_git),
            mock.patch.object(GUARD.Path, "exists", return_value=True),
        ):
            self.assertIsNotNone(GUARD._pull_preflight_reason("/workspace", "main"))

    def test_default_branch_switch_requires_origin_default_and_local_branch(self):
        def safe_git(_cwd, *args):
            values = {
                ("symbolic-ref", "--short", "refs/remotes/origin/HEAD"): "origin/main\n",
                ("rev-parse", "--verify", "refs/heads/main"): "abc123\n",
            }
            return values.get(args)

        with mock.patch.object(GUARD, "_run_git", side_effect=safe_git):
            self.assertIsNone(GUARD._git_switch_reason(["main"], "/workspace"))

        def wrong_default_git(cwd, *args):
            if args == ("symbolic-ref", "--short", "refs/remotes/origin/HEAD"):
                return "origin/develop\n"
            return safe_git(cwd, *args)

        with mock.patch.object(GUARD, "_run_git", side_effect=wrong_default_git):
            self.assertIsNotNone(GUARD._git_switch_reason(["main"], "/workspace"))

        def missing_local_git(cwd, *args):
            if args == ("rev-parse", "--verify", "refs/heads/main"):
                return None
            return safe_git(cwd, *args)

        with mock.patch.object(GUARD, "_run_git", side_effect=missing_local_git):
            self.assertIsNotNone(GUARD._git_switch_reason(["main"], "/workspace"))

    def test_read_only_git_rejects_side_effect_options(self):
        for command in (
            "git grep -O rm block_git_write",
            "git grep --open-files-in-pager=rm block_git_write",
            "git reflog expire --expire=now --all",
            "git reflog delete HEAD@{0}",
            "git reflog drop --all",
            "git diff --output=README.md",
            "git show --ext-diff HEAD",
            "git cat-file --filters HEAD:README.md",
        ):
            with self.subTest(command=command):
                self.assert_blocked(command, "/workspace")

    def test_origin_repository_supports_https_and_ssh_urls(self):
        for remote, expected in (
            ("https://github.com/owner/repo.git\n", "owner/repo"),
            ("git@github.com:owner/repo.git\n", "owner/repo"),
            ("ssh://git@github.com/owner/repo\n", "owner/repo"),
            ("https://example.com/owner/repo.git\n", None),
        ):
            with (
                self.subTest(remote=remote),
                mock.patch.object(GUARD, "_run_git", return_value=remote),
            ):
                self.assertEqual(GUARD._origin_repository("/workspace"), expected)

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
                "gh run download 123 --dir /tmp/output",
                "gh release download v1.0.0 --output /tmp/archive",
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
        for attribution in (
            "Generated" + "-by: ChatGPT",
            "Co-authored" + "-by: Claude <bot@example.com>",
            "Signed-off" + "-by: AI Assistant <bot@example.com>",
        ):
            with self.subTest(attribution=attribution):
                self.assertIsNotNone(GUARD.AI_ATTRIBUTION_RE.search(attribution))

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
            if args == ("remote", "get-url", "origin"):
                return "https://github.com/owner/repo.git\n"
            if args == ("remote", "get-url", "--push", "origin"):
                return "git@github.com:owner/repo.git\n"
            if args[:3] == ("rev-parse", "--abbrev-ref", "HEAD"):
                return "feature/example\n"
            if args == ("status", "--porcelain=v1", "--untracked-files=all"):
                return ""
            if args[:3] == ("rev-parse", "--verify", "origin/feature/example"):
                return None
            if args == ("rev-parse", "--verify", "refs/remotes/origin/main"):
                return "base123\n"
            if args == ("symbolic-ref", "--short", "refs/remotes/origin/HEAD"):
                return "origin/main\n"
            if args[:2] == ("merge-base", "HEAD"):
                return "base123\n" if args[2] == "origin/main" else None
            if args[:2] == ("rev-list", "--count"):
                return "2\n"
            if args[:3] == ("diff", "--name-only", "--diff-filter=ACMR"):
                return "src/main.rs\n"
            if args[:4] == (
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--unified=0",
            ):
                return "diff --git a/x b/x\n+safe = true\n"
            return None

        with mock.patch.object(GUARD, "_run_git", side_effect=clean_git):
            self.assertIsNone(
                GUARD._push_preflight_reason("/workspace", "feature/example")
            )

        github_token = "github_" + "pat_" + "D" * 24

        def secret_git(cwd, *args):
            result = clean_git(cwd, *args)
            if args[:4] == (
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--unified=0",
            ):
                return f"diff --git a/x b/x\n+TOKEN={github_token}\n"
            return result

        with mock.patch.object(GUARD, "_run_git", side_effect=secret_git):
            self.assertIsNotNone(
                GUARD._push_preflight_reason("/workspace", "feature/example")
            )

    def test_remote_refs_snapshot_requires_exact_unambiguous_response(self):
        default_oid = "a" * 40
        head_oid = "b" * 40
        response = (
            f"ref: refs/heads/main\tHEAD\n{default_oid}\tHEAD\n"
            f"{head_oid}\trefs/heads/feature/example\n"
        )
        with mock.patch.object(GUARD, "_run_git", return_value=response) as run_git:
            self.assertEqual(
                GUARD._remote_refs_snapshot("/workspace", "feature/example"),
                ("origin/main", head_oid),
            )
            run_git.assert_called_once_with(
                "/workspace",
                "ls-remote",
                "--symref",
                "origin",
                "HEAD",
                "refs/heads/feature/example",
            )

        for response in (
            None,
            "",
            f"ref: refs/heads/main\tHEAD\n{default_oid}\tHEAD\nextra\n",
            f"ref: refs/heads/main\tHEAD\nnot-an-oid\tHEAD\n",
            f"ref: refs/heads/main\tHEAD\n{default_oid}\tHEAD\n{head_oid}\trefs/heads/other\n",
            f"ref: refs/heads/release/1\tHEAD\n{default_oid}\tHEAD\n",
        ):
            with self.subTest(response=response):
                with mock.patch.object(GUARD, "_run_git", return_value=response):
                    self.assertIsNone(
                        GUARD._remote_refs_snapshot("/workspace", "feature/example")
                    )

    def test_push_preflight_uses_remote_default_only_when_origin_head_is_missing(self):
        def safe_git(_cwd, *args):
            values = {
                ("remote", "get-url", "origin"): "https://github.com/owner/repo.git\n",
                ("remote", "get-url", "--push", "origin"): "git@github.com:owner/repo.git\n",
                ("rev-parse", "--abbrev-ref", "HEAD"): "feature/example\n",
                ("status", "--porcelain=v1", "--untracked-files=all"): "",
                ("rev-parse", "--verify", "origin/feature/example"): None,
                ("symbolic-ref", "--short", "refs/remotes/origin/HEAD"): None,
                ("rev-parse", "--verify", "refs/remotes/origin/main"): "base123\n",
                ("merge-base", "HEAD", "origin/main"): "base123\n",
                ("diff", "--name-only", "--diff-filter=ACMR", "base123..HEAD"): "src/main.rs\n",
                ("diff", "--no-ext-diff", "--no-textconv", "--unified=0", "base123..HEAD", "--"): "diff --git a/x b/x\n+safe = true\n",
            }
            return values.get(args)

        with (
            mock.patch.object(GUARD, "_run_git", side_effect=safe_git),
            mock.patch.object(GUARD, "_remote_refs_snapshot", return_value=("origin/main", None)),
        ):
            self.assertIsNone(
                GUARD._push_preflight_reason("/workspace", "feature/example")
            )

        with (
            mock.patch.object(GUARD, "_run_git", side_effect=safe_git),
            mock.patch.object(GUARD, "_remote_refs_snapshot", return_value=("origin/release/1", None)),
        ):
            self.assertIsNotNone(
                GUARD._push_preflight_reason("/workspace", "feature/example")
            )

        def missing_base_git(cwd, *args):
            if args == ("rev-parse", "--verify", "refs/remotes/origin/main"):
                return None
            return safe_git(cwd, *args)

        with (
            mock.patch.object(GUARD, "_run_git", side_effect=missing_base_git),
            mock.patch.object(GUARD, "_remote_refs_snapshot", return_value=("origin/main", None)),
        ):
            self.assertIsNotNone(
                GUARD._push_preflight_reason("/workspace", "feature/example")
            )

    def test_add_and_commit_require_a_non_protected_work_branch(self):
        with mock.patch.object(GUARD, "_run_git", return_value="main\n"):
            self.assertIsNotNone(GUARD._current_work_branch_reason("/workspace"))
        with mock.patch.object(GUARD, "_run_git", return_value="feature/example\n"):
            self.assertIsNone(GUARD._current_work_branch_reason("/workspace"))
        with mock.patch.object(GUARD, "_run_git", return_value=" M README.md\n"):
            self.assertIsNotNone(GUARD._clean_worktree_reason("/workspace", "switch"))

    def test_draft_pr_preflight_binds_base_head_and_pushed_tip(self):
        with mock.patch.object(
            GUARD,
            "_run_git",
            side_effect=[
                "feature/example\n",
                "abc123\n",
                "abc123\n",
                "origin/main\n",
            ],
        ):
            self.assertIsNone(
                GUARD._draft_pr_preflight_reason(
                    "/workspace", "main", "feature/example"
                )
            )
        with mock.patch.object(GUARD, "_run_git", return_value="main\n"):
            self.assertIsNotNone(
                GUARD._draft_pr_preflight_reason(
                    "/workspace", "main", "feature/example"
                )
            )

    def test_draft_pr_preflight_uses_remote_head_when_tracking_ref_is_missing(self):
        head_oid = "a" * 40

        def safe_git(_cwd, *args):
            values = {
                ("rev-parse", "--abbrev-ref", "HEAD"): "feature/example\n",
                ("rev-parse", "HEAD"): f"{head_oid}\n",
                ("rev-parse", "--verify", "origin/feature/example"): None,
                ("symbolic-ref", "--short", "refs/remotes/origin/HEAD"): "origin/main\n",
            }
            return values.get(args)

        with (
            mock.patch.object(GUARD, "_run_git", side_effect=safe_git),
            mock.patch.object(GUARD, "_remote_refs_snapshot", return_value=("origin/main", head_oid)),
        ):
            self.assertIsNone(
                GUARD._draft_pr_preflight_reason(
                    "/workspace", "main", "feature/example"
                )
            )

        with (
            mock.patch.object(GUARD, "_run_git", side_effect=safe_git),
            mock.patch.object(GUARD, "_remote_refs_snapshot", return_value=("origin/main", "b" * 40)),
        ):
            self.assertIsNotNone(
                GUARD._draft_pr_preflight_reason(
                    "/workspace", "main", "feature/example"
                )
            )

        def missing_default_git(cwd, *args):
            if args == ("symbolic-ref", "--short", "refs/remotes/origin/HEAD"):
                return None
            return safe_git(cwd, *args)

        with (
            mock.patch.object(GUARD, "_run_git", side_effect=missing_default_git),
            mock.patch.object(GUARD, "_remote_refs_snapshot", return_value=None),
        ):
            self.assertIsNotNone(
                GUARD._draft_pr_preflight_reason(
                    "/workspace", "main", "feature/example"
                )
            )

    def test_malformed_hook_input_fails_closed(self):
        stdout = io.StringIO()
        stderr = io.StringIO()
        with (
            mock.patch.object(GUARD.sys, "stdin", io.StringIO("{")),
            mock.patch.object(GUARD.sys, "stdout", stdout),
            mock.patch.object(GUARD.sys, "stderr", stderr),
            self.assertRaises(SystemExit) as exit_status,
        ):
            GUARD.main()
        self.assertEqual(exit_status.exception.code, 2)
        self.assertIn('"permissionDecision": "deny"', stdout.getvalue())

        for payload in ("[]", "null", '{"tool_input": "unexpected"}'):
            stdout = io.StringIO()
            with (
                self.subTest(payload=payload),
                mock.patch.object(GUARD.sys, "stdin", io.StringIO(payload)),
                mock.patch.object(GUARD.sys, "stdout", stdout),
                mock.patch.object(GUARD.sys, "stderr", io.StringIO()),
                self.assertRaises(SystemExit) as exit_status,
            ):
                GUARD.main()
            self.assertEqual(exit_status.exception.code, 2)
            self.assertIn('"permissionDecision": "deny"', stdout.getvalue())

    def test_write_context_must_match_the_session_repository(self):
        repository = MODULE_PATH.resolve().parents[2]
        with mock.patch.object(
            GUARD,
            "_run_git",
            side_effect=[f"{repository}\n", f"{repository.parent}\n"],
        ):
            self.assertIsNotNone(
                GUARD._write_context_reason(
                    "git add -- README.md", str(repository), str(repository.parent)
                )
            )
        with mock.patch.object(
            GUARD,
            "_run_git",
            side_effect=[f"{repository}\n", f"{repository}\n"],
        ):
            self.assertIsNone(
                GUARD._write_context_reason(
                    "git add -- README.md", str(repository), str(repository / ".codex")
                )
            )

        stdout = io.StringIO()
        with (
            mock.patch.object(GUARD.sys, "stdin", io.StringIO('{"tool_input": {}}')),
            mock.patch.object(GUARD.sys, "stdout", stdout),
            mock.patch.object(GUARD.sys, "stderr", io.StringIO()),
            self.assertRaises(SystemExit) as exit_status,
        ):
            GUARD.main()
        self.assertEqual(exit_status.exception.code, 2)
        self.assertIn('"permissionDecision": "deny"', stdout.getvalue())


if __name__ == "__main__":
    unittest.main()

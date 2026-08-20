#!/usr/bin/env python3
"""Codex開発ワークフローガードの単体テスト。"""

import importlib.util
import io
import json
import os
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
    def setUp(self):
        self.git_environment = {
            key: GUARD.os.environ.pop(key)
            for key in list(GUARD.os.environ)
            if key.startswith("GIT_") or key in GUARD.GIT_NON_PREFIX_ENVIRONMENT_KEYS
        }

    def tearDown(self):
        GUARD.os.environ.update(self.git_environment)

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
            "git worktree list --porcelain",
            "git worktree list --porcelain -z",
            "git ls-remote --branches origin feature/example",
            "git ls-remote --heads origin refs/heads/feature/example",
            "git rev-parse --abbrev-ref --symbolic-full-name '@{upstream}'",
            "git --version",
        ):
            with self.subTest(command=command):
                self.assert_allowed(command)

    def test_read_only_git_restricts_worktree_and_ls_remote_shapes(self):
        for command in (
            "git worktree list",
            "git worktree list --porcelain --verbose",
            "git ls-remote origin feature/example",
            "git ls-remote --branches origin feature/example other/example",
            "git ls-remote --branches upstream feature/example",
            "git ls-remote --branches https://github.com/owner/repo feature/example",
            "git ls-remote --branches origin refs/heads/feature/*",
            "git ls-remote --branches origin main",
            "git ls-remote --upload-pack=evil --branches origin feature/example",
            "git ls-remote --server-option=evil --branches origin feature/example",
        ):
            with self.subTest(command=command):
                self.assert_blocked(command)

    def test_restricted_commands_reject_unquoted_redirection_only(self):
        for command in (
            "git status > /tmp/status",
            "git status >> /tmp/status",
            "git status 2> /tmp/status",
            "git status &> /tmp/status",
            "git status < /tmp/status",
            "git status <<EOF\nignored\nEOF",
            "git status <(printf x)",
            "> /tmp/status git status",
            "> /tmp/status rm -rf README.md",
            "2>&1 git reset --hard HEAD",
            "if > /tmp/status git reset --hard HEAD; then true; fi",
            "gh pr view 1 > /tmp/pr",
            "codex-delivery --help > /tmp/help",
        ):
            with self.subTest(command=command):
                self.assert_blocked(command)
        for command in (
            r"git status \> /tmp/status",
            "git status '>' /tmp/status",
            'git status ">" /tmp/status',
            "git status # a comment containing > is not redirection",
            "printf '%s' 'git status > /tmp/status'",
            "> /tmp/status printf '%s' git",
            "printf '%s' git > /tmp/status",
            "rg --files /tmp/git > /tmp/path",
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
            mock.patch.object(GUARD, "_git_write_target", return_value=("/workspace", None)),
        ):
            for command in (
                "git fetch origin main",
                "git add -- src/main.rs README.md",
                "git commit -m ':wrench: 設定を更新'",
                "git commit -S -m ':bug: Fix startup failure'",
                "git push -u origin HEAD:refs/heads/feature/example",
                "git push --set-upstream origin HEAD:refs/heads/refactor/example",
            ):
                with self.subTest(command=command):
                    self.assert_allowed(command, "/workspace")

    def test_pull_and_switch_require_explicit_single_checkout_rollback(self):
        commands = (
            "git pull --ff-only --no-rebase --no-autostash --no-recurse-submodules origin main",
            "git switch main",
            "git switch -c feature/example origin/main",
        )
        for command in commands:
            with self.subTest(command=command):
                self.assert_blocked(command, "/workspace")
        with (
            mock.patch.dict(
                GUARD.os.environ, {"CODEX_WORKTREE_MODE": "single-checkout"}, clear=False
            ),
            mock.patch.object(GUARD, "_pull_preflight_reason", return_value=None),
            mock.patch.object(GUARD, "_default_branch_switch_reason", return_value=None),
            mock.patch.object(GUARD, "_clean_worktree_reason", return_value=None),
        ):
            for command in commands:
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

    def test_git_write_rejects_command_bearing_environment(self):
        with (
            mock.patch.object(GUARD, "_git_write_target", return_value=("/workspace", None)),
            mock.patch.object(GUARD, "_current_work_branch_reason", return_value=None),
        ):
            for key in (
                "GIT_EXEC_PATH",
                "GIT_SSH",
                "GIT_SSH_COMMAND",
                "GIT_ASKPASS",
                "SSH_ASKPASS",
                "GIT_PROXY_COMMAND",
                "GIT_INDEX_FILE",
                "GIT_OBJECT_DIRECTORY",
                "GIT_ALTERNATE_OBJECT_DIRECTORIES",
                "GIT_EXTERNAL_DIFF",
                "GIT_CONFIG_NOSYSTEM",
            ):
                with self.subTest(key=key), mock.patch.dict(
                    GUARD.os.environ, {key: "/tmp/untrusted-command"}
                ):
                    self.assert_blocked("git add -- README.md", "/workspace")

    def test_exact_wrapper_removes_inherited_ssh_askpass(self):
        with (
            mock.patch.dict(
                GUARD.os.environ, {"SSH_ASKPASS": "/trusted/launcher/askpass"}
            ),
            mock.patch.object(GUARD, "_git_write_target", return_value=("/workspace", None)),
            mock.patch.object(GUARD, "_current_work_branch_reason", return_value=None),
        ):
            self.assert_allowed(
                "env -u SSH_ASKPASS git -C /workspace add -- README.md",
                "/session",
            )
            self.assert_blocked(
                "env -u GIT_SSH_COMMAND git -C /workspace add -- README.md",
                "/session",
            )
            with mock.patch.dict(
                GUARD.os.environ, {"GIT_SSH_COMMAND": "/tmp/untrusted-command"}
            ):
                self.assert_blocked(
                    "env -u SSH_ASKPASS git -C /workspace add -- README.md",
                    "/session",
                )

    def test_git_read_rejects_external_diff_but_allows_launcher_pager(self):
        with mock.patch.dict(
            GUARD.os.environ, {"GIT_EXTERNAL_DIFF": "/tmp/untrusted-command"}
        ):
            self.assert_blocked("git diff --stat", "/workspace")
        with mock.patch.dict(GUARD.os.environ, {"GIT_PAGER": "cat"}):
            self.assert_allowed("git status", "/workspace")

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
            "git worktree list",
            "git worktree add /tmp/worktree feature/example",
            "git worktree remove /tmp/worktree",
            "git worktree prune",
            "git -c alias.save=commit save -m test",
            "git -C /workspace add -- README.md",
            "env -u TOKEN git reset --hard HEAD",
            "sudo -u root git push -u origin HEAD:refs/heads/main",
            "sudo -D /tmp git reset --hard HEAD",
            "sudo -U root git reset --hard HEAD",
            "xargs --process-slot-var SLOT git reset --hard HEAD",
            "env -a ARG0 git reset --hard HEAD",
            "env -Sgit reset --hard HEAD",
            "env -Sgh issue delete 1 --repo owner/repo",
            "env -Scodex-worktree create --issue 1",
            "env -Srm -rf README.md",
            "command env -Sgit reset --hard HEAD",
            "nice env -Sgh issue delete 1 --repo owner/repo",
            "sudo -- env -Scodex-worktree create --issue 1",
            "if command env -Sgit reset --hard HEAD; then true; fi",
            "sudo --future-option value git reset --hard HEAD",
            "sudo --future-option value $GIT reset --hard HEAD",
            "/usr/libexec/git-core/git-reset --hard HEAD",
            "git-clean -fd",
            "git-push origin :main",
            "env git-reset --hard HEAD",
            "if git-reset --hard HEAD; then true; fi",
            "bash -c 'git-clean -fd'",
            "exec rm README.md",
            "nice rm README.md",
            "timeout 1 rm README.md",
            "xargs rm",
            "find . -delete",
            "find . -exec rm README.md ;",
            "x=rm; \"$x\" README.md",
            "sh -c 'x=rm; \"$x\" README.md'",
            "bash -c '{git,reset} --hard HEAD'",
            "bash -c '{gh,issue,delete} 1 --repo attacker/repo'",
            "bash -c '{rm,-rf} README.md'",
            "g{it,} status",
            "g* status",
            "=git reset --hard HEAD",
            "=gh issue delete 1 --repo owner/repo",
            "=codex-delivery deliver",
            "=rm -rf README.md",
            "~gitcmd reset --hard HEAD",
            "~ghcmd issue delete 1 --repo owner/repo",
            "~rmcmd -rf README.md",
            "g\\\nit reset --hard HEAD",
            "g\\\nh issue delete 1 --repo owner/repo",
            "r\\\nm -rf README.md",
            "bash -c 'g\\\nit reset --hard HEAD'",
            "x=gh; \"$x\" issue comment 9 --repo attacker/repo",
            "eval 'rm README.md'",
            "builtin eval 'git reset --hard HEAD'",
            "builtin source /tmp/payload.sh",
            "builtin . /tmp/payload.sh",
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
            "printf '%s' /usr/libexec/git-core/git-reset",
            "printf '%s' env -Sgit",
            "python3 tool.py env -Sgit",
            "python3 tool.py --path /usr/bin/git",
            "command -v codex-delivery",
            "command -V codex-worktree",
            "rg --files /home/daiki/.local/bin/codex-delivery",
            "python3 tool.py --path /usr/bin/codex-delivery",
        ):
            with self.subTest(command=command):
                self.assert_allowed(command)

        for command in (
            "python3 /home/user/.local/bin/codex-delivery deliver",
            "env python3 /home/user/.local/bin/codex-worktree list",
            "python3 -m codex-delivery deliver",
            "python3 -B -m codex-delivery deliver",
            "python3 -X dev -m codex-worktree list",
            "python3 -B -m runpy /home/user/.local/bin/codex-delivery",
            "python3 -B -m cProfile /home/user/.local/bin/codex-delivery deliver",
            "python3 -B -m trace --trace /home/user/.local/bin/codex-worktree create --issue 1",
            "python3 -m pdb /home/user/.local/bin/codex-delivery deliver",
        ):
            with self.subTest(command=command):
                self.assert_blocked(command)

    def test_git_and_gh_require_canonical_executables(self):
        for command in (
            "./git status",
            "/usr/bin/git status",
            "./gh pr view 1",
            "/usr/bin/gh pr view 1",
        ):
            with self.subTest(command=command):
                self.assert_blocked(command)

    def test_worktree_helper_accepts_only_canonical_operations(self):
        for command in (
            "codex-worktree --help",
            "codex-worktree list --help",
            "codex-worktree doctor -h",
            "codex-worktree list",
            "codex-worktree doctor",
            "codex-worktree doctor --task-id issue-22",
            "codex-worktree resume --task-id task-safe-id",
            "codex-worktree recover --task-id task-safe-id",
            "codex-worktree create",
            "codex-worktree create --issue 22 --branch feat/example",
            "codex-worktree create --task-id task-safe-id",
        ):
            with self.subTest(command=command):
                self.assert_allowed(command, "/workspace")

        for command in (
            "./codex-worktree list",
            "command codex-worktree create --issue 22",
            "codex-worktree remove --task-id issue-22",
            "codex-worktree list --all",
            "codex-worktree resume",
            "codex-worktree recover",
            "codex-worktree doctor --task-id ../other",
            "codex-worktree create --issue 0",
            "codex-worktree create --issue 22 --task-id task-other",
            "codex-worktree create --branch main",
            "codex-worktree create --branch feat/example --branch fix/example",
            "codex-worktree create --repository /tmp/other",
            "codex-worktree create --issue 22 && git status",
            "codex-worktree recover --task-id issue-22 && git status",
            "bash -lc 'codex-worktree create --issue 22'",
            "codex-worktree list --help --all",
            "codex-worktree doctor --help --task-id issue-22",
            "codex-worktree remove --help",
        ):
            with self.subTest(command=command):
                self.assert_blocked(command, "/workspace")

    def test_delivery_helper_accepts_only_canonical_operations(self):
        head = "a" * 40
        evidence = (
            "--tests-passed --neutral-review-passed --adversarial-review-passed"
        )
        for command in (
            "codex-delivery --help",
            "codex-delivery record-review --help",
            "codex-delivery approve-review -h",
            "codex-delivery deliver --help",
            "codex-delivery finish -h",
            f"codex-delivery record-review --task-id issue-24 --pr 27 --head {head} "
            "--risk medium --plan-id CODEX-COMPLETE-DELIVERY-20260819-v1 " + evidence,
            f"codex-delivery record-review --task-id issue-24 --pr 27 --head {head} "
            "--risk high --plan-id CODEX-COMPLETE-DELIVERY-20260819-v1 " + evidence,
            f"codex-delivery record-review --task-id issue-24 --pr 27 --head {head} "
            "--risk critical --plan-id CODEX-COMPLETE-DELIVERY-20260819-v1 "
            "--gate-mode github-free-private " + evidence,
            f"codex-delivery approve-review --task-id issue-24 --pr 27 --head {head} "
            "--risk high --plan-id CODEX-COMPLETE-DELIVERY-20260819-v1 " + evidence,
            f"codex-delivery approve-review --task-id issue-24 --pr 27 --head {head} "
            "--risk low --plan-id CODEX-COMPLETE-DELIVERY-20260819-v1 " + evidence,
            f"codex-delivery approve-review --task-id issue-24 --pr 27 --head {head} "
            "--risk high --plan-id CODEX-COMPLETE-DELIVERY-20260819-v1 "
            "--gate-mode github-free-private " + evidence,
            f"codex-delivery deliver --task-id issue-24 --pr 27 --head {head} "
            "--plan-id CODEX-COMPLETE-DELIVERY-20260819-v1",
            f"codex-delivery deliver --task-id issue-24 --pr 27 --head {head} "
            "--plan-id CODEX-COMPLETE-DELIVERY-20260819-v1 "
            "--gate-mode github-free-private",
            f"codex-delivery finish --task-id issue-24 --pr 27 --head {head} "
            "--plan-id CODEX-COMPLETE-DELIVERY-20260819-v1",
            f"codex-delivery finish --task-id issue-24 --pr 27 --head {head} "
            "--plan-id CODEX-COMPLETE-DELIVERY-20260819-v1 "
            "--gate-mode github-free-private",
        ):
            with self.subTest(command=command):
                self.assert_allowed(command, "/workspace")

        for command in (
            "./codex-delivery deliver --task-id issue-24",
            "command codex-delivery deliver --task-id issue-24",
            "python3 .codex/helpers/codex-delivery approve-review --task-id issue-24",
            "python3 /home/user/.local/bin/codex-delivery deliver --task-id issue-24",
            f"codex-delivery record-review --task-id issue-24 --pr 27 --head {head} "
            "--risk unknown --plan-id CODEX-COMPLETE-DELIVERY-20260819-v1 " + evidence,
            f"codex-delivery record-review --task-id issue-24 --pr 0 --head {head} "
            "--risk low --plan-id CODEX-COMPLETE-DELIVERY-20260819-v1 " + evidence,
            f"codex-delivery approve-review --task-id issue-24 --pr 27 --head {head} "
            "--risk high --plan-id CODEX-COMPLETE-DELIVERY-20260819-v1 "
            "--gate-mode automatic " + evidence,
            "codex-delivery deliver --task-id ../issue-24 --pr 27 --head bad "
            "--plan-id unsafe",
            f"codex-delivery deliver --task-id issue-24 --pr 27 --head {head} "
            "--plan-id CODEX-COMPLETE-DELIVERY-20260819-v1 --admin",
            f"codex-delivery finish --task-id issue-24 --pr 27 --head {head} "
            "--plan-id CODEX-COMPLETE-DELIVERY-20260819-v1 && git status",
            f"bash -lc 'codex-delivery finish --task-id issue-24 --pr 27 --head {head} "
            "--plan-id CODEX-COMPLETE-DELIVERY-20260819-v1'",
            "codex-delivery record-review --help --task-id issue-24",
            "codex-delivery inspect --help",
        ):
            with self.subTest(command=command):
                self.assert_blocked(command, "/workspace")

    def test_canonical_helper_help_does_not_require_repository_context(self):
        for command in (
            "codex-worktree create --help",
            "codex-worktree recover -h",
            "codex-delivery record-review --help",
            "codex-delivery deliver -h",
        ):
            with self.subTest(command=command):
                tokens = next(GUARD._command_segments(command))
                self.assertFalse(GUARD._has_write_operation(tokens))
                self.assertIsNone(GUARD._write_context_reason(command, "/tmp"))
                self.assert_allowed(command, "/tmp")

    def test_direct_ready_is_blocked_but_return_to_draft_is_allowed(self):
        with mock.patch.object(GUARD, "_origin_repository", return_value="owner/repo"):
            self.assert_blocked(
                "gh pr ready 27 --repo owner/repo",
                "/workspace",
            )
            self.assert_allowed(
                "gh pr ready 27 --undo --repo owner/repo",
                "/workspace",
            )

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

    def test_command_substitution_is_always_blocked(self):
        for command in (
            'git status "$(git push -u origin HEAD:refs/heads/feature/example)"',
            'git status "$(rm -rf /tmp/target)"',
            "git status `git push -u origin HEAD:refs/heads/feature/example`",
        ):
            with self.subTest(command=command):
                self.assert_blocked(command, "/workspace")

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

    def test_issue_and_pr_lifecycle_writes_are_allowed(self):
        with tempfile.NamedTemporaryFile(mode="w", encoding="utf-8") as body:
            body.write("## 変更内容\n\n安全な本文です。\n")
            body.flush()
            commands = (
                "gh issue create --repo owner/repo --title test --body-file " + body.name,
                "gh issue edit 23 --repo owner/repo --add-label U3 --add-assignee daiki",
                "gh issue edit 23 --repo owner/repo --remove-milestone",
                "gh issue comment 23 --repo owner/repo --body-file " + body.name,
                "gh issue close 23 --repo owner/repo --reason completed",
                "gh issue reopen 23 --repo owner/repo",
                "gh pr edit 25 --repo owner/repo --add-label U3",
                "gh pr edit 25 --repo owner/repo --add-reviewer daiki",
                "gh pr edit 25 --repo owner/repo --remove-reviewer daiki",
                "gh pr edit 25 --repo owner/repo --body-file " + body.name,
                "gh pr comment 25 --repo owner/repo --body-file " + body.name,
                "gh pr review 25 --repo owner/repo --approve --body-file " + body.name,
                "gh pr ready 25 --repo owner/repo --undo",
                "gh pr close 25 --repo owner/repo",
                "gh pr reopen 25 --repo owner/repo",
                "gh pr update-branch 25 --repo owner/repo",
            )
            with (
                mock.patch.object(GUARD, "_origin_repository", return_value="owner/repo"),
                mock.patch.object(
                    GUARD, "_pr_update_branch_preflight_reason", return_value=None
                ),
            ):
                for command in commands:
                    with self.subTest(command=command):
                        self.assert_allowed(command, "/workspace")

    def test_lifecycle_writes_require_numeric_id_and_reject_unsafe_forms(self):
        with (
            tempfile.NamedTemporaryFile(mode="w", encoding="utf-8") as body,
            mock.patch.object(GUARD, "_origin_repository", return_value="owner/repo"),
        ):
            body.write("安全な本文です。\n")
            body.flush()
            for command in (
                "gh issue close https://github.com/owner/repo/issues/23 --repo owner/repo",
                "gh issue reopen 0 --repo owner/repo",
                "gh issue edit 23 --repo owner/repo --body inline",
                "gh issue edit 23 --repo owner/repo --add-label U3 "
                "--title safe --title secret",
                "gh issue create --repo owner/repo --title test --body-file " + body.name
                + " --assignee @copilot",
                "gh issue close 23 --repo owner/repo --delete-branch",
                "gh pr comment 25 --repo owner/repo --body inline",
                "gh pr edit 25 --repo owner/repo --add-label U3 --body-file "
                + body.name + " --body-file=" + body.name,
                "gh pr review 25 --repo owner/repo --approve --body-file "
                + body.name + " --body-file=" + body.name,
                "gh pr close 25 --repo owner/repo --delete-branch",
                "gh pr update-branch 25 --repo owner/repo --rebase",
                "gh pr merge 25 --repo owner/repo --merge --match-head-commit "
                + "a" * 40,
                "gh pr merge 25 --repo owner/repo --squash",
                "gh pr edit 25 --repo owner/repo --add-project project",
                "gh issue comment 23 --repo attacker/repo --body-file " + body.name,
                "gh pr review 25 --repo owner/repo --approve --comment",
            ):
                with self.subTest(command=command):
                    self.assert_blocked(command, "/workspace")

    def test_update_branch_requires_current_open_non_protected_head(self):
        valid = {
            "number": 25,
            "state": "OPEN",
            "isCrossRepository": False,
            "headRepository": {"nameWithOwner": "owner/repo"},
            "headRefName": "feature/example",
            "headRefOid": "a" * 40,
        }
        with mock.patch.object(GUARD, "_run_gh_json", return_value=valid):
            self.assertIsNone(
                GUARD._pr_update_branch_preflight_reason("/workspace", "owner/repo", "25")
            )
        for override in (
            {"state": "CLOSED"},
            {"isCrossRepository": True},
            {"headRepository": {"nameWithOwner": "attacker/repo"}},
            {"headRefName": "main"},
            {"headRefOid": "unknown"},
        ):
            payload = valid | override
            with (
                self.subTest(override=override),
                mock.patch.object(GUARD, "_run_gh_json", return_value=payload),
            ):
                self.assertIsNotNone(
                    GUARD._pr_update_branch_preflight_reason(
                        "/workspace", "owner/repo", "25"
                    )
                )

    def test_gh_read_cannot_change_host_or_send_explicit_auth_header(self):
        for command in (
            "gh api --hostname attacker.example /user",
            "gh api /user -H 'Authorization: Bearer token'",
            "gh api https://attacker.example/user",
            "gh api //attacker.example/user",
            "gh api /user -H 'Authorization: Bearer $GH_TOKEN'",
        ):
            with self.subTest(command=command):
                self.assert_blocked(command, "/workspace")

    def test_has_write_operation_matches_github_lifecycle(self):
        for command in (
            "gh issue close 23 --repo owner/repo",
            "gh issue reopen 23 --repo owner/repo",
            "gh pr edit 25 --repo owner/repo --add-label U3",
            "gh pr comment 25 --repo owner/repo --body-file /tmp/body",
            "gh pr review 25 --repo owner/repo --approve",
            "gh pr ready 25 --repo owner/repo --undo",
            "gh pr close 25 --repo owner/repo",
            "gh pr reopen 25 --repo owner/repo",
            "gh pr update-branch 25 --repo owner/repo",
            "gh api repos/owner/repo -X PATCH -f name=test",
        ):
            with self.subTest(command=command):
                tokens = next(GUARD._command_segments(command))
                self.assertTrue(GUARD._has_write_operation(tokens))
        for command in (
            "gh issue view 23 --repo owner/repo",
            "gh pr view 25 --repo owner/repo",
            "gh api repos/owner/repo/issues/23 -X GET",
        ):
            with self.subTest(command=command):
                tokens = next(GUARD._command_segments(command))
                self.assertFalse(GUARD._has_write_operation(tokens))

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

    def test_issue_comment_inline_body_has_dedicated_body_file_diagnostic(self):
        for option in ("-b", "--body", "--body=inline", "-binline"):
            with self.subTest(option=option):
                reason = GUARD.blocked_reason(
                    f"gh issue comment 9 --repo owner/repo {option} inline",
                    "/workspace",
                )
                self.assertIsNotNone(reason)
                self.assertIn("--body-file", reason)

    def test_direct_gh_graphql_is_always_blocked(self):
        for query in (
            "query { viewer { login } }",
            "mutation { deleteIssue(input: {}) { clientMutationId } }",
            "subscription { events { id } }",
            "query Q { viewer { login } } mutation M { x }",
        ):
            command = "gh api graphql -f query=" + query
            with self.subTest(query=query):
                self.assert_blocked(command, "/workspace")
        self.assert_blocked("gh api --method GET graphql", "/workspace")
        for endpoint in (
            "/graphql",
            "graphql/",
            "GraphQL#fragment",
            "graphql?query=query%20%7Bviewer%7Blogin%7D%7D",
            "/graphql/?query=query%20%7Bviewer%7Blogin%7D%7D",
            "%67raphql?query=query%20%7Bviewer%7Blogin%7D%7D",
            "foo/../graphql",
            "graphql%2f",
            "foo/%2e%2e/%2567raphql",
            "graphql%",
        ):
            with self.subTest(endpoint=endpoint):
                self.assert_blocked(f"gh api '{endpoint}'", "/workspace")
        for command in (
            "gh api --preview corsair graphql",
            "gh api -p corsair graphql",
            "gh api -q . graphql",
            "gh api -t '{{.}}' graphql",
            "gh api --template '{{.}}' graphql",
        ):
            with self.subTest(command=command):
                self.assert_blocked(command, "/workspace")

    def test_run_cancel_requires_exact_shape_and_rest_readback(self):
        payload = {
            "id": 123,
            "repository": {"full_name": "OWNER/repo"},
            "status": "queued",
            "conclusion": None,
            "cancel_url": "https://api.github.com/repos/owner/repo/actions/runs/123/cancel",
        }
        with (
            mock.patch.object(GUARD, "_origin_repository", return_value="owner/repo"),
            mock.patch.object(GUARD, "_run_gh_json", return_value=payload) as run_json,
        ):
            self.assert_allowed(
                "gh run cancel 123 --repo OWNER/repo", "/workspace"
            )
            run_json.assert_called_once_with(
                "/workspace",
                "api", "--method", "GET",
                "repos/OWNER/repo/actions/runs/123",
            )
        for command in (
            "gh run cancel",
            "gh run cancel -1 --repo owner/repo",
            "gh run cancel abc --repo owner/repo",
            "gh run cancel 123 124 --repo owner/repo",
            "gh run cancel 123 --repo owner/repo --repo owner/repo",
            "gh run cancel 123 --repo owner/repo --force",
            "gh run cancel 123 -R owner/repo",
            "gh run cancel 123 --repo=owner/repo",
            "gh run cancel --repo owner/repo 123",
            "gh run cancel 123 --verbose --repo owner/repo",
            "gh run cancel https://github.com/owner/repo/actions/runs/123 --repo owner/repo",
            "gh run cancel 0 --repo owner/repo",
            "gh run cancel 0123 --repo owner/repo",
            "gh run cancel 123 --repo attacker/repo",
            "gh run cancel 123 --repo github.com/owner/repo",
            "gh run cancel --help",
            "gh run cancel 123 --repo owner/repo && gh run view 123",
            "gh run cancel 123 --repo owner/repo;",
            "gh run cancel 123 --repo owner/repo &",
            "(gh run cancel 123 --repo owner/repo)",
            "gh run cancel $RUN_ID --repo owner/repo",
            "gh run cancel 123 --repo owner/repo > /tmp/cancel",
            "gh run cancel 123 --repo owner/repo | tee /tmp/cancel",
            "gh run delete 123 --repo owner/repo",
            "gh run rerun 123 --repo owner/repo",
            "gh workflow run ci.yml --repo owner/repo",
            "gh api --method POST repos/owner/repo/actions/runs/123/cancel",
        ):
            with self.subTest(command=command):
                self.assert_blocked(command, "/workspace")

        with (
            mock.patch.dict(GUARD.os.environ, {"GH_REPO": "owner/repo"}),
            mock.patch.object(GUARD, "_origin_repository", return_value="owner/repo"),
        ):
            self.assert_blocked(
                "gh run cancel 123 --repo owner/repo", "/workspace"
            )
        with mock.patch.dict(GUARD.os.environ, {"GH_HOST": "example.com"}):
            self.assert_blocked(
                "gh run cancel 123 --repo owner/repo", "/workspace"
            )

    def test_run_cancel_rest_readback_fails_closed(self):
        command = "gh run cancel 123 --repo owner/repo"
        valid = {
            "id": 123,
            "repository": {"full_name": "owner/repo"},
            "status": "in_progress",
            "conclusion": None,
            "cancel_url": "https://api.github.com/repos/owner/repo/actions/runs/123/cancel",
        }
        with mock.patch.object(GUARD, "_origin_repository", return_value="owner/repo"):
            for override in (
                None,
                {"id": True},
                {"id": "123"},
                {"id": 124},
                {"repository": None},
                {"repository": {"full_name": "attacker/repo"}},
                {"repository": {"full_name": 123}},
                {"status": ["queued"]},
                {"conclusion": ""},
                {"cancel_url": "https://api.github.com/repos/owner/repo/actions/runs/124/cancel"},
                {"cancel_url": "https://api.github.com/repos/attacker/repo/actions/runs/123/cancel"},
                {"cancel_url": "https://api.github.com/repos/owner/repo/actions/runs/123/cancel?x=1"},
                {"cancel_url": "https://github.com/owner/repo/actions/runs/123/cancel"},
            ):
                payload = None if override is None else valid | override
                with (
                    self.subTest(override=override),
                    mock.patch.object(GUARD, "_run_gh_json", return_value=payload),
                ):
                    self.assert_blocked(command, "/workspace")
            for status in (
                "completed", "cancelled", "success", "failure", "waiting",
                "requested", "pending", "unknown",
            ):
                with (
                    self.subTest(status=status),
                    mock.patch.object(
                        GUARD, "_run_gh_json", return_value=valid | {"status": status}
                    ),
                ):
                    self.assert_blocked(command, "/workspace")
            missing_conclusion = dict(valid)
            missing_conclusion.pop("conclusion")
            with mock.patch.object(
                GUARD, "_run_gh_json", return_value=missing_conclusion
            ):
                self.assert_blocked(command, "/workspace")
            with mock.patch.object(GUARD, "_run_gh_json", return_value=valid):
                self.assert_allowed(command, "/workspace")
                tokens = next(GUARD._command_segments(command))
                self.assertTrue(GUARD._has_write_operation(tokens))

    def test_run_gh_json_disables_prompt_and_stdin(self):
        completed = GUARD.subprocess.CompletedProcess(
            ["gh"], 0, b'{"ok": true}', b""
        )
        with mock.patch.object(GUARD.subprocess, "run", return_value=completed) as run:
            self.assertEqual(
                GUARD._run_gh_json("/workspace", "api", "--method", "GET", "/user"),
                {"ok": True},
            )
        kwargs = run.call_args.kwargs
        self.assertIs(kwargs["stdin"], GUARD.subprocess.DEVNULL)
        self.assertEqual(kwargs["env"]["GH_PROMPT_DISABLED"], "1")

    def test_restricted_commands_reject_prior_shell_context_mutation(self):
        for command in (
            "export GIT_CONFIG_GLOBAL=/tmp/evil && git status",
            "export GIT_SSH_COMMAND=/tmp/evil && git ls-remote --branches origin refs/heads/feature/example",
            "export GH_HOST=evil.example && gh pr view 1",
            "GIT_CONFIG_GLOBAL=/tmp/evil && git status",
            "cd /tmp/other-repository && git status",
            "pushd /tmp/other-repository && git log --all",
            "eval 'export GH_HOST=evil.example' && gh pr view 1",
            "eval 'export GIT_CONFIG_GLOBAL=/tmp/evil' && git status",
            "eval 'cd /tmp/other-repository' && git status",
            "alias x=git; eval 'x reset --hard HEAD'",
            "alias x=gh; eval 'x issue delete 1 --repo owner/repo'",
            "hash x=/usr/bin/git; x reset --hard HEAD",
            "function x { printf ok; }; x; git status",
            "if hash x=/usr/bin/git; then x reset --hard HEAD; fi",
            "if hash x=/usr/bin/gh; then x issue delete 1 --repo owner/repo; fi",
            "if alias x=git; then x reset --hard HEAD; fi",
            "builtin hash x=/usr/bin/git; x reset --hard HEAD",
            "if builtin hash x=/usr/bin/git; then x reset --hard HEAD; fi",
            "hash git=/bin/echo; git status",
            "chdir /tmp/other-repository && git status",
            "autoload -Uz helper; git status",
            "readonly GIT_CONFIG_GLOBAL=/tmp/evil; git status",
            "integer GIT_CONFIG_COUNT=1; git status",
        ):
            with self.subTest(command=command):
                self.assert_blocked(command, "/workspace")

    def test_shell_compound_forms_cannot_hide_guarded_commands(self):
        for command in (
            "if gh pr close 1 --repo owner/repo; then true; fi",
            "if git add -- README.md; then true; fi",
            "time git add -- README.md",
            "! gh pr close 1 --repo owner/repo",
            "for x in 1; do codex-worktree create --issue 1; done",
            "{ git status; } > /tmp/status",
            "if cd /tmp/other-repository; then true; fi; git status",
            "if export GIT_CONFIG_GLOBAL=/tmp/evil; then true; fi; git status",
            "for x in /tmp/other-repository; do cd \"$x\"; done; git status",
            "f(){ cd /tmp/other-repository; }; f; git status",
            "if noglob git add -- README.md; then true; fi",
            "if time git reset --hard HEAD; then true; fi",
            "if ! gh pr close 1 --repo attacker/repo; then true; fi",
            "while noglob codex-worktree create --issue 1; do true; done",
            "repeat 1 git reset --hard HEAD",
            "nocorrect git reset --hard HEAD",
            "if nocorrect gh issue delete 1 --repo attacker/repo; then true; fi",
            "repeat 1 codex-worktree create --issue 1",
            "if time -p gh pr close 1 --repo owner/repo; then true; fi",
            "coproc gh pr close 1 --repo owner/repo",
            "coproc MYJOB git reset --hard HEAD",
            "function f { git reset --hard HEAD; }; f",
            "function f { gh issue delete 1 --repo owner/repo; }; f",
            "- git reset --hard HEAD",
            "if sudo --close-from 3 git add -- README.md; then true; fi",
            "if sudo --close-from 3 gh pr close 1 --repo owner/repo; then true; fi",
            "if sudo --close-from 3 codex-worktree create --issue 1; then true; fi",
            "if sudo --future-option value git add -- README.md; then true; fi",
            "if $GIT add -- README.md; then true; fi",
            "if ${GIT} add -- README.md; then true; fi",
            "if g* add -- README.md; then true; fi",
            "if =git reset --hard HEAD; then true; fi",
            "if ~gitcmd reset --hard HEAD; then true; fi",
            "if g\\\nit reset --hard HEAD; then true; fi",
            "if env -S 'git reset --hard HEAD'; then true; fi",
            "if env --split-string 'git add -- README.md'; then true; fi",
            "if env -S \"$CMD\"; then true; fi",
            "time env -Sgh issue delete 1 --repo owner/repo",
            "function f { env -Sgit reset --hard HEAD; }; f",
        ):
            with self.subTest(command=command):
                self.assert_blocked(command, "/workspace")
        for command in (
            "if command -v codex-delivery; then true; fi",
            "if echo git; then true; fi",
            "if echo codex-delivery; then true; fi",
            "if rg --files /tmp/codex-delivery; then true; fi",
            "repeat 1 echo git",
            "printf '%s' 'g\\\nit reset --hard HEAD'",
        ):
            with self.subTest(command=command):
                self.assert_allowed(command)

    def test_restricted_commands_reject_unquoted_shell_expansion(self):
        for command in (
            "git diff ${:---output=/tmp/hook-bypass}",
            "git diff $GIT_DIFF_ARGS",
            "git diff \"$GIT_DIFF_ARGS\"",
            "git diff {--stat,--output=/tmp/hook-bypass}",
            "git diff *",
            "git branch --list feature/?",
        ):
            with self.subTest(command=command):
                self.assert_blocked(command, "/workspace")
        for command in (
            "git branch --list 'feature/*'",
            "git diff -- '*'",
            r"git diff \*",
        ):
            with self.subTest(command=command):
                self.assert_allowed(command, "/workspace")

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
                "git@github.com:owner/repo.git\nhttps://github.com/attacker/repo.git\n",
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
            if args == ("remote", "get-url", "--all", "origin"):
                return "https://github.com/owner/repo.git\n"
            if args == ("remote", "get-url", "--push", "--all", "origin"):
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
                ("origin/main", default_oid, head_oid),
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
        default_oid = "c" * 40

        def safe_git(_cwd, *args):
            values = {
                ("remote", "get-url", "--all", "origin"): "https://github.com/owner/repo.git\n",
                ("remote", "get-url", "--push", "--all", "origin"): "git@github.com:owner/repo.git\n",
                ("rev-parse", "--abbrev-ref", "HEAD"): "feature/example\n",
                ("status", "--porcelain=v1", "--untracked-files=all"): "",
                ("rev-parse", "--verify", "origin/feature/example"): None,
                ("symbolic-ref", "--short", "refs/remotes/origin/HEAD"): None,
                ("rev-parse", "--verify", "refs/remotes/origin/main"): f"{default_oid}\n",
                ("merge-base", "HEAD", "origin/main"): f"{default_oid}\n",
                ("diff", "--name-only", "--diff-filter=ACMR", f"{default_oid}..HEAD"): "src/main.rs\n",
                ("diff", "--no-ext-diff", "--no-textconv", "--unified=0", f"{default_oid}..HEAD", "--"): "diff --git a/x b/x\n+safe = true\n",
            }
            return values.get(args)

        with (
            mock.patch.object(GUARD, "_run_git", side_effect=safe_git),
            mock.patch.object(GUARD, "_remote_refs_snapshot", return_value=("origin/main", default_oid, None)),
        ):
            self.assertIsNone(
                GUARD._push_preflight_reason("/workspace", "feature/example")
            )

        with (
            mock.patch.object(GUARD, "_run_git", side_effect=safe_git),
            mock.patch.object(GUARD, "_remote_refs_snapshot", return_value=("origin/release/1", default_oid, None)),
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
            mock.patch.object(GUARD, "_remote_refs_snapshot", return_value=("origin/main", default_oid, None)),
        ):
            self.assertIsNotNone(
                GUARD._push_preflight_reason("/workspace", "feature/example")
            )

        with (
            mock.patch.object(GUARD, "_run_git", side_effect=safe_git),
            mock.patch.object(GUARD, "_remote_refs_snapshot", return_value=("origin/main", "d" * 40, None)),
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

    def test_branch_worktree_resolves_exact_registered_head(self):
        with tempfile.TemporaryDirectory(prefix="guard-pr-worktree-") as directory:
            root = Path(directory)
            head = root / "head"
            head.mkdir()
            records = (
                f"worktree {root / 'main'}\0HEAD {'b' * 40}\0branch refs/heads/main\0\0"
                f"worktree {head}\0HEAD {'a' * 40}\0"
                "branch refs/heads/feature/example\0locked\0\0"
            )
            with (
                mock.patch.object(GUARD, "_run_git", return_value=records),
                mock.patch.object(GUARD, "_resolved_git_path", return_value=head),
            ):
                self.assertEqual(
                    GUARD._branch_worktree(str(root / "main"), "feature/example"),
                    (str(head), None),
                )

            for invalid in (
                f"worktree {head}\0HEAD {'a' * 40}\0detached\0\0",
                f"worktree {head}\0HEAD {'a' * 40}\0branch refs/heads/feature/example\0prunable stale\0\0",
                records + f"worktree {head}\0branch refs/heads/feature/example\0\0",
            ):
                with (
                    self.subTest(records=invalid),
                    mock.patch.object(GUARD, "_run_git", return_value=invalid),
                    mock.patch.object(GUARD, "_resolved_git_path", return_value=head),
                ):
                    self.assertIsNotNone(
                        GUARD._branch_worktree(
                            str(root / "main"), "feature/example"
                        )[1]
                    )

    def test_draft_pr_preflight_binds_registered_clean_and_pushed_head(self):
        head_oid = "a" * 40
        base_oid = "b" * 40
        session = "/workspace"
        head_worktree = "/worktrees/feature"
        common = Path("/workspace/.git")

        def resolved(cwd, argument):
            if argument == "--git-common-dir":
                return common
            return Path(cwd)

        def safe_git(cwd, *args):
            if cwd != head_worktree:
                return None
            values = {
                ("rev-parse", "--abbrev-ref", "HEAD"): "feature/example\n",
                ("rev-parse", "HEAD"): f"{head_oid}\n",
                ("status", "--porcelain=v1", "--untracked-files=all"): "",
                ("rev-parse", "--verify", "refs/remotes/origin/main"): f"{base_oid}\n",
            }
            return values.get(args)

        with (
            mock.patch.object(
                GUARD, "_branch_worktree", return_value=(head_worktree, None)
            ),
            mock.patch.object(GUARD, "_resolved_git_path", side_effect=resolved),
            mock.patch.object(GUARD, "_origin_repository", return_value="owner/repo"),
            mock.patch.object(GUARD, "_run_git", side_effect=safe_git),
            mock.patch.object(
                GUARD,
                "_remote_refs_snapshot",
                return_value=("origin/main", base_oid, head_oid),
            ),
        ):
            self.assertIsNone(
                GUARD._draft_pr_preflight_reason(session, "main", "feature/example")
            )

        for status, snapshot in (
            (" M README.md\n", ("origin/main", base_oid, head_oid)),
            ("", ("origin/main", base_oid, "c" * 40)),
            ("", ("origin/trunk", base_oid, head_oid)),
            ("", None),
        ):
            def changed_git(cwd, *args, worktree_status=status):
                if args == ("status", "--porcelain=v1", "--untracked-files=all"):
                    return worktree_status
                return safe_git(cwd, *args)

            with (
                self.subTest(status=status, snapshot=snapshot),
                mock.patch.object(
                    GUARD, "_branch_worktree", return_value=(head_worktree, None)
                ),
                mock.patch.object(GUARD, "_resolved_git_path", side_effect=resolved),
                mock.patch.object(GUARD, "_origin_repository", return_value="owner/repo"),
                mock.patch.object(GUARD, "_run_git", side_effect=changed_git),
                mock.patch.object(GUARD, "_remote_refs_snapshot", return_value=snapshot),
            ):
                self.assertIsNotNone(
                    GUARD._draft_pr_preflight_reason(
                        session, "main", "feature/example"
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

    def test_write_context_validates_the_session_repository(self):
        repository = MODULE_PATH.resolve().parents[2]
        common = repository / ".git"
        with mock.patch.object(
            GUARD,
            "_resolved_git_path",
            side_effect=[None, None],
        ):
            self.assertIsNotNone(
                GUARD._write_context_reason("git add -- README.md", str(repository))
            )
        with mock.patch.object(
            GUARD,
            "_resolved_git_path",
            side_effect=[repository, common],
        ):
            self.assertIsNone(
                GUARD._write_context_reason("git add -- README.md", str(repository))
            )

        linked = Path(
            f"/tmp/codex-home/worktrees/{GUARD._repository_key('owner/repo')}/task-safe"
        )
        with (
            mock.patch.object(
                GUARD,
                "_resolved_git_path",
                side_effect=[linked, common],
            ),
            mock.patch.object(GUARD, "_managed_worktree_reason", return_value=None) as managed,
        ):
            self.assertIsNone(
                GUARD._write_context_reason("git add -- README.md", str(linked))
            )
            managed.assert_called_once_with(repository, common, linked)

    def test_git_dash_c_targets_only_a_managed_worktree(self):
        repository = Path("/workspace")
        common = repository / ".git"
        linked = Path("/codex/worktrees/5-owner--4-repo/task-safe")

        def resolved(cwd, argument):
            if argument == "--git-common-dir":
                return common
            return repository if Path(cwd) == repository else linked

        with (
            mock.patch.object(GUARD.Path, "resolve", return_value=linked),
            mock.patch.object(GUARD, "_resolved_git_path", side_effect=resolved),
            mock.patch.object(GUARD, "_managed_worktree_reason", return_value=None),
        ):
            self.assertEqual(
                GUARD._git_write_target(str(repository), str(linked)),
                (str(linked), None),
            )

        for explicit in ("relative/worktree", "../worktree"):
            with self.subTest(explicit=explicit):
                self.assertIsNotNone(
                    GUARD._git_write_target(str(repository), explicit)[1]
                )
        self.assert_blocked(
            "git -C /one -C /two add -- README.md", str(repository)
        )

        with (
            mock.patch.object(GUARD.Path, "resolve", return_value=linked),
            mock.patch.object(GUARD, "_resolved_git_path", side_effect=resolved),
            mock.patch.object(
                GUARD, "_managed_worktree_reason", return_value="manifest mismatch"
            ),
        ):
            self.assertIsNotNone(
                GUARD._git_write_target(str(repository), str(linked))[1]
            )

    def test_git_dash_c_write_preflight_uses_the_managed_worktree(self):
        target = "/codex/worktrees/5-owner--4-repo/task-safe"
        with (
            mock.patch.object(
                GUARD, "_git_write_target", return_value=(target, None)
            ) as resolve_target,
            mock.patch.object(GUARD, "_current_work_branch_reason", return_value=None) as branch,
        ):
            self.assert_allowed(
                f"git -C {target} add -- README.md", "/workspace"
            )
        resolve_target.assert_called_once_with("/workspace", target)
        branch.assert_called_once_with(target)

    def test_git_dash_c_read_targets_only_the_registered_worktree(self):
        with tempfile.TemporaryDirectory(prefix="guard-read-worktree-") as directory:
            root = Path(directory)
            repository = root / "repository"
            linked = root / "linked"
            other = root / "other"
            repository.mkdir()
            linked.mkdir()
            other.mkdir()
            common = repository / ".git"

            def resolved(cwd, argument):
                if argument == "--git-common-dir":
                    return common
                return repository if Path(cwd) == repository else Path(cwd)

            with (
                mock.patch.object(GUARD, "_resolved_git_path", side_effect=resolved),
                mock.patch.object(GUARD, "_managed_worktree_reason", return_value=None),
            ):
                self.assert_allowed(f"git -C {linked} status", str(repository))
                self.assert_allowed(f"git -C {linked} status", str(linked))

            for command, cwd in (
                (f"git -C {repository} status", str(repository)),
                (f"git -C {other} status", str(linked)),
                (f"git -C {linked} --no-pager status", str(repository)),
                (f"git -C {linked} --show-toplevel", str(repository)),
                ("git -C relative status", str(repository)),
                (f"git -C {linked} -C {other} status", str(repository)),
            ):
                with self.subTest(command=command, cwd=cwd):
                    with (
                        mock.patch.object(GUARD, "_resolved_git_path", side_effect=resolved),
                        mock.patch.object(GUARD, "_managed_worktree_reason", return_value=None),
                    ):
                        self.assert_blocked(command, cwd)

    def test_hook_uses_official_session_cwd_not_tool_workdir(self):
        stdout = io.StringIO()
        payload = json.dumps({
            "cwd": "/session",
            "tool_input": {
                "command": "git add -- README.md",
                "workdir": "/ignored",
            },
        })
        with (
            mock.patch.object(GUARD.sys, "stdin", io.StringIO(payload)),
            mock.patch.object(GUARD.sys, "stdout", stdout),
            mock.patch.object(GUARD, "_write_context_reason", return_value=None) as context,
            mock.patch.object(GUARD, "blocked_reason", return_value=None) as blocked,
            self.assertRaises(SystemExit) as exit_status,
        ):
            GUARD.main()
        self.assertEqual(exit_status.exception.code, 0)
        context.assert_called_once_with("git add -- README.md", "/session")
        blocked.assert_called_once_with("git add -- README.md", "/session")

    def test_missing_hook_command_fails_closed(self):
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

    def test_managed_worktree_requires_matching_manifest_and_registration(self):
        with tempfile.TemporaryDirectory(prefix="guard-worktree-") as directory:
            root = Path(directory)
            repository = root / "repository"
            common = repository / ".git"
            worktree = (
                root
                / "codex-home/worktrees"
                / GUARD._repository_key("owner/repo")
                / "task-safe"
            )
            state = worktree.parent / ".state"
            common.mkdir(parents=True)
            worktree.mkdir(parents=True)
            state.mkdir()
            state.chmod(0o700)
            manifest = {
                "version": 1,
                "status": "ready",
                "task_id": "task-safe",
                "repository": str(repository),
                "common_git_dir": str(common),
                "github_name": "owner/repo",
                "branch": "feat/example",
                "worktree": str(worktree),
            }
            manifest_path = state / "task-safe.json"
            manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
            manifest_path.chmod(0o600)

            def safe_git(_cwd, *args):
                if args == ("rev-parse", "--abbrev-ref", "HEAD"):
                    return "feat/example\n"
                if args == ("worktree", "list", "--porcelain", "-z"):
                    return f"worktree {repository}\0\0worktree {worktree}\0\0"
                return None

            with (
                mock.patch.dict(os.environ, {"CODEX_HOME": str(root / "codex-home")}),
                mock.patch.object(GUARD, "_origin_repository", return_value="owner/repo"),
                mock.patch.object(GUARD, "_run_git", side_effect=safe_git),
            ):
                self.assertIsNone(
                    GUARD._managed_worktree_reason(repository, common, worktree)
                )
                manifest["status"] = "failed"
                manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
                manifest_path.chmod(0o600)
                self.assertIsNotNone(
                    GUARD._managed_worktree_reason(repository, common, worktree)
                )

                manifest["status"] = "ready"
                manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
                manifest_path.chmod(0o600)
                external_state = root / "external-state"
                state.rename(external_state)
                state.symlink_to(external_state, target_is_directory=True)
                self.assertIsNotNone(
                    GUARD._managed_worktree_reason(repository, common, worktree)
                )

    def test_managed_worktree_rejects_symlink_component_in_codex_home(self):
        with tempfile.TemporaryDirectory(prefix="guard-worktree-symlink-") as directory:
            root = Path(directory)
            real = root / "real"
            link = root / "link"
            real.mkdir()
            link.symlink_to(real, target_is_directory=True)
            with (
                mock.patch.dict(os.environ, {"CODEX_HOME": str(link / "codex-home")}),
                mock.patch.object(GUARD, "_origin_repository", return_value="owner/repo"),
            ):
                reason = GUARD._managed_worktree_reason(
                    root / "repository", root / "repository/.git", root / "worktree"
                )
                self.assertIsNotNone(reason)
                self.assertIn("symlink component", reason)

    def test_managed_worktree_rejects_symlinked_worktrees_root(self):
        with tempfile.TemporaryDirectory(prefix="guard-worktree-root-link-") as directory:
            root = Path(directory)
            codex_home = root / "codex-home"
            external = root / "external" / GUARD._repository_key("owner/repo") / "task-safe"
            external.mkdir(parents=True)
            codex_home.mkdir()
            (codex_home / "worktrees").symlink_to(root / "external", target_is_directory=True)
            with (
                mock.patch.dict(os.environ, {"CODEX_HOME": str(codex_home)}),
                mock.patch.object(GUARD, "_origin_repository", return_value="owner/repo"),
            ):
                reason = GUARD._managed_worktree_reason(
                    root / "repository", root / "repository/.git", external
                )
            self.assertIsNotNone(reason)


if __name__ == "__main__":
    unittest.main()

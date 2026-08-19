#!/usr/bin/env python3
"""codex-worktree lifecycle helperの単体・integration test。"""

from __future__ import annotations

import importlib.machinery
import importlib.util
import json
import multiprocessing
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock


MODULE_PATH = Path(__file__).with_name("codex-worktree")
LOADER = importlib.machinery.SourceFileLoader("codex_worktree", str(MODULE_PATH))
SPEC = importlib.util.spec_from_loader(LOADER.name, LOADER)
assert SPEC is not None
HELPER = importlib.util.module_from_spec(SPEC)
sys.modules[LOADER.name] = HELPER
LOADER.exec_module(HELPER)


def run(*args: str, cwd: Path) -> str:
    return subprocess.run(
        list(args), cwd=cwd, check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
    ).stdout


def create_in_child(repository: str, branch: str, task_id: str, codex_home: str, queue) -> None:
    os.environ["CODEX_HOME"] = codex_home
    try:
        path = HELPER.create_worktree(
            Path(repository), branch, task_id, allow_local_origin=True,
        )
        queue.put((True, str(path)))
    except Exception as error:  # test processから例外内容を回収する
        queue.put((False, str(error)))


class TemporaryRepository:
    def __init__(self) -> None:
        self.directory = tempfile.TemporaryDirectory(prefix="codex-worktree-test-")
        root = Path(self.directory.name)
        self.remote = root / "remote.git"
        self.repository = root / "main"
        self.codex_home = root / "codex-home"
        self.managed_repository = (
            self.codex_home / "worktrees" / HELPER.repository_key("test/local")
        )
        run("git", "init", "--bare", str(self.remote), cwd=root)
        run("git", "init", "--initial-branch=main", str(self.repository), cwd=root)
        run("git", "config", "user.name", "Test User", cwd=self.repository)
        run("git", "config", "user.email", "test@example.invalid", cwd=self.repository)
        (self.repository / "README.md").write_text("test\n", encoding="utf-8")
        run("git", "add", "--", "README.md", cwd=self.repository)
        run("git", "commit", "-m", "initial", cwd=self.repository)
        run("git", "remote", "add", "origin", str(self.remote), cwd=self.repository)
        run("git", "push", "-u", "origin", "main", cwd=self.repository)
        run("git", "symbolic-ref", "HEAD", "refs/heads/main", cwd=self.remote)

    def close(self) -> None:
        self.directory.cleanup()


class HelperTest(unittest.TestCase):
    def setUp(self) -> None:
        self.repo = TemporaryRepository()
        self.environment = mock.patch.dict(os.environ, {"CODEX_HOME": str(self.repo.codex_home)})
        self.environment.start()

    def tearDown(self) -> None:
        self.environment.stop()
        self.repo.close()

    def test_task_id_generation_and_validation(self):
        self.assertEqual(HELPER.normalize_task_id(22, None), "issue-22")
        generated = HELPER.normalize_task_id(None, None)
        self.assertRegex(generated, r"^task-[0-9]{8}t[0-9]{6}z-[0-9a-f]{8}$")
        self.assertEqual(HELPER.normalize_task_id(None, "task-safe-id"), "task-safe-id")
        for invalid in ("../task", "/tmp/task", "task-UPPER", "issue-0", "plain"):
            with self.subTest(invalid=invalid), self.assertRaises(HELPER.WorktreeError):
                HELPER.normalize_task_id(None, invalid)

    def test_branch_validation_rejects_unsafe_and_protected_names(self):
        for branch in ("main", "release/2026-08", "codex/example", "feat/a..b", "feat/x.lock", "Feat/x"):
            with self.subTest(branch=branch), self.assertRaises(HELPER.WorktreeError):
                HELPER.validate_branch(branch, self.repo.repository)
        HELPER.validate_branch("feat/safe-worktree", self.repo.repository)

    def test_github_remote_parsing_is_strict(self):
        for remote in (
            "https://github.com/owner/repo.git",
            "git@github.com:owner/repo.git",
            "ssh://git@github.com/owner/repo",
        ):
            self.assertEqual(HELPER._github_repository(remote), "owner/repo")
        self.assertIsNone(HELPER._github_repository("https://example.com/owner/repo.git"))
        self.assertNotEqual(
            HELPER.repository_key("a--b/c"), HELPER.repository_key("a/b--c")
        )

    def test_create_preserves_dirty_main_and_creates_clean_worktree(self):
        local = self.repo.repository / "local.txt"
        local.write_text("untracked\n", encoding="utf-8")
        before = HELPER._snapshot(self.repo.repository)
        target = HELPER.create_worktree(
            self.repo.repository, "feat/first", "issue-22", allow_local_origin=True,
        )
        self.assertEqual(HELPER._snapshot(self.repo.repository), before)
        self.assertEqual(run("git", "rev-parse", "--abbrev-ref", "HEAD", cwd=target).strip(), "feat/first")
        self.assertEqual(run("git", "status", "--porcelain=v1", cwd=target), "")
        manifest = json.loads(
            (self.repo.managed_repository / ".state/issue-22.json").read_text(encoding="utf-8")
        )
        self.assertEqual(manifest["status"], "ready")
        self.assertEqual(manifest["worktree"], str(target))

    def test_duplicate_task_branch_and_path_are_rejected_without_cleanup(self):
        target = HELPER.create_worktree(
            self.repo.repository, "feat/first", "issue-22", allow_local_origin=True,
        )
        with self.assertRaises(HELPER.WorktreeError):
            HELPER.create_worktree(
                self.repo.repository, "feat/second", "issue-22", allow_local_origin=True,
            )
        with self.assertRaises(HELPER.WorktreeError):
            HELPER.create_worktree(
                self.repo.repository, "feat/first", "task-another", allow_local_origin=True,
            )
        self.assertTrue(target.exists())

    def test_two_worktrees_can_be_created_concurrently(self):
        context = multiprocessing.get_context("fork")
        queue = context.Queue()
        processes = [
            context.Process(
                target=create_in_child,
                args=(
                    str(self.repo.repository), f"feat/concurrent-{index}", f"task-concurrent-{index}",
                    str(self.repo.codex_home), queue,
                ),
            )
            for index in (1, 2)
        ]
        for process in processes:
            process.start()
        for process in processes:
            process.join(30)
            self.assertEqual(process.exitcode, 0)
        results = [queue.get(timeout=5) for _ in processes]
        self.assertTrue(all(success for success, _ in results), results)
        paths = {path for _, path in results}
        self.assertEqual(len(paths), 2)
        self.assertTrue(all(Path(path).exists() for path in paths))

    def test_safe_environment_removes_command_bearing_git_variables(self):
        unsafe = {
            "GIT_EXEC_PATH": "/tmp/commands",
            "GIT_SSH": "/tmp/ssh",
            "GIT_SSH_COMMAND": "malicious command",
            "GIT_ASKPASS": "/tmp/askpass",
            "SSH_ASKPASS": "/tmp/ssh-askpass",
            "GIT_PROXY_COMMAND": "malicious proxy",
        }
        with mock.patch.dict(os.environ, unsafe, clear=False):
            environment = HELPER._safe_environment()
        self.assertTrue(unsafe.keys().isdisjoint(environment))

    def test_diagnose_rejects_symlinked_managed_root(self):
        HELPER.create_worktree(
            self.repo.repository, "feat/root-link", "task-root-link", allow_local_origin=True,
        )
        root = self.repo.codex_home / "worktrees"
        external = self.repo.codex_home / "external-worktrees"
        root.rename(external)
        root.symlink_to(external, target_is_directory=True)
        with self.assertRaises(HELPER.WorktreeError):
            HELPER.diagnose(
                self.repo.repository, "task-root-link", allow_local_origin=True,
            )

    def test_doctor_and_resume_report_ready_dirty_and_missing(self):
        target = HELPER.create_worktree(
            self.repo.repository, "feat/doctor", "task-doctor", allow_local_origin=True,
        )
        self.assertEqual(
            HELPER.diagnose(self.repo.repository, "task-doctor", allow_local_origin=True)[0][1],
            "ready",
        )
        self.assertEqual(
            HELPER.resume(self.repo.repository, "task-doctor", allow_local_origin=True), target,
        )
        (target / "dirty.txt").write_text("dirty\n", encoding="utf-8")
        self.assertEqual(
            HELPER.diagnose(self.repo.repository, "task-doctor", allow_local_origin=True)[0][1],
            "dirty",
        )
        target.rename(target.with_name("missing-preserved"))
        self.assertEqual(
            HELPER.diagnose(self.repo.repository, "task-doctor", allow_local_origin=True)[0][1],
            "missing",
        )

    def test_recover_promotes_only_a_valid_interrupted_worktree(self):
        target = HELPER.create_worktree(
            self.repo.repository, "feat/recover", "task-recover", allow_local_origin=True,
        )
        manifest_path = self.repo.managed_repository / ".state/task-recover.json"
        payload = json.loads(manifest_path.read_text(encoding="utf-8"))
        payload["status"] = "creating"
        manifest_path.write_text(json.dumps(payload), encoding="utf-8")

        self.assertEqual(
            HELPER.diagnose(self.repo.repository, "task-recover", allow_local_origin=True)[0][1],
            "interrupted",
        )
        with self.assertRaises(HELPER.WorktreeError):
            HELPER.resume(self.repo.repository, "task-recover", allow_local_origin=True)
        self.assertEqual(
            HELPER.recover(self.repo.repository, "task-recover", allow_local_origin=True), target,
        )
        self.assertEqual(
            HELPER.diagnose(self.repo.repository, "task-recover", allow_local_origin=True)[0][1],
            "ready",
        )
        payload = json.loads(manifest_path.read_text(encoding="utf-8"))
        payload["status"] = "failed"
        payload["detail"] = "simulated failure"
        manifest_path.write_text(json.dumps(payload), encoding="utf-8")
        (target / "dirty.txt").write_text("preserve\n", encoding="utf-8")
        self.assertEqual(
            HELPER.diagnose(self.repo.repository, "task-recover", allow_local_origin=True)[0][1],
            "failed",
        )
        with self.assertRaises(HELPER.WorktreeError):
            HELPER.resume(self.repo.repository, "task-recover", allow_local_origin=True)

    def test_recover_rejects_an_interrupted_worktree_with_a_changed_head(self):
        target = HELPER.create_worktree(
            self.repo.repository, "feat/diverged", "task-diverged", allow_local_origin=True,
        )
        manifest_path = self.repo.managed_repository / ".state/task-diverged.json"
        payload = json.loads(manifest_path.read_text(encoding="utf-8"))
        payload["status"] = "creating"
        manifest_path.write_text(json.dumps(payload), encoding="utf-8")
        (target / "changed.txt").write_text("changed\n", encoding="utf-8")
        run("git", "add", "--", "changed.txt", cwd=target)
        run("git", "commit", "-m", "changed head", cwd=target)

        self.assertEqual(
            HELPER.diagnose(self.repo.repository, "task-diverged", allow_local_origin=True)[0][1],
            "diverged",
        )
        with self.assertRaises(HELPER.WorktreeError):
            HELPER.recover(self.repo.repository, "task-diverged", allow_local_origin=True)

    @unittest.skipUnless(hasattr(os, "mkfifo"), "FIFO requires POSIX")
    def test_non_directory_worktree_is_isolated_during_diagnosis(self):
        target = HELPER.create_worktree(
            self.repo.repository, "feat/broken-target", "task-broken-target",
            allow_local_origin=True,
        )
        target.rename(target.with_name("broken-target-preserved"))
        os.mkfifo(target)
        self.assertEqual(
            HELPER.diagnose(
                self.repo.repository, "task-broken-target", allow_local_origin=True,
            )[0][1],
            "invalid",
        )

    def test_invalid_manifest_is_isolated_and_cannot_escape_managed_root(self):
        HELPER.create_worktree(
            self.repo.repository, "feat/invalid", "task-invalid", allow_local_origin=True,
        )
        manifest_path = self.repo.managed_repository / ".state/task-invalid.json"
        payload = json.loads(manifest_path.read_text(encoding="utf-8"))
        payload["worktree"] = str(self.repo.repository)
        payload["branch"] = "main"
        manifest_path.write_text(json.dumps(payload), encoding="utf-8")
        self.assertEqual(
            HELPER.diagnose(self.repo.repository, "task-invalid", allow_local_origin=True)[0][1],
            "invalid",
        )
        with self.assertRaises(HELPER.WorktreeError):
            HELPER.resume(self.repo.repository, "task-invalid", allow_local_origin=True)

        payload["worktree"] = 1
        manifest_path.write_text(json.dumps(payload), encoding="utf-8")
        self.assertEqual(
            HELPER.diagnose(self.repo.repository, "task-invalid", allow_local_origin=True)[0][1],
            "invalid",
        )

    @unittest.skipUnless(hasattr(os, "mkfifo"), "FIFO requires POSIX")
    def test_non_regular_manifest_is_reported_without_reading_it(self):
        state = self.repo.managed_repository / ".state"
        state.mkdir(parents=True)
        os.mkfifo(state / "task-fifo.json")
        self.assertEqual(
            HELPER.diagnose(self.repo.repository, "task-fifo", allow_local_origin=True)[0][1],
            "invalid",
        )

    def test_symlink_managed_root_is_rejected(self):
        real = Path(self.repo.directory.name) / "real-home"
        real.mkdir()
        self.repo.codex_home.symlink_to(real, target_is_directory=True)
        with self.assertRaises(HELPER.WorktreeError):
            HELPER.create_worktree(
                self.repo.repository, "feat/symlink", "task-symlink", allow_local_origin=True,
            )

    def test_symlink_parent_of_codex_home_is_rejected(self):
        real = Path(self.repo.directory.name) / "real-parent"
        link = Path(self.repo.directory.name) / "linked-parent"
        real.mkdir()
        link.symlink_to(real, target_is_directory=True)
        os.environ["CODEX_HOME"] = str(link / "codex-home")
        with self.assertRaises(HELPER.WorktreeError):
            HELPER.create_worktree(
                self.repo.repository, "feat/parent-symlink", "task-parent-symlink",
                allow_local_origin=True,
            )

    @unittest.skipUnless(os.name == "posix", "executable Git hook requires POSIX")
    def test_create_disables_repository_git_hooks(self):
        hooks = Path(self.repo.directory.name) / "hooks"
        hooks.mkdir()
        marker = Path(self.repo.directory.name) / "post-checkout-ran"
        post_checkout = hooks / "post-checkout"
        post_checkout.write_text(f"#!/bin/sh\ntouch '{marker}'\n", encoding="utf-8")
        post_checkout.chmod(0o755)
        run("git", "config", "core.hooksPath", str(hooks), cwd=self.repo.repository)
        HELPER.create_worktree(
            self.repo.repository, "feat/no-hooks", "task-no-hooks", allow_local_origin=True,
        )
        self.assertFalse(marker.exists())


if __name__ == "__main__":
    unittest.main()

#!/usr/bin/env python3
"""codex-deliveryのnetworkless safety test。"""

from __future__ import annotations

from contextlib import ExitStack, nullcontext
import importlib.machinery
import importlib.util
import fcntl
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock


MODULE_PATH = Path(__file__).with_name("codex-delivery")
LOADER = importlib.machinery.SourceFileLoader("codex_delivery", str(MODULE_PATH))
SPEC = importlib.util.spec_from_loader(LOADER.name, LOADER)
assert SPEC is not None
HELPER = importlib.util.module_from_spec(SPEC)
LOADER.exec_module(HELPER)


class DeliveryTest(unittest.TestCase):
    def setUp(self) -> None:
        self.directory = tempfile.TemporaryDirectory(prefix="codex-delivery-test-")
        self.root = Path(self.directory.name) / "repo"
        self.root.mkdir()
        self.codex_home = Path(self.directory.name) / "codex-home"
        managed = self.codex_home / "worktrees" / HELPER._repo_key("owner/repo")
        for path in (self.codex_home, self.codex_home / "worktrees", managed, managed / ".state", managed / ".locks"):
            path.mkdir()
            path.chmod(0o700)
        self.worktree = managed / "issue-24"
        self.worktree.mkdir()
        self.manifest = {
            "version": 1, "status": "ready", "task_id": "issue-24",
            "repository": str(self.root), "common_git_dir": str(self.root / ".git"),
            "github_name": "owner/repo", "branch": "feat/issue-24",
            "base": "origin/main", "base_oid": "a" * 40,
            "worktree": str(self.worktree), "created_at": "2026-08-19T00:00:00+00:00", "detail": "",
        }
        (managed / ".state" / "issue-24.json").write_text(json.dumps(self.manifest), encoding="utf-8")
        self.environment = mock.patch.dict("os.environ", {"CODEX_HOME": str(self.codex_home)}, clear=False)
        self.environment.start()

    def tearDown(self) -> None:
        self.environment.stop()
        self.directory.cleanup()

    def test_parser_requires_explicit_review_evidence_and_fixed_delivery_identity(self) -> None:
        with self.assertRaises(SystemExit):
            HELPER._parser().parse_args(["record-review", "--task-id", "issue-24"])
        args = HELPER._parser().parse_args([
            "record-review", "--task-id", "issue-24", "--pr", "24", "--head", "b" * 40,
            "--risk", "low", "--plan-id", "CODEX-DELIVERY-TEST-v1", "--tests-passed",
            "--neutral-review-passed", "--adversarial-review-passed",
        ])
        self.assertTrue(args.tests_passed)
        deliver = HELPER._parser().parse_args([
            "deliver", "--task-id", "issue-24", "--pr", "24", "--head", "b" * 40,
            "--plan-id", "CODEX-DELIVERY-TEST-v1",
        ])
        self.assertEqual((deliver.pr, deliver.head, deliver.plan_id), (24, "b" * 40, "CODEX-DELIVERY-TEST-v1"))

    def _record(self, changed: list[str], *, approve: bool = False) -> dict[str, object]:
        head = "b" * 40
        with mock.patch.object(HELPER, "_repository", return_value="owner/repo"), \
             mock.patch.object(HELPER, "_manifest", return_value=(self.manifest, self.worktree)), \
             mock.patch.object(HELPER, "_worktree"), \
             mock.patch.object(HELPER, "_git", side_effect=["b" * 40, "", "b" * 40]), \
             mock.patch.object(HELPER, "_changed_files", return_value=changed):
            return HELPER._write_review(
                self.root, "issue-24", 24, head, "high" if approve else "low", "CODEX-DELIVERY-TEST-v1", approve,
                True, True, True,
            )

    def test_record_review_writes_head_scoped_machine_receipt_and_is_idempotent(self) -> None:
        receipt = self._record(["src/main.py"])
        path = self.codex_home / "worktrees" / HELPER._repo_key("owner/repo") / ".state" / ("issue-24.receipt." + "b" * 40 + ".json")
        self.assertTrue(path.is_file())
        self.assertEqual(json.loads(path.read_text(encoding="utf-8"))["actionable"], 0)
        again = self._record(["src/main.py"])
        self.assertEqual(receipt, again)

    def test_low_review_rejects_safety_boundary_and_approve_review_allows_explicit_high(self) -> None:
        for path in (".github/actions/custom.yml", ".codex/config.base.toml", "packages/cli/Cargo.toml", "Cargo.lock"):
            with self.subTest(path=path), self.assertRaises(HELPER.DeliveryError):
                self._record([path])
        receipt = self._record([".github/workflows/ci.yml"], approve=True)
        self.assertTrue(receipt["human_approved"])
        self.assertEqual(receipt["risk"], "high")

    def test_human_approval_can_upgrade_same_head_receipt(self) -> None:
        low = self._record(["src/main.py"])
        self.assertFalse(low["human_approved"])
        high = self._record(["src/main.py"], approve=True)
        self.assertTrue(high["human_approved"])
        self.assertEqual(high["risk"], "high")

    def test_receipt_identity_mismatch_is_fail_closed_before_network(self) -> None:
        receipt = self._record(["src/main.py"])
        with self.assertRaises(HELPER.DeliveryError):
            HELPER._match_cli_receipt(receipt, pr=25, head="b" * 40, plan="CODEX-DELIVERY-TEST-v1")
        with self.assertRaises(HELPER.DeliveryError):
            HELPER._match_cli_receipt(receipt, pr=24, head="c" * 40, plan="CODEX-DELIVERY-TEST-v1")

    def test_required_ci_rejects_duplicate_or_wrong_app_without_network(self) -> None:
        pages = [{"check_runs": [
            {"name": "required-ci", "app": {"id": 15368}, "head_sha": "b" * 40,
             "status": "completed", "conclusion": "success", "completed_at": "now"},
            {"name": "required-ci", "app": {"id": 15368}, "head_sha": "b" * 40,
             "status": "completed", "conclusion": "success", "completed_at": "now"},
        ]}]
        with mock.patch.object(HELPER, "_gh_json", return_value=pages):
            with self.assertRaises(HELPER.DeliveryError):
                HELPER._check_required_ci(self.root, "owner/repo", "b" * 40)

    def test_required_ci_rejects_every_non_success_variant(self) -> None:
        variants = (
            {"status": "queued", "conclusion": None, "head_sha": "b" * 40, "app": {"id": 15368}},
            {"status": "completed", "conclusion": "failure", "head_sha": "b" * 40, "app": {"id": 15368}},
            {"status": "completed", "conclusion": "success", "head_sha": "c" * 40, "app": {"id": 15368}},
            {"status": "completed", "conclusion": "success", "head_sha": "b" * 40, "app": {"id": 999}},
        )
        for variant in variants:
            with self.subTest(variant=variant), mock.patch.object(
                HELPER, "_gh_json", return_value=[{"check_runs": [{
                    "name": "required-ci", "completed_at": "now", **variant,
                }]}],
            ), self.assertRaises(HELPER.DeliveryError):
                HELPER._check_required_ci(self.root, "owner/repo", "b" * 40)

    def test_graphql_pagination_rejects_unresolved_thread_and_changes_requested(self) -> None:
        thread_page = {"repository": {"pullRequest": {"reviewThreads": {
            "nodes": [{"isResolved": True}], "pageInfo": {"hasNextPage": True, "endCursor": "cursor-1"},
        }}}}
        unresolved_page = {"repository": {"pullRequest": {"reviewThreads": {
            "nodes": [{"isResolved": False}], "pageInfo": {"hasNextPage": False, "endCursor": None},
        }}}}
        with mock.patch.object(HELPER, "_graphql", side_effect=[thread_page, unresolved_page]):
            with self.assertRaises(HELPER.DeliveryError):
                HELPER._review_safety(self.root, "owner/repo", 24)

        resolved = {"repository": {"pullRequest": {"reviewThreads": {
            "nodes": [{"isResolved": True}], "pageInfo": {"hasNextPage": False, "endCursor": None},
        }}}}
        changes_requested = {"repository": {"pullRequest": {"reviewDecision": "CHANGES_REQUESTED"}}}
        with mock.patch.object(HELPER, "_graphql", side_effect=[resolved, changes_requested]):
            with self.assertRaises(HELPER.DeliveryError):
                HELPER._review_safety(self.root, "owner/repo", 24)
        approved = {"repository": {"pullRequest": {"reviewDecision": "APPROVED"}}}
        with mock.patch.object(HELPER, "_graphql", side_effect=[resolved, approved]):
            HELPER._review_safety(self.root, "owner/repo", 24)

    def test_draft_preflight_never_changes_ready_state_when_remote_check_fails(self) -> None:
        view = {
            "number": 24, "state": "OPEN", "isDraft": True, "headRefOid": "b" * 40,
            "headRefName": "feat/issue-24", "baseRefName": "main", "mergeable": "UNKNOWN",
            "mergeStateStatus": "DRAFT", "mergedAt": None,
            "headRepository": {"nameWithOwner": "owner/repo"},
            "headRepositoryOwner": {"login": "owner"}, "isCrossRepository": False,
            "autoMergeRequest": None,
        }
        receipt = {"repository": "owner/repo", "pr": 24, "head_sha": "b" * 40}
        with mock.patch.object(HELPER, "_pr_view", return_value=view), \
             mock.patch.object(HELPER, "_default_branch", side_effect=HELPER.DeliveryError("drift")), \
             mock.patch.object(HELPER, "_gh") as gh:
            with self.assertRaises(HELPER.DeliveryError):
                HELPER._validate_delivery(self.root, receipt, allow_draft=True, expected_branch="feat/issue-24")
            gh.assert_not_called()

    def test_old_head_receipt_is_ignored_when_exact_head_is_requested(self) -> None:
        first = self._record(["src/main.py"])
        old = dict(first)
        old["head_sha"] = "c" * 40
        old_path = HELPER._receipt_path("owner/repo", "issue-24", "c" * 40)
        old_path.write_text(json.dumps(old), encoding="utf-8")
        loaded = HELPER._load_receipt(self.root, "issue-24", "b" * 40, "owner/repo")
        self.assertEqual(loaded["head_sha"], "b" * 40)

    def test_manifest_symlink_is_rejected(self) -> None:
        managed = self.codex_home / "worktrees" / HELPER._repo_key("owner/repo")
        manifest = managed / ".state" / "issue-24.json"
        target = managed / ".state" / "manifest-target.json"
        manifest.rename(target)
        manifest.symlink_to(target)
        with mock.patch.object(HELPER, "_git", return_value=str(self.root)):
            with self.assertRaises(HELPER.DeliveryError):
                HELPER._manifest(self.root, "issue-24", "owner/repo")

    def test_managed_repository_root_symlink_is_rejected(self) -> None:
        worktrees = self.codex_home / "worktrees"
        managed = worktrees / HELPER._repo_key("owner/repo")
        moved = worktrees / "moved-repo"
        managed.rename(moved)
        managed.symlink_to(moved, target_is_directory=True)
        with self.assertRaises(HELPER.DeliveryError):
            HELPER._managed_root("owner/repo")

    def test_ruleset_bypass_and_drift_are_rejected(self) -> None:
        valid = {
            "id": 1, "source": "owner/repo", "source_type": "Repository",
            "target": "branch", "enforcement": "active", "bypass_actors": [],
            "current_user_can_bypass": "never",
            "conditions": {"ref_name": {"include": ["~DEFAULT_BRANCH"], "exclude": []}},
            "rules": [
                {"type": "deletion"}, {"type": "non_fast_forward"},
                {"type": "pull_request", "parameters": {"allowed_merge_methods": ["merge"], "required_review_thread_resolution": True}},
                {"type": "required_status_checks", "parameters": {"strict_required_status_checks_policy": True, "required_status_checks": [{"context": "required-ci", "integration_id": 15368}]}},
            ],
        }
        summary = {"id": 1, "target": "branch", "enforcement": "active"}
        with mock.patch.object(HELPER, "_gh_json", side_effect=[[summary], {**valid, "bypass_actors": [{"actor_id": 1}]}]):
            with self.assertRaises(HELPER.DeliveryError):
                HELPER._ruleset(self.root, "owner/repo")
        drift = {**valid, "rules": [valid["rules"][0], valid["rules"][1], {"type": "pull_request", "parameters": {"allowed_merge_methods": ["rebase"], "required_review_thread_resolution": True}}, valid["rules"][3]]}
        with mock.patch.object(HELPER, "_gh_json", side_effect=[[summary], drift]):
            with self.assertRaises(HELPER.DeliveryError):
                HELPER._ruleset(self.root, "owner/repo")

    def test_environment_pins_github_host_and_removes_repository_overrides(self) -> None:
        with mock.patch.dict(
            "os.environ",
            {"GH_HOST": "example.test", "GH_REPO": "attacker/repo", "GH_CONFIG_DIR": "/tmp/unsafe"},
            clear=False,
        ):
            environment = HELPER._safe_environment()
        self.assertEqual(environment["GH_HOST"], "github.com")
        self.assertEqual(environment["PATH"], "/usr/bin:/bin")
        self.assertNotIn("GH_REPO", environment)
        self.assertNotIn("GH_CONFIG_DIR", environment)

    def test_external_commands_use_fixed_non_writable_system_binaries(self) -> None:
        git = HELPER._git_command("status")
        self.assertEqual(git[:3], ["/usr/bin/git", "-c", "core.hooksPath=/dev/null"])
        unsafe = Path(self.directory.name) / "git"
        unsafe.write_text("#!/bin/sh\n", encoding="utf-8")
        unsafe.chmod(0o755)
        with mock.patch.object(HELPER, "GIT_BINARY", unsafe):
            with self.assertRaises(HELPER.DeliveryError):
                HELPER._git_command("status")

    def test_http_github_origin_is_rejected(self) -> None:
        self.assertIsNone(HELPER._github_repository("http://github.com/owner/repo.git"))

    def test_receipt_changed_files_are_recomputed_and_must_match_exactly(self) -> None:
        receipt = {
            "risk": "low", "human_approved": False,
            "head_sha": "b" * 40, "changed_files": ["src/main.py"],
        }
        with mock.patch.object(HELPER, "_changed_files", return_value=["src/other.py"]):
            with self.assertRaises(HELPER.DeliveryError):
                HELPER._validate_receipt_evidence(self.worktree, self.manifest, receipt)

    def test_operation_deadline_stops_before_starting_another_command(self) -> None:
        with mock.patch.object(HELPER.time, "monotonic", side_effect=[0.0, 301.0]), \
             mock.patch.object(HELPER.subprocess, "run") as run:
            with HELPER._operation_deadline(300):
                with self.assertRaises(HELPER.DeliveryError):
                    HELPER._run(["git", "status"], cwd=self.root)
        run.assert_not_called()

    def test_canonical_invocation_rejects_interpreter_path(self) -> None:
        installed = Path(self.directory.name) / "bin" / "codex-delivery"
        installed.parent.mkdir()
        installed.write_text("#!/bin/sh\n", encoding="utf-8")
        with mock.patch.object(HELPER.shutil, "which", return_value=str(installed)), \
             mock.patch.object(HELPER.sys, "argv", [str(MODULE_PATH)]):
            with self.assertRaises(HELPER.DeliveryError):
                HELPER._require_canonical_invocation()

    def test_ignored_files_make_managed_worktree_not_clean(self) -> None:
        with mock.patch.object(HELPER, "_git", return_value="!! target/\n"):
            with self.assertRaises(HELPER.DeliveryError):
                HELPER._worktree_clean_head(self.worktree, "b" * 40)

    def test_missing_delivery_state_is_fail_closed(self) -> None:
        receipt = {
            "repository": "owner/repo", "pr": 24, "head_sha": "b" * 40,
        }
        with self.assertRaises(HELPER.DeliveryError):
            HELPER._load_state("owner/repo", "issue-24", receipt, "feat/issue-24")

    def test_deliver_success_uses_ready_fixed_head_merge_and_persists_state(self) -> None:
        head = "b" * 40
        receipt = {
            "version": 1, "kind": "review", "task_id": "issue-24", "repository": "owner/repo",
            "pr": 24, "head_sha": head, "risk": "low", "plan_id": "CODEX-DELIVERY-TEST-v1",
            "actionable": 0, "human_approved": False, "tests_passed": True,
            "neutral_review_passed": True, "adversarial_review_passed": True,
            "changed_files": ["src/main.py"], "created_at": "now",
        }
        draft = {"state": "OPEN", "isDraft": True, "headRefOid": head}
        open_pr = {
            "state": "OPEN", "isDraft": False, "headRefOid": head,
            "headRefName": "feat/issue-24", "baseRefName": "main",
            "isCrossRepository": False, "headRepository": {"nameWithOwner": "owner/repo"},
            "headRepositoryOwner": {"login": "owner"}, "autoMergeRequest": None,
            "mergeable": "MERGEABLE", "mergeStateStatus": "CLEAN",
        }
        merged = {"state": "MERGED", "isDraft": False, "headRefOid": head}
        with mock.patch.object(HELPER, "_repository", return_value="owner/repo"), \
             mock.patch.object(HELPER, "_manifest", return_value=(self.manifest, self.worktree)), \
             mock.patch.object(HELPER, "_load_receipt", return_value=receipt), \
             mock.patch.object(HELPER, "_match_cli_receipt"), \
             mock.patch.object(HELPER, "_worktree"), \
             mock.patch.object(HELPER, "_worktree_clean_head"), \
             mock.patch.object(HELPER, "_validate_receipt_evidence"), \
             mock.patch.object(HELPER, "_validate_delivery"), \
             mock.patch.object(HELPER, "_pr_view", side_effect=[draft, open_pr, merged]), \
             mock.patch.object(HELPER, "_gh") as gh, \
             mock.patch.object(HELPER, "_task_lock", return_value=nullcontext()):
            state = HELPER._deliver_locked(
                self.root, "issue-24", expected_pr=24, expected_head=head,
                expected_plan="CODEX-DELIVERY-TEST-v1",
            )
        self.assertEqual(state["stage"], "merged")
        self.assertEqual(
            gh.call_args_list,
            [
                mock.call(self.root, "pr", "ready", "24", "--repo", "owner/repo"),
                mock.call(
                    self.root, "pr", "merge", "24", "--repo", "owner/repo", "--merge",
                    "--match-head-commit", head,
                ),
            ],
        )
    def _finish_mock(self, stage: str, *, unlocked_dirty: bool = False, remote: str | None = None) -> tuple[dict[str, object], list[list[str]], mock.Mock]:
        head = "b" * 40
        receipt = {
            "version": 1, "kind": "review", "task_id": "issue-24", "repository": "owner/repo",
            "pr": 24, "head_sha": head, "risk": "low", "plan_id": "CODEX-DELIVERY-TEST-v1", "actionable": 0,
            "human_approved": False, "tests_passed": True, "neutral_review_passed": True,
            "adversarial_review_passed": True, "changed_files": ["src/main.py"], "created_at": "now",
        }
        state = {"version": 1, "kind": "delivery", "task_id": "issue-24", "repository": "owner/repo", "pr": 24,
                 "head_sha": head, "branch": "feat/issue-24", "stage": stage, "updated_at": "now", "last_error": ""}
        view = {"state": "MERGED", "headRefOid": head, "headRefName": "feat/issue-24", "baseRefName": "main",
                "headRepository": {"nameWithOwner": "owner/repo"}, "headRepositoryOwner": {"login": "owner"},
                "isCrossRepository": False, "autoMergeRequest": None}
        records = [{"worktree": str(self.worktree), "branch": "refs/heads/feat/issue-24", "locked": "codex-task:issue-24"}]
        unlocked = [{"worktree": str(self.worktree), "branch": "refs/heads/feat/issue-24"}]
        calls: list[list[str]] = []
        status_count = 0
        clean_head = mock.Mock(
            side_effect=[None, HELPER.DeliveryError("dirty race")]
            if unlocked_dirty else None,
        )

        def fake_git(cwd: Path, *args: str, **kwargs: object) -> str:
            nonlocal status_count
            calls.append(list(args))
            if args[:1] == ("status",):
                status_count += 1
                return "M dirty" if unlocked_dirty and status_count > 1 else ""
            if args[:2] == ("rev-parse", "HEAD"):
                return head
            if args[:2] == ("rev-parse", "refs/heads/feat/issue-24"):
                return head
            return ""

        run_result = mock.Mock(returncode=0, stdout="", stderr="")
        run = mock.Mock(return_value=run_result)
        remote_values = [None] * 6 if remote is None else [remote]
        if remote == head:
            remote_values.extend([None, None, None, None])
        with ExitStack() as stack:
            stack.enter_context(mock.patch.object(HELPER, "_repository", return_value="owner/repo"))
            stack.enter_context(mock.patch.object(HELPER, "_manifest", return_value=(self.manifest, self.worktree)))
            stack.enter_context(mock.patch.object(HELPER, "_load_receipt", return_value=receipt))
            stack.enter_context(mock.patch.object(HELPER, "_load_state", return_value=state))
            stack.enter_context(mock.patch.object(HELPER, "_match_cli_receipt"))
            stack.enter_context(mock.patch.object(HELPER, "_pr_view", return_value=view))
            stack.enter_context(mock.patch.object(HELPER, "_default_branch"))
            stack.enter_context(mock.patch.object(HELPER, "_assert_main_clean"))
            stack.enter_context(mock.patch.object(HELPER, "_assert_main_synced"))
            stack.enter_context(mock.patch.object(HELPER, "_worktree"))
            stack.enter_context(mock.patch.object(HELPER, "_worktree_clean_head", clean_head))
            stack.enter_context(mock.patch.object(HELPER, "_validate_receipt_evidence"))
            stack.enter_context(mock.patch.object(HELPER, "_check_required_ci"))
            stack.enter_context(mock.patch.object(HELPER, "_review_safety"))
            stack.enter_context(mock.patch.object(HELPER, "_ruleset"))
            stack.enter_context(mock.patch.object(HELPER, "_fetch_main", return_value=head))
            stack.enter_context(mock.patch.object(HELPER, "_git", side_effect=fake_git))
            stack.enter_context(mock.patch.object(HELPER, "_run", run))
            stack.enter_context(mock.patch.object(HELPER, "_remote_branch", side_effect=remote_values))
            stack.enter_context(mock.patch.object(
                HELPER, "_worktree_records",
                side_effect=(
                    [records, records, unlocked]
                    if stage in {"merge_started", "merged", "main_synced"}
                    else [records, unlocked]
                ),
            ))
            stack.enter_context(mock.patch.object(
                HELPER,
                "_save_stage",
                side_effect=lambda repo, task, old, new, error="": {**old, "stage": new},
            ))
            stack.enter_context(mock.patch.object(HELPER, "_task_lock", return_value=nullcontext()))
            # The outer lock is irrelevant for this focused state-machine test.
            result = HELPER._finish_locked(self.root, "issue-24", expected_pr=24, expected_head=head, expected_plan="CODEX-DELIVERY-TEST-v1")
        return result, calls, run

    def test_finish_unlocks_exact_reason_then_removes_and_deletes_branch(self) -> None:
        result, calls, _ = self._finish_mock("remote_deleted")
        unlock = next(index for index, call in enumerate(calls) if call[:2] == ["worktree", "unlock"])
        remove = next(index for index, call in enumerate(calls) if call[:2] == ["worktree", "remove"])
        branch = next(index for index, call in enumerate(calls) if call[:2] == ["branch", "-d"])
        self.assertLess(unlock, remove)
        self.assertLess(remove, branch)
        self.assertEqual(result["stage"], "completed")

    def test_finish_never_removes_after_unlock_dirty_race(self) -> None:
        with self.assertRaises(HELPER.DeliveryError):
            self._finish_mock("remote_deleted", unlocked_dirty=True)

    def test_finish_can_resume_when_remote_branch_already_absent(self) -> None:
        result, calls, _ = self._finish_mock("main_synced", remote=None)
        self.assertEqual(result["stage"], "completed")
        self.assertTrue(any(call[:2] == ["worktree", "unlock"] for call in calls))

    def test_finish_rejects_remote_branch_different_head(self) -> None:
        with self.assertRaises(HELPER.DeliveryError):
            self._finish_mock("main_synced", remote="c" * 40)

    def test_finish_remote_delete_is_bound_to_reviewed_head_with_lease(self) -> None:
        head = "b" * 40
        _, calls, _ = self._finish_mock("main_synced", remote=head)
        push = next(call for call in calls if call[:1] == ["push"])
        self.assertIn(
            f"--force-with-lease=refs/heads/feat/issue-24:{head}",
            push,
        )

    def test_finish_resumes_from_unlock_started_and_worktree_removed(self) -> None:
        unlocked, calls, _ = self._finish_mock("worktree_unlock_started")
        self.assertEqual(unlocked["stage"], "completed")
        self.assertTrue(any(call[:2] == ["worktree", "unlock"] for call in calls))
        removed, calls, _ = self._finish_mock("worktree_removed")
        self.assertEqual(removed["stage"], "completed")
        self.assertFalse(any(call[:2] == ["worktree", "remove"] for call in calls))

    def test_finish_recovers_only_from_persisted_merge_started_state(self) -> None:
        recovered, _, _ = self._finish_mock("merge_started")
        self.assertEqual(recovered["stage"], "completed")

    def test_task_locks_are_isolated_and_serial_for_same_task(self) -> None:
        repository = "owner/repo"
        lock_path = self.codex_home / "worktrees" / HELPER._repo_key(repository) / ".locks" / "lifecycle.lock"
        with HELPER._task_lock(repository, "issue-24"):
            self.assertTrue(lock_path.is_file())
            with self.assertRaises(BlockingIOError):
                # A second task in the same repository must use the same lifecycle lock.
                descriptor = os.open(lock_path, os.O_RDWR | os.O_NONBLOCK)
                try:
                    fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
                finally:
                    os.close(descriptor)


if __name__ == "__main__":
    unittest.main()

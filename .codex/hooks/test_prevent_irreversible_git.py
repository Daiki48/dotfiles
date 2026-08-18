#!/usr/bin/env python3
"""不可逆操作だけを防ぐCodex hookの単体テスト。"""

import importlib.util
from pathlib import Path
import unittest


MODULE_PATH = Path(__file__).with_name("prevent_irreversible_git.py")
SPEC = importlib.util.spec_from_file_location("prevent_irreversible_git", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
HOOK = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(HOOK)


class HookTest(unittest.TestCase):
    def test_allows_normal_work(self):
        for command in (
            "git status && git commit -m 'save work'",
            "git push origin HEAD:refs/heads/feature/example",
            "git push -u origin HEAD:refs/heads/fix/example",
            "gh pr create --draft",
            "npm test",
        ):
            with self.subTest(command=command):
                self.assertIsNone(HOOK.blocked_reason(command))

    def test_blocks_irreversible_operations(self):
        for command in (
            "rm important.txt",
            "sudo rm important.txt",
            "env rm important.txt",
            "git rm important.txt",
            "git checkout -- important.txt",
            "git stash clear",
            "git branch -D feature/example",
            "git tag --delete v1.0.0",
            "git push --force origin feature/example",
            "git push origin --force-with-lease=refs/heads/feature/example",
            "git push origin --delete feature/example",
            "git push origin --delete=feature/example",
            "git push origin :feature/example",
            "git push origin HEAD:",
            "git push origin +HEAD:refs/heads/feature/example",
            "git push --prune origin",
            "git -c core.pager=cat branch -D feature/example",
            "git status && git push origin --force feature/example",
        ):
            with self.subTest(command=command):
                self.assertIsNotNone(HOOK.blocked_reason(command))


if __name__ == "__main__":
    unittest.main()

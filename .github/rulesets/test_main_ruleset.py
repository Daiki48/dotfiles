import json
from pathlib import Path
import unittest


RULESET_PATH = Path(__file__).with_name("main.json")


class MainRulesetTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.ruleset = json.loads(RULESET_PATH.read_text(encoding="utf-8"))
        cls.rules = {rule["type"]: rule for rule in cls.ruleset["rules"]}

    def test_targets_only_the_default_branch_without_bypass(self):
        self.assertEqual(self.ruleset["name"], "main-guardrails")
        self.assertEqual(self.ruleset["target"], "branch")
        self.assertEqual(self.ruleset["enforcement"], "active")
        self.assertEqual(self.ruleset["bypass_actors"], [])
        self.assertEqual(
            self.ruleset["conditions"],
            {"ref_name": {"include": ["~DEFAULT_BRANCH"], "exclude": []}},
        )

    def test_enforces_pull_requests_deletion_and_force_push_guards(self):
        self.assertEqual(
            set(self.rules),
            {"deletion", "non_fast_forward", "pull_request", "required_status_checks"},
        )
        pull_request = self.rules["pull_request"]["parameters"]
        self.assertEqual(pull_request["allowed_merge_methods"], ["merge"])
        self.assertEqual(pull_request["required_approving_review_count"], 0)
        self.assertTrue(pull_request["required_review_thread_resolution"])
        self.assertFalse(pull_request["require_code_owner_review"])
        self.assertFalse(pull_request["require_last_push_approval"])

    def test_requires_strict_github_actions_check(self):
        parameters = self.rules["required_status_checks"]["parameters"]
        self.assertTrue(parameters["strict_required_status_checks_policy"])
        self.assertFalse(parameters["do_not_enforce_on_create"])
        self.assertEqual(
            parameters["required_status_checks"],
            [{"context": "required-ci", "integration_id": 15368}],
        )


if __name__ == "__main__":
    unittest.main()

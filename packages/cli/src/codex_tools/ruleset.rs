use std::collections::BTreeMap;

use serde_json::{Value, json};

fn ruleset() -> Value {
    serde_json::from_str(include_str!("../../../../.github/rulesets/main.json"))
        .expect("main Ruleset must be valid JSON")
}

fn rules_by_type(ruleset: &Value) -> BTreeMap<&str, &Value> {
    ruleset["rules"]
        .as_array()
        .expect("rules must be an array")
        .iter()
        .map(|rule| {
            (
                rule["type"].as_str().expect("rule type must be a string"),
                rule,
            )
        })
        .collect()
}

#[test]
fn targets_only_the_default_branch_without_bypass() {
    let ruleset = ruleset();
    assert_eq!(ruleset["name"], "main-guardrails");
    assert_eq!(ruleset["target"], "branch");
    assert_eq!(ruleset["enforcement"], "active");
    assert_eq!(ruleset["bypass_actors"], json!([]));
    assert_eq!(
        ruleset["conditions"],
        json!({"ref_name": {"include": ["~DEFAULT_BRANCH"], "exclude": []}})
    );
}

#[test]
fn enforces_pull_requests_deletion_and_force_push_guards() {
    let ruleset = ruleset();
    let rules = rules_by_type(&ruleset);
    assert_eq!(
        rules.keys().copied().collect::<Vec<_>>(),
        vec![
            "deletion",
            "non_fast_forward",
            "pull_request",
            "required_status_checks"
        ]
    );
    let parameters = &rules["pull_request"]["parameters"];
    assert_eq!(parameters["allowed_merge_methods"], json!(["merge"]));
    assert_eq!(parameters["required_approving_review_count"], 0);
    assert_eq!(parameters["required_review_thread_resolution"], true);
    assert_eq!(parameters["require_code_owner_review"], false);
    assert_eq!(parameters["require_last_push_approval"], false);
}

#[test]
fn requires_strict_github_actions_check() {
    let ruleset = ruleset();
    let rules = rules_by_type(&ruleset);
    let parameters = &rules["required_status_checks"]["parameters"];
    assert_eq!(parameters["strict_required_status_checks_policy"], true);
    assert_eq!(parameters["do_not_enforce_on_create"], false);
    assert_eq!(
        parameters["required_status_checks"],
        json!([{"context": "required-ci", "integration_id": 15368}])
    );
}

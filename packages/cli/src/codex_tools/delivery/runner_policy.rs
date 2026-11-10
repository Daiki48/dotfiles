//! 通常CIの検証とは別に、hostのCI予約が必要かを確定する。
use super::*;

fn no_self_hosted_runners(repository: &str, mut fetch: impl FnMut(&str) -> Result<Value>) -> bool {
    let Some((owner, _)) = repository.split_once('/') else {
        return false;
    };
    let Ok(repo) = fetch(&format!("repos/{repository}")) else {
        return false;
    };
    // 組織/Enterpriseの継承runnerはrepo一覧だけでは確定できない。
    if !repo["full_name"]
        .as_str()
        .is_some_and(|name| name.eq_ignore_ascii_case(repository))
        || repo["owner"]["type"] != "User"
        || !repo["owner"]["login"]
            .as_str()
            .is_some_and(|login| login.eq_ignore_ascii_case(owner))
    {
        return false;
    }
    let Ok(runners) = fetch(&format!("repos/{repository}/actions/runners?per_page=1")) else {
        return false;
    };
    runners["total_count"].as_u64() == Some(0)
        && runners["runners"].as_array().is_some_and(Vec::is_empty)
}

fn hosted_tree(root: &Path, sha: &str) -> Result<bool> {
    let sha = oid(sha, "runner判定commit")?;
    let entries = git(
        root,
        &["ls-tree", "-r", "-z", &sha, "--", ".github/workflows"],
        true,
    )?;
    let mut found = false;
    for entry in entries.split_terminator('\0') {
        let (metadata, path) = entry
            .split_once('\t')
            .ok_or_else(|| error("workflow treeが不正です"))?;
        if !workflow_yaml_path(path) {
            continue;
        }
        let fields: Vec<_> = metadata.split_whitespace().collect();
        let ["100644" | "100755", "blob", blob] = fields.as_slice() else {
            return Ok(false);
        };
        let blob = oid(blob, "workflow blob")?;
        let source = git(root, &["cat-file", "blob", &blob], true)?;
        if !super::super::workflow_policy::standard_hosted_jobs(&source) {
            return Ok(false);
        }
        found = true;
    }
    Ok(found)
}

pub(super) fn hosted_only(
    root: &Path,
    repository: &str,
    base: &str,
    head: &str,
    merged: Option<&str>,
) -> bool {
    hosted_revisions(root, base, head, merged)
        && no_self_hosted_runners(repository, |endpoint| {
            gh_json(root, &["api".into(), endpoint.into()])
        })
}

fn hosted_revisions(root: &Path, base: &str, head: &str, merged: Option<&str>) -> bool {
    let revisions: HashSet<_> = [base, head].into_iter().chain(merged).collect();
    revisions
        .into_iter()
        .all(|sha| hosted_tree(root, sha) == Ok(true))
}

pub(super) fn hosted_only_for_scheduler(
    root: &Path,
    repository: &str,
    base: &str,
    head: &str,
    merged: Option<&str>,
) -> bool {
    // 実merge objectがまだなければ、観測済みbaseを再照合しながら取得する。
    let present = |sha: &str| {
        oid(sha, "runner判定commit").is_ok()
            && git(
                root,
                &["rev-parse", "--verify", &format!("{sha}^{{commit}}")],
                true,
            )
            .is_ok()
    };
    if (!present(base) || merged.is_some_and(|sha| !present(sha)))
        && fetch_main(root, repository).as_deref() != Ok(base)
    {
        return false;
    }
    hosted_only(root, repository, base, head, merged)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn fixed_base_head_and_actual_merge_workflows_must_all_be_hosted() {
        let root = env::temp_dir().join(format!(
            "codex-hosted-workflow-{}-{}",
            std::process::id(),
            now().replace(':', "-")
        ));
        fs::create_dir(&root).unwrap();
        let command = |args: &[&str]| {
            let output = Command::new(GIT_BINARY)
                .args([
                    "-c",
                    "core.hooksPath=/dev/null",
                    "-c",
                    "user.name=test",
                    "-c",
                    "user.email=test@example.com",
                ])
                .args(args)
                .current_dir(&root)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        };
        command(&["init", "-q", "-b", "main"]);
        fs::create_dir_all(root.join(".github/workflows")).unwrap();
        let commit = |runner: &str| {
            fs::write(
                root.join(".github/workflows/ci.yml"),
                format!("on: [push, pull_request]\njobs: {{check: {{runs-on: {runner}}}}}\n"),
            )
            .unwrap();
            command(&["add", "--", ".github/workflows/ci.yml"]);
            command(&["commit", "--allow-empty", "-qm", "fixture"]);
            command(&["rev-parse", "HEAD"])
        };
        let base = commit("ubuntu-24.04");
        let head = commit("ubuntu-24.04");
        let local = commit("self-hosted");
        assert!(hosted_revisions(&root, &base, &head, Some(&head)));
        assert!(!hosted_revisions(&root, &local, &head, None));
        assert!(!hosted_revisions(&root, &base, &local, None));
        assert!(!hosted_revisions(&root, &base, &head, Some(&local)));
        assert!(!hosted_revisions(
            &root,
            &base,
            &head,
            Some(&"f".repeat(40))
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn runner_inventory_requires_verified_personal_repo_and_an_explicit_empty_result() {
        for (kind, response, expected) in [
            ("User", Ok(json!({"total_count":0,"runners":[]})), true),
            (
                "Organization",
                Ok(json!({"total_count":0,"runners":[]})),
                false,
            ),
            ("User", Ok(json!({"total_count":0})), false),
            ("User", Ok(json!({"total_count":1,"runners":[]})), false),
            ("User", Ok(json!({"total_count":0,"runners":[{}]})), false),
            ("User", Err(error("API unavailable")), false),
        ] {
            assert_eq!(
                no_self_hosted_runners("example/product", |endpoint| match endpoint {
                    "repos/example/product" => Ok(
                        json!({"full_name":"Example/Product","owner":{"login":"Example","type":kind}})
                    ),
                    "repos/example/product/actions/runners?per_page=1" => response.clone(),
                    _ => unreachable!(),
                }),
                expected
            );
        }
    }
}

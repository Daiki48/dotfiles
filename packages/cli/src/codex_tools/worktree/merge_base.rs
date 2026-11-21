//! GitHubで競合した既存PRのbaseを、担当worktreeにだけ取り込む。

use super::*;

#[derive(Clone, Copy)]
struct Request<'a> {
    pr: u64,
    head: &'a str,
    base: &'a str,
}

#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Staged,
    Conflicted,
}

fn verify_pr(
    repository: &Repository,
    manifest: &Manifest,
    request: Request<'_>,
    response: &Value,
) -> Result<(), WorktreeError> {
    let matches_ref = |value: &Value, branch: &str| {
        value["ref"].as_str() == Some(branch)
            && value["repo"]["full_name"]
                .as_str()
                .is_some_and(|name| name.eq_ignore_ascii_case(&repository.github_name))
    };
    if response["number"].as_u64() != Some(request.pr)
        || response["state"].as_str() != Some("open")
        || response["merged"].as_bool() != Some(false)
        || !matches_ref(&response["head"], &manifest.branch)
        || response["head"]["sha"].as_str() != Some(request.head)
        // PR payloadのbase.shaは古い場合がある。現在のbase SHAは実refで照合する。
        || !matches_ref(&response["base"], &repository.default_branch)
        || response["head"]["repo"]["id"].as_u64().is_none_or(|id| {
            id == 0 || response["base"]["repo"]["id"].as_u64() != Some(id)
        })
    {
        return Err(error(
            "open PRのrepository・branch・HEAD・baseが一致しません",
        ));
    }
    if response["mergeable"].as_bool() != Some(false)
        || response["mergeable_state"].as_str() != Some("dirty")
    {
        return Err(error("GitHubが対象PRの競合を確認していません"));
    }
    Ok(())
}

fn verify_remote(
    repository: &Repository,
    manifest: &Manifest,
    request: Request<'_>,
) -> Result<(), WorktreeError> {
    let (fetch, push) = origin_urls(&repository.root)?;
    if fetch != repository.origin_url || push != fetch {
        return Err(error("取り込みには一致するorigin fetch/push URLが必要です"));
    }
    let base_ref = format!("refs/heads/{}", repository.default_branch);
    let head_ref = format!("refs/heads/{}", manifest.branch);
    let remote = git_stdout(
        &repository.root,
        &[
            "ls-remote",
            "--symref",
            "--exit-code",
            "--upload-pack=git-upload-pack",
            &repository.origin_url,
            "HEAD",
            &base_ref,
            &head_ref,
        ],
    )?;
    let mut actual: Vec<&str> = remote.lines().collect();
    actual.sort_unstable();
    let mut expected = [
        format!("ref: {base_ref}\tHEAD"),
        format!("{}\tHEAD", request.base),
        format!("{}\t{base_ref}", request.base),
        format!("{}\t{head_ref}", request.head),
    ];
    expected.sort_unstable();
    if actual != expected {
        return Err(error(
            "originのdefault branch・base・作業branchのHEADが一致しません",
        ));
    }
    Ok(())
}

fn verify_checkout(
    repository: &Repository,
    manifest: &Manifest,
    request: Request<'_>,
    allow_local_origin: bool,
) -> Result<(), WorktreeError> {
    verify_sync_task(repository, manifest, request.head, allow_local_origin)?;
    let target = Path::new(&manifest.worktree);
    if local_config_keys(target)?
        .iter()
        .any(|key| key.starts_with("branch.") && key.ends_with(".mergeoptions"))
    {
        return Err(error(
            "branchのmergeOptionsを設定したworktreeは取り込めません",
        ));
    }
    match fs::symlink_metadata(repository.common_git_dir.join("info/grafts")) {
        Err(cause) if cause.kind() == io::ErrorKind::NotFound => {}
        _ => return Err(error("graftsが存在するrepositoryの取り込みを拒否しました")),
    }
    let status = git_stdout(
        target,
        &[
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ],
    )?;
    let index = git_stdout(target, &["ls-files", "-v", "-z"])?;
    if !status.is_empty()
        || index
            .split_terminator('\0')
            .any(|entry| !entry.starts_with("H "))
    {
        return Err(error(
            "作業tree・indexの全変更を確認できるcleanな状態が必要です",
        ));
    }
    Ok(())
}

fn start(
    repository: &Repository,
    manifest: &Manifest,
    request: Request<'_>,
    allow_local_origin: bool,
    mut observe_pr: impl FnMut() -> Result<Value, WorktreeError>,
) -> Result<Outcome, WorktreeError> {
    let target = Path::new(&manifest.worktree);
    if repository.default_oid != request.base || manifest.branch == repository.default_branch {
        return Err(error("指定baseがoriginのdefault branchと一致しません"));
    }
    let before = snapshot(&repository.root)?;
    if before.0.trim() != format!("refs/heads/{}", repository.default_branch)
        || before.1.is_none()
        || !before.2.is_empty()
    {
        return Err(error(
            "親checkoutはcleanなdefault branchである必要があります",
        ));
    }
    verify_checkout(repository, manifest, request, allow_local_origin)?;
    verify_remote(repository, manifest, request)?;
    verify_pr(repository, manifest, request, &observe_pr()?)?;
    // refmapを空にし、取得対象のbranchにも書き込み先を指定しない。
    git_stdout(
        target,
        &[
            "fetch",
            "--no-tags",
            "--no-prune",
            "--no-prune-tags",
            "--no-write-fetch-head",
            "--no-recurse-submodules",
            "--no-auto-maintenance",
            "--refmap=",
            "--upload-pack=git-upload-pack",
            &repository.origin_url,
            &format!("refs/heads/{}", repository.default_branch),
        ],
    )?;
    for oid in [request.head, request.base] {
        let commit = git_stdout(
            target,
            &["rev-parse", "--verify", &format!("{oid}^{{commit}}")],
        )?;
        if commit.trim() != oid {
            return Err(error("指定SHAがcommitそのものを指していません"));
        }
    }
    let (ancestor, _) = git_allow_failure(
        target,
        &["merge-base", "--is-ancestor", request.base, request.head],
    )?;
    if ancestor != 1 {
        return Err(error("baseが取り込み済みか、祖先関係を確認できません"));
    }
    verify_pr(repository, manifest, request, &observe_pr()?)?;
    verify_remote(repository, manifest, request)?;
    verify_checkout(repository, manifest, request, allow_local_origin)?;
    if snapshot(&repository.root)? != before {
        return Err(error("取り込み前に親checkoutが変化しました"));
    }

    // 成否にかかわらず、この操作ではrefを進めない。通常のcommitで検査を受ける。
    let (code, _) = git_allow_failure(
        target,
        &[
            "-c",
            "rerere.enabled=false",
            "-c",
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
            "merge",
            "--strategy=ort",
            "--no-ff",
            "--no-commit",
            "--no-squash",
            "--no-edit",
            "--no-autostash",
            "--no-overwrite-ignore",
            "--no-verify-signatures",
            "--no-gpg-sign",
            request.base,
        ],
    )?;
    let merge_head = absolute_git_path(target, "--git-dir")?.join("MERGE_HEAD");
    if git_stdout(target, &["rev-parse", "--verify", "HEAD"])?.trim() != request.head
        || snapshot(&repository.root)? != before
        || fs::read_to_string(merge_head)
            .ok()
            .as_deref()
            .map(str::trim)
            != Some(request.base)
    {
        return Err(error(
            "取り込み結果を確認できません。現状を保持してgit statusを確認してください",
        ));
    }
    let unmerged = git_stdout(target, &["ls-files", "--unmerged", "-z"])?;
    match (code, unmerged.is_empty()) {
        (0, true) => Ok(Outcome::Staged),
        (1, false) => Ok(Outcome::Conflicted),
        _ => Err(error(
            "取り込みを確定できません。現状を保持してgit statusを確認してください",
        )),
    }
}

pub(super) fn run(
    cwd: &Path,
    task_id: &str,
    pr: u64,
    head: &str,
    base: &str,
) -> Result<i32, (i32, String)> {
    let execute = || {
        let repository = inspect_repository(cwd, false, true)?;
        let (_, _, lock_path) = managed_paths(&repository, task_id, false)?;
        let _lifecycle = ExclusiveLock::acquire(&lock_path)?;
        let manifest = load_manifests(&repository)?
            .into_iter()
            .find(|(_, manifest)| manifest.task_id == task_id && manifest.status == "ready")
            .map(|(_, manifest)| manifest)
            .ok_or_else(|| error("取り込み対象のready manifestがありません"))?;
        let marker = std::env::var_os("CODEX_TASK_WORKER");
        let marker = marker
            .as_ref()
            .map(|marker| {
                marker
                    .to_str()
                    .ok_or_else(|| error("worker scopeが不正です"))
            })
            .transpose()?;
        let _session = if marker.is_none() {
            Some(completion::lock_current_binding(&repository, &manifest).map_err(error)?)
        } else {
            None
        };
        let target = Path::new(&manifest.worktree);
        let check_writer = || {
            super::super::task::merge_base_writer(task_id, &manifest.branch, target, pr, marker)
                .map_err(error)
        };
        let _writer = check_writer()?;
        let args = [
            "api".into(),
            format!("repos/{}/pulls/{pr}", repository.github_name),
        ];
        start(
            &repository,
            &manifest,
            Request { pr, head, base },
            false,
            || {
                if marker.is_some() {
                    check_writer()?;
                }
                super::super::delivery::task_gh_json(&repository.root, &args)
                    .map_err(|_| error("対象PRの競合状態をGitHubから取得できません"))
            },
        )
    };
    match execute().map_err(|cause| (1, cause.to_string()))? {
        Outcome::Staged => {
            println!("指定baseを取り込みました。作業worktreeで差分を検証し、通常のgit commitで確定してください。");
            Ok(0)
        }
        Outcome::Conflicted => Err((3, "指定baseとの競合を作業worktreeに保持しました。同じworktreeでgit statusとMERGE_HEADを確認し、競合解消・検証・通常のgit commitへ進んでください。".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{TemporaryRepository, run_git};
    use super::*;
    use serde_json::json;

    const TASK: &str = "task-base";
    const BRANCH: &str = "fix/base";

    fn fixture(conflict: bool) -> (TemporaryRepository, Repository, Manifest, String) {
        let fixture = TemporaryRepository::new();
        let target = create_worktree(&fixture.repository, BRANCH, TASK, true).unwrap();
        fs::write(target.join("README.md"), "task\n").unwrap();
        git_stdout(&target, &["add", "--", "README.md"]).unwrap();
        git_stdout(&target, &["commit", "-m", "task"]).unwrap();
        git_stdout(&target, &["push", "origin", "HEAD:refs/heads/fix/base"]).unwrap();
        let head = task_head(&target).unwrap();

        let advance = fixture.repository.with_file_name("advance");
        let remote = fixture.repository.with_file_name("remote.git");
        run_git(
            fixture.repository.parent().unwrap(),
            &["clone", remote.to_str().unwrap(), advance.to_str().unwrap()],
        );
        git_stdout(&advance, &["config", "user.name", "Test User"]).unwrap();
        git_stdout(&advance, &["config", "user.email", "test@example.invalid"]).unwrap();
        let file = if conflict { "README.md" } else { "base.txt" };
        fs::write(advance.join(file), "base\n").unwrap();
        git_stdout(&advance, &["add", "--", file]).unwrap();
        git_stdout(&advance, &["commit", "-m", "base"]).unwrap();
        git_stdout(&advance, &["push", "origin", "main"]).unwrap();

        let repository = inspect_repository(&fixture.repository, true, true).unwrap();
        let manifest = load_manifests(&repository).unwrap().remove(0).1;
        assert_ne!(task_head(&repository.root).unwrap(), repository.default_oid);
        (fixture, repository, manifest, head)
    }

    fn pr(repository: &Repository, request: Request<'_>) -> Value {
        let repo = json!({"full_name": repository.github_name, "id": 41});
        json!({
            "number": request.pr, "state": "open", "merged": false,
            "mergeable": false, "mergeable_state": "dirty",
            "base": {"ref": "main", "sha": "stale PR payload", "repo": repo},
            "head": {"ref": BRANCH, "sha": request.head, "repo": repo}
        })
    }

    #[test]
    fn stages_or_preserves_conflict_on_target_only_and_normal_commit_uses_both_parents() {
        for (conflict, expected) in [(false, Outcome::Staged), (true, Outcome::Conflicted)] {
            let (_fixture, repository, manifest, head) = fixture(conflict);
            let request = Request {
                pr: 7,
                head: &head,
                base: &repository.default_oid,
            };
            let target = Path::new(&manifest.worktree);
            let before = snapshot(&repository.root).unwrap();
            let refs = git_stdout(&repository.root, &["show-ref"]).unwrap();
            let index_path = repository.common_git_dir.join("index");
            let index = fs::read(&index_path).unwrap();
            let fetch_head = absolute_git_path(target, "--git-dir")
                .unwrap()
                .join("FETCH_HEAD");
            fs::write(&fetch_head, "fixture\n").unwrap();
            assert_eq!(
                start(&repository, &manifest, request, true, || Ok(pr(
                    &repository,
                    request
                )))
                .unwrap(),
                expected
            );
            assert_eq!(snapshot(&repository.root).unwrap(), before);
            assert_eq!(fs::read(index_path).unwrap(), index);
            assert_eq!(git_stdout(&repository.root, &["show-ref"]).unwrap(), refs);
            assert_eq!(fs::read_to_string(fetch_head).unwrap(), "fixture\n");
            assert_eq!(task_head(target).unwrap(), head);
            assert!(!repository.common_git_dir.join("MERGE_HEAD").exists());
            assert!(start(&repository, &manifest, request, true, || unreachable!()).is_err());
            assert_eq!(resume(&repository.root, TASK, true).unwrap(), target);
            if conflict {
                let text = fs::read_to_string(target.join("README.md")).unwrap();
                assert!(text.contains("<<<<<<<") && text.contains("======="));
                fs::write(target.join("README.md"), "resolved\n").unwrap();
                git_stdout(target, &["add", "--", "README.md"]).unwrap();
            }
            git_stdout(target, &["commit", "-m", "integrate base"]).unwrap();
            assert_eq!(
                git_stdout(target, &["show", "-s", "--format=%P", "HEAD"])
                    .unwrap()
                    .trim(),
                format!("{head} {}", request.base),
            );
            verify_remote(&repository, &manifest, request).unwrap();
        }
    }

    #[test]
    fn refuses_unverified_prs_unknown_mergeability_and_stale_refs() {
        let (fixture, repository, manifest, head) = fixture(true);
        let request = Request {
            pr: 7,
            head: &head,
            base: &repository.default_oid,
        };
        let response = pr(&repository, request);
        let target = Path::new(&manifest.worktree);
        let before = snapshot(target).unwrap();
        for (pointer, value) in [
            ("/number", json!(8)),
            ("/state", json!("closed")),
            ("/merged", json!(true)),
            ("/head/ref", json!("fix/other")),
            ("/head/sha", json!(request.base)),
            ("/head/repo/full_name", json!("example/fork")),
            ("/head/repo/id", json!(42)),
            ("/base/repo/full_name", json!("example/other")),
            ("/base/ref", json!("develop")),
            ("/mergeable", Value::Null),
            ("/mergeable", json!(true)),
            ("/mergeable_state", json!("blocked")),
        ] {
            let mut invalid = response.clone();
            *invalid.pointer_mut(pointer).unwrap() = value;
            assert!(
                start(
                    &repository,
                    &manifest,
                    request,
                    true,
                    || Ok(invalid.clone())
                )
                .is_err(),
                "{pointer}"
            );
        }
        let stale = Request {
            base: &task_head(&repository.root).unwrap(),
            ..request
        };
        assert!(start(&repository, &manifest, stale, true, || unreachable!()).is_err());
        let mut second_observation = false;
        let advance = fixture.repository.with_file_name("advance");
        assert!(
            start(&repository, &manifest, request, true, || {
                if second_observation {
                    git_stdout(&advance, &["commit", "--allow-empty", "-m", "new base"]).unwrap();
                    git_stdout(&advance, &["push", "origin", "main"]).unwrap();
                }
                second_observation = true;
                Ok(response.clone())
            })
            .is_err()
        );
        assert_eq!(snapshot(target).unwrap(), before);
        ensure_no_git_operation(target).unwrap();
    }

    #[test]
    fn refuses_dirty_hidden_and_in_progress_changes_and_wrong_origin() {
        let (_fixture, repository, manifest, head) = fixture(false);
        let request = Request {
            pr: 7,
            head: &head,
            base: &repository.default_oid,
        };
        let target = Path::new(&manifest.worktree);
        let reject =
            || assert!(start(&repository, &manifest, request, true, || unreachable!()).is_err());
        fs::write(target.join("local.txt"), "preserve\n").unwrap();
        reject();
        assert_eq!(
            fs::read_to_string(target.join("local.txt")).unwrap(),
            "preserve\n"
        );
        fs::remove_file(target.join("local.txt")).unwrap();
        git_stdout(target, &["update-index", "--assume-unchanged", "README.md"]).unwrap();
        reject();
        git_stdout(
            target,
            &["update-index", "--no-assume-unchanged", "README.md"],
        )
        .unwrap();
        let merge_head = absolute_git_path(target, "--git-dir")
            .unwrap()
            .join("MERGE_HEAD");
        fs::write(&merge_head, request.base).unwrap();
        reject();
        assert_eq!(fs::read_to_string(&merge_head).unwrap(), request.base);
        fs::remove_file(merge_head).unwrap();
        git_stdout(
            target,
            &["config", "branch.fix/base.mergeOptions", "--squash"],
        )
        .unwrap();
        reject();
        git_stdout(
            target,
            &["config", "--unset", "branch.fix/base.mergeOptions"],
        )
        .unwrap();
        git_stdout(
            target,
            &[
                "config",
                "remote.origin.pushurl",
                "https://github.com/example/other.git",
            ],
        )
        .unwrap();
        reject();
        assert!(inspect_repository(target, true, true).is_err());
        assert_eq!(task_head(target).unwrap(), head);
    }
}

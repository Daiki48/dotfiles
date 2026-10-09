//! Issueには人間向け作業記録だけを保存する。receiptの監査ledgerとは独立。
use super::*;

const MAX_BODY: usize = 60_000;

pub(super) fn issue_for(task: &str, requested: Option<i64>, record: &Value) -> Result<Option<i64>> {
    let inferred = task
        .strip_prefix("issue-")
        .map(|v| v.parse::<i64>())
        .transpose()
        .map_err(|_| error("Issue番号が不正です"))?;
    let stored = if record.is_null() {
        None
    } else {
        Some(int_value(record, "number")?)
    };
    let choices = [inferred, requested, stored];
    let selected = choices.into_iter().flatten().next();
    if choices
        .into_iter()
        .flatten()
        .any(|n| n < 1 || Some(n) != selected)
    {
        return Err(error("task・指定Issue・保存済みIssueが一致しません"));
    }
    Ok(selected)
}

pub(super) fn validate_record(task: &str, record: &Value) -> Result<()> {
    issue_for(task, None, record)?;
    if !record.is_null() {
        expected_keys(record, &["number", "body"], "Issue作業記録")?;
        let body = str_value(record, "body")?;
        if body.trim().is_empty() || body.len() > MAX_BODY {
            return Err(error("Issue作業記録の本文が空または長すぎます"));
        }
    }
    Ok(())
}

pub(super) fn bind(root: &Path, state: &Value, requested: Option<i64>) -> Result<Value> {
    let task = str_value(state, "task_id")?;
    let record = get(state, "issue_record")?;
    if str_value(state, "stage")? == "completed" && record.is_null() {
        if requested.is_some() {
            return Err(error(
                "完了済みtaskへの記録先追加は行いません。Issueへ通常の作業文書を残してください",
            ));
        }
        return Ok(state.clone());
    }
    let Some(issue) = issue_for(&task, requested, record)? else {
        return Ok(state.clone());
    };
    if !record.is_null() {
        return Ok(state.clone());
    }
    let repo = str_value(state, "repository")?;
    let target = api(root, "GET", &format!("repos/{repo}/issues/{issue}"), None)?;
    if int_value(&target, "number")? != issue
        || target.get("pull_request").is_some()
        || str_value(&target, "html_url")? != format!("https://github.com/{repo}/issues/{issue}")
    {
        return Err(error("記録先が同一repositoryのIssueではありません"));
    }
    let pr = int_value(state, "pr")?;
    let description = gh_json(
        root,
        &[
            "pr".into(),
            "view".into(),
            pr.to_string(),
            "--repo".into(),
            repo,
            "--json".into(),
            "body".into(),
        ],
    )?;
    let record = object([
        ("number", Value::from(issue)),
        ("body", get(&description, "body")?.clone()),
    ]);
    validate_record(&task, &record)?;
    let mut bound = state.clone();
    bound["issue_record"] = record;
    Ok(bound)
}

fn marker(state: &Value) -> Result<String> {
    Ok(format!(
        "<!-- codex-work-log:{}:{}:{} -->",
        str_value(state, "task_id")?,
        int_value(state, "pr")?,
        str_value(state, "head_sha")?
    ))
}

fn body(state: &Value, complete: bool) -> Result<String> {
    let repo = str_value(state, "repository")?;
    let pr = int_value(state, "pr")?;
    let status = if complete {
        "main同期とmanaged worktree・登録済み成果物の物理回収が完了しました。"
    } else {
        "main同期済み。作業記録を保存し、managed worktree・登録済み成果物の回収を待っています。"
    };
    Ok(format!(
        "{}\n## 作業記録\n\nPR: https://github.com/{repo}/pull/{pr}\n\nTask: `{}`\n\nSource head: `{}`\n\n{}\n\n### 完了処理\n\n{status}\n",
        marker(state)?,
        str_value(state, "task_id")?,
        str_value(state, "head_sha")?,
        str_value(get(state, "issue_record")?, "body")?
    ))
}

fn validate_comment(value: &Value, issue_url: &str, login: &str) -> Result<i64> {
    let id = int_value(value, "id")?;
    if id < 1
        || str_value(value, "issue_url")? != issue_url
        || str_value(get(value, "user")?, "login")? != login
    {
        return Err(error("作業記録commentの所有者またはIssueが一致しません"));
    }
    Ok(id)
}

/// POST/PATCHの応答を失っても、次回は同じmarkerのcommentを再取得する。
fn sync_with(
    state: &Value,
    complete: bool,
    mut request: impl FnMut(&str, &str, Option<&str>) -> Result<Value>,
) -> Result<()> {
    let record = get(state, "issue_record")?;
    if record.is_null() {
        return Ok(());
    }
    let repo = str_value(state, "repository")?;
    let issue = int_value(record, "number")?;
    let endpoint = format!("repos/{repo}/issues/{issue}/comments");
    let issue_url = format!("https://api.github.com/repos/{repo}/issues/{issue}");
    let login = str_value(&request("GET", "user", None)?, "login")?;
    let marker = marker(state)?;
    let pending = body(state, false)?;
    let completed = body(state, true)?;
    let desired = if complete { &completed } else { &pending };
    let mut found = None;
    let mut exhausted = false;
    for page in 1..=MAX_PAGES {
        let values = request("GET", &format!("{endpoint}?per_page=100&page={page}"), None)?;
        let values = values
            .as_array()
            .ok_or_else(|| error("Issue comment一覧が不正です"))?;
        for value in values {
            let text = str_value(value, "body")?;
            if !text.starts_with(&marker) {
                continue;
            }
            let id = validate_comment(value, &issue_url, &login)?;
            if found.is_some() {
                return Err(error("作業記録commentが重複しています"));
            }
            // 人間による追記や未知の内容を上書きしない。
            if text != pending && text != completed {
                return Err(error(
                    "保存済み作業記録が変更されています。上書きせず保持します",
                ));
            }
            if !complete && text == completed {
                return Err(error("cleanup前に完了済み作業記録が存在します"));
            }
            found = Some((id, text));
        }
        if values.len() < 100 {
            exhausted = true;
            break;
        }
    }
    if !exhausted {
        return Err(error("Issue commentの取得上限に達しました"));
    }
    let id = match found {
        Some((id, text)) if text == *desired => id,
        Some((id, _)) => {
            let response = request(
                "PATCH",
                &format!("repos/{repo}/issues/comments/{id}"),
                Some(desired),
            )?;
            if validate_comment(&response, &issue_url, &login)? != id {
                return Err(error("更新された作業記録commentが一致しません"));
            }
            id
        }
        None if complete => return Err(error("cleanup前の作業記録が見つかりません")),
        None => {
            let response = request("POST", &endpoint, Some(desired))?;
            validate_comment(&response, &issue_url, &login)?
        }
    };
    let confirmed = request("GET", &format!("repos/{repo}/issues/comments/{id}"), None)?;
    if validate_comment(&confirmed, &issue_url, &login)? != id
        || str_value(&confirmed, "body")? != *desired
    {
        return Err(error("Issue作業記録の保存を確認できません"));
    }
    Ok(())
}

fn api(root: &Path, method: &str, endpoint: &str, body: Option<&str>) -> Result<Value> {
    let sandbox = GhSandbox::create(root)?;
    let mut command = vec![
        trusted_binary(GH_BINARY, "GitHub CLI")?,
        "api".into(),
        "--hostname".into(),
        "github.com".into(),
        "--method".into(),
        method.into(),
        endpoint.into(),
    ];
    if let Some(body) = body {
        let payload = serde_json::to_vec(&object([("body", string(body))]))
            .map_err(|_| error("Issue本文をJSONへ変換できません"))?;
        let path = sandbox.write_private("issue-body.json", &payload, "Issue本文")?;
        command.extend(["--input".into(), path.display().to_string()]);
    }
    let response = run_with_config(
        &command,
        root,
        Duration::from_secs(COMMAND_TIMEOUT),
        MAX_OUTPUT_BYTES,
        Some(&sandbox.path),
    )?;
    if !response.status.success() {
        return Err(error(
            "Issue作業記録のAPI操作に失敗しました。同じfinishで再取得して再開できます",
        ));
    }
    parse_json(&response.stdout, "Issue作業記録")
}

pub(super) fn sync(root: &Path, state: &Value, complete: bool) -> Result<()> {
    sync_with(state, complete, |method, endpoint, body| {
        api(root, method, endpoint, body)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> Value {
        object([
            ("repository", string("owner/repo")),
            ("task_id", string("task-example")),
            ("pr", Value::from(7)),
            ("head_sha", string("a".repeat(40))),
            (
                "issue_record",
                object([
                    ("number", Value::from(3)),
                    ("body", string("目的と判断。変更と検証。残存事項なし。")),
                ]),
            ),
        ])
    }

    #[derive(Default)]
    struct Api {
        comments: Vec<Value>,
        posts: usize,
        patches: usize,
        lose_response: Option<&'static str>,
    }
    impl Api {
        fn call(&mut self, method: &str, endpoint: &str, body: Option<&str>) -> Result<Value> {
            let value = match (method, endpoint) {
                ("GET", "user") => object([("login", string("author"))]),
                ("GET", path) if path.contains("?per_page=") => Value::Array(self.comments.clone()),
                ("GET", _) => self.comments.first().cloned().ok_or_else(|| error("404"))?,
                ("POST", "repos/owner/repo/issues/3/comments") => {
                    self.posts += 1;
                    let comment = object([
                        ("id", Value::from(10)),
                        ("body", string(body.unwrap())),
                        (
                            "issue_url",
                            string("https://api.github.com/repos/owner/repo/issues/3"),
                        ),
                        ("user", object([("login", string("author"))])),
                    ]);
                    self.comments.push(comment.clone());
                    comment
                }
                ("PATCH", "repos/owner/repo/issues/comments/10") => {
                    self.patches += 1;
                    self.comments[0]["body"] = string(body.unwrap());
                    self.comments[0].clone()
                }
                _ => return Err(error("unexpected request")),
            };
            if self.lose_response == Some(method) {
                self.lose_response = None;
                return Err(error("response lost after server accepted"));
            }
            Ok(value)
        }
        fn sync(&mut self, state: &Value, complete: bool) -> Result<()> {
            sync_with(state, complete, |method, endpoint, body| {
                self.call(method, endpoint, body)
            })
        }
    }

    #[test]
    fn issue_is_optional_but_cannot_be_changed_or_mismatched() {
        assert_eq!(issue_for("task-example", None, &Value::Null).unwrap(), None);
        assert_eq!(issue_for("issue-3", None, &Value::Null).unwrap(), Some(3));
        assert!(issue_for("issue-3", Some(4), &Value::Null).is_err());
        assert!(issue_for("task-example", Some(4), &state()["issue_record"]).is_err());
        assert!(issue_for("task-example", Some(0), &Value::Null).is_err());
        let mut without = state();
        without["issue_record"] = Value::Null;
        sync_with(&without, false, |_, _, _| {
            panic!("PR-only task must not call Issue API")
        })
        .unwrap();
    }

    #[test]
    fn lost_create_and_update_responses_resume_without_duplicate_or_regression() {
        let state = state();
        let mut api = Api {
            lose_response: Some("POST"),
            ..Api::default()
        };
        assert!(api.sync(&state, false).is_err());
        api.sync(&state, false).unwrap();
        assert_eq!(api.posts, 1);
        api.lose_response = Some("PATCH");
        assert!(api.sync(&state, true).is_err());
        api.sync(&state, true).unwrap();
        assert_eq!(api.patches, 1);
        assert_eq!(api.comments[0]["body"], body(&state, true).unwrap());
        assert!(api.sync(&state, false).is_err());
    }

    #[test]
    fn issue_read_failure_prevents_record_success_and_cleanup_permission() {
        let mut calls = 0;
        assert!(
            sync_with(&state(), false, |_, _, _| {
                calls += 1;
                Err(error("offline"))
            })
            .is_err()
        );
        assert_eq!(calls, 1);
        assert!(Api::default().sync(&state(), true).is_err());
    }

    #[test]
    fn foreign_edited_or_duplicate_comments_are_preserved() {
        let state = state();
        let mut api = Api::default();
        api.sync(&state, false).unwrap();
        let saved = api.comments[0].clone();
        for mutation in ["author", "body", "issue", "duplicate"] {
            api.comments = vec![saved.clone()];
            match mutation {
                "author" => api.comments[0]["user"]["login"] = string("someone-else"),
                "body" => {
                    api.comments[0]["body"] =
                        string(format!("{}\n手動追記", body(&state, false).unwrap()))
                }
                "issue" => {
                    api.comments[0]["issue_url"] =
                        string("https://api.github.com/repos/other/repo/issues/3")
                }
                _ => api.comments.push(saved.clone()),
            }
            assert!(api.sync(&state, true).is_err(), "{mutation}");
            assert_eq!(api.patches, 0);
        }
    }
}

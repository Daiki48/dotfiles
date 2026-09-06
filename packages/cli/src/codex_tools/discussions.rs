//! current origin内のDiscussionsを固定GraphQL操作だけで管理する。

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{guard, process, trust};

type Result<T> = std::result::Result<T, String>;

const USAGE: &str = "codex-discussions <操作> --repo OWNER/REPO [option]\n\
操作: categories | list | view | comments | replies | create | edit | comment | reply | edit-comment | close | reopen | mark-answer | unmark-answer\n\
対象: --discussion 正の数値番号、--comment-id GraphQL ID、--category-id GraphQL ID\n\
本文: --body-file /tmp/配下の通常file、title: --title 文字列\n\
一覧: --limit 1..100（既定20）、--after cursor（pageInfo.endCursor）\n\
close: --reason RESOLVED|OUTDATED|DUPLICATE（必須）\n\
reply/repliesは親comment-id、edit-comment/mark-answer/unmark-answerは対象comment-idを指定。\n\
書き込みは対象所属と送信内容を検査し、結果を再取得してJSONを返す。失敗時は自動再送しない。";

const READ_COMMANDS: &[&str] = &["categories", "list", "view", "comments", "replies"];
const WRITE_COMMANDS: &[&str] = &[
    "create",
    "edit",
    "comment",
    "reply",
    "edit-comment",
    "close",
    "reopen",
    "mark-answer",
    "unmark-answer",
];

pub(crate) fn is_write(command: &str) -> bool {
    WRITE_COMMANDS.contains(&command)
}

pub(crate) fn is_help(args: &[String]) -> bool {
    args == ["--help"]
        || args.len() == 2
            && (READ_COMMANDS.contains(&args[0].as_str()) || is_write(&args[0]))
            && args[1] == "--help"
}

#[derive(Debug)]
pub(crate) struct Request {
    command: String,
    pub(crate) repo: String,
    number: Option<u32>,
    comment: Option<String>,
    category: Option<String>,
    title: Option<String>,
    body: Option<String>,
    reason: Option<String>,
    limit: u32,
    after: Option<String>,
}

fn positive_number(value: &str) -> Option<u32> {
    (value.as_bytes().first().is_some_and(|b| *b != b'0')
        && value.bytes().all(|b| b.is_ascii_digit()))
    .then(|| value.parse::<u32>().ok())
    .flatten()
    .filter(|n| *n <= i32::MAX as u32)
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-=+/".contains(&b))
}

impl Request {
    pub(crate) fn parse(args: &[String]) -> Result<Self> {
        let command = args.first().ok_or("操作を指定してください")?;
        let allowed: &[&str] = match command.as_str() {
            "categories" | "list" => &["--limit", "--after"],
            "view" => &["--discussion"],
            "comments" => &["--discussion", "--limit", "--after"],
            "replies" => &["--discussion", "--comment-id", "--limit", "--after"],
            "create" => &["--category-id", "--title", "--body-file"],
            "edit" => &["--discussion", "--category-id", "--title", "--body-file"],
            "comment" => &["--discussion", "--body-file"],
            "reply" | "edit-comment" => &["--discussion", "--comment-id", "--body-file"],
            "close" => &["--discussion", "--reason"],
            "reopen" => &["--discussion"],
            "mark-answer" | "unmark-answer" => &["--discussion", "--comment-id"],
            _ => return Err("許可されていないDiscussions操作です".into()),
        };
        let mut values = BTreeMap::new();
        for pair in args[1..].chunks(2) {
            if pair.len() != 2
                || (pair[0] != "--repo" && !allowed.contains(&pair[0].as_str()))
                || pair[1].is_empty()
                || pair[1].contains('\0')
                || values.insert(pair[0].as_str(), pair[1].clone()).is_some()
            {
                return Err(
                    "Discussionsのoptionは許可された正規形で1回ずつ指定してください".into(),
                );
            }
        }
        let repo = values.get("--repo").ok_or("--repoを明示してください")?;
        let parts: Vec<_> = repo.split('/').collect();
        if parts.len() != 2
            || parts.iter().any(|part| {
                part.is_empty()
                    || *part == "."
                    || *part == ".."
                    || part.len() > 100
                    || !part
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
            })
        {
            return Err("repositoryはOWNER/REPOの正規形で指定してください".into());
        }
        let required: &[&str] = match command.as_str() {
            "categories" | "list" => &[],
            "create" => &["--category-id", "--title", "--body-file"],
            "reply" | "edit-comment" => &["--discussion", "--comment-id", "--body-file"],
            "replies" | "mark-answer" | "unmark-answer" => &["--discussion", "--comment-id"],
            "comment" => &["--discussion", "--body-file"],
            "close" => &["--discussion", "--reason"],
            _ => &["--discussion"],
        };
        if required.iter().any(|key| !values.contains_key(key)) {
            return Err("操作に必要な対象または内容を明示してください（--help参照）".into());
        }
        if command == "edit"
            && !["--title", "--body-file", "--category-id"]
                .iter()
                .any(|k| values.contains_key(k))
        {
            return Err("editには変更内容を明示してください".into());
        }
        let number = values
            .get("--discussion")
            .map(|s| positive_number(s).ok_or("Discussion番号が不正です"))
            .transpose()?;
        let limit = values
            .get("--limit")
            .map(|s| {
                positive_number(s)
                    .filter(|n| *n <= 100)
                    .ok_or("limitは1..100です")
            })
            .transpose()?
            .unwrap_or(20);
        for key in ["--comment-id", "--category-id", "--after"] {
            if values.get(key).is_some_and(|v| !valid_id(v)) {
                return Err("IDまたはcursorが不正です".into());
            }
        }
        if values
            .get("--reason")
            .is_some_and(|v| !["RESOLVED", "OUTDATED", "DUPLICATE"].contains(&v.as_str()))
        {
            return Err("close reasonが許可範囲外です".into());
        }
        let title = values.get("--title").cloned();
        if let Some(title) = &title {
            if title.trim().is_empty() || title.len() > 1024 || title.chars().any(char::is_control)
            {
                return Err("titleが空、大きすぎる、または制御文字を含んでいます".into());
            }
            if let Some(reason) = guard::body_content_reason(title, "Discussion title") {
                return Err(reason);
            }
        }
        // helper内で読み取った同じbytesを検査し、以降の送信にはこの値だけを使う。
        let body = values
            .get("--body-file")
            .map(|path| {
                if !std::path::Path::new(path).is_absolute() {
                    return Err("body-fileは/tmp/配下の絶対pathで指定してください".into());
                }
                let body = guard::safe_body_file_contents(path, "Discussion body")?;
                if body.trim().is_empty() || body.contains('\0') {
                    return Err("Discussion bodyが空または不正です".into());
                }
                if let Some(reason) = guard::body_content_reason(&body, "Discussion body") {
                    return Err(reason);
                }
                Ok(body)
            })
            .transpose()?;
        Ok(Self {
            command: command.clone(),
            repo: repo.clone(),
            number,
            limit,
            title,
            body,
            comment: values.get("--comment-id").cloned(),
            category: values.get("--category-id").cloned(),
            reason: values.get("--reason").cloned(),
            after: values.get("--after").cloned(),
        })
    }
}

// query構文と変数名はコード内で固定。ユーザー入力をqueryへ補間しない。
trait Api {
    fn call(&mut self, query: &str, variables: Value) -> Result<Value>;
}

struct Github {
    cwd: String,
    sandbox: guard::GuardGhSandbox,
    deadline: Instant,
}

fn graphql_args(query: &str, variables: Value) -> Result<Vec<String>> {
    let mut args = vec![
        "api".into(),
        "graphql".into(),
        "--hostname".into(),
        "github.com".into(),
        "-f".into(),
        format!("query={query}"),
    ];
    for (key, value) in variables.as_object().ok_or("GraphQL変数が不正です")? {
        let (flag, value) = match value {
            Value::String(value) => ("-f", value.clone()),
            Value::Number(number) if number.as_u64().is_some() => ("-F", number.to_string()),
            _ => return Err("GraphQL変数の型が不正です".into()),
        };
        args.extend([flag.into(), format!("{key}={value}")]);
    }
    Ok(args)
}

fn response(bytes: &[u8]) -> Result<Value> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| "GitHub応答のJSONが不正です")?;
    if value.get("errors").is_some() || !value.get("data").is_some_and(Value::is_object) {
        return Err(
            "GitHub GraphQLが失敗しました（認証権限・対象・API仕様を確認してください）".into(),
        );
    }
    Ok(value["data"].clone())
}

impl Api for Github {
    fn call(&mut self, query: &str, variables: Value) -> Result<Value> {
        let timeout = self
            .deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_secs(20));
        if timeout.is_zero() {
            return Err("Discussions操作全体がtimeoutしました".into());
        }
        let mut command = Command::new(trust::trusted_system_binary("/usr/bin/gh", "GitHub CLI")?);
        command
            .args(graphql_args(query, variables)?)
            .current_dir(&self.cwd);
        process::clear_environment(&mut command);
        command
            .env("GH_PROMPT_DISABLED", "1")
            .env("GH_HOST", "github.com")
            .env("GH_CONFIG_DIR", self.sandbox.path())
            .env("GH_NO_UPDATE_NOTIFIER", "1")
            .env("PATH", "/usr/bin:/bin");
        let output = process::run_with_limit(&mut command, timeout, 4 * 1024 * 1024)
            .map_err(|_| "GitHub呼び出しが失敗またはtimeoutしました")?;
        if !output.status.success() {
            return Err("GitHub呼び出しに失敗しました（認証権限・対象を確認してください）".into());
        }
        response(&output.stdout)
    }
}

const REPO_QUERY: &str = "query($owner:String!,$name:String!){repository(owner:$owner,name:$name){id nameWithOwner hasDiscussionsEnabled}}";
const DISCUSSION_FIELDS: &str = "id number url title body closed stateReason repository{id nameWithOwner} category{id isAnswerable} answer{id} viewerCanUpdate viewerCanClose viewerCanReopen";
const COMMENT_FIELDS: &str = "id url body isAnswer replyTo{id} discussion{id number repository{id nameWithOwner}} viewerCanUpdate viewerCanMarkAsAnswer viewerCanUnmarkAsAnswer";

fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| format!("GitHub応答の{key}を確認できません"))
}

fn repository_matches(value: &Value, repo: &Value) -> Result<()> {
    if string(value, "id")? != string(repo, "id")?
        || !string(value, "nameWithOwner")?.eq_ignore_ascii_case(string(repo, "nameWithOwner")?)
    {
        return Err("対象のrepository所属が一致しません".into());
    }
    Ok(())
}

fn discussion_matches(value: &Value, repo: &Value, number: u32) -> Result<()> {
    string(value, "id")?;
    repository_matches(&value["repository"], repo)?;
    if value["number"].as_u64() != Some(u64::from(number)) {
        return Err("対象のDiscussion番号が一致しません".into());
    }
    Ok(())
}

fn get_discussion(
    api: &mut impl Api,
    request: &Request,
    repo: &Value,
    number: u32,
) -> Result<Value> {
    let (owner, name) = request.repo.split_once('/').ok_or("repositoryが不正です")?;
    let query = format!(
        "query($owner:String!,$name:String!,$number:Int!){{repository(owner:$owner,name:$name){{discussion(number:$number){{{DISCUSSION_FIELDS}}}}}}}"
    );
    let data = api.call(&query, json!({"owner":owner,"name":name,"number":number}))?;
    let discussion = &data["repository"]["discussion"];
    discussion_matches(discussion, repo, number)?;
    Ok(discussion.clone())
}

fn comment_matches(value: &Value, repo: &Value, discussion: &Value, id: &str) -> Result<()> {
    if string(value, "id")? != id
        || string(&value["discussion"], "id")? != string(discussion, "id")?
    {
        return Err("commentのIDまたはDiscussion所属が一致しません".into());
    }
    discussion_matches(
        &value["discussion"],
        repo,
        discussion["number"]
            .as_u64()
            .ok_or("Discussion番号が不正です")? as u32,
    )
}

fn get_comment(api: &mut impl Api, repo: &Value, discussion: &Value, id: &str) -> Result<Value> {
    let query =
        format!("query($id:ID!){{node(id:$id){{... on DiscussionComment{{{COMMENT_FIELDS}}}}}}}");
    let data = api.call(&query, json!({"id":id}))?;
    let comment = &data["node"];
    comment_matches(comment, repo, discussion, id)?;
    Ok(comment.clone())
}

fn connection(value: &Value) -> Result<()> {
    if !value["nodes"].is_array()
        || !value["pageInfo"]["hasNextPage"].is_boolean()
        || !(value["pageInfo"]["endCursor"].is_string() || value["pageInfo"]["endCursor"].is_null())
        || value["pageInfo"]["hasNextPage"] == true && !value["pageInfo"]["endCursor"].is_string()
    {
        return Err("一覧のpageInfoまたはnodesを確認できません".into());
    }
    Ok(())
}

fn categories(api: &mut impl Api, repo: &Value, limit: u32, after: Option<&str>) -> Result<Value> {
    let query = "query($id:ID!,$limit:Int!,$after:String){node(id:$id){... on Repository{id nameWithOwner discussionCategories(first:$limit,after:$after){nodes{id name isAnswerable} pageInfo{hasNextPage endCursor}}}}}";
    let mut vars = json!({"id":string(repo,"id")?,"limit":limit});
    if let Some(after) = after {
        vars["after"] = json!(after);
    }
    let data = api.call(query, vars)?;
    repository_matches(&data["node"], repo)?;
    let list = &data["node"]["discussionCategories"];
    connection(list)?;
    Ok(list.clone())
}

fn verify_category(api: &mut impl Api, repo: &Value, id: &str) -> Result<()> {
    let data = api.call(
        "query($id:ID!){node(id:$id){... on DiscussionCategory{id repository{id nameWithOwner}}}}",
        json!({"id":id}),
    )?;
    same_field(&data["node"], "id", id)?;
    repository_matches(&data["node"]["repository"], repo)
}

fn read_list(
    api: &mut impl Api,
    request: &Request,
    repo: &Value,
    discussion: Option<&Value>,
    comment: Option<&Value>,
) -> Result<Value> {
    let (kind, field, fields, id) = match request.command.as_str() {
        "list" => (
            "Repository",
            "discussions",
            DISCUSSION_FIELDS,
            string(repo, "id")?,
        ),
        "comments" => (
            "Discussion",
            "comments",
            COMMENT_FIELDS,
            string(discussion.ok_or("Discussionが必要です")?, "id")?,
        ),
        "replies" => (
            "DiscussionComment",
            "replies",
            COMMENT_FIELDS,
            string(comment.ok_or("commentが必要です")?, "id")?,
        ),
        _ => return Err("一覧操作が不正です".into()),
    };
    let query = format!(
        "query($id:ID!,$limit:Int!,$after:String){{node(id:$id){{... on {kind}{{id {field}(first:$limit,after:$after){{nodes{{{fields}}} pageInfo{{hasNextPage endCursor}}}}}}}}}}"
    );
    let mut vars = json!({"id":id,"limit":request.limit});
    if let Some(after) = &request.after {
        vars["after"] = json!(after);
    }
    let data = api.call(&query, vars)?;
    if string(&data["node"], "id")? != id {
        return Err("一覧の対象IDが一致しません".into());
    }
    let list = &data["node"][field];
    connection(list)?;
    for node in list["nodes"].as_array().ok_or("一覧が不正です")? {
        if request.command == "list" {
            repository_matches(&node["repository"], repo)?;
        } else {
            comment_matches(
                node,
                repo,
                discussion.ok_or("Discussionが必要です")?,
                string(node, "id")?,
            )?;
            if request.command == "replies" && node["replyTo"]["id"] != id {
                return Err("返信の親commentが一致しません".into());
            }
        }
    }
    Ok(list.clone())
}

fn can(value: &Value, key: &str) -> Result<()> {
    if value[key] != true {
        return Err(format!("対象の{key}権限を確認できません"));
    }
    Ok(())
}

fn mutate(
    api: &mut impl Api,
    operation: &str,
    input_type: &str,
    payload: &str,
    vars: Value,
) -> Result<Value> {
    // operation/input/payloadも呼び出し元の固定literalだけ。任意GraphQLは公開しない。
    let object = vars.as_object().ok_or("mutation変数が不正です")?;
    let mut declarations = Vec::new();
    let mut input = Vec::new();
    for key in object.keys() {
        let ty = match key.as_str() {
            "title" | "body" => "String!",
            "reason" => "DiscussionCloseReason!",
            _ => "ID!",
        };
        declarations.push(format!("${key}:{ty}"));
        input.push(format!("{key}:${key}"));
    }
    // input_typeはAPI契約の可読性とfixture検査に用いる。
    debug_assert_eq!(
        input_type,
        format!(
            "{}{}Input",
            operation[..1].to_ascii_uppercase(),
            &operation[1..]
        )
    );
    let query = format!(
        "mutation({}){{{operation}(input:{{{}}}){{{payload}}}}}",
        declarations.join(","),
        input.join(",")
    );
    let data = api.call(&query, vars)?;
    data.get(operation)
        .filter(|v| v.is_object())
        .cloned()
        .ok_or_else(|| "mutation結果を確認できません".into())
}

fn same_field(actual: &Value, key: &str, expected: &str) -> Result<()> {
    if actual[key].as_str() != Some(expected) {
        return Err(format!("書き込み結果の{key}が一致しません"));
    }
    Ok(())
}

fn write(
    api: &mut impl Api,
    request: &Request,
    repo: &Value,
    discussion: Option<&Value>,
    comment: Option<&Value>,
) -> Result<Value> {
    if let Some(category) = &request.category {
        verify_category(api, repo, category)?;
    }
    if request.command == "create" {
        let result = mutate(
            api,
            "createDiscussion",
            "CreateDiscussionInput",
            "discussion{id number}",
            json!({
                "repositoryId":string(repo,"id")?,"categoryId":request.category,"title":request.title,"body":request.body
            }),
        )?;
        let created = &result["discussion"];
        let number = created["number"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .filter(|n| *n > 0)
            .ok_or("作成結果の番号を確認できません")?;
        let actual = get_discussion(api, request, repo, number)?;
        same_field(&actual, "id", string(created, "id")?)?;
        verify_discussion_changes(&actual, request)?;
        return Ok(actual);
    }
    let discussion = discussion.ok_or("Discussionが必要です")?;
    let discussion_id = string(discussion, "id")?;
    match request.command.as_str() {
        "edit" | "close" | "reopen" => {
            can(
                discussion,
                match request.command.as_str() {
                    "close" => "viewerCanClose",
                    "reopen" => "viewerCanReopen",
                    _ => "viewerCanUpdate",
                },
            )?;
            let mut vars = json!({"discussionId":discussion_id});
            let (op, ty) = match request.command.as_str() {
                "edit" => {
                    for (key, value) in [
                        ("title", &request.title),
                        ("body", &request.body),
                        ("categoryId", &request.category),
                    ] {
                        if let Some(value) = value {
                            vars[key] = json!(value);
                        }
                    }
                    ("updateDiscussion", "UpdateDiscussionInput")
                }
                "close" => {
                    vars["reason"] = json!(request.reason);
                    ("closeDiscussion", "CloseDiscussionInput")
                }
                _ => ("reopenDiscussion", "ReopenDiscussionInput"),
            };
            let result = mutate(api, op, ty, "discussion{id}", vars)?;
            same_field(&result["discussion"], "id", discussion_id)?;
            let actual =
                get_discussion(api, request, repo, request.number.ok_or("番号が必要です")?)?;
            same_field(&actual, "id", discussion_id)?;
            verify_discussion_changes(&actual, request)?;
            Ok(actual)
        }
        "comment" | "reply" | "edit-comment" => {
            let mut vars = json!({"body":request.body});
            let (op, ty) = if request.command == "edit-comment" {
                can(comment.ok_or("commentが必要です")?, "viewerCanUpdate")?;
                vars["commentId"] = json!(request.comment);
                ("updateDiscussionComment", "UpdateDiscussionCommentInput")
            } else {
                vars["discussionId"] = json!(discussion_id);
                if request.command == "reply" {
                    if !comment.ok_or("親commentが必要です")?["replyTo"].is_null() {
                        return Err("replyの対象はトップレベルcommentに限定します".into());
                    }
                    vars["replyToId"] = json!(request.comment);
                }
                ("addDiscussionComment", "AddDiscussionCommentInput")
            };
            let result = mutate(api, op, ty, "comment{id}", vars)?;
            let id = string(&result["comment"], "id")?;
            if request.command == "edit-comment" && Some(id) != request.comment.as_deref() {
                return Err("編集したcomment IDが一致しません".into());
            }
            let actual = get_comment(api, repo, discussion, id)?;
            same_field(
                &actual,
                "body",
                request.body.as_deref().ok_or("bodyが必要です")?,
            )?;
            if request.command == "reply"
                && actual["replyTo"]["id"].as_str() != request.comment.as_deref()
                || request.command == "comment" && !actual["replyTo"].is_null()
            {
                return Err("投稿したcommentの親が一致しません".into());
            }
            Ok(actual)
        }
        "mark-answer" | "unmark-answer" => {
            let comment = comment.ok_or("commentが必要です")?;
            let id = string(comment, "id")?;
            let mark = request.command == "mark-answer";
            can(
                comment,
                if mark {
                    "viewerCanMarkAsAnswer"
                } else {
                    "viewerCanUnmarkAsAnswer"
                },
            )?;
            if mark && discussion["category"]["isAnswerable"] != true {
                return Err("回答を指定できるcategoryではありません".into());
            }
            if !mark && discussion["answer"]["id"] != id {
                return Err("解除対象が現在の回答と一致しません".into());
            }
            let (op, ty) = if mark {
                (
                    "markDiscussionCommentAsAnswer",
                    "MarkDiscussionCommentAsAnswerInput",
                )
            } else {
                (
                    "unmarkDiscussionCommentAsAnswer",
                    "UnmarkDiscussionCommentAsAnswerInput",
                )
            };
            let result = mutate(api, op, ty, "discussion{id}", json!({"id":id}))?;
            same_field(&result["discussion"], "id", discussion_id)?;
            let actual =
                get_discussion(api, request, repo, request.number.ok_or("番号が必要です")?)?;
            same_field(&actual, "id", discussion_id)?;
            if mark && actual["answer"]["id"] != id || !mark && !actual["answer"].is_null() {
                return Err("回答の指定・解除結果が一致しません".into());
            }
            Ok(actual)
        }
        _ => Err("許可されていない書き込みです".into()),
    }
}

fn verify_discussion_changes(actual: &Value, request: &Request) -> Result<()> {
    if let Some(title) = &request.title {
        same_field(actual, "title", title)?;
    }
    if let Some(body) = &request.body {
        same_field(actual, "body", body)?;
    }
    if let Some(category) = &request.category {
        same_field(&actual["category"], "id", category)?;
    }
    match request.command.as_str() {
        "close"
            if actual["closed"] != true
                || actual["stateReason"].as_str() != request.reason.as_deref() =>
        {
            Err("close結果が一致しません".into())
        }
        "reopen" if actual["closed"] != false => Err("reopen結果が一致しません".into()),
        _ => Ok(()),
    }
}

fn execute(api: &mut impl Api, request: &Request) -> Result<Value> {
    let (owner, name) = request.repo.split_once('/').ok_or("repositoryが不正です")?;
    let data = api.call(REPO_QUERY, json!({"owner":owner,"name":name}))?;
    let repo = &data["repository"];
    string(repo, "id")?;
    if !string(repo, "nameWithOwner")?.eq_ignore_ascii_case(&request.repo)
        || repo["hasDiscussionsEnabled"] != true
    {
        return Err("repository identityまたはDiscussions有効状態を確認できません".into());
    }
    let discussion = request
        .number
        .map(|n| get_discussion(api, request, repo, n))
        .transpose()?;
    let comment = request
        .comment
        .as_ref()
        .map(|id| {
            get_comment(
                api,
                repo,
                discussion.as_ref().ok_or("Discussionが必要です")?,
                id,
            )
        })
        .transpose()?;
    let result = match request.command.as_str() {
        "categories" => categories(api, repo, request.limit, request.after.as_deref())?,
        "view" => discussion.ok_or("Discussionが必要です")?,
        "list" | "comments" | "replies" => {
            read_list(api, request, repo, discussion.as_ref(), comment.as_ref())?
        }
        _ => write(api, request, repo, discussion.as_ref(), comment.as_ref()).map_err(|e| {
            format!("{e}。送信済みの可能性があるため、対象を再取得してから再実行してください")
        })?,
    };
    Ok(json!({"repository":request.repo,"operation":request.command,"result":result}))
}

pub(crate) fn entrypoint<I: IntoIterator<Item = OsString>>(args: I) -> i32 {
    let run = || -> Result<()> {
        let args: Vec<String> = args
            .into_iter()
            .map(|v| {
                v.into_string()
                    .map_err(|_| "引数はUTF-8で指定してください".to_string())
            })
            .collect::<Result<_>>()?;
        if is_help(&args) {
            println!("{USAGE}");
            return Ok(());
        }
        let request = Request::parse(&args)?;
        let cwd = std::env::current_dir().map_err(|_| "cwdを確認できません")?;
        let cwd = cwd.to_str().ok_or("cwdはUTF-8である必要があります")?;
        if let Some(reason) = guard::repository_reason(&request.repo, cwd) {
            return Err(reason);
        }
        let mut api = Github {
            cwd: cwd.into(),
            sandbox: guard::GuardGhSandbox::create()
                .ok_or("GitHub認証設定を安全に固定できません")?,
            deadline: Instant::now() + Duration::from_secs(90),
        };
        let result = execute(&mut api, &request)?;
        println!(
            "{}",
            serde_json::to_string_pretty(&result).map_err(|_| "結果をJSONへ変換できません")?
        );
        Ok(())
    };
    match run() {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("codex-discussions: {error}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    struct Fake {
        responses: VecDeque<Value>,
        calls: Vec<(String, Value)>,
    }
    impl Fake {
        fn new(responses: Vec<Value>) -> Self {
            Self {
                responses: responses.into(),
                calls: Vec::new(),
            }
        }
        fn mutations(&self) -> Vec<&(String, Value)> {
            self.calls
                .iter()
                .filter(|(q, _)| q.starts_with("mutation"))
                .collect()
        }
    }
    impl Api for Fake {
        fn call(&mut self, query: &str, variables: Value) -> Result<Value> {
            self.calls.push((query.into(), variables));
            self.responses
                .pop_front()
                .ok_or_else(|| "想定外のAPI再送".into())
        }
    }
    fn repo() -> Value {
        json!({"id":"R_1","nameWithOwner":"owner/repo","hasDiscussionsEnabled":true})
    }
    fn discussion() -> Value {
        json!({"id":"D_1","number":1,"title":"変更","body":"本文","repository":repo(),
            "category":{"id":"CAT_1","isAnswerable":true},"closed":false,"stateReason":null,
            "answer":null,"viewerCanUpdate":true,"viewerCanClose":true,"viewerCanReopen":true})
    }
    fn comment() -> Value {
        json!({"id":"C_1","body":"本文","discussion":discussion(),"replyTo":null,
            "isAnswer":false,"viewerCanUpdate":true,"viewerCanMarkAsAnswer":true,"viewerCanUnmarkAsAnswer":true})
    }
    fn request(command: &str) -> Request {
        Request {
            command: command.into(),
            repo: "owner/repo".into(),
            number: Some(1),
            comment: None,
            category: None,
            title: None,
            body: None,
            reason: None,
            limit: 20,
            after: None,
        }
    }
    fn view_response(discussion: Value) -> Value {
        json!({"repository":{"discussion":discussion}})
    }
    fn page(nodes: Value) -> Value {
        json!({"nodes":nodes,"pageInfo":{"hasNextPage":false,"endCursor":null}})
    }

    #[test]
    fn all_writes_use_one_scoped_mutation_and_read_back() {
        for (command, operation) in [
            ("create", "createDiscussion"),
            ("edit", "updateDiscussion"),
            ("comment", "addDiscussionComment"),
            ("reply", "addDiscussionComment"),
            ("edit-comment", "updateDiscussionComment"),
            ("close", "closeDiscussion"),
            ("reopen", "reopenDiscussion"),
            ("mark-answer", "markDiscussionCommentAsAnswer"),
            ("unmark-answer", "unmarkDiscussionCommentAsAnswer"),
        ] {
            let mut req = request(command);
            let mut before = discussion();
            let mut after = discussion();
            let mut result_comment = comment();
            let mut responses = vec![json!({"repository":repo()})];
            if command == "create" {
                req.number = None;
            }
            if command == "unmark-answer" {
                before["answer"] = json!({"id":"C_1"});
            }
            if command == "reopen" {
                before["closed"] = json!(true);
            }
            if req.number.is_some() {
                responses.push(view_response(before));
            }
            if ["reply", "edit-comment", "mark-answer", "unmark-answer"].contains(&command) {
                req.comment = Some("C_1".into());
                responses.push(json!({"node":comment()}));
            }
            if ["create", "edit"].contains(&command) {
                req.title = Some("変更".into());
                req.category = Some("CAT_1".into());
                responses.push(json!({"node":{"id":"CAT_1","repository":repo()}}));
            }
            if ["create", "edit", "comment", "reply", "edit-comment"].contains(&command) {
                req.body = Some("本文".into());
            }
            if command == "close" {
                req.reason = Some("RESOLVED".into());
                after["closed"] = json!(true);
                after["stateReason"] = json!("RESOLVED");
            }
            if command == "mark-answer" {
                after["answer"] = json!({"id":"C_1"});
            }
            if command == "reply" {
                result_comment["replyTo"] = json!({"id":"C_1"});
                result_comment["id"] = json!("C_2");
            }
            let comment_write = ["comment", "reply", "edit-comment"].contains(&command);
            let payload = if comment_write {
                json!({"comment":{"id":result_comment["id"]}})
            } else {
                json!({"discussion":{"id":"D_1","number":1}})
            };
            responses.push(json!({operation:payload}));
            responses.push(if comment_write {
                json!({"node":result_comment})
            } else {
                view_response(after)
            });
            let mut api = Fake::new(responses);
            assert!(
                execute(&mut api, &req).is_ok(),
                "{command}: {:?}",
                api.calls
            );
            assert!(api.responses.is_empty(), "{command}: read-back未実行");
            let mutations = api.mutations();
            assert_eq!(mutations.len(), 1, "{command}");
            let (query, vars) = mutations[0];
            assert!(query.contains(&format!("{operation}(input:")), "{query}");
            assert!(!query.contains("本文"));
            match command {
                "create" => {
                    assert_eq!(vars["repositoryId"], "R_1");
                    assert_eq!(vars["categoryId"], "CAT_1");
                }
                "edit-comment" => assert_eq!(vars["commentId"], "C_1"),
                "mark-answer" | "unmark-answer" => assert_eq!(vars["id"], "C_1"),
                _ => assert_eq!(vars["discussionId"], "D_1"),
            }
            if command == "reply" {
                assert_eq!(vars["replyToId"], "C_1");
            }
        }
    }

    #[test]
    fn target_mismatch_or_missing_permission_prevents_every_mutation() {
        for field in ["repository", "number", "id", "viewerCanClose"] {
            let mut d = discussion();
            d[field] = match field {
                "repository" => json!({"id":"OTHER","nameWithOwner":"other/repo"}),
                "number" => json!(2),
                "viewerCanClose" => json!(false),
                _ => Value::Null,
            };
            let mut req = request("close");
            req.reason = Some("RESOLVED".into());
            let mut api = Fake::new(vec![json!({"repository":repo()}), view_response(d)]);
            assert!(execute(&mut api, &req).is_err(), "{field}");
            assert!(api.mutations().is_empty(), "{field}");
        }
        for mismatch in ["repository", "discussion", "comment"] {
            let mut c = comment();
            match mismatch {
                "repository" => c["discussion"]["repository"]["id"] = json!("R_OTHER"),
                "discussion" => c["discussion"]["id"] = json!("D_OTHER"),
                _ => c["id"] = json!("C_OTHER"),
            }
            let mut req = request("edit-comment");
            req.comment = Some("C_1".into());
            req.body = Some("本文".into());
            let mut api = Fake::new(vec![
                json!({"repository":repo()}),
                view_response(discussion()),
                json!({"node":c}),
            ]);
            assert!(execute(&mut api, &req).is_err());
            assert!(api.mutations().is_empty());
        }
        let mut req = request("create");
        req.number = None;
        req.category = Some("CAT_OTHER".into());
        req.title = Some("変更".into());
        req.body = Some("本文".into());
        let mut api = Fake::new(vec![
            json!({"repository":repo()}),
            json!({"node":{"id":"CAT_OTHER","repository":{"id":"R_OTHER","nameWithOwner":"other/repo"}}}),
        ]);
        assert!(execute(&mut api, &req).is_err());
        assert!(api.mutations().is_empty());
    }

    #[test]
    fn read_back_mismatch_and_transport_failure_never_retry_writes() {
        for result in [Value::Null, json!({"id":"D_OTHER"}), json!({"id":"D_1"})] {
            let mut req = request("edit");
            req.title = Some("更新後".into());
            let mut api = Fake::new(vec![
                json!({"repository":repo()}),
                view_response(discussion()),
                json!({"updateDiscussion":{"discussion":result}}),
                view_response(discussion()),
            ]);
            assert!(execute(&mut api, &req).is_err());
            assert_eq!(api.mutations().len(), 1);
        }
        let mut req = request("comment");
        req.body = Some("本文".into());
        let mut api = Fake::new(vec![
            json!({"repository":repo()}),
            view_response(discussion()),
        ]);
        assert!(execute(&mut api, &req).is_err());
        assert_eq!(api.mutations().len(), 1);
        for bytes in [
            b"{}".as_slice(),
            b"{\"data\":null}",
            b"{\"data\":{},\"errors\":[{}]}",
            b"invalid",
        ] {
            assert!(response(bytes).is_err());
        }
    }

    #[test]
    fn all_reads_return_explicit_pagination_and_check_child_membership() {
        for command in READ_COMMANDS {
            let mut req = request(command);
            let mut responses = vec![json!({"repository":repo()})];
            if ["list", "categories"].contains(command) {
                req.number = None;
            }
            if req.number.is_some() {
                responses.push(view_response(discussion()));
            }
            let mut c = comment();
            if *command == "replies" {
                req.comment = Some("C_1".into());
                responses.push(json!({"node":comment()}));
                c["replyTo"] = json!({"id":"C_1"});
            }
            let (id, field, nodes) = match *command {
                "categories" => ("R_1", "discussionCategories", json!([{"id":"CAT_1"}])),
                "list" => ("R_1", "discussions", json!([discussion()])),
                "comments" => ("D_1", "comments", json!([c])),
                "replies" => ("C_1", "replies", json!([c])),
                _ => ("", "", Value::Null),
            };
            if *command != "view" {
                req.after = Some("cursor".into());
                req.limit = 3;
                responses
                    .push(json!({"node":{"id":id,"nameWithOwner":"owner/repo",field:page(nodes)}}));
            }
            let mut api = Fake::new(responses);
            assert!(execute(&mut api, &req).is_ok(), "{command}");
            assert!(api.responses.is_empty());
            assert!(api.mutations().is_empty());
            if *command != "view" {
                let (_, vars) = api.calls.last().unwrap();
                assert_eq!(vars["after"], "cursor");
                assert_eq!(vars["limit"], 3);
            }
        }
        let mut api = Fake::new(vec![
            json!({"node":{"id":"D_1","comments":page(json!([{"id":"C_1","discussion":{"id":"D_OTHER"}}]))}}),
        ]);
        assert!(
            read_list(
                &mut api,
                &request("comments"),
                &repo(),
                Some(&discussion()),
                None
            )
            .is_err()
        );
        assert!(connection(&json!({"nodes":[],"pageInfo":{"hasNextPage":true}})).is_err());
    }

    #[test]
    fn cli_rejects_unknown_operations_options_and_ambiguous_targets() {
        for args in [
            "delete --repo owner/repo --discussion 1",
            "transfer --repo owner/repo --discussion 1",
            "view --repo owner/repo --discussion 0",
            "view --repo owner/repo --discussion 01",
            "view --repo owner/repo --discussion 2147483648",
            "view --repo owner/repo --discussion 1 --discussion 2",
            "view --repo owner/repo --discussion 1 --query mutation",
            "list --repo owner/repo --hostname example.com",
            "list --repo owner/repo --limit 101",
            "list --repo owner/repo --after @/tmp/query",
            "list --repo https://github.com/owner/repo",
            "list --repo owner/repo --repo owner/repo",
            "edit --repo owner/repo --discussion 1",
            "close --repo owner/repo --discussion 1 --reason UNKNOWN",
            "comment --repo owner/repo --discussion 1 --body text",
        ] {
            assert!(
                Request::parse(
                    &args
                        .split_whitespace()
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                )
                .is_err(),
                "{args}"
            );
        }
        assert!(Request::parse(&["list", "--repo", "owner/repo"].map(str::to_string)).is_ok());
        assert!(is_help(&["view", "--help"].map(str::to_string)));
        assert!(!is_help(&["delete", "--help"].map(str::to_string)));
        // -fでユーザー文字列を渡し、@file読み込み・型変換・query注入を起こさない。
        let args = graphql_args(
            "fixed",
            json!({"body":"@/tmp/input\nmutation { deleteRepository }","number":1}),
        )
        .unwrap();
        assert!(
            args.windows(2)
                .any(|p| p == ["-f", "body=@/tmp/input\nmutation { deleteRepository }"])
        );
        assert!(args.windows(2).any(|p| p == ["-F", "number=1"]));
    }

    #[cfg(unix)]
    #[test]
    fn body_bytes_are_validated_once_and_symlinks_secrets_and_fifos_fail_closed() {
        use std::os::unix::fs::symlink;
        let directory = std::env::temp_dir().join(format!(
            "discussions-body-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("body.md");
        let args = vec![
            "comment".into(),
            "--repo".into(),
            "owner/repo".into(),
            "--discussion".into(),
            "1".into(),
            "--body-file".into(),
            path.display().to_string(),
        ];
        std::fs::write(&path, "検査した本文").unwrap();
        let req = Request::parse(&args).unwrap();
        std::fs::write(&path, "書き換え").unwrap();
        assert_eq!(req.body.as_deref(), Some("検査した本文"));
        for contents in [
            concat!("github", "_pat_abcdefghijklmnopqrstuvwxyz"),
            "@copilot please",
            "",
        ] {
            std::fs::write(&path, contents).unwrap();
            assert!(Request::parse(&args).is_err());
        }
        let link = directory.join("link");
        symlink(&path, &link).unwrap();
        let mut unsafe_args = args.clone();
        *unsafe_args.last_mut().unwrap() = link.display().to_string();
        assert!(Request::parse(&unsafe_args).is_err());
        let fifo = directory.join("fifo");
        let name = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        *unsafe_args.last_mut().unwrap() = fifo.display().to_string();
        assert!(Request::parse(&unsafe_args).is_err());
        std::fs::remove_dir_all(directory).unwrap();
    }
}

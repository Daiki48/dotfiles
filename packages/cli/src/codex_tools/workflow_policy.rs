//! 明示承認のlocal-only gateで扱う、タグpushだけのworkflowの判定。
use serde_json::{Map, Value};
use std::collections::VecDeque;
use yaml_rust2::Yaml;
use yaml_rust2::parser::{Event, Parser};
use yaml_rust2::scanner::TScalarStyle;

const MAX_BYTES: usize = 256 * 1024;
const MAX_EVENTS: usize = 10_000;
const MAX_DEPTH: usize = 64;

fn node(events: &mut VecDeque<Event>, depth: usize) -> Result<Value, &'static str> {
    if depth > MAX_DEPTH {
        return Err("workflow YAMLが深すぎます");
    }
    match events.pop_front() {
        Some(Event::Scalar(value, style, 0, None)) => {
            // tagsの数値・null・booleanを文字列と同一視しない。
            if style != TScalarStyle::Plain || matches!(Yaml::from_str(&value), Yaml::String(_)) {
                Ok(Value::String(value))
            } else {
                Ok(Value::Null)
            }
        }
        Some(Event::SequenceStart(0, None)) => {
            let mut values = Vec::new();
            while events.front() != Some(&Event::SequenceEnd) {
                values.push(node(events, depth + 1)?);
            }
            events.pop_front();
            Ok(Value::Array(values))
        }
        Some(Event::MappingStart(0, None)) => {
            let mut values = Map::new();
            while events.front() != Some(&Event::MappingEnd) {
                let Some(Event::Scalar(key, _, 0, None)) = events.pop_front() else {
                    return Err("workflow YAMLのkeyが不正です");
                };
                if key == "<<" || values.contains_key(&key) {
                    return Err("workflow YAMLのmerge key・重複keyは扱えません");
                }
                values.insert(key, node(events, depth + 1)?);
            }
            events.pop_front();
            Ok(Value::Object(values))
        }
        _ => Err("workflow YAMLのanchor・alias・tagまたは構造が不正です"),
    }
}

pub(super) fn tag_push_only(source: &str) -> Result<bool, &'static str> {
    if source.len() > MAX_BYTES {
        return Err("workflow YAMLが容量上限を超えています");
    }
    let mut parser = Parser::new_from_str(source);
    let mut events = VecDeque::new();
    loop {
        let (event, _) = parser
            .next_token()
            .map_err(|_| "workflow YAMLを解析できません")?;
        let end = event == Event::StreamEnd;
        events.push_back(event);
        if events.len() > MAX_EVENTS {
            return Err("workflow YAMLが要素上限を超えています");
        }
        if end {
            break;
        }
    }
    if events.pop_front() != Some(Event::StreamStart)
        || events.pop_front() != Some(Event::DocumentStart)
    {
        return Err("workflow YAMLのdocumentが不正です");
    }
    let workflow = node(&mut events, 0)?;
    if events.pop_front() != Some(Event::DocumentEnd)
        || events.pop_front() != Some(Event::StreamEnd)
        || !events.is_empty()
    {
        return Err("workflow YAMLは単一documentに限定します");
    }
    let Some(triggers) = workflow.get("on").and_then(Value::as_object) else {
        return Ok(false);
    };
    let Some(push) = triggers.get("push").and_then(Value::as_object) else {
        return Ok(false);
    };
    let Some(tags) = push.get("tags").and_then(Value::as_array) else {
        return Ok(false);
    };
    Ok(triggers.len() == 1
        && push.len() == 1
        && !tags.is_empty()
        && tags.iter().all(|tag| {
            tag.as_str().is_some_and(|pattern| {
                !pattern.is_empty()
                    && !pattern.starts_with('!')
                    && !pattern.contains("${{")
                    && !pattern.chars().any(char::is_control)
            })
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_block_and_flow_tag_only_triggers_with_quoted_keys() {
        for yaml in [
            "name: CD\non:\n  push:\n    tags: [\"v*.*.*\"]\njobs:\n  deploy:\n    steps:\n      - run: |\n          echo pull_request\n          echo 'on: push'\n",
            "'on': {push: {tags: ['v*', '*release*']}}\n",
        ] {
            assert_eq!(tag_push_only(yaml), Ok(true));
        }
    }

    #[test]
    fn rejects_ci_manual_unknown_and_ambiguous_workflows() {
        for yaml in [
            "on: push",
            "on: [push, pull_request]",
            "on: {push: null}",
            "on: {push: {branches: [main], tags: ['v*']}}",
            "on: {push: {tags: ['v*'], paths: ['src/**']}}",
            "on: {push: {tags-ignore: ['docs*']}}",
            "on: {push: {tags: ['v*']}, pull_request: null}",
            "on: {push: {tags: ['v*']}, workflow_dispatch: null}",
            "on: {workflow_call: null}",
            "name: CD",
            "on: {unknown: {tags: ['v*']}}",
            "on: {push: {tags: []}}",
            "on: {push: {tags: [null]}}",
            "on: {push: {tags: [true]}}",
            "on: {push: {tags: [1]}}",
            "on: {push: {tags: ['!v*']}}",
            "on: {push: {tags: ['${{ env.TAGS }}']}}",
            "on: {push: {tags: ['v*']}}\non: pull_request",
            "on: {push: {tags: ['v*'], tags: ['v*']}}",
            "on: &trigger {push: {tags: ['v*']}}",
            "on: !trigger {push: {tags: ['v*']}}",
            "on: {push: {tags: ['v*']}}\nother: *missing",
            "on: {push: {tags: ['v*']}}\n---\non: push",
            "on: {push: {tags: ['v*']}, <<: {pull_request: null}}",
            "on: {push: [",
            "on: {push: {tags: ['v*']}}\non: push\n",
        ] {
            assert_ne!(tag_push_only(yaml), Ok(true), "{yaml}");
        }
        assert!(tag_push_only(&" ".repeat(MAX_BYTES + 1)).is_err());
        let deep = format!(
            "on: {}0{}",
            "[".repeat(MAX_DEPTH + 2),
            "]".repeat(MAX_DEPTH + 2)
        );
        assert!(tag_push_only(&deep).is_err());
    }
}

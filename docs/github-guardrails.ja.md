# GitHub CI・Ruleset運用ガイド

このrepositoryでは、ローカルhookを誤操作の早期検出、GitHub ActionsとRulesetを
remote `main` の最終安全境界として扱います。hookは通常のIssue・PR管理を妨げず、
protected branchへの直接push、force push、削除、秘密情報、任意API mutationを拒否します。

## Required CI

`.github/workflows/ci.yml`のjob表示名`required-ci`を唯一のrequired checkとして使用します。
pull requestと`main`へのpushで、次を同じjob内で実行します。

| 検証 | ローカルとCIのcommand |
|---|---|
| Git/GitHub hook | `python3 -m unittest discover -s .codex/hooks -p 'test_*.py'` |
| worktree helper | `python3 -m unittest discover -s .codex/helpers -p 'test_*.py'` |
| Ruleset宣言 | `python3 -m unittest discover -s .github/rulesets -p 'test_*.py'` |
| Rust workspace | `cargo test --workspace --locked` |
| Rust format | `cargo fmt --all -- --check` |

workflowは`contents: read`だけを要求し、credentialをcheckout後に保持しません。
`pull_request_target`は使用せず、forkや未信頼branchのコードへsecretを渡しません。
path filterは設定しないため、required checkがskipされてpendingのまま残る経路を作りません。

## hookと明示確認の境界

通常のIssue・PR lifecycleは、hookがcurrent repository、単一対象、正規形の引数、送信内容を
検査した上で自律実行します。force pushや削除は、検査を外して直接許可せずhard denyを維持します。

Codexの[`PreToolUse` hook](https://learn.chatgpt.com/docs/hooks)は現時点で承認要求を新規に
発生させられません。そのため、危険操作を
「Daikiの確認後だけCodexが実行」へ移す場合は、対象の退避・repository・refを検査する専用helperと、
毎回確認する[`prompt` rule](https://learn.chatgpt.com/docs/agent-configuration/rules)を組み合わせます。その経路が実装・検証されるまでは、確認済みという
会話上の事実だけでhard denyを迂回しません。

## main Ruleset

正本は`.github/rulesets/main.json`です。対象は`~DEFAULT_BRANCH`で、次を強制します。

- pull request経由の変更
- GitHub Actions App（integration ID `15368`）由来の`required-ci`
- merge前の最新baseに対するCI成功
- 未解決review conversationがないこと
- merge commit方式だけを許可
- force pushとbranch削除の禁止
- bypass actorなし

required review数は0です。個人repositoryで同一actorの形式的なself approvalを要求せず、
review済みhead SHAの固定とrisk-based reviewは後続Issue #24の完了フローで担保します。
この構成だけでは、PR作者が`required-ci` workflow自体を変更する攻撃をGitHub上で完全には
防げません。#24ではworkflow差分を含む固定head SHAを独立reviewし、check名だけでなく実行内容も
確認します。独立したreview identityを用意できる場合は、CODEOWNERSとrequired approvalを再検討します。

## 適用

Ruleset変更はrepository全体へ影響するため、Daikiの明示確認後だけ実行します。
今回のIssue #23実装では2026-08-19の依頼を適用承認として記録しています。
ただし、このJSONをcommitしただけではremoteへ適用されません。readbackと`gh ruleset check`が
完了するまでは、remote `main`が保護済みとは扱いません。

適用前に既存設定を保存します。

```sh
gh api repos/Daiki48/dotfiles/rulesets > /tmp/dotfiles-rulesets-before.json
```

同名Rulesetが存在しないことを確認して作成します。

```sh
gh api repos/Daiki48/dotfiles/rulesets \
  --method POST \
  --input .github/rulesets/main.json
```

作成後に返された数値IDを使い、readbackと適用対象を確認します。

```sh
gh api repos/Daiki48/dotfiles/rulesets/<RULESET_ID>
gh ruleset check main --repo Daiki48/dotfiles
```

次が正本と一致しない場合は完了扱いにしません。

- `enforcement=active`
- `bypass_actors=[]`
- `~DEFAULT_BRANCH`だけが対象
- `required-ci`のsourceがGitHub Actions App `15368`
- strict status check、conversation解決、PR必須、merge-only、deletion、non-fast-forward

## 拒否系の確認

ローカルhookを迂回して`main`へのforce pushや削除を実試行してはいけません。
Rulesetのreadbackと`gh ruleset check`でremote設定を確認し、CI失敗や未解決conversationは
使い捨てPRのmergeabilityで確認します。誤設定時に危険な更新が成功し得る試験は行いません。

## ロールバック

Rulesetを削除せず、同じ数値IDを`enforcement=disabled`へ更新します。実行前に現在の
readbackを保存し、Daikiの明示確認を得ます。disabled版のJSONは正本を`/tmp`へコピーして
`enforcement`だけを変更し、差分を確認してから使用します。

```sh
gh api repos/Daiki48/dotfiles/rulesets/<RULESET_ID> \
  --method PUT \
  --input /tmp/main-ruleset-disabled.json
```

復旧後もRulesetを無条件に削除したり、bypass actorを追加したりしません。workflow変更は
通常のrevert PRで戻し、required checkを外す必要がある障害では原因と復旧時刻を記録します。

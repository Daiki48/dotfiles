# GitHub CI・Ruleset運用ガイド

このrepositoryでは、ローカルhookを誤操作の早期検出、GitHub ActionsとRulesetを
remote `main` の最終安全境界として扱います。hookは通常のIssue・PR管理を妨げず、
protected branchへの直接push、force push、削除、秘密情報、任意API mutationを拒否します。

## Required CI

`.github/workflows/ci.yml`のjob表示名`required-ci`を唯一のrequired checkとして使用します。
pull requestと`main`へのpushで、次を同じjob内で実行します。

| 検証 | ローカルとCIのcommand |
|---|---|
| hook・helper・Ruleset | `cargo test --workspace --locked` |
| Rust format | `cargo fmt --all -- --check` |
| Rust lint | `cargo clippy --workspace --all-targets --locked -- -D warnings` |

workflowは`contents: read`だけを要求し、credentialをcheckout後に保持しません。
`pull_request_target`は使用せず、forkや未信頼branchのコードへsecretを渡しません。
path filterは設定しないため、required checkがskipされてpendingのまま残る経路を作りません。

## hookと明示確認の境界

通常のIssue・PR lifecycleは、hookがcurrent repository、単一対象、正規形の引数、送信内容を
検査した上で自律実行します。force pushや削除は、検査を外して直接許可せずhard denyを維持します。

読み取り操作も、外部command実行、別repository参照、file書き込みへ変わらない正規形だけを
許可します。代表的な許可形は次のとおりです。

```sh
git worktree list --porcelain
git worktree list --porcelain -z
git ls-remote --branches origin refs/heads/<作業branch>
git -C <同一repositoryの検証済みmanaged worktree> status --short
codex-delivery record-review --help
```

`git ls-remote --heads`はGitの後方互換aliasとして受け付けますが、新しい利用では
`--branches`を使用します。任意URL、追加ref、glob、`--upload-pack`、`--server-option`、
別repositoryを指す`git -C`、その他のGit global optionは許可しません。Git、GitHub CLI、
`codex-worktree`、`codex-delivery`のshell redirectionも、読み取りに見せかけたfile書き込みを
避けるため拒否します。

GitHub Actionsのrun取消は、次の正規形だけを例外的なwrite allowlistとします。

```sh
gh run cancel <正の数値run ID> --repo <current originのowner/repository>
```

hookは実行前に固定したread-only REST GETで同じrepositoryとrun IDをread-backし、run ID、
repository、cancel URL、status、conclusionが一致することを確認します。公式にcancel対象として
案内されている`queued`または
`in_progress`だけを許可し、完了済み、別repository、取得不能、競合状態ではfail closedにします。
`GH_CONFIG_DIR`、proxy・CA override、`http_unix_socket`による接続先変更も拒否し、probe出力は
timeoutとサイズ上限を適用しながら読み取ります。
read-back後にrunが完了するraceではcancel側が失敗するだけで、別runへ対象を切り替えません。
`--force`、追加option、runのdelete・rerun、その他のGitHub writeはこの例外へ含めません。

GitHub GraphQLは`query`でもfield指定によりPOSTになるため、直接の
`gh api graphql -f/-F ...`を読み取りとして一括許可しません。固定queryと対象検証を持つ
専用helperだけを信頼経路にします。helper名が検索commandや`command -v`の引数に現れるだけでは
実行と扱いませんが、wrapperやPython interpreterのscript operandからhelperを起動する形は
引き続き拒否します。任意program内部から生成されるcommandまでは解析しないため、sandboxやrulesとの
多層防御を維持します。

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
review済みhead SHAの固定とrisk-based reviewは`codex-delivery`による完了フローで担保します。
この構成だけでは、PR作者が`required-ci` workflow自体を変更する攻撃をGitHub上で完全には
防げません。#24ではworkflow差分を含む固定head SHAを独立reviewし、check名だけでなく実行内容も
確認します。独立したreview identityを用意できる場合は、CODEOWNERSとrequired approvalを再検討します。

## GitHub Free/private profile

Rulesetを利用できないGitHub Free/private repositoryは、既定のstrict gateを暗黙に緩和しません。
`--gate-mode github-free-private`を明示したcurrent private repositoryだけが低保証profileを使用できます。
receipt v6へmode、risk、decision、riskに応じたreview、Plan版を保存します。新規receiptはPR commentへ内部監査用JSONを投稿しません。
既存v5 receiptは進行中taskの再開時だけ旧ledger comment chainを読み取り専用で検証します。既存v1〜v4 receiptは履歴の読み取り互換に限定し、delivery時はcurrent headのv6再reviewを要求します。

Free/private profileでも固定headで実際に起動したGitHub Actions check、App ID `15368`、GitHubが成功と扱う
`success`・`skipped`・`neutral`、PR identity、最新mainのancestor、review thread、mergeabilityを検証します。
Rulesetの代替としてprivate/default branch/archive/disable/merge/auto-merge設定をlive readbackし、
設定driftや取得不能を拒否します。Ruleset APIの403、404、timeoutはfallback条件ではありません。

hosted/self-hosted CIを意図的に運用しない場合は、Daikiが残存リスクを明示承認したときだけ
`--gate-mode github-free-private-local`を選べます。これはhigh/criticalの`approve-review`専用で、
固定SHAのlocal test・従来必須の独立review・専門review、receipt、上記のrepository/PR/review検証を維持し、
PRのbaseと固定headの双方にworkflow YAMLがない場合だけ`required-ci` check runを要求しません。workflow YAMLがあれば
local modeは選択せず、Rulesetの有無に応じてstrictまたは`github-free-private` modeを使い、YAMLの
`runs-on`に従って通常CIを実行します。CI failureやpendingから自動fallbackしません。

PRのbaseと固定headの双方にworkflow YAMLが存在しない通常のrepositoryでは`--gate-mode local-validation`を選び、公開・非公開を
問わずproduct固有のformat、lint、型検査、test、buildを固定headで実行します。CI不在だけを理由に
human approvalやrisk引き上げを要求しません。workflowが存在するbase/headではこのmodeを拒否し、固定headにGitHub Actions checkが存在する場合は全件の完了と成功系conclusionを要求します。
GitHub-hosted/self-hostedの選択は各jobの`runs-on`をそのまま尊重します。

通常の`github-free-private` profileではGitHubサーバーが直接push、helper外merge、force push、branch削除を拒否しません。
そのため実装内容にかかわらずdelivery riskをhigh/criticalへ引き上げます。ただしdecision requirementは
riskと分離し、根拠を確定できる場合は`record-review`、Daikiだけが決められる事項がある場合だけ
`approve-review`を使います。Rulesetを利用可能になった場合はstrict gateへ戻せます。

## Delivery gate（Issue #24）

Draft PR作成後は、PRのrepository、base branch、head branch、head SHAを固定し、固定SHAに対する
testとriskに応じたreview（low/mediumはSolのself-review、highは独立review、criticalは必要時の専門review）の完了証拠をreceiptへ記録し、
CIとGitHub review状態はdelivery直前にも再取得します。
review後にpushされた場合、以前のreceipt、review、CIを
再利用せず、新しいSHAで最初からやり直します。`review-branch`は読み取り専用であり、receiptの記録と
delivery判断は呼び出し元の専用`codex-delivery` helperが担当します。

修正可能なactionable指摘はSolが根拠を確認し、共通root causeごとの1つのbatchで自律修正します。同じ問題が修正後も再発するか、2回続けて受け入れ条件・test・既知指摘に証拠上の進展がない場合は、root cause、実装境界、検証手段を再確認します。その後も同じ問題が続く場合だけblockedとし、無変更retryを続けません。

PRやIssueへ内部監査用のschema JSON、fingerprint、digest chain、round logを投稿しません。PR bodyとcommentは、人間が読む変更概要、判断が必要な論点、検証結果、残存事項に限ります。作業はPlanの有限な受け入れ条件・実装単位・対象経路から目的外へ増殖させません。`codex-delivery`はv6 receiptをprivateなmanaged stateへ保存し、固定SHA、test、review、Plan、decision、gateを検証します。v6のdeliveryはPR comments APIに依存しません。既存v5 receiptだけは進行中taskの再開時に旧ledger comment chainを読み取り専用で検証します。
次の条件が同一SHAで同時に成立した場合だけReady化・merge候補になります。

- workflowで起動したGitHub Actions checkがすべて`success`、`skipped`、`neutral`である
  （cancelled、timed out、failure、pending、判定不能は成功と扱わない）
- actionableな指摘が0件、GitHub review conversationの未解決件数が0件
- PRがopen、baseがdefault branch、headがreceiptのSHAと一致し、merge conflictがない
- branchが最新baseを満たし、strict modeでは実行時点のlive Ruleset gateがrequired CI、PR必須、
  conversation解決、merge-only、force push/branch deletion禁止などの正本と一致する
- 明示したFree/private profileではhigh/criticalのdecision receiptとlive repository identityが一致する

riskはreview深度を決め、decision requirementとは分離します。仕様、既存権限、rollback、検証を確定できる
場合はhigh/criticalでも`record-review`で自律deliveryします。製品判断、scope拡大、追加権限、費用、
不可逆性、重大な残存リスク受容などDaikiだけが決められる場合は、回答後だけ`approve-review`を使います。
技術gateの失敗や不明状態はapprovalで迂回しません。

Ready化、merge、main同期、cleanupは`codex-delivery deliver`と`finish`だけが行う経路です。
直接の`gh pr merge`、直接のReady化、`git worktree remove/prune`や任意branch削除でこのgateを
迂回しません。merge後は人間用checkoutをmain・cleanに確認し、fetch後の`git merge --ff-only origin/main`だけで同期して
local main=`origin/main`を検証します（`ff-only`はlocal同期の条件であり、Rulesetのmerge methodを
変更するものではありません）。

`finish`はmerge済み、head commitのmain到達性、対象worktreeがmanaged root内、clean、未pushなし、
別taskと競合しないことを証明できた場合だけmanaged cleanupを許可します。失敗、timeout、pending、
dirty、stale、conflict、判定不能時はPR、branch、worktreeを保持します。任意削除はDaikiの確認を得て
`.codex-trash/<timestamp>/`へ退避する従来の規則に従います。

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

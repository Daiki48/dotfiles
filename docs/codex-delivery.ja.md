# Codex delivery運用ガイド

このガイドは、Draft PR作成後の完了条件を定義します。Draft PRは中間点であり、完了では
ありません。レビュー済みの正確なcommitだけをReady化・mergeし、merge後のmain同期と
managed worktree cleanupまでを同じdeliveryとして扱います。

## 正本と経路

### main同期が途中で止まった場合

`finish`は、同期先blob・実行bitと一致する変更済みfileを検証してから同期前のHEADへ戻します。
同期先で新規追加された未追跡fileも、HEADに不存在・通常file・同一UID・hardlinkなし・
symlink parentなし・内容一致をすべて確認できる場合だけ、`.codex-trash/<日時>-main-sync/`へ
退避してからff-only同期します。退避は削除せず、pathを標準エラーへ表示します。
全候補を検証する前に元fileを変更せず、操作直前にもbranch・HEAD・status・inode・mode・blobを再確認します。
未知の未追跡file、独自のlocal変更、staged変更、削除、rename、type変更は復旧しません。

退避にはHEADの`.gitignore`への`.codex-trash/`登録と実効ignoreが必要です。
Dockerfileまたは`.dockerignore`がある場合は、HEADの`.dockerignore`にも同じ登録が必要です。
否定patternがあるDocker除外設定は自動判定せず停止します。

通常の`--sandbox-retry`は従来どおり1回だけです。その再試行を使い切って停止したtaskは、
Daikiが**今回の追加復旧を含む同一taskの完了処理**を明示承認した場合だけ、
同じtask・PR・head・planの`approve-review`で承認を記録し、次を1回だけ実行できます。

```text
codex-delivery finish --task-id <task> --pr <pr> --head <head> --plan-id <plan-v1> --plan-version 1 --recover-main-sync --recover-main-sync-from <同期前mainの40桁SHA> --recover-main-sync-to <同期先origin/mainの40桁SHA>
```

追加復旧もcanonical helperの経路に限定します。stage=merged、sandbox再試行済み、
human-approved receipt、現在HEAD=from、fetch後origin/main=to、既存のCI・review・到達性gateを要求します。
外部検証前に追加復旧の消費を永続化し、失敗しても自動再試行しません。`--sandbox-retry`との併用、
stateの手編集、直接のmain同期やcleanupによる迂回は禁止です。

`codex-delivery` helperを、次の一連の唯一のreceipt・delivery・finish経路とします。

```text
(autonomous: record-review | human-required: approve-review) -> deliver -> finish
```

すべてのcommandはcurrent repositoryのrootで実行し、`--task-id`、`--pr`、`--head`、
`--plan-id`と`--plan-version`を必須とします。review記録では`--risk`と`--tests-passed`を必須とし、
high/criticalでは`--independent-review-passed`、criticalで別の高リスク境界を専門reviewした場合は
`--specialist-review-passed`も指定します。Codexや利用者が直接
`gh pr ready`、`gh pr merge`、任意のGitHub
merge API、`git worktree remove/prune`、任意branch削除を実行してこの経路を迂回してはいけません。

```sh
codex-delivery record-review --task-id <task-id> --pr <PR番号> --head <40桁SHA> --risk <low|medium> --plan-id <Plan ID> --plan-version <Plan版> --tests-passed
codex-delivery approve-review --task-id <task-id> --pr <PR番号> --head <40桁SHA> --risk <low|medium> --plan-id <Plan ID> --plan-version <Plan版> --tests-passed
codex-delivery record-review --task-id <task-id> --pr <PR番号> --head <40桁SHA> --risk <high> --plan-id <Plan ID> --plan-version <Plan版> --tests-passed --independent-review-passed
codex-delivery record-review --task-id <task-id> --pr <PR番号> --head <40桁SHA> --risk <critical> --plan-id <Plan ID> --plan-version <Plan版> --tests-passed --independent-review-passed --specialist-review-passed
codex-delivery deliver --task-id <task-id> --pr <PR番号> --head <40桁SHA> --plan-id <Plan ID> --plan-version <Plan版>
codex-delivery finish --task-id <task-id> --pr <PR番号> --head <40桁SHA> --plan-id <Plan ID> --plan-version <Plan版>
```

既定は`strict-ruleset` modeです。GitHub Free/private repositoryでは、次の低保証profileを
明示できます。API errorからこのmodeへ自動fallbackしません。

```sh
codex-delivery record-review --task-id <task-id> --pr <PR番号> --head <40桁SHA> --risk <high|critical> --plan-id <Plan ID> --plan-version <Plan版> --gate-mode github-free-private --tests-passed --independent-review-passed --specialist-review-passed
codex-delivery approve-review --task-id <task-id> --pr <PR番号> --head <40桁SHA> --risk <high|critical> --plan-id <Plan ID> --plan-version <Plan版> --gate-mode github-free-private --tests-passed --independent-review-passed --specialist-review-passed
codex-delivery deliver --task-id <task-id> --pr <PR番号> --head <40桁SHA> --plan-id <Plan ID> --plan-version <Plan版> --gate-mode github-free-private
codex-delivery finish --task-id <task-id> --pr <PR番号> --head <40桁SHA> --plan-id <Plan ID> --plan-version <Plan版> --gate-mode github-free-private
```

hosted/self-hosted CIを意図的に使わないGitHub Free/private repositoryでは、Daikiが残存リスクを
明示承認し、live baseと固定headのworkflowが両方とも不在、またはすべてタグpushのみの場合に限り、次のlocal-only profileを
使用できます。PR・branch pushなどのCIを含むworkflowがあれば通常CIを使います。
`record-review`では記録できません。

```sh
codex-delivery approve-review --task-id <task-id> --pr <PR番号> --head <40桁SHA> --risk <high|critical> --plan-id <Plan ID> --plan-version <Plan版> --gate-mode github-free-private-local --tests-passed --independent-review-passed --specialist-review-passed
codex-delivery deliver --task-id <task-id> --pr <PR番号> --head <40桁SHA> --plan-id <Plan ID> --plan-version <Plan版> --gate-mode github-free-private-local
codex-delivery finish --task-id <task-id> --pr <PR番号> --head <40桁SHA> --plan-id <Plan ID> --plan-version <Plan版> --gate-mode github-free-private-local
```

公開・非公開を問わずPRのlive baseと固定headの双方にworkflow YAMLが存在しないrepositoryでは、変更riskに応じたreview evidenceと
product固有のlocal検証を使って`local-validation`を自律的に選択できます。CI不在だけを理由に
`approve-review`へ切り替えません。

```sh
codex-delivery record-review --task-id <task-id> --pr <PR番号> --head <40桁SHA> --risk <low|medium> --plan-id <Plan ID> --plan-version <Plan版> --gate-mode local-validation --tests-passed
codex-delivery deliver --task-id <task-id> --pr <PR番号> --head <40桁SHA> --plan-id <Plan ID> --plan-version <Plan版> --gate-mode local-validation
codex-delivery finish --task-id <task-id> --pr <PR番号> --head <40桁SHA> --plan-id <Plan ID> --plan-version <Plan版> --gate-mode local-validation
```

`review-branch`は読み取り専用です。reviewerはreceipt、Issue、PR、Git、worktreeを変更せず、
呼び出し元がdecision assessmentに応じてreview結果を`record-review`または`approve-review`へ渡します。

helperは固定した`~/.local/bin/codex-delivery`のprivate regular fileから直接起動し、
`/usr/bin/git`と`/usr/bin/gh`だけを使います。
`PATH`、`GH_HOST`、`GH_REPO`、`GH_CONFIG_DIR`、Git環境変数による対象・実行fileの差し替えを
拒否または無効化し、1回の`deliver`/`finish`全体を5分でfail closedにします。この境界は
同一userが任意programを実行してhelperやmanaged stateそのものを書き換える攻撃を防ぐsandboxでは
ありません。その権限を持つ主体と、repository設定やPRを同時変更できる管理者は信頼境界内です。
したがって`approve-review` receipt自体はDaiki確認の暗号学的証明ではなく、会話上の確認を守る
Codexのinstructionとtool promptに対する構造証拠です。同一userの任意programによる子process起動や
state直接改変を防ぐものとは説明しません。

## DELIVERY-01: 固定SHAのreview receipt

Draft PRを作成したら、receiptとmanaged manifestを合わせて次の対象を固定します。

- repository、PR番号、base branch/ref、head branch/ref
- review対象のhead SHAとbase SHA
- task ID、worktree path、risk分類
- remote gate mode
- riskに応じたreview（low/mediumはmain agentのself-review、highは独立review、criticalは必要時の専門review）、testの完了判定、actionable件数

receiptは対象SHAに束縛します。review後にcommitをpushしてhead SHAが変わった場合、以前の
receipt、CI、review、確認を新SHAへ引き継ぎません。新しいSHAでCIとrisk上必要なreviewを実行し、
新しいreceiptを記録します。

receipt v6は`independent_review_passed`をlow/mediumでは任意、high/criticalではtrueに固定し、
`specialist_review_passed`をcriticalで別境界をreviewした場合だけ必須にします。既存receiptの追加review証拠も有効です。`gate_mode`とdecision
（`autonomous`または`human-approved`）、Plan版も固定し、riskから意思決定要否を推測しません。新規receiptはPR commentへ内部監査用のJSONを投稿せず、privateなmanaged stateへ保存します。
既存v5 receiptは進行中taskを再開するため従来のledger comment chainを読み取り専用で検証します。既存v1〜v4 receiptは履歴表示の読み取り互換形式として解析できますが、delivery・finishには使えずcurrent headをv6で再reviewします。旧形式のhigh/criticalやFree/privateを遡及的にautonomousへ緩和しません。CLIで
指定したmodeとreceiptが一致しない場合はdeliveryもfinishも停止します。

receiptのreview/test flagは、定めた手順を完了したことをmachine-readableに束縛する構造証拠であり、
review品質を暗号学的に証明するものではありません。helperはdelivery直前にbase SHAと固定head SHAの
実差分を再計算し、receiptのchanged-filesと順序も含めて完全一致させます。安全境界pathを含む差分は
low/mediumへ分類できません。

remote CIではGitHubがrequired checkの成功状態として扱う`success`、`skipped`、`neutral`を成功とします。
`cancelled`、`timed out`、`failure`、`pending`、取得不能、判定不能は成功として扱いません。CI結果はreceiptの自己申告を
信頼せず、`deliver`が固定head上のGitHub Actions check runをlive取得し、job名を固定せずapp ID、head SHA、
`completed`、conclusion、完了時刻を確認します。

## DELIVERY-05: riskとdecision assessment

### risk

通常の実装・修正で、delivery安全境界や高リスクデータ・権限に影響しないタスクをlow/mediumと
します。CI/workflow、hook、rules、AGENTS、Skills、helper、installer、auth/secrets、production、
不可逆migration、breaking change、重大なsecurity・互換性・データ損失影響はhigh/criticalです。
riskはreview深度と残存影響を決めますが、人間確認を自動決定しません。low/mediumはmain agentのself-review、
highは独立reviewを1つ実行し、criticalは別の高リスク境界が実在する場合だけ専門reviewを1つ追加します。
一般的な反論役や肯定役は使いません。actionableな指摘は今回修正すべき具体的な欠陥に限り、fileまたは実行経路、期待結果と実際の結果、再現・確認方法、修正後の観測条件を明らかにします。将来改善、好み、具体的な影響根拠のない懸念は含めません。

共通root causeの指摘は1つのbatchで修正し、可能なら回帰testを追加します。修正でSHAが変わった場合は新SHAでreview、CI、receiptをやり直します。同じ問題が修正後も再発するか、2回続けて受け入れ条件・test・既知指摘に証拠上の進展がない場合は、main agentがroot cause、実装境界、検証手段を再確認します。その後も同じ問題が続く場合だけblockedとし、無変更retryを続けません。

PRやIssueへ内部監査用のschema JSON、fingerprint、digest chain、round logを投稿しません。PR bodyとcommentは、人間が読む変更概要、判断が必要な論点、検証結果、残存事項に限ります。作業範囲は依頼の目的、観測可能な受け入れ条件、変更対象経路、必須検証で区切り、その集合外の改善を自律loopへ追加しません。

`codex-delivery record-review`はv6 receiptをprivateなmanaged stateへ記録し、固定SHA、変更file、test、riskに応じたreview、Plan、decision、gate modeを固定します。v6 receiptは機械監査commentに依存しません。Issueを記録先にしたtaskの`finish`だけは、後述の人間向け作業記録をIssue comments APIで保存・確認します。既存v5 receiptだけは進行中taskを安全に再開するため、記録済みの旧ledger comment chainを読み取り専用で再検証します。新しいledger commentは作成しません。

新規Codex sessionでは[公式Configuration Reference](https://developers.openai.com/codex/config-reference)でunder-developmentかつ既定無効のrollout budget trackingを無効のまま使います。失敗時は有限なPlan scopeと観測可能な進展で制御し、同じ問題の無制限retryや目的外の監査作業へ広げません。

次の条件が同じhead SHAで成立した場合だけ、`codex-delivery deliver`へ進みます。

1. PRがopenで、baseがrepositoryのdefault branchである。
2. 現在のhead SHAがreceiptのreview済みSHAと一致する。
3. workflowがある場合は固定headのGitHub Actions checkがすべて`success`、`skipped`、`neutral`で、
   failure、cancel、timeout、pending、取得不能がない。workflow不在の`local-validation`では固定SHAのlocal検証が成功し、固定headにGitHub Actions checkが存在する場合は同じ成功条件を満たしている。
4. actionableな指摘が0件で、GitHub review conversationの全pageに未解決threadがなく、reviewerごとの
   現在有効な個別reviewに`CHANGES_REQUESTED`がなく、`reviewDecision`も`CHANGES_REQUESTED`、
   `REVIEW_REQUIRED`、不明値ではない。
5. merge conflictがなく、branchが最新baseの要件を満たす。
6. 選択したremote gateが実行時点でも成立する。strict modeではlive Rulesetがrequired CI、PR経由、
   conversation解決、merge method、force push/branch deletion禁止などのrepository正本と一致する。
   Free/private modeでは後述するlive repository identityと設定が一致する。

### decision assessment

- `autonomous`: 依頼済みscope内で仕様、既存権限、rollback、test・CI・reviewを根拠付きで確定できる。
  全riskで`record-review`を使う。
- `human-required`: 製品・仕様の実質判断、scope拡大、新規credentialや権限付与、repository設定、
  billing・購入、不可逆性、重大な残存リスク受容、releaseなどDaikiだけが決められる事項がある。
  明示回答後だけ`approve-review`を使う。
- `blocked`: test/CI/review失敗、dirty/stale/conflict、secret混入、identity不一致、network/API不明、
  rollback未評価、仕様矛盾など必須証拠が不足する。approvalで迂回せず、receiptを作らない。

優先順位は`blocked > human-required > autonomous`です。同一headのdecisionは
`autonomous -> human-approved`への単調な更新だけを許し、risk downgradeや承認の取消で既存receiptを
弱めません。

### GitHub Free/private

GitHub Freeのprivate repositoryではRulesetやprotected branchをserver-sideで利用できません。
この制約をAPI errorから自動推測してfallbackせず、`--gate-mode github-free-private`を明示した場合だけ
低保証profileを使用します。

このmodeではserver-side強制がない保証差を反映してriskをhigh/criticalとして扱います。ただし
decision assessmentが`autonomous`なら`record-review`、Daikiの判断が必要な場合だけ`approve-review`を
使います。live repository readbackはcurrent repositoryと完全一致するrepository identity、
`private=true`、`default_branch=main`、`archived=false`、`disabled=false`、
`allow_merge_commit=true`、`allow_auto_merge=false`を要求します。さらに固定headで実際に起動したGitHub Actions App
由来checkの完了、最新mainのancestor、全review thread解決、`CHANGES_REQUESTED`なし、
same-repository PR、固定head、mergeable/CLEANをstrict modeと同じく検証します。

GitHub側ではmainへの直接push、helper外merge、force push、branch削除、最新base CIを強制できません。
ローカルhook、decision receipt、live readback、`--match-head-commit`で低減しますが、Rulesetと
同等の保証とは説明しません。identity不一致、public化、repository設定drift、mode省略・不一致、
API取得不能ではfail closedにします。

### GitHub Free/private local-only

`github-free-private-local`は`github-free-private`と同じlive repository identity、固定head、最新mainの
ancestor、mergeability、review thread、`CHANGES_REQUESTED`、receiptを検証しますが、唯一
GitHub Actions checkの存在を要求しません。代わりに固定SHAで完了したlocal test・従来必須の独立review・専門reviewを
receiptへ固定し、後述のworkflow条件、high/critical、`human-approved`を必須にします。同一headの既存
`github-free-private` receiptは、同じPlan・evidenceのまま明示承認されたこのmodeへだけ更新できます。

このmodeの`tests-passed`はlocal実行結果の構造的な申告であり、GitHubが実行・強制するCI証明では
ありません。CI失敗・pendingを迂回するfallbackとしては使わず、CI自体を運用しない方針への明示承認が
ある場合だけ選択します。GitHub側で直接push等を拒否できない残存リスクも引き続き存在します。

live baseと固定headの両方で全workflowを調べます。workflowが存在する場合は、YAMLとして解析した
`on`が`push`のみ、そのfilterが非空の文字列配列`tags`のみであることを要求します。これは
[GitHubのタグfilter仕様](https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax#onpushbranchestagsbranches-ignoretags-ignore)に従い、PRやbranch pushでは起動しない構成を明示承認のlocal検証で扱うための限定です。
PR・manual・reusable・schedule等の追加trigger、branches/paths等の追加filter、不明な形式、
重複key、複数document、anchor/alias/tag/merge keyは拒否します。通常のremote gateとworkflow不在の
`local-validation`の条件は変更しません。実際のActions checkが存在すれば全件の成功系conclusionを必須とし、pendingや失敗を迂回しません。

### workflow不在のlocal validation

PRのlive baseと固定headの双方に`.github/workflows/*.yml|*.yaml`がない場合は`--gate-mode local-validation`を使います。
公開・非公開を問わず選択でき、CI不在だけを理由にhuman approvalやrisk引き上げを要求しません。
README、CONTRIBUTING、package scripts、build manifestからformat、lint、型検査、test、buildのうち
変更に該当するcommandを実行し、固定headの`tests-passed` evidenceへ記録します。

workflow YAMLがbaseまたはheadに1つでも存在する場合はこのmodeを拒否します。固定headにGitHub Actions checkが存在する場合も全件の完了と成功系conclusionを要求し、CI failure、pending、runner unavailableを
local検証で迂回せず、workflowのtrigger、`runs-on`、job、matrixとlive repository ruleを正本にします。
mergeやpushで自動起動する既存CDはそのworkflowへ任せて状態を報告します。manual dispatch、release、
production deploy、新しいenvironment approvalはdelivery helperのscopeに含めません。

### CI予約と納品予約

既存のCI起動labelの選択はCodexへ継続委任されています。workflowと用途を読み、review receiptを作成してから
親checkoutで次を実行します。新labelやdeploy承認を作成するcommandではありません。

```console
codex-delivery authorize-ci --task-id <TASK_ID> --pr <PR> --head <HEAD_SHA> --base <LATEST_MAIN_SHA> --ci-label <EXISTING_CI_LABEL>
```

PR identity、cleanな固定head、review、最新mainの包含、labelの実在を照合してから付与します。
同じlabelが他のopen PRにある場合は待ちます。coordinatorの`--ci-label`も同じ経路を使います。
共有CI枠は`worktrees/.delivery-lane/state.json`、repository単位の納品予約はその配下の
`repository-<repo_key>/state.json`へprivateに保存します。通常CIを起動するtaskは原則として両方を取得し、
CI起動・merge・main CI・main同期・記録・cleanupを終えた`finish`だけが共有枠を解放します。
head/base更新、gate変更、セッション終了、process停止、待機時間では枠を解放しません。
中断時は所有taskを再開します。CIなしで進めるtaskもrepository単位の納品予約を保持するため、
同一repositoryのmerge・main同期・cleanupは競合しません。

### Self-hosted CI枠の容量設定

必要なrunner容量を実測した場合に限り、privateな
`$CODEX_HOME/worktrees/.delivery-lane/capacity.json`で共有枠を最大3 jobまで設定できます。
設定例は合成値です。`workflow_tree`は固定SHAに対する
`git rev-parse <SHA>:.github/workflows`の結果を記録します。

```json
{
  "version": 1,
  "max_jobs": 3,
  "disk_path": "/srv/runner-work",
  "cpu_per_job": 2,
  "memory_mib_per_job": 4096,
  "disk_mib_per_job": 8192,
  "reserve_cpus": 1,
  "reserve_memory_mib": 2048,
  "reserve_disk_mib": 16384,
  "repositories": [
    {"repository": "synthetic-org/build-a", "workflow_tree": "1111111111111111111111111111111111111111", "jobs": 2},
    {"repository": "synthetic-org/build-b", "workflow_tree": "2222222222222222222222222222222222222222", "jobs": 1}
  ]
}
```

`capacity.json`は同一ユーザー所有の通常file（0600）にします。`disk_path`はrunnerの実ストレージを
指す絶対canonical pathにします。登録するjob数には、同じ予約中にPR CI、main CI、再実行が
重なる場合の最大同時実行数を使います。workflow変更後は容量設定を編集する前に
head/base双方の最大同時実行数を再検証します。開始時の固定headとbaseのworkflow treeが
登録値に一致する場合だけ重み付き予約を使います。未登録、tree不一致、構成不明は従来の
排他予約です。既存のv1 stateも排他として扱います。

判定は使用可能CPU数とload average、利用可能RAM、`disk_path`上の`statvfs`を参照し、
稼働中予約の合計と設定reserveを保守的に差し引きます。容量不足なら新規開始を待たせますが、
既存予約の`finish`は妨げません。同一repositoryの納品排他は維持します。修復taskは他の
重み付き予約が解放されるまで待ち、その後に排他枠を取得します。ホスト計測は外部triggerを
制御しないため、実際のrunner設定とhard limitを別途合わせてください。計測なしに有効化・増枠しません。

初回は既存のmanaged refreshでhelperを更新し、新規Codex sessionから読み取り専用の
`codex-delivery ci-capacity`で確認します。容量設定は受付ごとに読み直します。
重み付きstateを読めない旧helperはfail closedし、
設定なし・空stateの予約は従来のv1動作です。binaryを戻す前にcanonicalな`finish`で予約をdrainします。

### GitHub-hostedの通常CIで共有枠を省く条件

明示的な`--ci-label`で起動する場合は、通常CIでも従来どおり共有枠を取得しfinishまで保持します。
labelを明示しない通常CIは、live base・固定head・finish時の実mergeに含まれる全workflow jobが
[公式標準GitHub-hosted runner](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)の固定scalar `runs-on`
（例: `ubuntu-24.04`）で、job-level reusable workflow `uses`を含まず、live repository APIでownerが個人（`User`）かつ
repository self-hosted runner一覧が明示的に空と確認できた場合に限り、共有枠を省略できます。
これはGitHub-hosted runner利用が確認できる場合に運用上の共有待ちだけを除く条件です。CI check、レビュー、同一repositoryの納品予約は従来どおり必要です。
`runs-on`のmatrix展開・式・`group`・複数labelなど固定scalarでない指定、標準hostedと確認できないlabel、job-level `uses`、
self-hosted runner、organization/enterpriseなどからのrunner継承が不明なrepository、workflow treeやAPI応答が欠落・不明の場合は
共有枠を取得します。runner labelの対象はGitHubの公式runner一覧を正本とし、独自labelを推測でhosted扱いしません。
owner種別とrunner一覧は毎回live APIで照合します。owner情報・runner一覧が取得できない、runner APIが明示的な空一覧を返さない場合も枠を省略しません。
判定はlive base、固定head、merge後の実merge revisionごとに行います。finishで実merge等の証拠が条件外なら通常の共有枠を取得し（必要なら待機）、CI gate・所有チェックを維持します。
省略可能なのは共有枠だけで、repository単位の納品予約は必ず維持します。既存の共有枠ownerはfinishまで保持し、
この条件に該当する別repositoryのtaskが既存ownerを変更・解放することはありません。

live baseと固定headに適用されるworkflow policyを検証した`local-validation`は共有CI枠なしで進めます。
`emergency-ci`はgate名だけで枠を免除しません。有効なreceiptを照合した後、最新API観測でheadとmerge targetの
CI状態がpending/queued/in_progress等でないことを確認できた場合に限り、共有枠を使わず納品予約だけ取得します。
API不明、未知状態、対象不一致、期限切れ・scope不一致の証拠は従来の予約要件に従うかfail closedします。
通常CI ownerが既にあるtaskはgate変更後もfinishまで所有を維持し、CI不要taskは別repositoryのCI ownerを
変更・解放しません。旧共有stateはCI ownerとして読み、移行時に稼働中のownerを削除しません。
直接のlabel操作で迂回しません。検証済みのmain CI修復taskだけは親の枠を一時的に使い、finish後に親へ戻します。

自動triggerによるCIの並列実行、他hostや人間の操作を共有枠で排他できるわけではありません。
GitHubのlabel APIもPR単位なので、照合と付与の間のpushは取り消せません。直列化は事故ゼロの保証ではなく、
merge前の最新base・head・conflict・check・reviewの再確認とサーバー側保護を維持します。

### CI基盤障害時の継続委任

`--gate-mode emergency-ci`はrepositoryを限定しない例外経路です。通常のstrictを既定に保ち、CI失敗やpendingだけで
免除しません。CI基盤障害の判断はCodexへ継続委任され、対象ごとの人間承認は不要です。
repository/task/PR/head/live base/Planと版、理由、免除対象と状態は各deliveryの証拠へ固定します。
workflowが存在しても利用できます。最低riskはhighで、固定headのlocal test/build、変更に該当するformat/lint/型検査、
独立review、actionable=0、未解決thread=0を必須にし、`record-review`で作成します。

基盤障害を製品testの失敗と区別し、原因調査・ローカル検証・reviewを終えてから根拠を記録します。
既知のtest失敗は対象外です。原因をログから自動証明する機能ではなく、継続委任の根拠、原因区分と根拠、
成功した検証command、独立reviewを構造化して保存します。CIの実測値は書き換えません。

requestはGit管理外の非公開領域へ、同一UID所有・0600の通常fileとして作成します。次は合成例です。
placeholderを検証した値へ置き換え、公開先へ貼り付けないでください。

```json
{
  "version": 1,
  "repository": "<OWNER>/<REPO>",
  "task_id": "<TASK_ID>",
  "pr": 7,
  "head_sha": "<HEAD_SHA>",
  "base_sha": "<BASE_SHA>",
  "plan_id": "<EXISTING_PLAN_ID>",
  "plan_version": 1,
  "reason": "今回CIを利用できない具体的な理由",
  "authorization": "AGENTSに記録されたCI基盤障害判断の継続委任",
  "local_validation": [
    {"kind": "test", "command": "<実行済みtest command>", "result": "passed"},
    {"kind": "build", "command": "<実行済みbuildまたは構成検証command>", "result": "passed"}
  ],
  "independent_review": "固定headの独立reviewの確認範囲と結果",
  "known_test_failures": false,
  "acknowledge_workflow_runs": true,
  "public_summary": "互換fieldの非公開検証メモ。公開本文は定型文だけ。",
  "waivers": [
    {"kind": "check-run", "name": "<CHECK_NAME>", "source": "app:<APP_ID>",
     "states": ["failure", "queued", "in_progress"], "cause": "admission-failure", "evidence": "基盤障害の根拠"},
    {"kind": "workflow", "name": ".github/workflows/ci.yml", "source": "actions:pull_request",
     "states": ["failure", "queued", "in_progress", "missing"], "cause": "service-outage", "evidence": "PR実行の障害根拠"},
    {"kind": "workflow", "name": ".github/workflows/ci.yml", "source": "actions:push",
     "states": ["failure", "queued", "in_progress", "missing"], "cause": "service-outage", "evidence": "同じdeliveryの実mergeにも適用する障害根拠"},
    {"kind": "workflow", "name": ".github/workflows/ci.yml", "source": "actions",
     "states": ["missing"], "cause": "runner-unavailable", "evidence": "実行が作成されない場合の根拠"}
  ]
}
```

`local_validation.kind`は`test`/`build`/`lint`/`format`/`typecheck`で、testとbuildを必須にします。
buildがないrepositoryでは構文検証等の実行可能性の検証を記録します。全resultは`passed`を要求します。
`cause`は`admission-failure`/`runner-unavailable`/`service-outage`だけです。
免除可能な`states`は`failure`/`error`/`startup_failure`/`cancelled`/`timed_out`/`queued`/`in_progress`/
`pending`/`requested`/`waiting`/`missing`です。`action_required`や未知状態は免除できません。
checkは`check-run`と`app:<ID>`、commit statusは`commit-status`と`user:<CREATOR_ID>`、
workflow runは`workflow`、workflow pathと`actions:<EVENT>`で識別します。全て完全一致でwildcardはありません。
承認base/head両方のworkflow定義から、headでは`push`/`pull_request`/`pull_request_target`、実mergeでは`push`の
候補をevent別に列挙します。baseにのみ存在するworkflowも対象です。branch/path/type filterは不在を免除する根拠にせず、
保守的に`actions:<EVENT>`・state=`missing`を観測します。push runでPR runの不在を隠しません。
解釈不能なYAML、重複key、anchor/alias/tag、複数documentは判定不能として拒否し、不在へ読み替えません。
該当する自動eventがないworkflowはsource=`actions`・state=`missing`として別に承認します。
workflowも観測もない場合だけ`ci`・`no-observations`・`github`・`missing`を使います。
別のcheck/提供元/状態は自動免除しません。check/status/run IDと実際の状態を保存し、生ログを取り込みません。

親checkoutから既存taskを照合し、それぞれ独立commandとして実行します。

```console
codex-worktree doctor --task-id <TASK_ID>
codex-worktree resume --task-id <TASK_ID>
codex-delivery record-review --task-id <TASK_ID> --pr <PR> --head <HEAD_SHA> --plan-id <EXISTING_PLAN_ID> --plan-version <VERSION> --risk high --tests-passed --independent-review-passed --gate-mode emergency-ci --emergency-request <PRIVATE_REQUEST_ABSOLUTE_PATH>
codex-delivery deliver --task-id <TASK_ID> --pr <PR> --head <HEAD_SHA> --plan-id <EXISTING_PLAN_ID> --plan-version <VERSION> --gate-mode emergency-ci
codex-delivery finish --task-id <TASK_ID> --pr <PR> --head <HEAD_SHA> --plan-id <EXISTING_PLAN_ID> --plan-version <VERSION> --gate-mode emergency-ci --issue <ISSUE>
```

既存receiptから移行する場合だけreview commandへ`--replace-receipt`を追加します。同じrepository/task/PR/head/
Planと版、変更file、検証範囲を保持し、riskを下げません。Plan版がない古いreceiptは移行できません。
ledger付き旧v5は移行時にも旧証拠を検証します。merge後の移行は拒否します。
同じrequestの再実行は同じreceiptを再利用します。理由/base/免除/証拠の変更は原因・local検証・reviewを再確認して
requestを更新し、`--replace-receipt`を明示します。head変更では新headの証拠を作ります。人間の再承認待ちは追加しません。
task lockと既存stageで二重実行を制限し、merge済みなら同じreceiptのfinishから再開します。
CI回復後に同じheadを通常CIへ戻す場合は、gate指定なしの`record-review --replace-receipt`を使えます。
移行元はemergency-ciに限り、同じ対象・Plan・検証範囲と既存review証拠を保持します。既存のhuman-approved decisionも保持します。
open PRと最新baseを照合し、merge後の移行は拒否します。移行後のdeliverでは通常のstrict gateをすべて要求します。
stateやinstalled helperの手編集は不要です。公開記録に所有不明の編集がある場合も保持して停止します。

既存coordinatorへ渡す場合は旧版coordinatorと旧writerの停止・回収を確認し、receipt作成後に新版で次を使います。
`start`の`already_running`は旧版の更新を保証しません。重複submitはしません。

```console
codex-task resume --task-id <TASK_ID> --instruction '既存PRと固定証拠のdeliver/finishを継続' --gate-mode emergency-ci
codex-task start
```

coordinatorはreceipt digestを束縛して再観測し、CI失敗の実測を保持してIntegration/MainCi/Finishへ進みます。
coordinatorはDiagnosisから専用CiEvidence段階へ進め、private requestとreceiptを作成できます。
証拠の失効時は実CIを通常gateで再取得し、head/base変更なら実装・検証・reviewへ戻します。API不明は通しません。
後続taskには古い証拠を継承せずstrictから始め、必要な場合だけ新しい固定証拠を作ります。
CI以外のidentity、最新baseのancestor、conflict、review、clean、所有、保存記録、ff-only同期とcleanupは維持します。
`UNSTABLE`は非成功CIが全て承認範囲に入る場合だけ許可し、`BLOCKED`/`UNKNOWN`/`DIRTY`/`BEHIND`等は拒否します。
このmodeはローカルstrict Ruleset契約の代わりに、通常のGitHub mergeでサーバー側の保護要件を適用します。
GitHubが拒否した場合は停止し、Ruleset変更、admin merge、保護branch直pushへ広げません。
[GitHubの状態定義](https://docs.github.com/en/graphql/reference/pulls#mergestatestatus)と
[保護checkの扱い](https://docs.github.com/en/pull-requests/how-tos/merge-and-close-pull-requests/troubleshooting-required-status-checks)が外部の正本です。

finishは実mergeが承認base/headを順番通りの親に持ち、treeがlocal検証済みheadと同じでmainへ到達したことを検証します。
CI免除はこの実mergeだけです。mainがさらに進んだ場合は最新SHAの通常Actions成功を別途要求します。
承認と初回観測はprivate receipt v7、最新実測は同じheadの`.emergency.json`へ分離保存します。

### merge後に判明した基盤障害の証拠更新

通常receiptでmergeした後の容量不足やrunner障害も、同じ`record-review`へ
`--replace-receipt --gate-mode emergency-ci --emergency-request <PRIVATE_REQUEST_ABSOLUTE_PATH>`を付けて更新できます。
request schemaは上記と共通です。`base_sha`は現在のmainではなく、実mergeの第1親です。
PR/head/Planと版、変更範囲、local test/build、独立reviewを固定し、既知の製品test失敗は免除しません。
merge済みPR identity、2親の順序、検証済みheadと同じtree、live mainへの到達、headと実merge双方のCIを再照合します。
旧emergencyの承認baseも変更できません。別のmain更新には通常CIが必要です。

対象stageは`merge_started`、`merged`、`main_synced`までです。cleanup開始後は更新せず保持します。
元receiptと旧観測を`.history-<digest>.json`へ、次の証拠を`.transition-<digest>.json`へ非公開保存してから
receiptをatomic更新します。同じrequestの再実行は既存receiptを再利用します。途中停止時も元履歴を消しません。
更新後は同じtask/PR/head/Planの`finish --gate-mode emergency-ci`を使います。state手編集や再mergeは不要です。
回復後の実CI成功はfinishで再観測し、免除範囲を拡張せず通常の同期・記録・cleanupへ戻ります。

### 別repositoryの基盤修復へ共有枠を一時移譲する

同一host/CODEX_HOMEで先行PRがmerge後の基盤障害に止まり、別repositoryの修復PRがその枠を待つ場合は、
修復repositoryの親checkoutから`codex-delivery repair-lane --request <PRIVATE_REQUEST_ABSOLUTE_PATH>`を使います。
先行PRは対象別emergency receiptと未回復の実CI観測、修復PRはhigh以上・独立review済みの固定receiptが必要です。
API不明や製品test失敗を修復の根拠へ読み替えません。修復PRの新規作成は元の依頼scopeに従います。

request v1は次のfieldを持つ0600のGit管理外JSONです。実値は公開しません。

- `version: 1`、`parent_checkout`、`reason`（今回の基盤修復との関係）
- `purpose: "repair-ci-infrastructure"`、`authorization`（元依頼に含まれる修復の根拠）
- `product_test_failures: false`、`parent_failure_evidence`（最新の失敗が基盤障害である根拠）
- `parent`: 非公開枠の固定`repository/task/pr/head/base/labels`のobject
- `parent_receipt_sha256`、`parent_merge`: 先行receiptと実mergeの固定値
- `child_task`、`child_pr`、`child_head`、`child_plan`、`child_plan_version`、`child_receipt_sha256`

このrequestが交差repositoryの固定親子関係の明示登録です。Codexは元依頼と原因を照合して作成します。
helperは修復内容の意味を自動証明せず、登録scopeと証拠・競合を検証します。無関係な待機PRを自動選択しません。
helperは双方のreceipt、親のmerged state・実merge・障害、子のopen PR・clean head・最新base・reviewを照合し、
private証拠を保存した後、固定した親scopeに一致する場合だけ枠を交換します。親の所有はsuspendedとして残ります。
親の新しいCI状態が旧waiver外でも、修復のための実測は取得できます。この観測は`approved: false`であり、
親のCI免除、receipt更新、finish許可には使いません。Codexが最新の基盤障害を分類してrequestへ根拠を記録します。
mainが実mergeより先へ進んでいれば、そのlive SHAのCIもAPI取得して同じ非公開観測へ束縛します。
既知の製品test失敗やAPI不明はこの修復枠にも使いません。親の既存receipt/auditとdelivery gateは維持します。
先行checkoutへのfetchがsandboxの書込み範囲外なら、同一UIDの通常ホストで正規helperを実行します。
修復PRのCI起動前にはrequired checks待ちの`BLOCKED`を枠認可だけに許容します。適用Rulesetとcheck-runを取得し、
CI以外のrule、配備・review承認、classic protectionなどの判定不能な阻害条件がなく、未完了required Actionsだけが
残っていることを確認してprivate証拠へ保存します。API不明や、全required CI成功後も理由不明のBLOCKEDなら保持します。conflict/UNKNOWNも保持し、
merge時のCI・Ruleset gateは通常のdeliverで再検証します。CI起動labelを必要とする修復PRも先行枠との相互待ちにしません。
枠取得前にもchildの通常deliveryに必要な非CI契約を確認します。strictは既存の完全なRuleset契約を使い、
default Ruleset不足・bypass・配備条件などを枠取得後へ持ち越しません。UNSTABLEのchildは保持し、修復・再検証へ戻します。
交差repositoryの修復は1段に制限し、競合・循環・別の修復中は保持します。再実行は同じ固定scopeで再照合します。
修復PRは通常のauthorize-ci/deliver/finishを使い、finish完了時だけ親scopeへ戻ります。親のfinishを再開して
CI回復または更新済みの限定証拠を確認します。無条件release、state手編集、旧helperへの退避を使いません。

Actionsの容量棚卸し・保持方針・回復確認は[artifact運用](codex-actions-artifacts.ja.md)を参照してください。
PRには定型の検証結果・免除の限界と、検証済みの原因区分だけを保存確認します。
`public_summary`はrequest v1の互換fieldとして非公開に保持し、自由記述・task名・path・command・check名は投稿しません。
旧形式の公開記録は、privateな保存記録と一致する自分のcommentだけを定型文へ移行します。所有不明の編集は保持します。
Issue記録とcleanupは既存finishを使い、API取得不能や保存確認失敗では削除しません。

CI待機の免除はworkflowの停止ではありません。事前にbase/headのtriggerと既存CDへの影響を調べて報告し、
`acknowledge_workflow_runs`で記録します。push/mergeの自動起動は既存設定に従います。
manual dispatch、release、production deploy、新しいenvironment approvalは含みません。
次の新しいtask/deliveryではgate指定を省略して通常経路から始めます。新しい権限・費用・deploy承認は継続委任に含みません。

## deliver

`deliver`はreceiptとlive状態をもう一度読み取り、上記の全条件を満たした同一SHAだけを対象にします。
`autonomous`または`human-approved` decisionを持つreceiptだけをReady化し、
選択したremote gateが許可するmerge methodでmergeします。Ready化やmergeの前後でhead SHA、
PR状態、remote gate、CI（local-only modeでは固定済みlocal test証拠）を取り直し、
raceやstale状態を検出したら停止します。

merge直前にはrepository、PR番号、open/Ready、base=`main`、head branch、head SHA、同一repository、
auto-merge無効、mergeable/CLEANを再取得します。merge commandは
`gh pr merge <number> --repo <owner/repo> --merge --match-head-commit <review済みSHA>`だけです。
GitHubが提供するcompare-and-swapはhead SHAに対するものなので、直前readとmergeの間に権限主体が
base branchやrepository設定を同時変更する競合をatomicには拘束できません。その同時変更は上記の
信頼境界内とし、通常の外部状態変化は直前readback、Ruleset、固定head条件で縮小・検出します。
`--admin`、`--auto`、`--delete-branch`、squash/rebaseは使いません。

確認できない状態を成功扱いしません。失敗、timeout、pending、dirty、stale、conflict、mergeable
不明、network/auth障害、RulesetまたはFree/private readback不一致ではdeliverしません。PR、remote/local branch、
worktree、receiptを保持し、再開時に状態を再取得します。

mergeのGitHub CLIが失敗した場合は、終了codeと固定分類だけを返します。stderr本文・URL・認証値は
表示しません。分類は原因調査の手掛かりであり、gateを弱めたり自動再送を許可したりする判定ではありません。
PRが実際にmergeされたかを再取得し、保存済みdelivery stateと照合してから同じhelperで再開します。

## finishとmain同期

mergeがGitHubで完了した後、`finish`は次を順に検証します。

1. PRがmergedである。
2. receiptのhead commitが`origin/main`の履歴へ到達している。
3. 人間用checkoutがmainで、未commit・未追跡のないclean状態である。直前の`finish`が中断した場合だけ、後述の限定条件で中断状態を復旧する。
4. fetchで固定したSHAへの`git merge --ff-only <SHA>`だけでlocal mainを更新できる。
5. local mainと`origin/main`が一致する。

`merged` stageのmain同期が中断してworking treeの一部だけ更新された場合、helperは現在のmain HEADが
取得済み`origin/main`のancestorであることを復旧前に確認します。対象はunstagedの通常fileと、
元のHEADに存在しない未追跡の通常fileだけです。各fileのbyteと実行bitが取得済みtargetのblobと完全一致し、
staged、削除、rename、type変更、symlink parentがないことを証明できる場合に限ります。
unstaged fileは元のHEADへ戻し、検証済みの未追跡fileは前述の`.codex-trash/`へ退避して保持します。
各fileの変更直前にcheckout branchが`main`であること、固定HEAD、固定origin/main、残りstatus、inode、mode、blobを再検証し、
途中で1つでも変化した場合は未処理fileへ触れず停止します。
merge直前にも固定HEADとorigin/mainを再確認し、可変の参照名ではなく固定SHAへff-onlyします。
固有のlocal変更や判定不能なpathは上書きしません。

この限定復旧条件に合わないdirty checkout、mainのdiverge、remote到達性が判定不能な場合はreset、rebase、
force update、強制cleanupを行いません。PR、branch、worktreeを保持して再開条件を報告します。

### Issue記録と回収の再開

`issue-N` taskはIssue Nを記録先にします。通常の`task-*`で関連Issueがある場合は、`finish`にだけ`--issue N`を追加してください。Issueなしの軽微なtaskはPRを記録先とし、Issue APIを呼びません。

helperはPR本文を作業記録として固定し、task、PR、source head、完了処理の状態とともに同一repositoryのIssueへ保存します。PR本文に目的、判断、変更、検証、残存事項を記載してください。空本文、別repository、PRをIssueとして指定、taskと異なるIssueを拒否します。削除前の保存確認に失敗したらworktreeは保持します。

同じtask・PR・headの記録は同じcommentへ更新します。投稿の応答を失った場合は再取得して重複を避け、他者のcommentや手動追記のある記録は上書きしません。Issue番号と本文はdelivery state v2へ保存し、再開時に`--issue`を省略しても維持します。v1の進行中stateは読み替え、receipt v6と旧receiptの検証条件は変えません。

回収後は`cleanup_complete`を保存してからIssueを完了へ更新し、成功後に`completed`へ進みます。Issue更新に失敗したら物理回収済み・記録更新待ちとして同じ`finish`で再開でき、sourceの再作成や再削除はしません。完了済みの古いtaskへ後付けでIssueを追加しません。必要な説明は通常のIssue文書として残してください。

初回の親checkoutがunborn mainの場合は、indexと`.git`以外の全内容が空で、固定origin/mainを確認できる場合だけnative Gitで初期同期します。既存file、別branch、壊れたHEAD、中断後の不明な状態は保持し、自動resetや削除をしません。

## managed cleanup

### 手動マージ・main同期後の管理状態の復旧

人間がPRをmergeしてmainを同期し、delivery stateが作成されていない場合には、次の明示的な復旧経路を使えます。
通常の同一headのmode変更禁止は維持し、この復旧操作だけで既存v6 receiptのstrict-rulesetからprivate profileへの変更を認めます。
Free/private経路と復旧について、先に人間の明示承認を得てください。

```sh
codex-delivery approve-review --task-id <task> --pr <pr> --head <head> --plan-id <plan-v1> --plan-version 1 --risk high --tests-passed --independent-review-passed --gate-mode github-free-private --recover-merged
codex-delivery finish --task-id <task> --pr <pr> --head <head> --plan-id <plan-v1> --plan-version 1 --gate-mode github-free-private
```

復旧は既存receiptと同じtask・PR・head・Plan・risk・review evidenceに限定します。既存stateがある場合は拒否し、従来のfinish経路で扱います。
PRの同一repository/branch/headでのmerge、headのmain到達、cleanかつ最新mainの親checkout、cleanな作業worktree、private repository identity、
PR headと現在のmainの両方の全Actions check成功、未解決reviewなしをliveで検証します。明示承認済みの
local-only構成では、同じevidenceの`github-free-private-local` receiptと`--recover-merged`を使えます。
その場合も両treeのworkflow条件と、存在する全Actions checkの成功系conclusionを再検証します。
receiptがない旧作業は、固定headの検証・reviewを完了し通常の`approve-review`で作成してから復旧します。CI失敗からのfallbackはしません。
条件を再照合した後、承認済みreceiptと`merged`状態だけをatomic保存します。receipt更新後に中断した場合は再実行して全条件を検証できます。
この操作自体はGitHub merge・main同期・worktree削除を行いません。stateの手編集・削除で代用しないでください。

finishの最後に、管理root内の対象worktreeだけをcleanupできます。helperは少なくとも次を同時に
証明しなければなりません。

- manifestのrepository、task ID、worktree path、branchが対象PRと一致する。
- PRがmergedで、receiptのhead commitがmainへ到達している。
- worktreeにtracked/untrackedの変更がなく、ignored artifactが既知の再生成可能なdirectoryだけで、
  未push commitがなく、別taskのworktreeではない。`.codex-trash`と未知のignored artifactは保持して停止する。
- cleanup対象が`$CODEX_HOME/worktrees`のmanaged root内にあり、pathやGit登録を再解決できる。
- 必要なIssue記録を保存確認でき、同一UIDのprocessがworktreeを使用していない。unlock前とremove直前に使用状況を確認し、使用中・判定不能は保持する。

この証明を満たしたmanaged cleanupだけが、自律的なworktree・対応branch削除の例外です。証明が
一つでも不足する、対象がdirty、未merge、未push、別taskと競合する、または判定不能な場合は対象を
容量・件数・経過日数に関係なく保持し、activeなworktreeの作業を制限しません。管理root外や任意の
ファイル・directory・branchの削除は従来どおりDaikiの確認を得て、
プロジェクト直下の`.codex-trash/<timestamp>/`へ退避してから扱います。直接`rm`や直接GitHub API
削除で代替してはいけません。

cleanupは5分のdeadline付きrepository-wide lifecycle lockとtask単位のmanifest/stateを使う再開可能な
state machineです。lock取得待ちもdeadlineへ含め、競合processが停止していても無期限には待ちません。
`merge_started`、`merged`、`main_synced`、`remote_delete_started`、`remote_deleted`、
`worktree_unlock_started`、`worktree_removed`、`cleanup_complete`、`completed`をatomic保存し、state欠落・schema不一致を
成功扱いしません。remote branch削除だけは、削除直前に確認したreview済みSHAを
`--force-with-lease=refs/heads/<branch>:<SHA>`へ固定して競合更新を拒否します。これはbranch内容を
上書きするforce pushの許可ではなく、managed finish内部のexpected-SHA付き削除に限る例外です。
`rm`、`prune`、`branch -D`は使いません。途中失敗時はstateと未完了の対象を残し、同じtaskだけを
再検証して再開します。

## 停止と再開

次のどれかが起きたら、外部状態を破壊せず停止します。

- head SHAがreview済みreceiptから変化した
- CIが失敗、skip、cancel、timeout、pending、neutral、または取得不能
- actionableな指摘、未解決thread、merge conflict、stale branchが残る
- Rulesetがinactive、またはFree/private repository設定を含むremote gateのreadback不一致、
  mergeability判定不能
- decision assessmentが未確定、または`human-required`なのにDaikiの判断がない
- PR、branch、worktree、human checkoutがdirtyまたは対象を一意に解決できない
- mainがff-onlyで同期できない、またはremote/localの到達性を確認できない

停止時の報告には、PR URL、repository、base/head、現在SHAとreceipt SHA、CI状態、review状態、
未解決件数、risk、残存PR/branch/worktree、再開に必要な操作を含めます。失敗した試行を理由に
PR、branch、worktreeを削除・reset・force updateしません。

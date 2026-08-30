# Codex delivery運用ガイド

このガイドは、Draft PR作成後の完了条件を定義します。Draft PRは中間点であり、完了では
ありません。レビュー済みの正確なcommitだけをReady化・mergeし、merge後のmain同期と
managed worktree cleanupまでを同じdeliveryとして扱います。

## 正本と経路

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
明示承認し、固定headに`.github/workflows/*.yml|*.yaml`がない場合に限り、次のlocal-only profileを
使用できます。workflow YAMLがあれば`runs-on`のrunner種別に従って通常CIを使います。
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
- riskに応じたreview（low/mediumはSolのself-review、highは独立review、criticalは必要時の専門review）、testの完了判定、actionable件数

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
riskはreview深度と残存影響を決めますが、人間確認を自動決定しません。low/mediumはSolのself-review、
highは独立reviewを1つ実行し、criticalは別の高リスク境界が実在する場合だけ専門reviewを1つ追加します。
一般的な反論役や肯定役は使いません。actionableな指摘は今回修正すべき具体的な欠陥に限り、fileまたは実行経路、期待結果と実際の結果、再現・確認方法、修正後の観測条件を明らかにします。将来改善、好み、具体的な影響根拠のない懸念は含めません。

共通root causeの指摘は1つのbatchで修正し、可能なら回帰testを追加します。修正でSHAが変わった場合は新SHAでreview、CI、receiptをやり直します。同じ問題が修正後も再発するか、2回続けて受け入れ条件・test・既知指摘に証拠上の進展がない場合は、Solがroot cause、実装境界、検証手段を再確認します。その後も同じ問題が続く場合だけblockedとし、無変更retryを続けません。

PRやIssueへ内部監査用のschema JSON、fingerprint、digest chain、round logを投稿しません。PR bodyとcommentは、人間が読む変更概要、判断が必要な論点、検証結果、残存事項に限ります。作業範囲は依頼の目的、観測可能な受け入れ条件、変更対象経路、必須検証で区切り、その集合外の改善を自律loopへ追加しません。

`codex-delivery record-review`はv6 receiptをprivateなmanaged stateへ記録し、固定SHA、変更file、test、riskに応じたreview、Plan、decision、gate modeを固定します。v6の`deliver`と`finish`はPR comments APIや機械監査commentに依存しません。既存v5 receiptだけは進行中taskを安全に再開するため、記録済みの旧ledger comment chainを読み取り専用で再検証します。新しいledger commentは作成しません。

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
`required-ci` check runを要求しません。代わりに固定SHAで完了したlocal test・従来必須の独立review・専門reviewを
receiptへ固定し、workflow YAML不在、high/critical、`human-approved`を必須にします。同一headの既存
`github-free-private` receiptは、同じPlan・evidenceのまま明示承認されたこのmodeへだけ更新できます。

このmodeの`tests-passed`はlocal実行結果の構造的な申告であり、GitHubが実行・強制するCI証明では
ありません。CI失敗・pendingを迂回するfallbackとしては使わず、CI自体を運用しない方針への明示承認が
ある場合だけ選択します。GitHub側で直接push等を拒否できない残存リスクも引き続き存在します。

### workflow不在のlocal validation

PRのlive baseと固定headの双方に`.github/workflows/*.yml|*.yaml`がない場合は`--gate-mode local-validation`を使います。
公開・非公開を問わず選択でき、CI不在だけを理由にhuman approvalやrisk引き上げを要求しません。
README、CONTRIBUTING、package scripts、build manifestからformat、lint、型検査、test、buildのうち
変更に該当するcommandを実行し、固定headの`tests-passed` evidenceへ記録します。

workflow YAMLがbaseまたはheadに1つでも存在する場合はこのmodeを拒否します。固定headにGitHub Actions checkが存在する場合も全件の完了と成功系conclusionを要求し、CI failure、pending、runner unavailableを
local検証で迂回せず、workflowのtrigger、`runs-on`、job、matrixとlive repository ruleを正本にします。
mergeやpushで自動起動する既存CDはそのworkflowへ任せて状態を報告します。manual dispatch、release、
production deploy、新しいenvironment approvalはdelivery helperのscopeに含めません。

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

## finishとmain同期

mergeがGitHubで完了した後、`finish`は次を順に検証します。

1. PRがmergedである。
2. receiptのhead commitが`origin/main`の履歴へ到達している。
3. 人間用checkoutがmainで、未commit・未追跡のないclean状態である。直前の`finish`が中断した場合だけ、後述の限定条件で中断状態を復旧する。
4. fetch後に`git merge --ff-only origin/main`だけでlocal mainを更新できる。
5. local mainと`origin/main`が一致する。

`merged` stageのmain同期が中断してworking treeの一部だけ更新された場合、helperは現在のmain HEADが
取得済み`origin/main`のancestorであることを復旧前に確認します。そのうえでunstagedの通常fileだけを対象にし、
各fileのbyteと実行bitが取得済み`origin/main`のblobと完全一致し、staged、未追跡、削除、rename、type変更、
symlink parentがないことを証明できるときだけ、そのfileを元のHEADへ戻してff-onlyを再試行します。
復旧対象を固定した後も各fileのrestore直前にcheckout branchが`main`であること、HEAD、残りstatus、inode、mode、blobを再検証し、
途中で1つでも変化した場合は未処理fileへ触れず停止します。
固有のlocal変更や判定不能なpathは上書きしません。

この限定復旧条件に合わないdirty checkout、mainのdiverge、remote到達性が判定不能な場合はreset、rebase、
force update、強制cleanupを行いません。PR、branch、worktreeを保持して再開条件を報告します。

## managed cleanup

finishの最後に、管理root内の対象worktreeだけをcleanupできます。helperは少なくとも次を同時に
証明しなければなりません。

- manifestのrepository、task ID、worktree path、branchが対象PRと一致する。
- PRがmergedで、receiptのhead commitがmainへ到達している。
- worktreeがignored artifactも含めてcleanで、未push commitがなく、別taskのworktreeではない。
- cleanup対象が`$CODEX_HOME/worktrees`のmanaged root内にあり、pathやGit登録を再解決できる。

この証明を満たしたmanaged cleanupだけが、自律的なworktree・対応branch削除の例外です。証明が
一つでも不足する、対象がdirty、未merge、未push、別taskと競合する、または判定不能な場合は対象を
保持します。管理root外や任意のファイル・directory・branchの削除は従来どおりDaikiの確認を得て、
プロジェクト直下の`.codex-trash/<timestamp>/`へ退避してから扱います。直接`rm`や直接GitHub API
削除で代替してはいけません。

cleanupは5分のdeadline付きrepository-wide lifecycle lockとtask単位のmanifest/stateを使う再開可能な
state machineです。lock取得待ちもdeadlineへ含め、競合processが停止していても無期限には待ちません。
`merge_started`、`merged`、`main_synced`、`remote_delete_started`、`remote_deleted`、
`worktree_unlock_started`、`worktree_removed`、`completed`をatomic保存し、state欠落・schema不一致を
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

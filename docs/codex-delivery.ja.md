# Codex delivery運用ガイド

このガイドは、Draft PR作成後の完了条件を定義します。Draft PRは中間点であり、完了では
ありません。レビュー済みの正確なcommitだけをReady化・mergeし、merge後のmain同期と
managed worktree cleanupまでを同じdeliveryとして扱います。

## 正本と経路

`codex-delivery` helperを、次の一連の唯一のreceipt・delivery・finish経路とします。

```text
record-review -> (high/critical/判定不能だけ approve-review) -> deliver -> finish
```

すべてのcommandはcurrent repositoryのrootで実行し、`--task-id`、`--pr`、`--head`、
`--plan-id`を必須とします。review記録では`--risk`と`--tests-passed`、
`--neutral-review-passed`、`--adversarial-review-passed`も必須です。Codexや利用者が直接
`gh pr ready`、`gh pr merge`、任意のGitHub
merge API、`git worktree remove/prune`、任意branch削除を実行してこの経路を迂回してはいけません。

```sh
codex-delivery record-review --task-id <task-id> --pr <PR番号> --head <40桁SHA> --risk <low|medium> --plan-id <Plan ID> --tests-passed --neutral-review-passed --adversarial-review-passed
codex-delivery approve-review --task-id <task-id> --pr <PR番号> --head <40桁SHA> --risk <high|critical> --plan-id <Plan ID> --tests-passed --neutral-review-passed --adversarial-review-passed
codex-delivery deliver --task-id <task-id> --pr <PR番号> --head <40桁SHA> --plan-id <Plan ID>
codex-delivery finish --task-id <task-id> --pr <PR番号> --head <40桁SHA> --plan-id <Plan ID>
```

`review-branch`は読み取り専用です。reviewerはreceipt、Issue、PR、Git、worktreeを変更せず、
呼び出し元がreview結果を`record-review`へ渡します。

helperはcanonical install先から直接起動し、`/usr/bin/git`と`/usr/bin/gh`だけを使います。
`PATH`、`GH_HOST`、`GH_REPO`、`GH_CONFIG_DIR`、Git環境変数による対象・実行fileの差し替えを
拒否または無効化し、1回の`deliver`/`finish`全体を5分でfail closedにします。この境界は
同一userが任意programを実行してhelperやmanaged stateそのものを書き換える攻撃を防ぐsandboxでは
ありません。その権限を持つ主体と、repository設定やPRを同時変更できる管理者は信頼境界内です。

## DELIVERY-01: 固定SHAのreview receipt

Draft PRを作成したら、次の対象を固定します。

- repository、PR番号、base branch/ref、head branch/ref
- review対象のhead SHAとbase SHA
- task ID、worktree path、risk分類
- 必須CIのcheck名と実行結果、実行時刻
- 独立reviewの判定、actionable件数、未解決review thread件数

receiptは対象SHAに束縛します。review後にcommitをpushしてhead SHAが変わった場合、以前の
receipt、CI、review、確認を新SHAへ引き継ぎません。新しいSHAでCIと独立reviewを実行し、
新しいreceiptを記録します。

receiptのreview/test flagは、定めた手順を完了したことをmachine-readableに束縛する構造証拠であり、
review品質を暗号学的に証明するものではありません。helperはdelivery直前にbase SHAと固定head SHAの
実差分を再計算し、receiptのchanged-filesと順序も含めて完全一致させます。安全境界pathを含む差分は
low/mediumへ分類できません。

required checkは文字通り`success`だけを成功とします。`skipped`、`cancelled`、`timed out`、
`neutral`、`pending`、取得不能、判定不能は成功として扱いません。check名が同じでもworkflowの
内容や対象SHAを確認できない場合は不合格です。review receiptにはcheck名だけでなく実行内容を
確認した根拠を残します。

## DELIVERY-05: risk別のdelivery

### low/medium

通常の実装・修正で、delivery安全境界や高リスクデータ・権限に影響しないタスクをlow/mediumと
します。固定SHAに対する独立reviewでactionableな指摘があれば、同じworktreeで修正、検証、
commit、pushを自律的に行います。そのpushでSHAが変わるため、review、CI、receiptを最初から
やり直します。低・中程度の指摘を自律修正するために確認待ちへ遷移しません。

次の条件が同じhead SHAで成立した場合だけ、`codex-delivery deliver`へ進みます。

1. PRがopenで、baseがrepositoryのdefault branchである。
2. 現在のhead SHAがreceiptのreview済みSHAと一致する。
3. required CIがすべて文字通り`success`で、失敗、skip、cancel、timeout、neutral、pending、
   取得不能がない。
4. actionableな指摘が0件で、GitHub review conversationの全pageに未解決threadがなく、現在有効な
   `reviewDecision`が`CHANGES_REQUESTED`、`REVIEW_REQUIRED`、不明値ではない。
5. merge conflictがなく、branchが最新baseの要件を満たす。
6. 実行時点のlive Ruleset gateがactiveで、required CI、PR経由、conversation解決、merge method、
   force push/branch deletion禁止などのrepository正本と一致する。

### high/critical/判定不能

次はhighです。criticalはhighより厳格に扱い、判定不能もhighへ寄せます。

- CI/workflow、Ruleset、hook、rules、AGENTS、Skills、helper、installerなどdelivery安全境界
- auth、secrets、billing、production、本番データ、不可逆migration
- breaking change、後方互換性を壊す変更、security・データ損失に関わる変更
- 影響範囲またはリスクを確定できない変更

上記以外でも、reviewで同等の重大な懸念が出た場合はhighへ引き上げます。high/critical/判定不能
は、CI、独立review、actionable=0、未解決thread=0、live Ruleset gateが成立しても、毎回
会話でDaikiの今回のreceiptに対する明示確認を得てから`codex-delivery approve-review`を実行します。
commandのpromptや自動approval reviewだけをDaikiの確認とは扱いません。確認はタスク全体や以前の
SHAへ引き継がず、push、SHA変更、再reviewの
たびに求めます。

Issue #24自身は、AGENTS、Skills、rules、delivery経路を変更するためhighです。#24のDraft PR後は
low/mediumの自律経路へ降格せず、Daikiの確認を待ちます。

## deliver

`deliver`はreceiptとlive状態をもう一度読み取り、上記の全条件を満たした同一SHAだけを対象にします。
low/medium、または`approve-review`済みのhigh/critical/判定不能だけをReady化し、Rulesetが許可する
merge methodでmergeします。Ready化やmergeの前後でhead SHA、PR状態、Ruleset、CIを取り直し、
raceやstale状態を検出したら停止します。

merge直前にはrepository、PR番号、open/Ready、base=`main`、head branch、head SHA、同一repository、
auto-merge無効、mergeable/CLEANを再取得します。merge commandは
`gh pr merge <number> --repo <owner/repo> --merge --match-head-commit <review済みSHA>`だけです。
GitHubが提供するcompare-and-swapはhead SHAに対するものなので、直前readとmergeの間に権限主体が
base branchやrepository設定を同時変更する競合をatomicには拘束できません。その同時変更は上記の
信頼境界内とし、通常の外部状態変化は直前readback、Ruleset、固定head条件で縮小・検出します。
`--admin`、`--auto`、`--delete-branch`、squash/rebaseは使いません。

確認できない状態を成功扱いしません。失敗、timeout、pending、dirty、stale、conflict、mergeable
不明、network/auth障害、Ruleset readback不一致ではdeliverしません。PR、remote/local branch、
worktree、receiptを保持し、再開時に状態を再取得します。

## finishとmain同期

mergeがGitHubで完了した後、`finish`は次を順に検証します。

1. PRがmergedである。
2. receiptのhead commitが`origin/main`の履歴へ到達している。
3. 人間用checkoutがmainで、未commit・未追跡のないclean状態である。
4. fetch後に`git merge --ff-only origin/main`だけでlocal mainを更新できる。
5. local mainと`origin/main`が一致する。

ff-onlyで同期できない、checkoutがdirty、mainがdiverge、remote到達性が判定不能な場合はreset、
rebase、force update、強制cleanupを行いません。PR、branch、worktreeを保持して再開条件を報告します。

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

cleanupはrepository-wide lifecycle lockとtask単位のmanifest/stateを使う再開可能なstate machineです。
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
- Rulesetがinactive、readback不一致、mergeability判定不能
- high/critical/判定不能で`approve-review`がない
- PR、branch、worktree、human checkoutがdirtyまたは対象を一意に解決できない
- mainがff-onlyで同期できない、またはremote/localの到達性を確認できない

停止時の報告には、PR URL、repository、base/head、現在SHAとreceipt SHA、CI状態、review状態、
未解決件数、risk、残存PR/branch/worktree、再開に必要な操作を含めます。失敗した試行を理由に
PR、branch、worktreeを削除・reset・force updateしません。

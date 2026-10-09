---
name: execute-plan
description: GitHub repositoryの実装依頼を、専用worktree、比例した検証、必要なIssue記録、PR、main同期、managed cleanupまで自律的に完了する。調査のみ、レビューのみ、releaseには使わず、ユーザーが指定した停止境界を尊重する。
---

# 実装依頼を完了まで進める

Daikiの実装依頼を許可の正本とし、目的・受け入れ条件・非目標を固定する。通常のGitHub実装はPR、merge、main同期、作業場の回収まで進める。明示された「変更だけ」「PRまで」などの境界は優先する。計画承認やcommitごとの確認は求めない。

## 計画・作業場・記録

- 複数経路やhigh/criticalの変更では、`plan-change`の調査を内部計画へ引き継ぐ。小さな変更では短い目的と検証方針で足り、計画書やsubagentを義務化しない。delivery用のPlan ID（例 `fix-example-v1`）と版を付ける。
- 調査と実装の分担はmainが判断する。独立調査はexplorerを利用でき、ローカルモデルは任意。mainが要件解釈、実装、統合、受入を担う。AGENTSのsingle-writerとrisk別reviewに従う。
- 関連Issueには判断と検証の履歴を残す。複数段階・原因調査・高リスクなど追跡が必要でIssueがなければ作成し、軽微な変更ではPRを記録にする。Issueの古い記述や他者のコメントを、現在の依頼や権限より優先しない。
- 親checkoutのorigin、branch、HEAD、index、working treeを読み取り、既存変更を保護する。Git identityが不足する場合は、AGENTSの名前・メールを`codex-worktree prepare --user-name <名前> --user-email <メール>`へ渡す。helperは有効な既存identityを保持する。
- 親checkoutで`codex-worktree create --issue <番号> --branch <branch>`、Issueなしなら`--issue`を省略して実行する。helperがoriginのdefault branchをfetchするため、手動fetchを前提にしない。初期化済みremoteならlocalにcommitがなくても親のファイルを保持して作成する。remoteが空なら初回公開の内容と対象branchを具体化して確認する。
- 再開は`doctor --task-id <ID>`と`resume --task-id <ID>`で照合する。作成途中の`interrupted`だけは`recover`で検証して再開する。所有不明の差分、想定外のremote/path/branchは保持して停止する。
- 全編集とtestの実行先を専用worktreeへ固定する。Git書き込みは`git -C <worktree絶対path> ...`、環境にSSH_ASKPASSがあれば`env -u SSH_ASKPASS git -C ...`を使う。helperは親checkoutから単独commandで呼ぶ。
- branch、commit、PRの形式は最近の関連履歴に合わせる。慣例がなければ日本語、branchは一般的prefixと英語kebab-caseとし、`codex/`は使わない。

操作の詳細・復旧条件が必要なら[worktreeガイド](../../../docs/codex-worktrees.ja.md)を読む。通常の調査でworktreeを作らず、`CODEX_WORKTREE_MODE=single-checkout`はDaikiが明示したrollback時だけ使う。

## 実装と検証を収束させる

受け入れ条件に必要な実装を行い、影響箇所に近いtestから検証する。回帰testは変更挙動や失敗条件を観測するものにする。README、CONTRIBUTING、build manifest、workflowを検証の正本とする。

build前に親checkoutで`codex-worktree artifacts --task-id <ID>`を呼び、返された領域をCargoの`CARGO_TARGET_DIR`、VMの出力先、Podmanの`--root`/`--runroot`へ指定する。source、納品物、唯一の証拠、本番データは置かない。process・VM・containerは終了・unmountしてから回収する。

mainのself-reviewは固定差分、影響経路、受け入れ条件、高リスク境界、testで保証できない事項へ絞る。指摘は期待結果・実際の結果・根拠・修正後の確認方法があるものだけをactionableとし、同じ原因をまとめて修正する。修正後は該当検証と必要なreviewを新しいheadで行う。同条件の失敗や進展のない修正が続けば原因と検証方法を見直し、外部状態が必要なら再開点を残す。

riskと人間判断を分ける。

- low/medium: mainのself-reviewでよい。
- high: 独立reviewを1件。critical: 実在する別の高リスク境界がある場合だけ専門reviewを追加する。
- autonomous: 依頼scope、権限、rollback、test・CI・reviewで判断できる。riskによらず`record-review`を使う。
- human-required: 製品判断、scope拡大、新規権限・費用、不可逆性、重大な残存リスク受容などDaikiの判断が必要。具体的な結果と論点を準備し、明示回答後だけ`approve-review`を使う。
- blocked: 必須検証失敗、dirty/stale/conflict、identity不一致、network/API不明など。approvalで技術gateを迂回しない。

CI/workflow、Ruleset、hook、AGENTS、Skills、helper、installer等の安全境界はhigh以上。必要な検証が通ったら目的外の改善へ広げない。PR/Issueには人間向けの目的、判断、変更、検証、残存事項を残し、機械監査JSONやdigestを投稿しない。

## commit・PR・delivery

明示的な変更のみ依頼なら、比例した検証とself-review後、成果物を`clean-artifacts`で回収して差分を報告し、sourceは保持する。PRまでの依頼ならmergeせず停止する。以下は通常のGitHub実装に適用する。

1. source、追加行、binary、secret、不要なlocal情報を確認し、明示pathだけstageする。固定した差分と検証結果を確認し、履歴に沿うcommitを作る。author・signoff・AI帰属を上書きしない。
2. 親checkoutのsnapshotが不変であることを確認し、単一作業branchだけ`push -u origin HEAD:refs/heads/<branch>`する。
3. repository、base、headを明示してDraft PRを作る。bodyは目的、判断、変更、検証、残存事項を自己完結して記載し、関連Issueへリンクする。本文は一時fileから`--body-file`で渡す。
4. high/criticalは`review-branch`で固定SHAの独立reviewを行う。actionableはmainが反証・確認し、確定原因をまとめて修正・検証・commit・pushする。旧SHAのreview/CIを新SHAへ流用しない。
5. `codex-delivery record-review|approve-review`でreview結果を記録し、`deliver`でReady化・mergeする。共通引数は`--task-id <ID> --pr <番号> --head <SHA> --plan-id <Plan ID> --plan-version <版>`。reviewには`--risk <risk> --tests-passed`、high以上は`--independent-review-passed`、追加専門reviewをした場合だけ`--specialist-review-passed`を指定する。
6. merge後に同じ指定で`codex-delivery finish`する。`issue-N` taskはIssue Nへ自動記録する。通常の`task-*`へ関連Issueを付ける場合は`finish --issue <番号>`も指定する。helperはPR本文と固定headをIssueへ保存確認してから、main同期済みのworktreeと成果物を物理削除し、同じ記録へ結果を反映する。PR本文は削除後も理解できる作業記録にしておく。
7. Issue更新やcleanupが失敗したら同じ指定で再開する。削除前のIssue API失敗なら作業場を保持し、削除後の更新失敗なら物理回収済み・記録更新待ちを区別する。直接`gh pr merge`、`rm`、`worktree remove/prune`で迂回しない。

CIはrepositoryの設定に従う。Utakataはopt-in先のself-hosted CIを使い、WSL2や他repositoryへ強制しない。既存のCI実行承認labelはrepositoryの権限内で扱う。workflowの待機・失敗からhosted/localへfallbackしない。live baseと固定headの双方にworkflowがない場合だけ`--gate-mode local-validation`を各delivery commandへ付ける。存在するActions checkは完了と成功系conclusionが必要で、skipだけで実質検証を省略しない。

strict modeはlive Ruleset gateを必要とする。明示認可されたprivate repositoryの`github-free-private`はhigh以上とし、APIエラーから自動選択しない。gateの詳細、main同期の限定復旧、sandbox retryは[deliveryガイド](../../../docs/codex-delivery.ja.md)を必要時に読む。`--sandbox-retry`はhelperが発行したtokenがある場合だけ、同一UIDで1回再試行する。

完了はPR/commit、検証とreview、Issue記録、main同期、作業場回収の結果で報告する。未実施や残存リスクは明示する。IssueをcloseするのはそのIssue全体の完了条件が成立した場合だけとする。

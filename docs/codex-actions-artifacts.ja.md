# GitHub Actions artifactの容量管理

`codex-delivery actions-artifacts`は、明示登録したrepositoryと用途の通常CI artifactを棚卸し・整理します。
実運用のpolicy、参照確認、障害根拠、対象PR/head/base/Plan、削除記録はGit管理外の非公開ローカル領域へ置きます。
公開repositoryのfixture・Issue・PR・Actionsログには合成値か公開可能な要約だけを使います。
本repositoryへの導入で、別repositoryの設定やworkflowは変更しません。

## 保持と登録

通常CIの共通保持既定は作成から7日です。repository・用途の上書きを適用し、用途の指定を優先します。
helperが整理候補とする日付と、GitHubが保持する`expires_at`は別に棚卸しします。
配備候補、saved plan、rollback、監査・検証証拠は既存repositoryの保持規定と参照を優先し、自動削除しません。
用途・必要期間が不明、複数の用途登録に一致、参照あり、再生成不能、唯一の証拠の場合も保持します。

新規uploadの保持期間は、各repositoryで用途を確認して個別に設定します。通常CIの例は次の通りです。
配備候補などへ一律に適用しません。

```yaml
- uses: actions/upload-artifact@<repositoryで検証済みの固定SHA>
  with:
    name: ordinary-ci-test-report
    path: test-results/
    retention-days: 7
```

[GitHubの保持設定](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/enabling-features-for-your-repository/managing-github-actions-settings-for-a-repository)
の変更は新たに作成するartifactに適用され、既存artifactには遡及しません。
helperのpolicyを変えてもGitHubのupload設定は変わらず、その逆も同様です。

## 棚卸しと削除

```console
codex-delivery actions-artifacts inventory --policy <PRIVATE_POLICY_ABSOLUTE_PATH>
codex-delivery actions-artifacts cleanup --policy <PRIVATE_POLICY_ABSOLUTE_PATH>
```

policyは所有者だけが読める通常file（0600）、保存先directoryは0700にします。出力にも対象情報が含まれるため、
公開ログや添付へ転記しません。棚卸しでは容量・作成日時・GitHub保持期限・所属run・整理候補・保護理由・削減見込みを確認します。

Artifact APIには用途横断の参照グラフがありません。参照確認は、repositoryのworkflow、昇格候補、rollback、
監査・検証証拠などの正本を調べたうえで、正確なartifact/run/headへ束縛する非公開clearanceへ保存します。
clearanceの有効期間は5分です。名前prefixだけで新しいartifactへ参照なしの判断を流用しません。
用途や参照の更新を行うwriterとは運用上調整し、整理中に新しい参照を作らないようにします。
GitHubの参照取得とDELETEに跨がるtransactionはなく、helperだけで外部writerとの競合を完全に防ぐことはできません。
参照の安定を確認できない場合はclearanceを発行せず保持します。

helperは進行中run、未統合PR、配備の参照と所属runを照合します。削除直前にpolicy、artifact実体、所属runと
保護対象を再取得し、変化・API失敗・不完全な取得では保持します。repository単位のlockと非公開write-ahead記録で
同時実行・途中停止を制限します。許可条件内のartifact削除には毎回の確認を求めません。
削除APIは[単一artifact](https://docs.github.com/en/rest/actions/artifacts#delete-an-artifact)だけです。
workflow run全体、cache、Packagesをまとめて削除しません。削除したartifactは復元できません。

policy v1の合成例です。実値と確認日時は非公開fileでだけ置換します。`retention_days`は用途、repository、共通7日の順です。

```json
{
  "version": 1,
  "repositories": [{
    "repository": "example/product",
    "retention_days": 7,
    "usages": [
      {"name_prefix": "ordinary-ci-", "purpose": "ordinary-ci"},
      {"name_prefix": "extended-ci-", "purpose": "ordinary-ci", "retention_days": 14},
      {"name_prefix": "candidate-", "purpose": "deployment-candidate"}
    ],
    "clearances": [{
      "artifact_id": 101, "run_id": 202,
      "head_sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
      "references": [], "regenerable": true, "sole_evidence": false,
      "verified_at": "2026-01-20T00:00:00Z"
    }],
    "recovery_jobs": [{
      "workflow_path": ".github/workflows/ci.yml", "job_name": "unit-tests",
      "ci_only": true, "deployment_free": true
    }]
  }]
}
```

`recovery_jobs`は再実行してよいCI専用のworkflow/jobを登録します。配備や昇格を含むjobは登録しません。
依存先jobの再実行も含めて配備・昇格が起きないことを確認します。
GitHubが返すworkflow pathの`@ref` suffixを検証して除き、jobのattempt所属は
[attempt専用のjobs API](https://docs.github.com/en/rest/actions/workflow-jobs#list-jobs-for-a-workflow-run-attempt)で照合します。
参照確認のないartifactも棚卸しには表示しますが、削除候補にはしません。

## 容量障害からdeliveryへ戻る

GitHubの容量表示は[6〜12時間で更新](https://docs.github.com/en/billing/concepts/product-billing/github-actions)されます。
削除は現在の使用量を減らしますが、月内の累積利用を取り消しません。課金・予算変更、権限追加、本番配備は対象外です。
削除成功直後に同じupload失敗を繰り返さず、非公開記録の反映待ちと次の観測時点を使います。
対象run/job、head/base/PR/Planと原因を固定し、容量回復を確認してから対象CI jobだけを一度再実行します。
HTTP結果が不明なら再送せず観測へ戻します。既知の製品test失敗を容量障害として登録しません。

```console
codex-delivery actions-artifacts recovery record --policy <PRIVATE_POLICY_ABSOLUTE_PATH> --repository <OWNER/REPO> --run-id <RUN_ID> --job-id <JOB_ID> --head <PR_HEAD_SHA> --evidence <PRIVATE_FAILURE_EVIDENCE_ABSOLUTE_PATH>
codex-delivery actions-artifacts recovery confirm --policy <PRIVATE_POLICY_ABSOLUTE_PATH> --repository <OWNER/REPO> --run-id <RUN_ID> --job-id <JOB_ID> --head <PR_HEAD_SHA> --evidence <PRIVATE_RECOVERY_EVIDENCE_ABSOLUTE_PATH>
codex-delivery actions-artifacts recovery rerun --policy <PRIVATE_POLICY_ABSOLUTE_PATH> --repository <OWNER/REPO> --run-id <RUN_ID> --job-id <JOB_ID> --head <PR_HEAD_SHA>
```

failure evidence v1は`version: 1`、`repository/task_id/pr/head_sha/base_sha/plan_id/plan_version/run_id/job_id`、
`cause: "capacity-exhausted"`、`product_test_failures: false`、`observed_at`（24時間以内）、`summary`を持ちます。
summaryには容量不足と製品test失敗を区別した観測根拠を記録します。単なるjob failureだけでは登録しません。
recovery evidence v1は同じ固定scope、`version: 1`、`storage_recovered: true`、`product_test_failures: false`、
`source`（容量回復を確認した情報源）、`observed_at`（15分以内）、`summary`を持ちます。
日時はUTCのISO 8601表記です。両evidenceはGit管理外の0700 directory内に0600で保存し、本文とdigestをprivate記録に残します。
回復観測前は`awaiting-storage-refresh`を維持します。削除後の6〜12時間を観測目安とし、確認できた回復を根拠に進めます。
rerun前にscopeと証拠の期限を再照合し、要求前に一度性を保存します。run/jobが変われば新たな固定記録に分けます。
非公開ledgerは読み取り上限を超える前に履歴をarchiveし、未確定の削除intentを現行記録へ引き継ぎます。
旧履歴を削除せず、分割後も未確定結果の照合から再開します。

再実行後のActions check成功を確認したら、既存receiptと同じ対象の通常deliveryを再開します。
merge後に初めて障害が判明した場合も、[merge後証拠更新](codex-delivery.ja.md#merge後に判明した基盤障害の証拠更新)
で実mergeの親・tree・main到達を照合します。別repositoryの基盤修復が共有枠を待つ場合は同ガイドの
`repair-lane`を使い、親の所有を保持します。state手編集やguard解除で復旧しません。

## 導入手順

1. 通常のCodex installerで同一版のguard/helperを配置する。
2. 対象repositoryのartifact用途・保持規定・参照元・再生成手順を調べ、非公開policyへ明示登録する。
3. 必要な場合だけそのrepositoryの新規uploadへ`retention-days`を設定する。
4. 棚卸しを確認し、最新の参照確認をartifactごとのclearanceへ記録する。
5. cleanup後は非公開記録とGitHubの反映・CI結果を確認し、既存deliveryを再開する。

追加権限がない場合やAPI取得不能では保持し、与えられた権限のまま診断を続けます。

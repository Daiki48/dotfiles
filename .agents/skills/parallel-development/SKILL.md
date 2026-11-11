---
name: parallel-development
description: 複数Issueの並列実装、進行中に別タスクを追加する依頼、codex-taskの状況確認・引継ぎ・再開を扱う。相談だけでは実装を開始しない。
---

# 永続taskを介して並列開発する

利用者の自然な依頼を`codex-task`の操作へ変換する。固定の発話形式は要求しない。通常の単発実装は`execute-plan`を使い、既にtask workerとして実行中なら新taskを再登録しない。workerに指定されたPRまで・mergeまで等の停止境界を優先する。

- 最初に親checkoutで`codex-task doctor --json`を確認する。未導入ならdotfilesのセットアップ状態を確認し、通常運用でtaskごとにCargo buildしない。
- Codexのshell sandboxからのworker起動は、親sandboxのread-only mountを継承させないため、既存approvalに従って同じUIDの通常ホスト側へ渡す。`doctor`で`host_launch_required=true`なら、toolの`sandbox_permissions="require_escalated"`で同じdoctorを実行する。`start`・継続`run`・workerを起動する`reconcile`も同じ通常ホスト側で実行する。scope内の通常起動は既存auto-reviewへ審査を委ね、改めて人間の許可待ちにしない。子Codexの`codex-autonomous`・auto-review・hookは保持し、bypass flagやconfig書換えで権限を広げない。通常ホストでもread-onlyの場合やapproval拒否時は理由を具体化して止まり、迂回しない。
- 別Issueの実装依頼は`submit --task-id issue-<番号> --objective '<許可された目的・受入条件・停止境界>'`へ変換する。Issueなしは衝突しない`task-<ID>`を使う。相談・調査・Issue化だけの依頼を実装へ変換しない。
- `--authorization`には利用者の元の目的・明示境界を保存し、親が分割した`--objective`と区別する。利用者がPRまで・mergeまでを指定した場合だけ`--stop-after pr|merge`を付け、通常は既定のfinishまで進める。既存repository方針で明示されたFree/private運用は受付時に`--gate-mode github-free-private`を付ける。403から推測して切り替えない。既存taskのpolicyはstateを手編集せず、所有と既存receiptを確認する。
- 受付を担当するmainがAGENTSの基準でriskを判断し、`--risk low|medium|high|critical`へ渡す。判断できなければ既定のhighを使う。登録riskはレビュー深度の下限であり、実装workerの自己申告では下げない。
- CI起動labelの選択は継続委任されています。workflow・用途・labelの実在を確認して`submit`または停止回収後の`resume`へ`--ci-label <既存label>`を渡し、レビュー後にRustがPR/head/最新baseを照合する。repository横断の共有CI予約や容量設定はありません。既存の納品予約はrepository単位で保持し、同一repositoryの排他、CI check・finish要件を維持します。単独sessionも`codex-delivery authorize-ci`を使います。PR-wide labelは固定SHA単位の実行認可を保証せず、他host・自動trigger・人間の操作はliveで再確認します。runnerの稼働数とホスト資源はrunner運用側で管理します。新label・deploy承認・追加権限や費用は推測で登録しません。
- `submit`は登録だけで、`start`がOSタイマーなしのcoordinatorを起動する。登録後は`codex-task start --json`を呼ぶ。稼働済みなら同じcoordinatorが新taskを扱う。受付後も別依頼や相談に応じ、元workerの完了を会話の前提にしない。
- 通常実装のCI待ちを引き継ぐ場合も、既存taskとmanaged worktreeを照合する。未登録の既存worktreeには同じtask IDと実際の`--branch`を指定し、既存PR・固定head・Plan/版・残るdelivery・元の終了境界を保存する。旧writer停止・非使用を確認してから引き継ぎ、別IDや既定branchで同じ作業を再登録しない。`start`成功だけでなく、対象taskが既存PR/headと次段階を認識したことを`show`で確認する。CI待ちや先行作業待ちを便宜的なpauseへ変換しない。
- CI起動は完了ではない。finishまでの依頼は、CI後のreview・delivery・main CI・finish・物理cleanupまで担当を維持する。親終了を許可された場合だけ確認済みの自律継続を報告し、親を継続する指示があれば最終回答で終了しない。画面のWorked表示はcoordinatorの稼働やtask完了の証拠に使わない。
- 同じtaskへの追加指示は`show`で状態を確認する。実行中にwriterを追加しない。必要なら`pause`で停止要求を出し、`run`が空になるまで`list`／`reconcile`で回収を確認してから`resume --task-id <ID> --instruction '<追加指示>'`、`start`へ進む。対象taskを文脈から特定できない場合だけ確認する。
- 依存taskは`--depends-on <ID>`、共有port・DB等は`--resource <衝突を避ける名前>`で登録する。各taskのbuild・DB・container等にもtask固有の名前と既存の登録済みartifact領域を使う。
- 状況確認は`list --json`、詳細・引継ぎ・個別CI証跡は`show --task-id <ID> --json`、未処理イベントは`events --task-id <ID> --json`を使う。確認して必要な対応を済ませたsequenceまで`ack --task-id <ID> --through <sequence>`で累積ackする。表示だけではackしない。`events --all`は保持中のack済み履歴も返す。保持期間は`retention --task-id <ID> --seconds <秒>`（既定30日）で指定し、`collect --task-id <ID>`で完了・全ack・worker停止を確認して期限超過の旧結果だけを回収する。task本体・checkpoint・最新結果は保持される。これらはローカル情報であり、公開Issueへ生出力を転載しない。
- 通常のCI待ち・コード修正・レビュー・既存権限内の統合は自律的に進める。製品判断・追加権限・費用・不可逆操作は`awaiting_decision`の内容を具体化して利用者へ返す。未知の承認をworkerへ代わって与えず、guard・trust・sandboxを迂回しない。
- workerの技術的なblockedや未分類の判断待ちは読み取り専用の診断へ戻す。診断は元の許可、親の分割、証拠、次の対応、外部解消条件を構造化し、修復・監視・元依頼内の後続taskへ接続する。真正な意思決定だけquestion/impact/recommendationを整理して人間へ返す。旧writerの停止確認前に新writerを加えない。CI待ちはRustが監視し、host資源と待機時間を見て独立作業を進める。未観測のCI runner資源は推測しない。
- coordinatorはlive base refと固定headのcommit/treeから、両方にworkflowがない場合だけ`local-validation`を選ぶ。API不明・不完全なtree・実在checkの失敗や未完了を成功へ変換しない。`record-review`・`deliver`・`finish`はlocal時に`--gate-mode local-validation`を明示し、strict時は`--gate-mode`を省略する。固定headの比例したlocal検証、riskに必要なレビュー、既存receiptとhead/baseの検証を維持する。追加push/base変更では旧CI・旧レビューを使わない。merge後も実merge commitのmain到達、workflowと実在checkを確認してからFinishへ進む。起動直前の再観測やreceiptのgate照合を省略しない。
- CI基盤障害の判断は継続委任され、[deliveryガイド](../../../docs/codex-delivery.ja.md)の`emergency-ci`を使える。読み取り専用Diagnosisが実測と原因を照合し、`next_action=emergency-ci`でCiEvidenceへ進む。high以上の固定head独立reviewとlocal test/buildを確認し、private requestと`record-review`で証拠を記録する。人間の再承認待ちは追加しない。既存receiptの移行は`--replace-receipt`、既存taskの引継ぎは停止回収後に同じIDの`resume --gate-mode emergency-ci`を使う。failure/pendingをsuccessへ変換せず、対象別receiptのdigestで検証する。head/base・根拠変更は再検証・review・証拠更新へ戻す。後続taskは通常CIから始め、既知test失敗、API不明、CI以外のgate、無関係なmain更新は免除しない。
- Finishでmainが最初cleanかつmanaged Skill等の更新を含む場合は、同一UIDの通常ホスト側でexecの`require_escalated`/既存auto-reviewを通して正規helperを実行する。permission設定を変更しない。通常sandboxで失敗しhelperが`sandbox-retry-ready`を発行した場合だけ、同じhostで1回の`--sandbox-retry`を使う。消費後の追加復旧には人間の明示承認が必要で、他taskの承認を流用しない。未知dirtyは保持する。
- FedoraとWSL2はそれぞれ実行先のpath・認証・stateを使う。SSH経由の依頼は接続先が所有host。host間の暗黙移動やfailoverを行わない。

- 容量障害は[artifact運用](../../../docs/codex-actions-artifacts.ja.md)の棚卸し・明示登録内の整理・反映待ち・対象jobの回復確認を使う。merge後も同じPR/head/base/Planと実mergeの親/tree/main到達を検証して固定証拠を更新し、既存taskのfinishへ戻せる。別repositoryの基盤修復は独立taskとして進め、親の納品予約を保持する。新taskの無断登録やstate手編集で回避しない。

起動・停止・復旧・現在の対応範囲は[タスク運用ガイド](../../../docs/codex-tasks.ja.md)を必要時に読む。

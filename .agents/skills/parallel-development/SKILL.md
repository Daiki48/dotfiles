---
name: parallel-development
description: 複数Issueの並列実装、進行中に別タスクを追加する依頼、codex-taskの状況確認・引継ぎ・再開を扱う。相談だけでは実装を開始しない。
---

# 永続taskを介して並列開発する

利用者の自然な依頼を`codex-task`の操作へ変換する。固定の発話形式は要求しない。通常の単発実装は`execute-plan`を使い、既にtask workerとして実行中なら新taskを再登録しない。workerに指定されたPRまで・mergeまで等の停止境界を優先する。

- 最初に親checkoutで`codex-task doctor --json`を確認する。未導入ならdotfilesのセットアップ状態を確認し、通常運用でtaskごとにCargo buildしない。
- Codexのshell sandboxからのworker起動は、親sandboxのread-only mountを継承させないため、既存approvalに従って同じUIDの通常ホスト側へ渡す。`doctor`で`host_launch_required=true`なら、toolの`sandbox_permissions="require_escalated"`で同じdoctorを実行する。`start`・継続`run`・workerを起動する`reconcile`も同じ通常ホスト側で実行する。scope内の通常起動は既存auto-reviewへ審査を委ね、改めて人間の許可待ちにしない。子Codexの`codex-autonomous`・auto-review・hookは保持し、bypass flagやconfig書換えで権限を広げない。通常ホストでもread-onlyの場合やapproval拒否時は理由を具体化して止まり、迂回しない。
- 別Issueの実装依頼は`submit --task-id issue-<番号> --objective '<許可された目的・受入条件・停止境界>'`へ変換する。Issueなしは衝突しない`task-<ID>`を使う。相談・調査・Issue化だけの依頼を実装へ変換しない。
- 受付を担当するmainがAGENTSの基準でriskを判断し、`--risk low|medium|high|critical`へ渡す。判断できなければ既定のhighを使う。登録riskはレビュー深度の下限であり、実装workerの自己申告では下げない。
- repositoryの既存方針で対象PRと後続pushのCI起動が明示的に委任されているlabelだけを、受付時に`--ci-label <既存label>`へ渡す。PR-wide labelは固定SHA単位の実行認可を保証しない。SHAごとの承認が必要な運用、未確認のlabel、新しい承認や権限を推測して登録しない。レビュー後のlabel追加はRustがPR・HEAD・baseとlabelの実在を照合して行い、CI起動専用のAI workerは作らない。
- `submit`は登録だけで、`start`がOSタイマーなしのcoordinatorを起動する。登録後は`codex-task start --json`を呼ぶ。稼働済みなら同じcoordinatorが新taskを扱う。受付後も別依頼や相談に応じ、元workerの完了を会話の前提にしない。
- 同じtaskへの追加指示は`show`で状態を確認する。実行中にwriterを追加しない。必要なら`pause`で停止要求を出し、`run`が空になるまで`list`／`reconcile`で回収を確認してから`resume --task-id <ID> --instruction '<追加指示>'`、`start`へ進む。対象taskを文脈から特定できない場合だけ確認する。
- 依存taskは`--depends-on <ID>`、共有port・DB等は`--resource <衝突を避ける名前>`で登録する。各taskのbuild・DB・container等にもtask固有の名前と既存の登録済みartifact領域を使う。
- 状況確認は`list --json`、詳細と引継ぎは`show --task-id <ID> --json`、イベントは`events --task-id <ID> --json`を使う。これらはローカル情報であり、公開Issueへ生出力を転載しない。
- 通常のCI待ち・コード修正・レビュー・既存権限内の統合は自律的に進める。製品判断・追加権限・費用・不可逆操作は`awaiting_decision`の内容を具体化して利用者へ返す。未知の承認をworkerへ代わって与えず、guard・trust・sandboxを迂回しない。
- FedoraとWSL2はそれぞれ実行先のpath・認証・stateを使う。SSH経由の依頼は接続先が所有host。host間の暗黙移動やfailoverを行わない。

起動・停止・復旧・現在の対応範囲は[タスク運用ガイド](../../../docs/codex-tasks.ja.md)を必要時に読む。

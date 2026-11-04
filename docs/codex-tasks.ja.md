# Codexの並列タスク運用

`codex-task`は各host内でtaskを永続化し、別々のmanaged worktreeとworkerで開発します。
Fedora上のproductはFedora、WSL2内のproductはWSL2で実行します。SSHでFedoraへ接続した場合も、
接続先のhome・repository・認証・状態を使います。接続元の設定値は持ち込みません。

## 導入と会話からの利用

dotfilesで`cargo run -- codex`を実行すると、既存helperと同じ検証・更新契約で`codex-task`を導入します。
Codex CLI、Git、GitHub CLI、Rust toolchainは既存の導入を使います。新規Codexセッションで
`parallel-development` Skillが複数Issueの依頼・別taskの追加・再開をCLIへ変換します。
通常の単発実装は従来の`execute-plan`を使います。

```sh
codex-task doctor --json
codex-task submit --task-id issue-123 --objective 'Issue #123の受入条件まで実装する'
codex-task submit --task-id issue-124 --objective 'Issue #124の受入条件まで実装する'
codex-task start
codex-task list
codex-task show --task-id issue-123 --json
codex-task events --task-id issue-123 --json
```

`submit`は登録、`start`は端末から切り離したcoordinatorの起動です。既に稼働中なら重複起動しません。
OSサービスを登録せずに使えます。`run`は前景で継続し、`reconcile`は一度だけ照合・起動します。
`run --poll-seconds 15 --timeout-seconds 3600`のように監視期限を指定できます。
期限到達はCI失敗として扱わず、動作中workerと状態を保持します。

実装workerは検証・commit・push・Draft PRまで、必要な独立レビューworkerは読み取り専用、
統合workerは既存の`codex-delivery record-review→deliver`、後処理workerは`finish`を担当します。
受付を担当するmainが`submit --risk low|medium|high|critical`でレビュー深度の下限を決めます。
既定はhighです。実装workerの報告では引き下げず、必要な場合は引き上げます。
受付時点でlow/mediumと判断できた場合は実装時のself-review、高以上は別workerの独立レビューを使います。
統合はrepository単位で順序を管理し、merge commitのmain CI完了まで後続統合を待たせます。
対象PRと後続pushのCI起動が既存方針で明示的に委任されているlabelだけを、受付時に`--ci-label <既存label>`へ渡します。
固定HEADのレビュー後、RustがPR・HEAD・baseとlabelの実在を照合し、登録されたlabelの追加だけを行います。
CI起動用のAI workerは起動せず、RustがCI成功を確認してから統合workerを起動します。
GitHubの[label API](https://docs.github.com/en/rest/issues/labels#add-labels-to-an-issue)はPR単位です。
照合と追加の間にpushが入る可能性があり、後から検知してもCI起動を取り消せません。
このadapterは固定SHA単位のCI実行認可が必要な運用には使いません。
labelをレビュー証拠には使わず、HEAD/base変更時はレビューを無効化し、merge前に再照合します。
既存deliveryのreview receipt・固定SHA・remote gate・cleanup検証を省略しません。

## 保存場所とhost境界

account homeはsetupと同じ検証を使います。`CODEX_HOME`が絶対pathで指定されていれば優先し、
未指定なら検証済みhomeの`.codex`です。既存helperと同じmanaged rootの
`<CODEX_HOME>/worktrees/<repo-key>/.tasks/`にtask、checkpoint、イベント、worker結果を保存します。
worktree自体は既存の`<repo-key>/<task-id>`を再利用します。

task stateは0700 directory・0600通常fileに限定し、symlink・hardlink・他UID・別hostのstateを拒否します。
host参照にはmachine-idとUIDのhashを使い、実hostnameは保存しません。
stateを共有home・ネットワーク共有・別OSへコピーして分散lockとして使う運用は未対応です。
同じrepositoryを別checkoutへ無断で再紐付けすることも拒否します。

XDG変数はこのtask stateの配置を上書きしません。XDG_CONFIG_HOME等は必要な子processへ明示的に渡します。
ユーザーSkillsはHOME配下、global instructionsとrulesはCODEX_HOMEの公式探索場所へsetupが配置します。
非標準CARGO_HOME／RUSTUP_HOMEは、このworker adapterでは未対応として診断します。

生のCodex JSONL、shell引数、APIレスポンス、認証情報はログへ保存しません。
workerのthread参照・正常終了情報・構造化した結果だけをhost内へ保持します。
目的・引継ぎ・結果には非公開productの情報が含まれ得るため、これらも公開repositoryへcommit・転載しません。
CLIは外部Issue更新を直接組み立てず、既存の対象repositoryの運用と公開範囲を守るworkerが必要な記録を行います。

## 所有権・停止・復旧

状態の短い更新lock、coordinatorの生存lock、taskごとのwriter生存lockを分けています。
worker生存lockは子processへ継承し、coordinatorやworker監督processが異常終了しても、
旧workerが残る間は新writerを起動しません。heartbeatやPID番号だけでは所有権を奪いません。

```sh
codex-task pause --task-id issue-123
codex-task list --json
codex-task resume --task-id issue-123 --instruction '確認した判断と次の作業'
codex-task start
```

`pause`はworkerへ停止を要求し、既存のLinux process-tree監督が子processを回収します。
`resume`はworkerの状態と生存lockが解放されるまで拒否します。古い実行世代の結果を新しい結果へ混ぜません。
完了報告前に中断した場合も、旧writer停止を確認した後に既存のGit・PR・delivery状態から再開します。
同じtaskの修正・中断が3回続けば作業を保持して停止します。これはtask受付数の上限ではありません。
CI待ちにはCodexを起動せず、Rustが現在のPR・head・base・test merge commitとcheck/statusを照合します。
APIエラーはCI失敗と分けてbackoffし、check不足・cancel・timeout・skipのみを成功扱いしません。

CPU、MemAvailable、disk空き容量を測り、実行枠を決めます。受付task数に固定上限はありません。
host-localの`.tasks/budget.json`でworkerごとのRAM予約量、hostに残すRAM、最低disk空きを調整できます。
既定はそれぞれ2048、1024、2048 MiBです。portやDB等の共有資源は`--resource`で排他し、
依存関係は`--depends-on`で記録します。単なるtext conflictの有無で互換性を保証しません。

## OS起動管理と現在の対応範囲

状態遷移の正本は単発照合です。`run`、`start`、将来のOS起動adapterは同じ処理を使います。
`launch-command --json`は起動に必要な実行file・引数・cwd・CODEX_HOMEを返すだけで、OS設定を変更しません。
coordinator自体の異常終了後やOS再起動後の自動起動が必要なら、systemd user service等の任意adapterを使えます。
この初期版はサービス・timer・linger・Windows起動設定を自動導入しません。

WSLのsystemd serviceはWSL instanceを生存させません。停止中は待機し、次回`start`／`run`で照合します。
OS起動adapterを使う場合も、WSL起動前からの処理継続は保証しません。
Linux user/PID namespace・pidfd等の必要機能はdoctorで確認します。利用不能なら安全性を落として実行しません。
macOSのhost identity・process監督・launchd adapterは将来対応です。

この段階ではmain CIが存在するrepositoryの自律pipelineを対象とします。workflow不在repositoryの
`local-validation`を使うdeliveryは既存helperで可能ですが、coordinatorによるその自動選択は今後の対応です。
CI起動に独自承認labelが必要なrepositoryは、既存の委任と運用を守り、未知の承認を自動付与しません。
イベントは直近128件を状態に保持します。長期event archive・結果ack・worker結果の保持期限・OS起動adapterの
導入管理はIssue #75で継続します。タスク完了後のworktree/artifact回収は既存helperで行い、引継ぎ記録は保持します。

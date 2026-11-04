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
codex-task ack --task-id issue-123 --through 42
```

親が作業を分割するときは、`--authorization '利用者の元の目的・明示境界'`を別に保存します。
`--objective`は今回の部分作業です。通常はfinishまで進み、利用者が明示した場合だけ
`--stop-after pr`または`--stop-after merge`を指定します。内部workerの「PRまで」は段階間の
引継ぎであり、上位の依頼の終了境界にはなりません。終了境界へ到達したtaskは
`boundary_reached`となり、未統合のsource・PRを保持します。

既存方針でFree/private経路が明示されているrepositoryは、受付時に
`--gate-mode github-free-private`を付けます。risk下限はhighです。Rules APIの403から推測せず、
live private repository policy、同repositoryのPR、固定headと実mergeのActions・statusを確認します。
既存の停止taskへこの明示policyを反映する場合は、worker停止を確認後、
`resume --task-id <ID> --instruction '<既存許可と次の作業>' --gate-mode github-free-private`
を使います。旧review/CIを無効化して実装・検証から再照合し、merge後のpolicy変更は拒否します。

`submit`は登録、`start`は端末から切り離したcoordinatorの起動です。既に稼働中なら重複起動しません。
Codexのshell sandboxからは、既存approvalで同UIDの通常ホスト側に`start`／`run`／`reconcile`を渡します。
子Codex runtimeはローカル設定・cache・stateへの書込みが必要で、`setsid`は親sandboxのread-only mountを解放しません。
`doctor`の`host_launch_required=true`はこの状態を示します。子Codexのtool権限・hook・auto-reviewは設定どおり維持します。
登録・状態確認・停止要求・追加指示は通常のsandbox内から行えます。
OSサービスを登録せずに使えます。`run`は前景で継続し、`reconcile`は一度だけ照合・起動します。
`run --poll-seconds 15 --timeout-seconds 3600`のように監視期限を指定できます。
期限到達はCI失敗として扱わず、動作中workerと状態を保持します。

実装workerは検証・commit・push・Draft PRまで、必要な独立レビューworkerは読み取り専用、
統合workerは既存の`codex-delivery record-review→deliver`、後処理workerは`finish`を担当します。
受付を担当するmainが`submit --risk low|medium|high|critical`でレビュー深度の下限を決めます。
既定はhighです。実装workerの報告では引き下げず、必要な場合は引き上げます。
受付時点でlow/mediumと判断できた場合は実装時のself-review、高以上は別workerの独立レビューを使います。
統合はrepository単位で順序を管理し、merge commitのmain側gate確認まで後続統合を待たせます。
対象PRと後続pushのCI起動が既存方針で明示的に委任されているlabelだけを、受付時に`--ci-label <既存label>`へ渡します。
固定HEADのレビュー後、RustがPR・HEAD・baseとlabelの実在を照合し、登録されたlabelの追加だけを行います。
CI起動用のAI workerは起動せず、RustがCI成功または後述のlocal gate条件を確認してから統合workerを起動します。
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
`show --json`の`ci_evidence`には、head・base・実際のCI対象SHA、観測時刻（Unix秒）、必要checkと
個別check run／commit statusを保存します。check ID・app ID/slug・check suite ID・状態/結論・参照URL、
status ID・提供者ID/login・対象SHA・更新時刻・参照URLを含みます。Actionsはcheck suiteから
公式APIで取得したrun ID/attempt・runの対象SHA・状態/結論・URLを別に保存します。
check suite IDをrun IDとして扱いません。Actions以外の提供元や未提供のURL等は`null`です。
head/base変更時は旧成功を無効化し、API失敗時は`ci_evidence`と成功状態を消して
`ci_observation_error_at`へ失敗観測時刻を残します。統合・後処理の起動直前にも再観測して保存します。
`gate_mode`は`strict`または`local-validation`で、merge後もreceiptに選んだgateを保持します。
local gateの`ci_state`はCI成功と区別した`local_validation`です。`ci_evidence.workflows`には
live base・固定head・main側対象commitのSHA、tree SHA、workflow一覧を保存します。
旧task JSONでこれらがない場合はstrictを既定とし、現在の一次情報で再観測します。
APIレスポンスのログ本文・check出力・annotations・認証値は保存対象にしません。
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
技術的なblocked、未分類の判断待ち、中断、反復修正は、旧writerの停止確認後に読み取り専用の
`diagnosis`へ戻します。診断は元のauthorizationと親のfile分割、原因、証拠、次の対応、解消条件を
分け、修復・再分割・元依頼内の後続task登録へ接続します。高riskや回数だけで人間待ちにしません。
真正な製品・scope・新権限・費用・不可逆性・重大リスクの判断だけ、質問・影響・推奨案を保存して
`awaiting_decision`へ返します。利用者によるpauseと終了境界は自動解除しません。

同じ原因・証拠での診断再試行は有限です。3回の再試行後も進展がなければ`waiting`へ移し、
CI/Git状態、後続task、hostの実行可否、非秘密のworkspace相対pathのmetadataをRustが120秒ごとに
監視します。変化がない限りAI workerを再起動せず、変化後に再診断します。
watch_pathsは診断が指定する相対pathで、絶対path・親参照・.git・symlink経由は拒否し、内容を読みません。
外部条件を直接観測できない場合は、その条件を反映する非秘密file等の観測対象を指定します。
OS停止中の実行はできませんが、同hostで次回start/runしたときに保存状態から再照合できます。

コードレビューは固定差分と比例したlocal検証を確認します。CI起動と成功はその後の段階です。
Rustはレビュー後に委任済みの既存labelだけをPR/head/baseと照合して付与し、最大5分の状態反映待ちを
挟んで実CIを観測します。CI未起動やadmission failureだけでコードレビューをblockedにしません。
CI状態が15分進まない場合は診断へ渡し、許可内の再実行・復旧と外部待機を区別します。

main CI失敗では後続mergeのbarrierを維持し、読み取り専用診断から元依頼内の修復task/PRを登録できます。
その修復taskだけは親barrierを通せます。元の失敗証跡を保持し、修復task完了後に修復mergeの成功・
固定head・元mergeからの祖先関係を再観測した証拠を別に保存してから、元taskのfinishへ進みます。
元のreceipt、PR head CI、review、live policy、main到達性、cleanupのhelper検証は維持します。
CI待ちにはCodexを起動せず、Rustが現在のPR・head・base・test merge commitとcheck/statusを照合します。
APIエラーはCI失敗と分けてbackoffし、check不足・cancel・timeout・skipのみを成功扱いしません。

### workflow不在repositoryのlocal gate

PR payloadの`base.sha`は古い場合があるため、対象branchのlive refを観測の前後で直接照合します。
live baseと固定headのcommit/treeをGitHub APIから取得し、両方の`.github/workflows/`直下に
YAMLが存在しないと確認した場合だけ`local-validation`を選びます。APIエラー、不完全なtree、
対象SHA不一致は不在へ変換せず待機し、どちらかにworkflowがあれば従来のstrict gateを使います。
local gateでも固定headに実在するcheck/statusと必須checkを確認し、test mergeの成功で失敗や未完了を隠しません。
実在checkのcancel・timeout・skipのみ、必須check不足でも統合workerを起動しません。

Review・Integration・Finishのworkerにはgate-modeと観測証跡を明示します。
固定headに比例したlocal検証とriskに必要なレビューを揃え、`record-review`・`deliver`・`finish`の
すべてへ`--gate-mode local-validation`を渡します。local検証の実行とreview receiptの記録はworkerが担当します。
strict時は既存parserの契約どおり`--gate-mode`を省略します。`strict`や`strict-ruleset`の明示指定はしません。
head/base一致、最新base、review receipt、未解決thread等の既存delivery gateは引き続きhelperが検証します。
追加pushやbase変更では旧CI・旧レビューを無効化し、現在のheadで実装・検証・レビューをやり直します。

merge後は実merge commitがlive mainの履歴に含まれることを確認し、そのcommitのworkflowと
実在check/statusを観測します。local gateの条件を満たしたときだけmain-ci待機からFinishへ進みます。
Finish起動直前にも再観測し、古い証跡やAPIエラーでは起動しません。mainのworkflow追加をlocalへ読み替えず、
finishの既存receipt・live main・固定headの検証とmanaged cleanupは維持します。

CPU、MemAvailable、disk空き容量を測り、実行枠を決めます。受付task数に固定上限はありません。
host-localの`.tasks/budget.json`でworkerごとのRAM予約量、hostに残すRAM、最低disk空きを調整できます。
既定はそれぞれ2048、1024、2048 MiBです。portやDB等の共有資源は`--resource`で排他し、
依存関係は`--depends-on`で記録します。単なるtext conflictの有無で互換性を保証しません。

CI待ちはAI worker枠を占有しません。実行可能なtaskを段階と待機時間で並べ、review・統合・後処理に
小さな優先度を付けつつ、長く待った実装にも順番を回します。各起動直前にもhost資源を再計測します。
ここでのRAM・diskはCodex実行hostの値です。CI runnerの余力・queue・完了見込を推測して選びません。
`list --json`の`waiting_on`はCI、依存、共有資源、host容量、統合barrier、外部観測、人間判断を区別します。
診断の後続taskはpolicy・risk・CI label・利用者の終了境界を継承し、同じ目的の再報告を二重登録しません。
実装段階で後続taskの先行待ちを指定すると親sourceの完了を待ち、診断中は独立した部分作業へ分けます。

## OS起動管理と現在の対応範囲

状態遷移の正本は単発照合です。`run`、`start`、将来のOS起動adapterは同じ処理を使います。
`launch-command --json`は起動に必要な実行file・引数・cwd・CODEX_HOMEを返すだけで、OS設定を変更しません。
coordinator自体の異常終了後やOS再起動後の自動起動が必要なら、systemd user service等の任意adapterを使えます。
この初期版はサービス・timer・linger・Windows起動設定を自動導入しません。

WSLのsystemd serviceはWSL instanceを生存させません。停止中は待機し、次回`start`／`run`で照合します。
OS起動adapterを使う場合も、WSL起動前からの処理継続は保証しません。
Linux user/PID namespace・pidfd等の必要機能はdoctorで確認します。利用不能なら安全性を落として実行しません。
worker内のhelperがPIDを照合できるよう、worker専用のmount namespaceで`/proc`をマウントします。
doctorはこのprocfsの作成も確認し、通常の短時間commandの隔離とは分けて診断します。
macOSのhost identity・process監督・launchd adapterは将来対応です。

main CIが存在するrepositoryと、前述の一次情報でworkflow不在を確認できるrepositoryを対象とします。
CI起動に独自承認labelが必要なrepositoryは、既存の委任と運用を守り、未知の承認を自動付与しません。
OS起動adapterの導入は今回の対応範囲に含みません。

## 未処理イベントと保持・回収

`events --task-id <ID> --json`は未ackイベントと現在の`event_sequence`・`acknowledged_sequence`を返します。
`--all`で保持中のack済みイベントも確認できます。内容を確認して必要な対応を済ませてから、
`ack --task-id <ID> --through <確認したsequence>`でそのsequenceまでを累積ackします。
表示やackによる削除はなく、確認後に届いた大きいsequenceは未処理のままです。未来のsequenceやackの巻戻しは拒否します。
旧stateにackがなければ、残っているイベントはすべて未処理として読み込みます。旧版が既に捨てた履歴は復元できません。

件数による自動破棄はありません。state全体の安全上限（1 MiB）を超える保存は明示エラーになり、
保存済みstate・worker結果を保持します。長期event archiveは未対応です。

```sh
codex-task retention --task-id issue-123 --seconds 2592000
codex-task collect --task-id issue-123 --json
```

保持期間は既定30日で、`retention`は秒単位の設定だけを保存します。自動削除はせず、`collect`で明示的に回収します。
回収は完了task、worker停止（生存lock取得）、全イベントackを必要とし、進行中・停止中・判断待ちや未ackがあれば拒否します。
期間を超えたack済みイベントと旧worker世代の既知fileだけを回収し、未知file・symlink・危険な権限は保持してエラーにします。
worker fileは更新時刻、イベントは発生時刻を使い、期限と同時刻のものは保持します。
現在世代と最新の結果がある世代は期限によらず残します。task本体・目的・checkpoint・レビュー・最新CI証跡も
期限によらず保持するため、一覧・依存関係・唯一の引継ぎを失いません。task自体の削除は提供しません。
タスク完了後のworktree/artifact回収は従来どおり既存delivery helperで行います。
新しいstate項目を旧binaryは読み込めないため、導入時はcoordinatorを停止して全helperを揃えて更新し、その後再開します。

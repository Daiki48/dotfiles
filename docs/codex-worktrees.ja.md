# Codex worktree運用ガイド

このリポジトリのCodex実装タスクは、専用コマンド `codex-worktree` で管理します。
worktreeの管理rootは、OpenAIのGit worktree運用に合わせて `$CODEX_HOME/worktrees` を
使います。`CODEX_HOME` が未設定の場合は `~/.codex` です。

## 適用範囲

実装・修正・追加・構築の依頼を実装へ進める時点で、親checkoutからworktreeを作成します。
調査、設計、レビュー、診断だけの依頼では作成しません。相談が実装依頼へ変わった時点で
作成してください。

この運用は、親checkoutを実装用に切り替える代わりに、タスクごとのcleanなworktreeを
用意するものです。作成時には `origin` の最新default branchをfetchし、そこから新しい
非保護branchを作ります。親checkoutのbranch、HEAD、index、working treeは作成前後で
変わらず、親checkoutがdirtyでも変更・破棄しません。

## 初回インストール

CLIとRust製hook/helperのrelease build・配布、`codex-autonomous` permission profileとmanaged workspace rootの設定・移行は次のコマンドで行います。何度実行
しても既存のローカル設定や認証情報を壊さない冪等な移行です。

```sh
cargo run -- codex
```

installerは`HOME`がpassword database上のcurrent account homeと一致し、`~/.local/bin`が
`PATH`に含まれることを事前検査します。不一致時は書き込み前に停止します。設定を反映するため、
実行後はCodexを再起動してください。同じmulti-call binaryをowner-onlyのregular fileとして
hookとhelperのcanonical pathへatomic installし、内容hashで更新を管理します。
profileはbuilt-in workspace権限を継承し、各workspace rootの`.git`だけを明示的にwriteへ
上書きします。旧`sandbox_workspace_write.writable_roots`も新profileへ保持移行し、`~`は検証済みの
account home、相対pathは移行実行directoryを基準に絶対pathへ正規化します。既存profileの通常table・
inline・quoted・dotted表現を正規化して保持し、legacy rootとの競合では新profileの明示値を優先します。
root path自体も`~`・相対・末尾slash・実在symlinkを同じ基準で正規化します。同値pathに矛盾する
profile値、不正なcontainerや値の型は黙って選ばず停止します。
このpath権限はprocessを限定しません。managed PreToolUse hookは通常のGit、GitHub、helperと明示的な
破壊操作を検査する運用guardであり、任意programを敵対的に封じ込めるsandboxとは扱いません。
helperをcanonical pathから起動できることを確認できます。
private helperの導入・更新中に中断した場合は、owner-onlyのpending journalと内容hashが示す
到達可能な途中状態だけを次回setupで再開し、不整合なfileやstateは変更せず停止します。

```sh
codex-worktree --help
codex-delivery --help
```

## 標準手順

以降のhelperコマンドは、対象repositoryの親（main checkout）から実行します。既定branch
以外のcheckoutや、すでに作成したlinked worktreeから、別worktreeを作成・診断しないで
ください。

### 作成

Issueをtask IDにする場合:

```sh
codex-worktree create --issue 22
```

明示したtask IDとbranchを使う場合:

```sh
codex-worktree create --task-id task-api-refresh --branch feat/api-refresh
```

`--issue N` と `--task-id task-...` は同時に指定できません。両方を省略すると安全な
timestampベースの `task-...` IDが生成されます。branchを省略すると `feat/<task-id>` が
生成されます。Issue番号は1以上、task IDは `issue-<番号>` または `task-` で始まる安全な
IDにしてください。branchは `feat/`、`fix/`、`docs/` などの許可された作業用prefixを
使い、保護branchや既存branchを指定しないでください。

コマンドは作成したworktreeの絶対pathを標準出力へ返します。安全hookは`create`をshellの
command substitutionやchain内で実行することを拒否するため、helperは必ず単独で実行し、
返されたpathを次の操作の`workdir`として明示して実装、testを行います。Codex hookの`cwd`はsession開始directoryを示すため、Git書き込みでは`git -C <返された絶対path> ...`として対象worktreeもcommand内で明示します。launcher環境に`SSH_ASKPASS`がある場合は、外部command実行の境界を明確にするため`env -u SSH_ASKPASS git -C ...`の正規形で実Gitから除去します。

privateなGitHub HTTPS originでは、`codex-worktree`が既存のGit credential設定とtoken環境変数を隔離し、
owner-onlyの一時GH設定と固定したsystem `gh auth git-credential`だけで認証します。tokenをURL、引数、manifest、
logへ展開しません。SSH origin、GitHub以外のHTTPS origin、不正形式やcredentialを含むURLにはこのhelperを
追加せず、従来どおりfail-closedにします。

Draft PR作成時は`--head`を必ず明示します。guardはそのbranchを所有する登録済みworktreeを解決し、同一repository、clean、push済みHEADであることを確認してから`gh pr create --draft`を許可します。

```sh
codex-worktree create --issue 22
# 出力例: /home/user/.codex/worktrees/5-owner--4-repo/issue-22
```

作成されるpathは次の形式です。

```text
$CODEX_HOME/worktrees/<length-owner--length-repo>/<task-id>
```

`CODEX_HOME` 未設定時は
`~/.codex/worktrees/<length-owner--length-repo>/<task-id>`です。repository名は
`owner/repo`を小文字化し、各segmentの文字数を付けることで、区切り文字を含む名前同士も
曖昧にならない管理keyにします。taskごとのmanifestはrepository管理rootの
`.state/<task-id>.json`、lifecycle lockは`.locks/lifecycle.lock`に保存され、Git
worktree自体もlocked状態で保持されます。これらのmetadataやlockを作業用ファイルとして
編集・移動しないでください。

### 一覧・診断

親checkoutへ戻り、管理対象を一覧表示します。

```sh
codex-worktree list
codex-worktree doctor
codex-worktree doctor --task-id issue-22
```

出力は `task ID<TAB>状態<TAB>詳細` の形式です。代表的な状態は次のとおりです。

- `ready`: cleanで再開可能
- `dirty`: commit前または未追跡の変更あり。変更を保持したまま再開可能
- `interrupted`: worktree作成完了前に処理が中断。`recover`による再検証が必要
- `diverged`: 中断後にworktreeのHEADが作成時baseから変化。自動復旧不可
- `missing`: worktree directoryがない
- `unregistered`: Gitのworktree登録がない
- `branch-mismatch`: manifestのbranchと現在のbranchが異なる
- `invalid` / `failed`: manifestまたは作成処理に問題がある

`doctor` は全対象が `ready` の場合に成功し、それ以外の状態があれば異常終了します。
診断は変更を自動修復せず、dirtyな変更、未commit、未pushをclean/reset/削除しません。

### 再開

異常終了、端末再起動、作業場所を見失った場合は、まず一覧と診断を確認してから再開pathを
取得します。

```sh
codex-worktree list
codex-worktree doctor
codex-worktree resume --task-id issue-22
cd "$(codex-worktree resume --task-id issue-22)"
```

`resume` が返すのはpathだけで、directoryの移動や変更のclean化は行いません。`ready` と
`dirty` のworktreeだけが再開対象です。`missing`、`unregistered`、`branch-mismatch`、
`invalid`、`failed` の場合は、該当taskのmanifest、Git登録、branch、作業内容を保全した
まま停止し、手動で削除・作り直しをしないでください。locked worktreeの復旧やcleanupは
metadataとの不整合や変更消失を招くため、対象pathとtask IDを確認してから別途判断します。

作成済みworktreeの登録・branch・path・common Git dir・作成時HEADがすべて一致する一方、
manifestが`creating`のまま残った場合だけ、`doctor`は`interrupted`を返します。次の
単独commandで同じ安全条件をlock内でもう一度検証し、manifestを`ready`へ進められます。

```sh
codex-worktree recover --task-id issue-22
codex-worktree resume --task-id issue-22
```

`recover`はworktreeを作り直さず、reset、clean、削除を行いません。それ以外の異常状態は
変更せずに拒否します。

## 状態とライフサイクル

作成処理はおおむね `creating` → `ready` の順でmanifestを更新します。作成中に失敗した
場合は`failed`と詳細を記録します。強制終了で`creating`が残った場合は、上記の`recover`
だけが検証済み状態へ進めます。helperは同じrepository管理rootのlifecycle lockを取得して、
同じtask ID、path、branchの競合を防ぎます。

PRが未mergeの間はworktreeを自動削除しません。`codex-worktree`は作成・診断・再開だけを
担当し、Ready化、merge、main同期、finishのcleanupは専用`codex-delivery`へ委ねます。
`gh pr merge`や`git worktree remove/prune`などの直接delivery・cleanupは禁止です。
未commit・未push・dirtyな変更を破壊する操作も実装しません。失敗、timeout、pending、dirty、
stale、conflict、判定不能の場合はPR・branch・worktreeを保持し、`list`と`doctor`で再開点を
確認します。

`codex-delivery finish`が、管理root内の対象についてrepository、task、branch、PR、merged状態、
head commitのmain到達性、clean、未pushなしを厳格に証明できた場合だけmanaged cleanupを許可します。
これは任意削除の許可ではありません。管理root外や証明できない対象の削除は、Daikiの確認を得て
従来どおり`.codex-trash/<timestamp>/`へ退避し、直接削除しないでください。deliveryの固定SHA、
review、CI、Ruleset、risk確認は[Codex delivery運用ガイド](codex-delivery.ja.md)を正本とします。

## rollback

運用を旧single-checkout方式へ戻す場合は、Daikiが明示的に
`CODEX_WORKTREE_MODE=single-checkout`を設定してから`execute-plan`を使い、親checkoutを
使う方式へ戻します。rollbackでも既存worktree、branch、manifest、lockは保持します。
既存taskの変更やPRを失わせるためにreset、clean、branch削除、worktree削除を行わないで
ください。

## deliveryとの境界

Draft PR後のdeliveryは、専用`codex-delivery` helperの次の経路だけを使います。
各commandでは`--task-id`、`--pr`、`--head`、`--plan-id`、`--plan-version`を明示します。review記録では
`--risk`、`--tests-passed`を必須とし、high/criticalは`--independent-review-passed`、criticalで
別の高リスク境界をreviewした場合だけ`--specialist-review-passed`も指定します。

```text
(autonomous: record-review | human-required: approve-review)  ->  deliver  ->  finish
```

`record-review`は、repository、PR、固定head SHA、Plan ID、risk分類、testとriskに応じたreviewの完了証拠を
receiptとして記録します。riskとは別にdecision requirementを判定し、仕様・既存権限・rollback・検証を
確定できる場合は全riskで`record-review`を使います。Daikiだけが決められる事項がある場合は明示回答後に
`approve-review`を使い、技術gateが不明または失敗ならblockedとしてreceiptを作りません。receiptと
現在のSHAが一致し、delivery直前に再取得したGitHub Actions checkが`success`・`skipped`・`neutral`
（workflow不在の`local-validation`では固定SHAのlocal検証完了）、actionable=0、未解決thread=0、選択したremote gateが成立した
場合だけ`deliver`へ進みます。

既定はlive Rulesetを要求するstrict modeです。GitHub Free/private repositoryでは、`record-review`
または`approve-review`、`deliver`、`finish`へ`--gate-mode github-free-private`を明示します。
このmodeは保証差によりriskをhigh/criticalとし、Rulesetの代わりにlive private repository identityと
decision receiptを検証します。403やnetwork
errorから自動fallbackせず、modeを省略した既存commandとv1 receiptはstrictとして扱います。

hosted/self-hosted CIを使わない方針をDaikiが明示承認した場合だけ、high/criticalの
`approve-review`、`deliver`、`finish`へ`--gate-mode github-free-private-local`を明示できます。
このmodeはPRのbaseと固定headの双方にworkflow YAMLがないこと、private repositoryとPR/reviewのlive検証を要求し、
`required-ci`だけをlocal testのreceiptへ置き換えます。workflow YAMLがあれば`runs-on`のrunner種別に
従って通常CIを使い、`record-review`やCI failureからの自動fallbackには使えません。

PRのbaseと固定headの双方にworkflow YAMLが存在しない通常のrepositoryでは`--gate-mode local-validation`を使い、product固有の
format、lint、型検査、test、buildから該当するlocal検証を固定headへ記録します。CI不在だけを理由に
human approvalやrisk引き上げを要求せず、workflowが存在するbase/headやCI failureからは選択しません。固定headにGitHub Actions checkが存在する場合は全件の完了と成功系conclusionも必須です。

確認待ち、blocked、delivery途中の異常で、PR、branch、worktreeを自動cleanupしません。

## 安全境界

- 作成・診断・再開・recoverのworktree lifecycle書き込みは `codex-worktree` に限定します。
  merge後のmanaged cleanupだけは`codex-delivery finish`が厳格な証明後に実行します。`git worktree
  add/remove/prune`などを直接実行して状態を合わせようとしないでください。
- managed cleanupは既知の再生成可能なignored directoryをworktreeとともに破棄できます。
  `.codex-trash`、未知のignored artifact、tracked/untrackedの変更があるworktreeは保持して停止します。
- active、未merge、dirty、判定不能なworktreeには容量・件数・経過日数によるcleanupや作業制限を
  適用しません。cleanup判定は、merge後の`codex-delivery finish`で終了条件を証明した対象だけに限定します。
- 親checkoutのbranch切り替え、index、working treeを変更しないことを作成前後に検証します。
- `origin` のfetch/push先とrepository identityを検証し、最新default branchのOIDを確認して
  からbranchを作成します。
- manifest、lock、認証情報、session情報、local databaseを公開repositoryへcommitしないで
  ください。
- `CODEX_HOME`自身と既存の親componentにsymlinkを使わず、helperが検証した管理rootだけを
  使用してください。
- pathやtask IDを手入力で組み替えず、helperの出力と `resume` の結果を使ってください。

## 対象外

Issue #22では、worktree間のbuild output、port、test database、cache、その他のruntime
resource分離は扱いません。task間でこれらを分離する仕組みは別途設計・実装が必要です。
release、protected branchへのpush、任意のworktreeやbranchの削除は、この運用の自動処理には
含めません。mergeとmanaged cleanupは`codex-delivery`のlive gateを通った場合だけ行います。

関連するOpenAI公式のmanaged worktree rootの説明は、[Git worktrees](https://learn.chatgpt.com/docs/environments/git-worktrees)
を参照してください。

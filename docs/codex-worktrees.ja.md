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

CLIとhelperの配布、managed writable rootの設定・移行は次のコマンドで行います。何度実行
しても既存のローカル設定や認証情報を壊さない冪等な移行です。

```sh
cargo run -- codex
```

設定を反映するため、実行後はCodexを再起動してください。helperがPATHから見えることを
確認できます。

```sh
command -v codex-worktree
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
返されたpathを次の操作の`workdir`として明示して実装、testを行います。Codex hookの`cwd`はsession開始directoryを示すため、Git書き込みでは`git -C <返された絶対path> ...`として対象worktreeもcommand内で明示します。

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

PRが未mergeの間はworktreeを自動削除しません。`remove`、`prune`、branch削除、自動cleanup
はこのIssueの対象外です。未commit・未push・dirtyな変更を破壊する操作も実装しません。
不要になったworktreeがあっても、まず作業内容とPRの状態を確認し、対象を明示した別の
運用判断なしにdirectoryやGit metadataを削除しないでください。

## rollback

運用を旧single-checkout方式へ戻す場合は、Daikiが明示的に
`CODEX_WORKTREE_MODE=single-checkout`を設定してから`execute-plan`を使い、親checkoutを
使う方式へ戻します。rollbackでも既存worktree、branch、manifest、lockは保持します。
既存taskの変更やPRを失わせるためにreset、clean、branch削除、worktree削除を行わないで
ください。

## 安全境界

- lifecycleのGit書き込みは `codex-worktree` に限定します。`git worktree add/remove/prune`
  などを直接実行して状態を合わせようとしないでください。
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
merge、release、protected branchへのpush、worktreeやbranchの削除も、この運用の自動処理
には含めません。

関連するOpenAI公式のmanaged worktree rootの説明は、[Git worktrees](https://learn.chatgpt.com/docs/environments/git-worktrees)
を参照してください。

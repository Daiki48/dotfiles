# disk cleanup運用

`cargo run -- clean-disk`は、プロジェクト名をdotfilesへ列挙せず、設定した開発rootとCodex managed
worktree rootを1回だけ走査します。daemon、常駐監視、controllerの自動停止は行いません。

## 判定

- `.codex-trash`直下のentryは、tree全体の最新mtimeから3日経過し、対応repositoryまたはtrashを使用中の
  processがなければ`y/N`で確認します。退避データは自動削除しません。
- `Cargo.toml`直下の`target`は、tree全体の最新mtimeから30日経過し、repositoryを使用中のprocessが
  なければ自動削除します。削除後の初回buildでは再コンパイルが発生します。
- open中のpath、最近更新されたpath、走査上限超過、filesystem境界、特殊file、検証中に内容が変わった
  pathは削除しません。active判定は`y`でも上書きできません。
- symlinkは辿りません。削除直前にdevice、inode、entry数、割当byte、最新mtimeを再検証します。
- 削除候補がある場合、open pathの確認は非root実行でも`sudo lsof`を使用します。認証できない場合や、走査対象と
  重なるfilesystemを`lsof`が確認できない場合はfail closedで停止し、削除しません。

設定は`.config/clean-disk.json`です。`scan_roots`にはプロジェクトそのものではなく、将来のプロジェクトも
配置される親directoryだけを指定します。Codex worktree rootは`CODEX_HOME`または`~/.codex`から自動で
追加されます。走査root同士、またはCodex worktree rootと包含・重複する設定は拒否します。

```bash
cargo run -- clean-disk --dry-run
cargo run -- clean-disk
```

非対話環境では`y/N`対象を適用しません。`--dry-run`は自動削除も含め、候補表示だけを行います。
Codex managed worktreeは本体もbuild cacheも走査せず、trashだけを確認候補にします。worktreeの終了時削除は
`codex-delivery finish`、ChatGPT desktop appが所有するworktreeのsnapshotと削除はCodex自身に任せます。

## Self-hosted runner adapter

runner imageのcanonical、rollback、recovery phase、controller、VM、operation lockはプロダクトごとに契約が
異なります。共通CLIはqcow2を直接探索・削除せず、各プロダクトが所有するcleanup adapterのJSON reportだけを
検証します。

manifestはmachine-localな
`~/.config/runner-storage-cleanup/targets.d/<target-id>.json`へ配置します。dotfilesには固有ID、
repository、storage pathをcommitしません。

```json
{
  "schema_version": 1,
  "id": "<target-id>",
  "working_directory": "/absolute/path/to/repository",
  "storage_root": "/absolute/path/to/runner-storage",
  "command": {
    "program": "/usr/bin/make",
    "args": ["--no-print-directory", "runner-image-cleanup"],
    "apply_args": ["APPLY=1"]
  }
}
```

manifest directory、manifest、working directory、storage rootは現在user所有でgroup/world writableではない
通常pathに限定します。programは安全な`/usr/bin`直下のsystem executableだけを許可します。IDとfilenameは
一致が必要で、未知field、symlink、hard link、過大な入力を拒否します。安全なmanifest directoryが空の場合、
`clean-disk`はrunner cleanupをskipします。

adapterのstdoutは次のJSON契約です。監査は`applied=false`、適用は`applied=true`を返します。

```json
{
  "applied": false,
  "candidate_count": 1,
  "reclaimable_bytes": 1073741824,
  "candidates": [
    {
      "kind": "candidate-b",
      "path": "/absolute/path/to/runner-storage/base/candidate.qcow2",
      "recovery_key": "completed-recovery-key"
    }
  ]
}
```

候補pathは登録したstorage root内だけを許可します。`clean-disk`はrunner候補を自動削除せず`y/N`で確認し、
adapterを監査、適用、再監査の順で実行します。adapterは適用直前にもcontroller停止、busy/provisioning=0、
VM reconcile、runtime canonical、recovery phase、operation lock、候補不変性をfail closedで再検証する必要が
あります。共通CLIはserviceを停止・再起動しません。

adapterだけを監査・適用する場合は次を使います。

```bash
runner-storage-cleanup audit
runner-storage-cleanup audit --target <target-id>
runner-storage-cleanup apply --target <target-id>
```

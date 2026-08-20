# Codex guardrailのRust実装

## Pythonで実装されていた理由

従来のhookとhelperは、Python標準ライブラリだけで素早く安全規則を追加でき、単一scriptをそのまま
`$HOME`へ配置できるためPythonで実装していました。これはこのrepositoryでの実装上の選択であり、
Codexの言語要件ではありません。Codex hookの外部契約は、commandへのJSON入力、JSON出力、標準エラー、
終了code、timeoutです。`codex-worktree`と`codex-delivery`も独自CLIなので、Python固有APIは使っていません。

## Rust版の構成

`packages/cli`をmulti-call binaryとしてrelease buildし、同じ実行fileを次の名前でprivateなregular fileへ
atomic installします。起動時は実行file名で処理を分岐します。

- `~/.codex/hooks/block-git-write`
- `~/.local/bin/codex-worktree`
- `~/.local/bin/codex-delivery`

hookのallow/deny、helperのCLI、worktree manifest、delivery receipt v1/v2/v3と再開stateは従来の外部契約を
維持します。設定移行では旧Python hookの定義を取り除き、Rust binaryを直接実行する定義へ置き換えます。

## エラー処理と停止

入力、path、schema、repository identityを検証できない場合は処理を続行せず、hookはdeny、helperは非0で
終了します。外部commandは標準出力と標準エラーを同時に回収し、それぞれ4 MiBを上限とします。deadline
または上限超過時はchildだけでなくprocess groupを停止し、`wait`で回収してからerrorを返します。
managed file、manifest、receipt、stateは一時fileの同期とrenameを使い、途中状態を明示的に再開または拒否します。

## 性能と品質保証

通常経路ではPython interpreterを起動せず、release binaryを直接起動します。CIはRust unit/integration test、
Ruleset契約test、`cargo fmt`、warningをerrorにする`cargo clippy`を実行します。hookのJSON・終了code・安全判断、
helperのmanifest/receipt/state互換性、timeout、出力上限、改竄時のfail-closedをtest対象とします。

ローカル更新は次で行います。

```console
cargo run -- codex
```

setupは`packages/cli`を`--release --locked`でbuildしてから3つの配置先を同一hashへ更新します。

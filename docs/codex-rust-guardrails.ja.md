# Codex guardrailのRust実装

## Pythonで実装されていた理由

従来のhookとhelperは、Python標準ライブラリだけで素早く安全規則を追加でき、単一scriptをそのまま
`$HOME`へ配置できるためPythonで実装していました。これはこのrepositoryでの実装上の選択であり、
Codexの言語要件ではありません。Codex hookの外部契約は、commandへのJSON入力、JSON出力、標準エラー、
終了code、timeoutです。[Codex Hooks公式仕様](https://learn.chatgpt.com/docs/hooks)もhookを外部commandとして
定義しており、実装言語をPythonに限定していません。`codex-worktree`と`codex-delivery`も独自CLIなので、
Python固有APIは使っていません。

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
または上限超過時はchildだけでなくprocess groupを停止し、`wait`で回収してからerrorを返します。Linuxでは
検証済みsystem `unshare`で外部commandごとにuser/PID namespaceを作り、namespaceのPID 1終了時に
`setsid`や環境変数解除で離脱した子孫もkernelが停止します。補助追跡のsignalはpidfdを使い、PID再利用競合を
避けます。capture readerにもnon-blocking cancelとhard deadlineを設けています。unprivileged user namespaceを
利用できないLinux、および同等のprocess tree隔離を実装していない非Linuxでは、安全な子孫停止を保証できないため
release binaryは外部commandを実行せず停止します。CIのunit test binaryだけは、namespace非対応hostでもfixtureを検証できるよう、
process group・subreaper・run markerによるtest専用fallbackを使用します。
managed file、manifest、receipt、stateは一時fileの同期とrenameを使い、途中状態を明示的に再開または拒否します。
Git実行時はsystem/global設定と環境変数を隔離し、repository-localの外部実行・transport変更設定を検出した場合も
停止します。平文HTTPのGitHub remoteと`file`/`git`/`ext` transportを拒否し、TLS検証を固定します。SSHは
`-F /dev/null`でuser設定を隔離します。GitHub CLIは認証情報だけをprivateな一時設定へsnapshotし、
proxyや`http_unix_socket`などの既存設定を引き継ぎません。browser・editorを起動するoptionも拒否します。
`--body-file`は同じfile descriptorから検査した内容をprivate snapshotへ固定してから実行し、内容を安全に検査できない
binary差分はcommit・push前に拒否します。

`codex-worktree`は安全境界としてglobalの`credential.helper`を無効化します。そのためprivate HTTPS remoteで
credential helperだけに依存する構成はfail-closedになります。private repositoryでは検証可能なSSH originを使うか、
helperに依存しない認証経路を事前に用意してください。`codex-delivery`のHTTPS認証は固定したsystem `gh`の
`auth git-credential`だけを使用します。

## 性能と品質保証

通常経路ではPython interpreterを起動せず、release binaryを直接起動します。CIはRust unit/integration test、
release build、Ruleset契約test、`cargo fmt`、warningをerrorにする`cargo clippy`を実行します。hookのJSON・終了code・安全判断、
helperのmanifest/receipt/state互換性、timeout、出力上限、改竄時のfail-closedをtest対象とします。

ローカル更新は次で行います。

```console
cargo run -- codex
```

setupは`~/.local/bin/codex`、`~/.cargo/bin/cargo`、Rust compilerの所有者・mode・symlink target・祖先を検証し、
PATHやbuild overrideを隔離します。Codex CLIの自動npm installは行わないため、先に固定pathへ導入してください。
config、symlinkの全親component、Cargo config、legacy state、managed binary transactionをread-onlyでpreflightした後、
setup全体のdurable resume journalを開始します。`packages/cli`をbounded process runnerで`--release --locked` buildし、
3つの配置先を同一hashへ更新します。途中失敗時はjournalと各file transactionを次回setupで再開します。

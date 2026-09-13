# Codexからローカルgpt-ossを使う

親はGPT-6 Astra、native explorer/reviewerはLuna xhighを維持し、限定した読み取り作業をOllamaのgpt-oss:20bへ渡す。Qwenへの自動振り分けは行わない。Codex CLI 0.154.0ではrole設定だけで親と異なるmodel_providerを適用できなかったため、native subagentではなく固定loopback APIを呼ぶPython helperを使用する。

## 役割と推論段階

| role | 用途 | 既定 |
|---|---|---|
| extract | 原文の識別子を指定して該当行を選ぶ | low |
| summarize | 局所的な挙動を一文で説明 | medium |
| inspect | Rustの具体入力・失敗条件・局所変更案に関する主張を真偽判定 | medium |

inspectは質問内に判定する主張を順序付きで明示し、--checks 1〜12で判定数を指定する。比較測定と同じ行番号付きコード・decisions・explanation・evidence形式を使い、自由形式の読解へ広げない。説明は300文字を推奨し、helperは1200文字・引用12件を上限として検証する。

--effort low/medium/highで呼出しごとに変更できる。highはユーザー明示時だけ選び、自動昇格しない。親の/modelはローカルhelperの推論段階を変更しない。

| effort | 生成上限（思考を含む） | 時間上限 |
|---|---|---|
| low | 2048 | 60秒 |
| medium | 4096 | 90秒 |
| high | 8192 | 180秒 |

入力は1ファイル・160行・8000 UTF-8 bytes以内、質問1600 bytes以内、contextは16384に固定。上限は運用上の制限であり、全ての入力の精度を保証する値ではない。
探索先不明、横断判断、大きな入力、全体設計、複雑な原因解析は最初からLunaへ渡す。上限のために横断判断を分割しない。変更と独立reviewはローカルへ任せない。単純なrgなら親が直接行う。

## 実行

    python3 ~/.codex/local-model/run.py --cwd /home/daiki/dotfiles --file packages/cli/src/neovim.rs --start 7 --end 8 --role extract --question MIN_TREE_SITTER_CLI_VERSIONの設定行

返り値のmodel、effort、status、findings、usage、elapsed_secondsで利用モデルと結果を確認できる。行番号と引用はhelperが原文から再構成する。引用された内容が主張を裏付けるかは親が確認する。
不正な入力・出力、busy、timeout、生成上限、接続エラー、abstainはstatus=fallback、exit 2。helper自身はクラウドを呼ばず、親がLuna xhigh explorerへ引き継ぐ。同じ依頼をローカルで繰り返さない。成功した結果をLunaへ二重委譲しない。

固定127.0.0.1:11434以外へ接続せず、proxyとredirectを拒否する。会話履歴、ツール、認証情報を送らず、モデル出力のコードは実行しない。秘密情報を資料から除く最終責任は呼び出す親が持つ。

## 新規セッションへの反映

main統合後の正本はdotfiles checkoutの.codex/AGENTS.mdと.codex/agents/local-modelである。~/.codex/AGENTS.mdは前者、~/.codex/local-modelは後者へのsymlinkとする。config.toml、認証、親モデル、native agent設定は変更しない。

通常のdotfiles checkoutで初期設定を行う場合：

    python3 .codex/agents/local-model/activate.py
    python3 .codex/agents/local-model/activate.py --activate

1行目は事前確認だけ。既存のAGENTS.override.mdや別のlocal-model pathがあれば停止して保持する。新しく起動するcodexがglobal AGENTSを読み込む。既存・再開セッションへの反映は保証しない。

worktreeでactivate.pyを実行した場合は、そのworktreeを参照する一時的な有効化となる。有効化中のworktreeは削除しない。mainへの統合・同期時には、現在のリンク先がそのtaskであることを確認し、旧リンクを~/.codex/local-model-backupsへ保存してから通常checkoutへ参照先を戻す。別のリンクやregular fileを上書きしない。過去のbackupは自動削除しない。

Neovimでの編集：

    nvim ~/.codex/local-model/settings.config ~/.codex/local-model/roles.json

## 検証と限界

    python3 -B .codex/agents/local-model/test_run.py
    python3 -B .codex/agents/local-model/evaluate.py

前者は入力制限、effort、根拠再構成、未完了拒否、停止、activationの非破壊性を検証する。後者は実モデルを使う小さい固定例であり、キーワード採点のため人による意味の確認も必要。

過去の独立した比較測定（/tmp/dotfiles-reasoning-eval-20260913/report.md）ではmediumが6課題35判断を正答したが、これは同じprompt/形式に限定した結果。helperで質問の書き方を変えたTOML読解では誤答も観測した。成功statusは意味の正しさを保証しない。inspectでは具体的な入力文字列と条件を明示し、親が根拠を確認する。highは4Kで未完了が多く、8Kで完了した1課題もmediumより遅かった。

### 構文解釈の除外

TOML読解は同じ具体的な質問でも再測定で誤答が再発したため、現行のinspect/summarize対象から除外した。TOML・引用符・構文パーサの意味解釈は最初からLunaへ渡す。helperも入力内のtoml/DocumentMut/引用符/クォートを検出して推論前にsyntax_semantics_requires_lunaを返す。これは保守的な品質フィルタで、全てのパーサを識別する安全境界ではない。extractの文字列抽出は引き続き許可する。eval_casesのtoml_keysは正答テストとして成功扱いせず、推論前のLuna振り分けが期待値。実モデル3例と、この振り分け1例を区別して集計する。

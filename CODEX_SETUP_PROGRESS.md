# Codex 自律モード設定 作業記録（2026-05-30）

## ステータス

**ほぼ完成。** 端末非依存の設定（profile v2 / モード判定 / rules / hook / テンプレート分離 / モデル統一）は
すべて dotfiles で管理され、ビルド・構文検証済み。
**残るは対話 `codex` でしか実施できない最終確認 2 件のみ**（後述「残課題」A・B）。

## 背景・目的

Daiki の要望:

- Codex を「危険コマンド以外は自動・安全寄り」に動かしたい（Claude Code の auto mode 相当）
- 教師モード / 自律モードを使い分けたい
- **Git 書き込み系（push / pull / fetch / commit 等）はどの場面でも絶対に実行させない**（プロンプトではなく設定で確実にガード）
- 端末非依存の設定は dotfiles で管理、端末依存（projects の trust 等）は管理外

環境: Codex CLI **v0.135.0**（npm / mise 管理）

---

## 判明した「自動で動かなかった」真因（2つ）

1. **legacy profile の廃止**
   v0.134 以降、`config.toml` 内のインライン `[profiles.*]` 形式は廃止された。
   Daiki の `[profiles.teacher]` / `[profiles.autonomous]` がこれに該当し、
   `--profile autonomous` を指定すると **エラーで起動できず**、base の `read-only` にフォールバックして毎回確認が出ていた。
   （実証エラー: `--profile autonomous cannot be used while config.toml contains legacy [profiles.autonomous]`）

2. **AGENTS.md が profile 名でモード判定していた**
   モデルは起動 profile 名を認識できない（`environment_context` に cwd/shell/date はあるが **profile 名は無い**）。
   そのため「profile でモードを決める」ルールが機能せず、モデルは常に「デフォルト = 教師モード」を選び、**編集を拒否**していた。
   → モデルは **sandbox mode（workspace-write 等）は認識できる** ことが判明したので、判定基準を sandbox mode に変更して解決。

---

## 実施・完了した変更

### 1. profile v2 へ移行（端末非依存・dotfiles 管理）

| ファイル | 内容 | 配置 |
|---|---|---|
| `~/dotfiles/.codex/teacher.config.toml` | read-only / on-request / **gpt-5.5** | `~/.codex/` へ symlink |
| `~/dotfiles/.codex/autonomous.config.toml` | workspace-write / on-request / **gpt-5.5** / network_access=true | `~/.codex/` へ symlink |

- base `~/.codex/config.toml` から legacy `[profiles.*]` を削除済み。
- **実証済み**: `codex exec --profile autonomous` のヘッダで `sandbox: workspace-write` を確認。profile v2 は正しく読まれる。
- 注意: `codex sandbox -p <profile>` は検証ツールで sandbox_mode を反映しない。profile 検証は `codex exec` で行うこと。

### 2. AGENTS.md のモード判定を sandbox mode ベースに変更

- `~/dotfiles/.codex/AGENTS.md`（既存 symlink）の「モード切り替え」節を改訂。
- **read-only → 教師モード / workspace-write・danger-full-access → 自律モード**。
- プロンプトでの「自律モード」明示は不要に。
- **実証済み**: 修正後、autonomous で同じ英語プロンプトに対しモデルが「自律モード」と名乗りファイルを作成。

### 3. rules（コマンド単位制御）を dotfiles 管理

- `~/dotfiles/.codex/rules/default.rules`（`~/.codex/rules/default.rules` へ symlink）。
- git 書き込み系 = `forbidden` / `rm`・`rmdir`・`mv` = `prompt` / git 読み取り系 = `allow` / 既存 allow（curl, npm install, cargo 等）維持。
- **実証済み**: `codex execpolicy check` で 42 ケース全て期待どおり。
- ⚠️ ただし rules の `forbidden` は approval=never では効かない（下記「重要な制約」参照）。

### 4. Git ガード hook

- `~/dotfiles/.codex/hooks/block_git_write.py` … git 書き込み系コマンドを `permissionDecision: deny` でブロックする PreToolUse hook。
  - **単体ロジックは全ケース合格**（push/commit/fetch/pull/reset/add/`git -c k=v commit`/`echo hi && git push` → DENY、status/log/diff/show/branch/`git log --grep=push`/一般コマンド → allow）。
  - deny は **stdout JSON（permissionDecision: deny）+ stderr 理由 + exit code 2** の三重で表現（公式仕様: exit 2 = block）。
    旧実装は exit 0 で、JSON が読まれなかった場合に素通り（fail-open）するリスクがあったため、exit 2 で **fail-close** 化済み（code-reviewer 指摘を反映）。
- base `~/.codex/config.toml` に `[[hooks.PreToolUse]]`（matcher=`^Bash$`）を登録。hooks feature は `stable` / 有効。
- 🟡 base `~/.codex/config.toml` の `[hooks.state]` に **trusted_hash が登録済み**（= `/hooks` での trust 操作は実施された形跡あり）。
  ただし「現在の hook 定義とハッシュが一致し、実際に `git push` が deny されるか」の最終確認は対話必須（残課題 A）。

### 5. base config テンプレートを config.base.toml に分離（今回）

- **真因（追加で判明）**: Codex は **trusted project の `.codex/config.toml` を project config として読み、`--profile` の値を上書きする**
  （公式 Config Reference: 優先順位は `project config > profile`、closest wins for trusted projects only）。
  base config の `[projects."<dotfiles path>"] trust_level = "trusted"` のため、
  `~/dotfiles/.codex/config.toml`（read-only）が `~/dotfiles` 内での `--profile autonomous` を read-only に上書きしていた。
- 一方この同じファイルは install CLI（`packages/cli`）の `copy_if_not_exists` で
  **新端末の `~/.codex/config.toml` テンプレート**としても使われており、2 役割を兼任していた。
- **対応**: テンプレートを `~/dotfiles/.codex/config.base.toml`（端末非依存）に分離し、旧 `.codex/config.toml` を削除。
  - Codex が project config として読むのは `.codex/config.toml` のみ。`.codex/config.base.toml` は読まれない
    → dotfiles 内で profile がそのまま効くようになった。
  - `packages/cli/src/codex.rs` の `CODEX_COPY_FILES` を `(.codex/config.base.toml → .codex/config.toml)` に変更。**ビルド成功**。
  - テンプレートは最新化（model=gpt-5.5 + hook 定義込み）。端末依存の `[projects.*]` / `[hooks.state]` / `[tui.*]` は含めない。
  - `README.md` の codex セクションを上記に合わせ更新。

### 6. モデル統一（今回）

- base config が `gpt-5.4 → gpt-5.5` にマイグレート済みだったのに対し、profile 群が gpt-5.4 のままだった不整合を解消。
- teacher / autonomous / config.base.toml すべて **gpt-5.5** に統一。

---

## 検証結果サマリ

| 項目 | 結果 |
|---|---|
| profile v2 で `--profile` 起動 | ✅ エラーなく動作 |
| autonomous で自律編集 | ✅ 実証（ファイル作成・「自律モード」自己認識） |
| sandbox mode 判定で 教師/自律 自動切替 | ✅ 実証 |
| rules forbidden（execpolicy ツール） | ✅ 42 ケース合格 |
| hook 単体（deny=exit2+JSON+stderr / allow=exit0） | ✅ 全ケース期待どおり |
| TOML 構文（base/teacher/autonomous） | ✅ tomllib パース OK |
| install CLI（config.base.toml 分離） | ✅ `cargo build` 成功 |
| project config 上書き問題 | ✅ 解消（config.toml 削除で profile が効く） |
| rules forbidden（実 exec / approval=never） | ❌ 効かない（仕様。hook で補完） |
| hook による git ブロック（実 deny） | 🟡 trusted_hash 登録済み・最終動作確認は対話必須（残課題 A） |

---

## 重要な制約（学び）

- **rules の `forbidden` は `approval=never`（exec 等）では評価されない**。承認（escalation）フローが走らないと効かない。対話（on-request）では効く見込み（残課題 B で確認）。
- **hook は trust 登録が必須**。`/hooks`（対話）で信頼するまでスキップされる。trusted_hash は登録済みだが現定義との一致確認が必要。
- **`codex exec` は approval を強制的に `never` にする**。対話の on-request 挙動は exec では再現できない。
- **Codex の project config（trusted project の `.codex/config.toml`）は profile を上書きする**。dotfiles 配下にこのファイルを置かないこと（テンプレートは `config.base.toml`）。
- Claude Code 側の安全機構により、**検証目的でも `git push` は実行できない**。そのため hook の実 deny 確認は Daiki に委ねる。

---

## 残課題（Daiki が対話で行う ＝ これが完成の最後の 2 ステップ）

### A. hook の trust 登録と実動作確認【最優先】

1. 任意の git リポジトリで対話起動: `codex --profile autonomous`
2. `/hooks` を開き、`block_git_write.py` が **trusted** になっているか確認（未登録なら trust 登録）
3. `git push`（または `git commit -m test`）の実行を指示し、**hook で deny される**ことを確認
   - ブロックされれば Git ガード完成
   - されなければ、matcher / command パス / trusted_hash を再確認（hook 定義を変えると hash が変わり再 trust が必要）
   - 補足: trust は hook **定義（command 文字列）**のハッシュに記録される（公式仕様。参照先スクリプト内容は対象外）。
     今回 block_git_write.py の中身（exit code）のみ変更し hook 定義は不変なので trusted_hash は維持される見込み。
     `/hooks` で untrusted 表示なら再 trust すれば足りる。

### B. 対話での rules 動作確認

- 対話 `codex --profile autonomous`（approval=on-request）で
  - `rm test.txt` が確認（prompt）されるか
  - `git` 書き込み系が rules でも止まるか（hook と二重防御）

> 補足: 新端末セットアップ時は `cargo run -- codex` 後、対話 `codex` の `/hooks` で hook を trust する必要がある（テンプレートに hooks.state は含めないため）。

---

## ファイル構成（現状）

```
~/dotfiles/.codex/
├── AGENTS.md                  # モード定義（sandbox mode で判定）         -> ~/.codex/ へ symlink
├── teacher.config.toml        # profile v2: read-only / on-request / gpt-5.5  ※ install CLI が ~/.codex/ へ copy
├── autonomous.config.toml     # profile v2: workspace-write / gpt-5.5         ※ install CLI が ~/.codex/ へ copy
├── config.base.toml           # 新端末用 base テンプレート（端末非依存）      ※ install CLI が ~/.codex/config.toml へ copy
├── rules/default.rules        # コマンド単位制御（git forbidden / rm prompt） -> symlink
└── hooks/block_git_write.py    # git 書き込みガード hook（command で参照）

~/.codex/（実体・ローカル管理）
├── config.toml                # base: グローバルデフォルト + projects(端末依存) + hooks定義 + hooks.state(trusted_hash)
├── teacher.config.toml        # profile v2 + projects(端末依存)
├── autonomous.config.toml     # profile v2 + projects(端末依存)
├── AGENTS.md              -> dotfiles（symlink）
└── rules/default.rules    -> dotfiles（symlink）
```

※ 旧 `~/dotfiles/.codex/config.toml` は削除（project config として profile を上書きしていたため）。
   役割は `config.base.toml`（テンプレート専用・Codex は project config として読まない）に移管。

## 起動方法

- 教師モード: `codex --profile teacher`（read-only。提案・調査・レビューのみ）
- 自律モード: `codex --profile autonomous`（workspace-write。危険以外は自動）
- profile 無し起動 = base の read-only（安全側の教師モード相当）

## バックアップ

- `~/.codex/config.toml.bak.<timestamp>`
- `~/.codex/rules/default.rules.bak.<timestamp>`

---

## 多層防御の整理（Git 書き込み禁止）

| 層 | 手段 | approval=never | approval=on-request（対話） |
|---|---|---|---|
| 1 | hook（PreToolUse deny） | ◯（trust 登録後・残課題 A で確認） | ◯（trust 登録後・残課題 A で確認） |
| 2 | rules（forbidden） | ✕（評価されない） | ◯（見込み・残課題 B で確認） |
| 3 | AGENTS.md（自主規制） | △（破られうる） | △（破られうる） |
| 4 | teacher は read-only sandbox | ◯（物理的に不可） | ◯（物理的に不可） |

→ **hook（層1）の trust が有効なら、approval 設定に依存しない確実なガードになる**。残課題 A がこれを最終確定する。

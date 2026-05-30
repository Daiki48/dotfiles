-- Codex CLI をモード別に起動するユーザーコマンド。
--
-- Codex は教師/自律の本質である sandbox mode（教師=read-only / 自律=workspace-write）を
-- 起動時にしか決められず、セッション中のトグル（Claude の Shift+Tab 相当）ができない。
-- そのため起動時に profile を指定する。AGENTS.md 側は sandbox mode でモードを判定する。
--   :Codex     → 教師モード（read-only。提案・調査・レビューに徹する）
--   :CodexAuto → 自律モード（workspace-write。危険コマンド以外は自動で編集・実行）
--
-- ※ 既存フローの `:terminal codex`（profile 無し）も base config が read-only のため
--    教師モードで起動する。:Codex は profile を明示してそれと等価。

local function open_codex(profile)
  vim.cmd("terminal codex --profile " .. profile)
  -- ターミナルを開いたら即入力できるよう挿入モードへ
  vim.cmd("startinsert")
end

vim.api.nvim_create_user_command("Codex", function()
  open_codex("teacher")
end, { desc = "Codex を教師モード（read-only）で起動" })

vim.api.nvim_create_user_command("CodexAuto", function()
  open_codex("autonomous")
end, { desc = "Codex を自律モード（workspace-write）で起動" })

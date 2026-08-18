-- Codex CLIを標準設定（workspace-write + auto-review）で起動する。

vim.api.nvim_create_user_command("Codex", function()
  vim.cmd("terminal codex")
  -- ターミナルを開いたら即入力できるよう挿入モードへ移る。
  vim.cmd("startinsert")
end, { desc = "Codexを標準設定で起動" })

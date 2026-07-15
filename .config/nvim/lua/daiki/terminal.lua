-- tmux内のNeovim端末はWezTermのOSCシーケンスを外側へ透過できないため、
-- 子シェルでのWezTermシェル統合を無効化する。
if vim.env.TMUX then
  vim.env.WEZTERM_SHELL_SKIP_ALL = "1"
end

-- ターミナルバッファの幅を偶数に調整（全角文字の折り返し欠け対策）
-- Neovimの:terminalで幅が奇数カラムの場合、全角文字が行末境界で消失するため
vim.api.nvim_create_autocmd("TermOpen", {
  callback = function()
    local width = vim.fn.winwidth(0)
    if width % 2 == 1 then
      vim.cmd("vertical resize " .. (width - 1))
    end
  end,
})

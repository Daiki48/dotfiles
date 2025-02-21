-- Config for gopls
-- vim.cmd("autocmd BufWritePre *.go :call CocAction('runCommand', 'editor.action.organizeImport')")

vim.diagnostic.config({
  virtual_text = false,
  underline = true,
})

vim.api.nvim_create_autocmd("CursorHold", {
  callback = function()
    local diagnostics = vim.diagnostic.get(0, { cursor = true })
    if #diagnostics > 0 then
      vim.diagnostic.open_float({
        border = "rounded",
      })
    end
  end,
})

vim.keymap.set("n", "g]", "<cmd>lua vim.diagnostic.goto_next()<CR>")
vim.keymap.set("n", "g[", "<cmd>lua vim.diagnostic.goto_prev()<CR>")

local keymap = vim.api.nvim_set_keymap

keymap("t", "<ESC>", "<C-\\><C-n>", { noremap = true, silent = true })

vim.api.nvim_create_user_command("T", function(opts)
  vim.cmd("vsplit")
  vim.cmd("wincmd l")
  if opts.args == "" and vim.fn.has("win64") == 1 then
    vim.cmd('terminal "C:\\Program Files\\PowerShell\\7\\pwsh.exe"')
  else
    vim.cmd("terminal " .. opts.args)
  end
	vim.cmd("autocmd TermOpen * startinsert")
end, { nargs = "*" })

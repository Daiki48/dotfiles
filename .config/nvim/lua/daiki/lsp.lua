-- Config for gopls
vim.cmd("autocmd BufWritePre *.go :call CocAction('runCommand', 'editor.action.organizeImport')")

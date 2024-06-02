vim.keymap.set("i", "<Tab>", "<cmd>call pum#map#insert_relative(+1)<CR>")
vim.keymap.set("i", "<S-Tab>", "<cmd>call pum#map#insert_relative(-1)<CR>")

-- vim.api.nvim_command('autocmd User skkeleton-enable-pre lua vim.b.coc_suggest_disable = true')
-- vim.api.nvim_command('autocmd User skkeleton-disable-pre lua vim.b.coc_suggest_disable = false')

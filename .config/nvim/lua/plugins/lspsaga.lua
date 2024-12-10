require("lspsaga").setup()
vim.keymap.set("n", "K", "<Cmd>Lspsaga hover_doc<CR>")
vim.keymap.set("n", "me", "<Cmd>Lspsaga diagnostic_jump_next<CR>")
vim.keymap.set("n", "mw", "<Cmd>Lspsaga diagnostic_jump_prev<CR>")

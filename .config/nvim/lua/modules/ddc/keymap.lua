vim.keymap.set("i", "<Tab>", "<cmd>call pum#map#insert_relative(+1)<CR>")
vim.keymap.set("i", "<S-Tab>", "<cmd>call pum#map#insert_relative(-1)<CR>")

vim.keymap.set("i", "<C-a>", "<Plug>(neosnippet_expand_or_jump)")
vim.keymap.set("s", "<C-a>", "<Plug>(neosnippet_expand_or_jump)")
vim.keymap.set("x", "<C-a>", "<Plug>(neosnippet_expand_target)")

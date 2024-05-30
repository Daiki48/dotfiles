local keymap = vim.api.nvim_set_keymap

-- buffer moving
keymap('n', '<C-l>', '<cmd>bnext<cr>', { noremap = true, silent = true })
keymap('n', '<C-h>', '<cmd>bprevious<cr>', { noremap = true, silent = true })
keymap('n', '<C-q>', '<cmd>bd<cr>', { noremap = true })

-- Visual block mode
keymap('n', '<C-a>', '<C-v>', { noremap = true })
keymap('v', '<C-a>', '<C-v>', { noremap = true })

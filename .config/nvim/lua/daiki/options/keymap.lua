local keymap = vim.api.nvim_set_keymap

-- buffer moving
keymap('n', '<C-l>', '<cmd>bnext<cr>', { noremap = true, silent = true })
keymap('n', '<C-h>', '<cmd>bprevious<cr>', { noremap = true, silent = true })
keymap('n', '<C-q>', '<cmd>bd<cr>', { noremap = true })

-- packer command
keymap('n', 'ps', '<cmd>PackerSync<cr>', { noremap = true, silent = true })
keymap('n', 'pi', '<cmd>PackerInstall<cr>', { noremap = true, silent = true })

-- lsp keymaps
local set = vim.keymap.set
-- set("n", "gd", "<cmd>lua vim.lsp.buf.definition()<CR>")
-- set("n", "<leader>K", "<cmd>lua vim.lsp.buf.hover()<CR>")
-- set("n", "<C-m>", "<cmd>lua vim.lsp.buf.signature_help()<CR>")
-- set("n", "gy", "<cmd>lua vim.lsp.buf.type_definition()<CR>")
-- set("n", "rn", "<cmd>lua vim.lsp.buf.rename()<CR>")
-- set("n", "ma", "<cmd>lua vim.lsp.buf.code_action()<CR>")
-- set("n", "gr", "<cmd>lua vim.lsp.buf.references()<CR>")
-- set("n", "<leader>e", "<cmd>lua vim.lsp.diagnostic.show_line_diagnostics()<CR>")
-- set("n", "[d", "<cmd>lua vim.lsp.diagnostic.goto_prev()<CR>")
-- set("n", "]d", "<cmd>lua vim.lsp.diagnostic.goto_next()<CR>")

-- fern
set("n", ";fj", "<cmd>Fern .<CR>")

-- place this in one of your configuration file(s)
local hop = require("hop")
local directions = require("hop.hint").HintDirection

hop.setup({
  multi_windows = true,
})

vim.keymap.set("n", "<space>f", "<cmd>HopWord<CR>")

-- vim.keymap.set('n', 'f', function()
--   hop.hint_words({ direction = directions.AFTER_CURSOR, current_line_only = true })
-- end, {remap=true})
vim.keymap.set("n", "F", function()
  hop.hint_char1({ direction = directions.BEFORE_CURSOR, current_line_only = true })
end, { remap = true })
vim.keymap.set("n", "t", function()
  hop.hint_char1({ direction = directions.AFTER_CURSOR, current_line_only = true, hint_offset = -1 })
end, { remap = true })
vim.keymap.set("n", "T", function()
  hop.hint_char1({ direction = directions.BEFORE_CURSOR, current_line_only = true, hint_offset = 1 })
end, { remap = true })

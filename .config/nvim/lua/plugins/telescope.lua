require("telescope").setup({
	pickers = {
		find_files = {
			theme = "ivy",
		},
	},
})

local builtin = require("telescope.builtin")
vim.keymap.set("n", ";ff", builtin.find_files, {})
vim.keymap.set("n", ";fg", builtin.live_grep, {})
vim.keymap.set("n", ";fb", builtin.buffers, {})
vim.keymap.set("n", ";fh", builtin.help_tags, {})

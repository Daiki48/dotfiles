local status, rt = pcall(require, "rust-tools")
if (not status) then return end

rt.setup({
	server = {
		on_attach = function(_, bufnr)
			-- Hover actions
			vim.keymap.set("n", "<C-;>", rt.hover_actions.hover_actions, { buffer = bufnr })
			-- Code action groups
			vim.keymap.set("n", "<Leader>a", rt.code_action_group.code_action_group, { buffer = bufnr })
		end,
	},
	tools = {
		hover_actions = {
			border = {
				{ '╭', 'NormalFloat' },
				{ '─', 'NormalFloat' },
				{ '╮', 'NormalFloat' },
				{ '│', 'NormalFloat' },
				{ '╯', 'NormalFloat' },
				{ '─', 'NormalFloat' },
				{ '╰', 'NormalFloat' },
				{ '│', 'NormalFloat' },
			},
			-- auto_focus = true,
		},
	}
})

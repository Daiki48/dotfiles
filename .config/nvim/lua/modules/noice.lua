local status, noice = pcall(require, "noice")
if (not status) then return end

noice.setup({
	-- cmdline = {
	-- 	enabled = true,
	-- 	icons = {
	-- 		['/'] = { icon = '/', hl_group = 'DiagnosticWarn' },
	-- 		['?'] = { icon = '?', hl_group = 'DiagnosticWarn' },
	-- 		[':'] = { icon = ':', hl_group = 'DiagnosticInfo', firstc = false },
	-- 	},
	-- 	view = "cmdline",
	-- },
	popupmenu = {
		enabled = false,
	},
	messages = {
		view_search = "mini",
		enabled = true,
	},
  lsp = {
		signature = {
			enabled = false,
		},
		hover = {
			enabled = false,
		},
		messages = {
			enabled = false,
		},
  },
	commands = {
		history = {
			view = "split",
		}
	},
	routes = {
		{
			view = "mini",
			filter = { event = "msg_showmode" },
		},
	},
  -- you can enable a preset for easier configuration
  presets = {
  --   bottom_search = true, -- use a classic bottom cmdline for search
  --   command_palette = true, -- position the cmdline and popupmenu together
    long_message_to_split = true, -- long messages will be sent to a split
  --   inc_rename = false, -- enables an input dialog for inc-rename.nvim
  --   lsp_doc_border = false, -- add a border to hover docs and signature help
  },
	hacks = {
		skip_duplicate_messages = true,
	}
})

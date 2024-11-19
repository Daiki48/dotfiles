require("snacks").setup({
	-- your configuration comes here
	-- or leave it empty to use the default settings
	-- refer to the configuration section below
	dashboard = { enabled = true },
	bigfile = { enabled = true },
	notifier = { enabled = true },
	quickfile = { enabled = true },
	statuscolumn = { enabled = true },
	words = { enabled = true },
	styles = {
		notification = {
			wo = { wrap = true } -- Wrap notifications
		}
	}
})

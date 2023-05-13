local status, treesitter = pcall(require, 'nvim-treesitter.configs')
if (not status) then return end

treesitter.setup {
	ensure_installed = {
		"html",
		"lua",
		"vim",
		"typescript",
		"javascript",
		"toml",
		"rust",
		"css",
		"json",
		"yaml",
		"markdown",
	},
	autotag = {
		enable = true,
		filetypes = {
			"html",
			"xml",
			"javascript",
			"typescript",
			"javascriptreact",
			"typescriptreact",
			"svelte",
			"vue",
			"tsx",
			"jsx",
			"rescript",
			"markdown",
			"cs",
			"rust",
			"razor",
		},
	},
	indent = {
		-- disable = true,
		enable = true,
	},
	sync_install = false,
	-- yati = {
	--   enable = true,
	-- },
}

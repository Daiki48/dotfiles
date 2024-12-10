local mason = require("mason")
local mason_lspconfig = require("mason-lspconfig")

local config = {
	capabilities = require("blink.cmp").get_lsp_capabilities(),
}

local denols_setup = require("plugins.mason-config.denols").setup(config)
local lua_setup = require("plugins.mason-config.lua_ls").setup(config)
local rust_setup = require("plugins.mason-config.rust_analyzer").setup(config)
local svelte_setup = require("plugins.mason-config.svelte").setup(config)
local ts_ls_setup = require("plugins.mason-config.ts_ls").setup(config)

local server_setup = {
	rust_analyzer = rust_setup,
	lua_ls = lua_setup,
	ts_ls = ts_ls_setup,
	denols = denols_setup,
	svelte = svelte_setup,
}

local function default_setup(config)
	config.on_attach = require("plugins.mason-config.utils").on_attach
end

mason.setup({
	PATH = "prepend",
	ui = {
		border = "double",
	},
})

mason_lspconfig.setup({
	ensure_installed = {
		"rust_analyzer",
		"lua_ls",
		"ts_ls",
		"denols",
		"cssls",
		"svelte",
		"lemminx",
		"taplo",
		"jsonls",
		"omnisharp",
	},
})

mason_lspconfig.setup_handlers({
	function(server_name)
		if server_setup[server_name] then
			server_setup[server_name](config)
		else
			default_setup(config)
		end

		require("lspconfig")[server_name].setup(config)
	end,
})

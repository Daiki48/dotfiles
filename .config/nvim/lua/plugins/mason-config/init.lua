local mason_status, mason = pcall(require, "mason")
if not mason_status then
	return
end

local mason_lspconfig_status, mason_lspconfig = pcall(require, "mason-lspconfig")
if not mason_lspconfig_status then
	return
end

local config = {
	capabilities = require("blink.cmp").get_lsp_capabilities(),
}

local server_setup = {
	rust_analyzer = require("plugins.mason-config.rust_analyzer").setup(config),
}

mason.setup({
	PATH = "prepend",
	ui = {
		border = "rounded",
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
		-- "omnisharp",
	},
})

mason_lspconfig.setup_handlers({
	function(server_name)
		if server_setup[server_name] then
			server_setup[server_name](config)
		end

		require("lspconfig")[server_name].setup(config)
	end,
})

local status, mason = pcall(require, "mason")
if not status then
	return
end

local status2, mason_lspconfig = pcall(require, "mason-lspconfig")
if not status2 then
	return
end

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
		local config = {
			capabilities = require("blink.cmp").get_lsp_capabilities(),
		}

		require("lspconfig")[server_name].setup(config)
	end,
})

local lspconfig = require("lspconfig")

local opts = {
	servers = {
		lua_ls = {
			settings = {
				Lua = {
					runtime = {
						version = "LuaJIT",
						-- path = vim.split(package.path, ";"),
					},
					diagnostics = {
						globals = { "vim" },
					},
				},
			},
		},
		ts_ls = {},
		denols = {},
		svelte = {},
		cssls = {},
		rust_analyzer = {},
		csharp_ls = {},
	},
}

for server, config in pairs(opts.servers) do
	-- passing config.capabilities to blink.cmp merges with the capabilities in your
	-- `opts[server].capabilities, if you've defined it
	config.capabilities = require("blink.cmp").get_lsp_capabilities(config.capabilities)
	lspconfig[server].setup(config)
end

-- example calling setup directly for each LSP
-- config = function()
--   local capabilities = require('blink.cmp').get_lsp_capabilities()
--   local lspconfig = require('lspconfig')
--
--   lspconfig['lua-ls'].setup({ capabilities = capabilities })
-- end

-- local tools = require("plugins.nvim-lspconfig.tools")

-- lspconfig["cssls"].setup({
-- 	root_dir = lspconfig.util.root_pattern("package.json"),
-- 	on_attach = tools.on_attach,
-- 	flags = tools.lsp_flags,
-- 	init_options = {},
-- })
--
-- lspconfig["lua_ls"].setup({
-- 	on_attach = tools.on_attach,
-- 	flags = tools.lsp_flags,
-- 	settings = {
-- 		Lua = {
-- 			runtime = {
-- 				version = "LuaJIT",
-- 				path = vim.split(package.path, ";"),
-- 			},
-- 			diagnostics = {
-- 				globals = { "vim" },
-- 			},
-- 		},
-- 	},
-- })
--
-- lspconfig["tsserver"].setup({
-- 	root_dir = lspconfig.util.root_pattern("package.json"),
-- 	on_attach = tools.on_attach,
-- 	flags = tools.lsp_flags,
-- 	init_options = {},
-- })
--
-- lspconfig["denols"].setup({
-- 	root_dir = lspconfig.util.root_pattern("deno.json"),
-- 	on_attach = tools.on_attach,
-- 	flags = tools.lsp_flags,
-- 	init_options = {},
-- })
--
-- lspconfig["svelte"].setup({
-- 	root_dir = lspconfig.util.root_pattern("package.json"),
-- 	on_attach = tools.on_attach,
-- 	flags = tools.lsp_flags,
-- 	init_options = {},
-- })
--
-- lspconfig["rust_analyzer"].setup({
-- 	on_attach = tools.on_attach,
-- 	flags = tools.lsp_flags,
-- 	-- Server-specific settings...
-- 	settings = {
-- 		["rust-analyzer"] = {},
-- 	},
-- })
--
-- lspconfig["clangd"].setup({
-- 	on_attach = tools.on_attach,
-- 	flags = tools.lsp_flags,
-- 	init_options = {},
-- })

local tools = require("daiki.plugins.nvim-lspconfig.tools")
local lspconfig = require('lspconfig')

lspconfig['cssls'].setup {
	root_dir = lspconfig.util.root_pattern("package.json"),
	on_attach = tools.on_attach,
	flags = tools.lsp_flags,
	init_options = {},
}

lspconfig['lua_ls'].setup{
	on_attach = tools.on_attach,
	flags = tools.lsp_flags,
	settings = {
		Lua = {
			runtime = {
				version = 'LuaJIT',
				path = vim.split(package.path, ';')
			},
			diagnostics = {
				globals = {'vim'}
			},
		}
	}
}

lspconfig['tsserver'].setup{
	root_dir = lspconfig.util.root_pattern("package.json"),
	on_attach = tools.on_attach,
	flags = tools.lsp_flags,
	init_options = {},
}

lspconfig['denols'].setup{
	root_dir = lspconfig.util.root_pattern("deno.json"),
	on_attach = tools.on_attach,
	flags = tools.lsp_flags,
	init_options = {},
}

lspconfig['svelte'].setup{
	root_dir = lspconfig.util.root_pattern("package.json"),
	on_attach = tools.on_attach,
	flags = tools.lsp_flags,
	init_options = {},
}

lspconfig['rust_analyzer'].setup{
	on_attach = tools.on_attach,
	flags = tools.lsp_flags,
	-- Server-specific settings...
	settings = {
		["rust-analyzer"] = {}
	}
}

lspconfig['clangd'].setup{
	on_attach = tools.on_attach,
	flags = tools.lsp_flags,
	init_options = {},
}


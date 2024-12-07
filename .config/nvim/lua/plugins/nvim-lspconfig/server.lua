local lspconfig = require("lspconfig")
local util = require("lspconfig.util")

local tools = require("plugins.nvim-lspconfig.tools")

lspconfig["cssls"].setup({
	root_dir = util.root_pattern("package.json"),
	on_attach = tools.on_attach,
	flags = tools.lsp_flags,
	init_options = {},
})

lspconfig["lua_ls"].setup({
	on_attach = tools.on_attach,
	flags = tools.lsp_flags,
	settings = {
		Lua = {
			runtime = {
				version = "LuaJIT",
				path = vim.split(package.path, ";"),
			},
			diagnostics = {
				globals = { "vim" },
			},
		},
	},
})

lspconfig["ts_ls"].setup({
	root_dir = util.root_pattern("package.json", "tsconfig.json"),
	on_attach = tools.on_attach,
	flags = tools.lsp_flags,
	init_options = {},
})

lspconfig["denols"].setup({
	root_dir = util.root_pattern("deno.json"),
	on_attach = tools.on_attach,
	flags = tools.lsp_flags,
	init_options = {},
})

lspconfig["svelte"].setup({
	root_dir = util.root_pattern("package.json"),
	on_attach = tools.on_attach,
	flags = tools.lsp_flags,
	init_options = {},
})

lspconfig["rust_analyzer"].setup({
	on_attach = tools.on_attach,
	flags = tools.lsp_flags,
	-- Server-specific settings...
	settings = {
		["rust-analyzer"] = {
			check = {
				command = "clippy",
			},
		},
	},
})

lspconfig["clangd"].setup({
	on_attach = tools.on_attach,
	flags = tools.lsp_flags,
	init_options = {},
})

lspconfig["omnisharp"].setup({
	cmd = { "dotnet", "/path/to/omnisharp/OmniSharp.dll" },
	filetypes = { "cs", "vb" },
	root_dir = util.root_pattern("*.sln", "*.csproj", "omnisharp.json", "function.json"),
	on_attach = tools.on_attach,
	flags = tools.lsp_flags,
	init_options = {},
})

lspconfig["taplo"].setup({
	on_attach = tools.on_attach,
	flags = tools.lsp_flags,
	init_options = {},
})

local tools = require("modules.nvim-lspconfig.tools")
local lspconfig = require('lspconfig')

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

local omnisharp_path = vim.fn.expand('~/.local/share/omnisharp/run')
lspconfig['omnisharp'].setup{
	on_attach = tools.on_attach,
	flags = tools.lsp_flags,
	cmd = { "dotnet", omnisharp_path },
	enable_editorconfig_support = true,
	enable_ms_build_load_projects_on_demand = false,
	enable_roslyn_analyzers = false,
	filetypes = { "cs", "vb", "cshtml", "razor" },
	organize_imports_on_format = false,
	enable_import_completion = false,
	sdk_include_prereleases = true,
	analyze_open_documents_only = false,
	init_options = {},
}

local lspconfig = require("lspconfig")
local util = require("lspconfig.util")

local tools = require("plugins.nvim-lspconfig.tools")

-- local ts_ls_path = vim.env.USERPROFILE .. "\\scoop\\apps\\nvm\\current\\nodejs\\nodejs\\typescript-language-server"
-- local mason_ts_lsp_path= vim.fn.stdpath("data") .. "/mason/packages/typescript-language-server"
-- local mason_lsp_path = path.concat { vim.fn.stdpath "data", "mason" }

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
	filetypes = {
		"typescript",
		"typescriptreact",
		"typescript.tsx",
		"javascript",
		"javascriptreact",
		"javascript.jsx",
		"vue",
	},
	root_dir = util.root_pattern("package.json", "tsconfig.json"),
	on_attach = tools.on_attach,
	flags = tools.lsp_flags,
	cmd = { "typescript-language-server", "--stdio" },
	init_options = {
		-- cmd = { ts_ls_path, "--stdio" },
	},
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

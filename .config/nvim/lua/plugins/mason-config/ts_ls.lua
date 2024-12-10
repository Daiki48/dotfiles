M = {}

local util = require("lspconfig.util")

M.setup = function(config)
	-- config.filetypes = {
	-- 	"typescript",
	-- 	"typescriptreact",
	-- 	"typescript.tsx",
	-- 	"javascript",
	-- 	"javascriptreact",
	-- 	"javascript.jsx",
	-- 	"vue",
	-- }
	config.root_dir = util.root_pattern("package.json", "tsconfig.json")
	config.flags = require("plugins.mason-config.utils").lsp_flags
	config.cmd = { "typescript-language-server", "--stdio" }

	config.on_attach = require("plugins.mason-config.utils").on_attach
	config.settings = {}
end

return M

-- local util = require("lspconfig.util")
--
-- return {
-- 	filetypes = {
-- 		"typescript",
-- 		"typescriptreact",
-- 		"typescript.tsx",
-- 		"javascript",
-- 		"javascriptreact",
-- 		"javascript.jsx",
-- 		"vue",
-- 	},
-- 	root_dir = util.root_pattern("package.json", "tsconfig.json"),
-- 	flags = require("plugins.mason-config.utils").lsp_flags,
-- 	cmd = { "typescript-language-server", "--stdio" },
-- 	on_attach = require("plugins.mason-config.utils").on_attach,
-- 	settings = {},
-- }

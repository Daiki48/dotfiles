M = {}

local util = require("lspconfig.util")

M.setup = function(config)
	config.root_dir = util.root_pattern("svelte.config.js")
	config.on_attach = require("plugins.mason-config.utils").on_attach
	config.flags = require("plugins.mason-config.utils").lsp_flags
	config.settings = {}
	config.cmd = { "svelteserver", "--stdio" }
end

return M

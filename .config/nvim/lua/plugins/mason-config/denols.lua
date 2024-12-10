M = {}

local util = require("lspconfig.util")

M.setup = function(config)
	config.root_dir = util.root_pattern("deno.json")
	config.flags = require("plugins.mason-config.utils").lsp_flags

	config.on_attach = require("plugins.mason-config.utils").on_attach
	config.settings = {}
end

return M

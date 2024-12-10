M = {}

M.setup = function(config)
	config.on_attach = require("plugins.mason-config.utils").on_attach
	config.settings = {
		Lua = {
			runtime = {
				version = "LuaJIT",
				path = vim.split(package.path, ";"),
			},
			diagnostics = {
				globals = { "vim" },
			},
		},
	}
end

return M

-- return {
-- 	on_attach = require("plugins.mason-config.utils").on_attach,
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
-- }

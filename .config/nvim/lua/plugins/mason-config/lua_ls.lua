M = {}

M.lua_setup = function(config)
	config.on_attach = require("lua.plugins.mason-config.utils").on_attach
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

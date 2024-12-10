M = {}

M.default_setup = function(config)
	config.on_attach = require("plugins.mason-config.utils").on_attach
	config.settings = {}
end

return M

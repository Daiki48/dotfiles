local lazypath = vim.fn.stdpath("data") .. "/lazy/lazy.nvim"
if not vim.loop.fs_stat(lazypath) then
	vim.fn.system({
		"git",
		"clone",
		"--filter=blob:none",
		"https://github.com/folke/lazy.nvim.git",
		"--branch=stable", -- latest stable release
		lazypath,
	})
end
vim.opt.rtp:prepend(lazypath)

local plugins = require("daiki.plugins")

local opts = {
	defaults = {
		lazy = true,
	},
	performance = {
		cache = {
			enabled = true,
		},
	},
	dev = {
		path = "/mnt/sabrent/dev/nvim-plugin-dev/Daiki48",
		-- patterns = {'Daiki48'},
		-- fallback = true,
	},
	-- install = {
	-- 	colorscheme = { 'sakurajima' }
	-- }
}

require("lazy").setup(plugins, opts)

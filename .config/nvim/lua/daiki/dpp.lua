local denops_src = vim.fn.stdpath("cache") .. "/dpp/repos/github.com/denops/denops.vim"
local dpp_src = vim.fn.stdpath("cache") .. "/dpp/repos/github.com/Shougo/dpp.vim"
local dpp_ext_installer_src = vim.fn.stdpath("cache") .. "/dpp/repos/github.com/Shougo/dpp-ext-installer"
local dpp_protocol_git_src = vim.fn.stdpath("cache") .. "dpp/repos/github.com/dpp-protocol-git"

local denops_url = "https://github.com/vim-denops/denops.vim"
local dpp_url = "https://github.com/Shougo/dpp.vim"
local dpp_ext_installer_url = "https://github.com/Shougo/dpp-ext-installer"
local dpp_protocol_git_url = "https://github.com/Shougo/dpp-protocol-git"

local repo_clone = function(url, branch, src)
	if not vim.loop.fs_stat(src) then
		vim.fn.system({
			"git",
			"clone",
			url,
			"--filter=blob:none",
			"--branch=" .. branch,
			src,
		})
		vim.notify("Successfull clone " .. url)
	end
end

repo_clone(dpp_url, "main", dpp_src)
repo_clone(denops_url, "main", denops_src)
repo_clone(dpp_ext_installer_url, "main", dpp_ext_installer_src)
repo_clone(dpp_protocol_git_url, "main", dpp_protocol_git_src)

vim.opt.runtimepath:prepend(dpp_src)

local dpp = require("dpp")

local dpp_base = vim.fn.stdpath("cache") .. "/dpp"
local dpp_config = vim.fn.stdpath("config") .. "/dpp/config.ts"

vim.g["denops#debug"] = 1

if dpp.load_state(dpp_base) then
	vim.opt.runtimepath:prepend(denops_src)
	vim.opt.runtimepath:prepend(dpp_ext_installer_src)
	vim.opt.runtimepath:prepend(dpp_protocol_git_src)

	vim.api.nvim_create_autocmd("User", {
		pattern = "DenopsReady",
		callback = function()
			vim.notify("dpp load_state() is failed")
			dpp.make_state(dpp_base, dpp_config)
		end,
	})
end

vim.api.nvim_create_autocmd("User", {
	pattern = "Dpp:makeStatePost",
	callback = function()
		vim.notify("dpp make_state() is done")
	end,
})

vim.api.nvim_create_user_command("DppInstall", "call dpp#async_ext_action('installer', 'install)", { nargs = 0 })
vim.api.nvim_create_user_command("DppUpdate", "call dpp#async_ext_action('installer', 'update)", { nargs = 0 })

vim.cmd("filetype indent plugin on")
vim.cmd("syntax on")

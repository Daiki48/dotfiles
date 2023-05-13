-- *************************** --
--      dein.vim settings      --
-- *************************** --
local cache_dir = vim.env.HOME .. '/.cache'
local dein_dir = cache_dir .. '/dein'
local dein_repo_dir = dein_dir .. '/repos/github.com/Shougo/dein.vim'

if not string.match(vim.o.runtimepath, '/dein.vim') then
	if vim.fn.isdirectory(dein_repo_dir) ~= 1 then
		os.execute('git clone https://github.com/Shougo/dein.vim ' .. dein_repo_dir)
	end
	vim.o.runtimepath = dein_repo_dir .. ',' .. vim.o.runtimepath
end

if vim.call('dein#min#load_state', dein_dir) ~= 1 then
	return
end

local dein_toml_dir = vim.env.HOME .. '/.config/nvim/toml'
local dein_toml_start = dein_toml_dir .. '/start.toml'
local dein_toml_lazy = dein_toml_dir .. '/lazy.toml'
-- local dein_toml_telescope = dein_toml_dir .. '/telescope.toml'
-- local dein_toml_cmp = dein_toml_dir .. '/cmp.toml'
local dein_toml_lspconfig = dein_toml_dir .. '/lspconfig.toml'
local dein_toml_noice = dein_toml_dir .. '/noice.toml'
local dein_toml_ddu = dein_toml_dir .. '/ddu.toml'
local dein_toml_ddc = dein_toml_dir .. '/ddc.toml'

vim.call('dein#begin', dein_dir)

vim.call('dein#load_toml', dein_toml_start, { lazy = 0 })
vim.call('dein#load_toml', dein_toml_lazy, { lazy = 1 })
-- vim.call('dein#load_toml', dein_toml_telescope, { lazy = 1 })
-- vim.call('dein#load_toml', dein_toml_cmp, { lazy = 1 })
vim.call('dein#load_toml', dein_toml_lspconfig, { lazy = 1 })
vim.call('dein#load_toml', dein_toml_noice, { lazy = 1 })
vim.call('dein#load_toml', dein_toml_ddu, { lazy = 1 })
vim.call('dein#load_toml', dein_toml_ddc, { lazy = 1 })

vim.call('dein#end')
vim.call('dein#save_state')

-- install check
if (vim.call('dein#check_install') ~= 0) then
	vim.call('dein#install')
end

-- plugin remove check
local removed_plugins = vim.call('dein#check_clean')
if vim.fn.len(removed_plugins) > 0 then
	vim.fn.map(removed_plugins, "delete(v:val, 'rf')")
	vim.call('dein#recache_runtimepath')
	local impatient_ok, impatient = pcall(require, "impatient")
	if impatient_ok then
	  impatient.enable_profile()
	end
end

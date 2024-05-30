vim.cmd("autocmd!")

vim.scriptencoding = "utf-8"
vim.o.encoding = "utf-8"
vim.o.fileencoding = "utf-8"

vim.g.mapleader = "<Space>"

vim.wo.number = true
vim.wo.relativenumber = true
vim.wo.signcolumn = "yes"

vim.o.shell = "zsh"
vim.o.scrolloff = 10
vim.o.cmdheight = 1
vim.o.inccommand = "split"
vim.o.breakindent = true
vim.o.helplang = "ja,en"
vim.o.ignorecase = true
vim.o.smartcase = true
vim.o.splitright = true
vim.o.termguicolors = true
vim.o.hidden = true
vim.o.updatetime = 300
vim.o.tabstop = 2
vim.o.shiftwidth = 2
vim.o.cursorline = true
vim.o.pumblend = 20
vim.o.clipboard = "unnamedplus"
vim.o.swapfile = false
vim.o.backup = false

vim.bo.expandtab = true
vim.bo.autoindent = false
vim.bo.smartindent = false
vim.bo.autoread = true

vim.o.background = "dark" -- "dark" or "light" for light mode
-- vim.cmd 'syntax enable'
-- vim.cmd 'syntax on'
vim.cmd("set wildoptions=pum")

-- copy/paste between WSL and Windows
-- https://github.com/neovim/neovim/wiki/FAQ#how-to-use-the-windows-clipboard-from-wsl
if vim.fn.has("wsl") == 1 then
	vim.g.clipboard = {
		name = "win32yank.exe",
		copy = {
			["+"] = { "win32yank.exe", "-i", "--crlf" },
			["*"] = { "win32yank.exe", "-i", "--crlf" },
		},
		paste = {
			["+"] = { "win32yank.exe", "-o", "--lf" },
			["*"] = { "win32yank.exe", "-o", "--lf" },
		},
		cache_enabled = 1,
	}
end

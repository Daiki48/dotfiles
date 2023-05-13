M = {}

local tools = require("modules.ddu.tools")

local ddu_ff = {
	do_action = vim.fn["ddu#ui#ff#do_action"];
}

local ddu_ff_filter = {
	close = vim.fn["ddu#ui#ff#close"];
}

local ddu_filer = {
	do_action = vim.fn["ddu#ui#filer#do_action"];
	is_tree = vim.fn["ddu#ui#filer#is_tree"];
}

-- ###################################### --
--      ddu-ui-ff FileType autocmd        --
--           ddu-ui-ff keymap             --
-- ###################################### --
vim.api.nvim_create_autocmd("FileType", {
	pattern = {"ddu-ff"},
	callback = function()
		vim.keymap.set("n", "<CR>", function() return ddu_ff.do_action("itemAction") end, { buffer = 0 })
		vim.keymap.set("n", "i", function() return ddu_ff.do_action("openFilterWindow") end, { buffer = 0 })
		vim.keymap.set("n", "p", function() return ddu_ff.do_action("preview") end, { buffer = 0 })
		vim.keymap.set("n", "q", function() return ddu_ff.do_action("quit") end, { buffer = 0 })
		vim.keymap.set("n", "<ESC>", function() return ddu_ff.do_action("quit") end, { buffer = 0 })
	end,
})

vim.api.nvim_create_autocmd("FileType", {
	pattern = {"ddu-ff-filter"},
	callback = function()
		vim.keymap.set("i", "<CR>", "<ESC><cmd>call ddu#ui#ff#close()<CR>", { buffer = 0 })
		vim.keymap.set("i", "<ESC>", "<ESC><cmd>call ddu#ui#ff#close()<CR>", { buffer = 0 })
		vim.keymap.set("n", "<ESC>", function() return ddu_ff_filter.close() and ddu_ff.do_action("quit") end, { buffer = 0 })
		vim.keymap.set("n", "q", function() return ddu_ff_filter.close() and ddu_ff.do_action("quit") end, { buffer = 0 })
	end,
})


-- ###################################### --
--     ddu-ui-filer FileType autocmd      --
--          ddu-ui-filer keymap           --
-- ###################################### --
vim.api.nvim_create_autocmd("FileType", {
	pattern = {"ddu-filer"},
	callback = function()
		vim.keymap.set("n", "<Space>", function() return ddu_filer.do_action("toggleSelectItem") end, { buffer = 0 })
		vim.keymap.set("n", "o", function() return ddu_filer.do_action("expandItem", { mode = "toggle" }) end, { buffer = 0 })
		vim.keymap.set("n", "<ESC>", function() return ddu_filer.do_action("quit") end, { buffer = 0 })
		vim.keymap.set("n", "q", function() return ddu_filer.do_action("quit") end, { buffer = 0 })

		vim.keymap.set("n", "N", function() return ddu_filer.do_action("itemAction", { name = "newFile" }) end, { buffer = 0 })
		vim.keymap.set("n", "K", function() return ddu_filer.do_action("itemAction", { name = "newDirectory" }) end, { buffer = 0 })
		vim.keymap.set("n", "R", function() return ddu_filer.do_action("itemAction", { name = "rename" }) end, { buffer = 0 })
		vim.keymap.set("n", "D", function() return ddu_filer.do_action("itemAction", { name = "delete" }) end, { buffer = 0 })

		vim.keymap.set("n", "<CR>", function ()
			if ddu_filer.is_tree() then
				ddu_filer.do_action("itemAction", { name = "narrow" })
			else
				ddu_filer.do_action("itemAction", { name = "open" })
			end
		end, { buffer = 0 })
		vim.keymap.set("n", "l", function ()
			if ddu_filer.is_tree() then
				ddu_filer.do_action("itemAction", { name = "narrow" })
			else
				ddu_filer.do_action("itemAction", { name = "open" })
			end
		end, { buffer = 0 })
		vim.keymap.set("n", "h", function ()
			return ddu_filer.do_action("itemAction", { name = "narrow", params = { path = ".." } })
		end, { buffer = 0 })
		vim.keymap.set("n", "<C-h>", function() return ddu_filer.do_action("collapseItem") end, { buffer = 0 })
		vim.keymap.set("n", "<C-l>", function() return ddu_filer.do_action("expandItem", { mode = "toggle" }) end, { buffer = 0 })
	end,
})

vim.api.nvim_create_autocmd("FileType", {
	pattern = {"ddu-filer-filter"},
	callback = function()
		vim.keymap.set("i", "<CR>", "<ESC><cmd>call ddu#ui#filer#close()<CR>", { buffer = 0 })
		vim.keymap.set("i", "<ESC>", function() return vim.fn["ddu#ui#filer#close"]() and ddu_filer.do_action("quit") end, { buffer = 0 })
		vim.keymap.set("n", "<ESC>", function() return vim.fn["ddu#ui#filer#close"]() and ddu_filer.do_action("quit") end, { buffer = 0 })
		vim.keymap.set("n", "q", function() return vim.fn["ddu#ui#filer#close"]() and ddu_filer.do_action("quit") end, { buffer = 0 })
	end,
})

-- ###################################### --
--            starting keymap             --
-- ###################################### --
vim.keymap.set("n", "ff", function() return tools.find_files() end)
vim.keymap.set("n", ";;", function() return tools.filer() end)
vim.keymap.set("n", "DU", function() return tools.dein_update() end)
-- vim.keymap.set("n", "DU", "<cmd>Ddu dein_update -ui-param-winHeight=20 -ui-param-winRow=2 -ui-param-winWidth=60 -ui-param-winCol=10<CR>", { buffer = 0 })

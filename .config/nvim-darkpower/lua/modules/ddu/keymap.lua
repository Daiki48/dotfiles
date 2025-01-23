M = {}

local tools = require("modules.ddu.tools")

local do_action = vim.fn["ddu#ui#do_action"];
local get_item = vim.fn["ddu#ui#get_item"];

-- ###################################### --
--      ddu-ui-ff FileType autocmd        --
--           ddu-ui-ff keymap             --
-- ###################################### --
vim.api.nvim_create_autocmd("FileType", {
	pattern = {"ddu-ff"},
	callback = function()
		vim.keymap.set("n", "<CR>", function() return do_action("itemAction") end, { buffer = 0 })
		vim.keymap.set("n", "i", function() return do_action("openFilterWindow") end, { buffer = 0 })
		vim.keymap.set("n", "p", function() return do_action("preview") end, { buffer = 0 })
		vim.keymap.set("n", "q", function() return do_action("quit") end, { buffer = 0 })
		vim.keymap.set("n", "<ESC>", function() return do_action("quit") end, { buffer = 0 })
	end,
})

vim.api.nvim_create_autocmd("FileType", {
	pattern = {"ddu-ff-filter"},
	callback = function()
		vim.keymap.set("i", "<CR>", function() do_action("closeFilterWindow") vim.cmd('stopinsert') end, { buffer = 0 })
		vim.keymap.set("i", "<ESC>", function() do_action("closeFilterWindow") vim.cmd('stopinsert') end, { buffer = 0 })
		vim.keymap.set("n", "<ESC>", function() return do_action("closeFilterWindow") and do_action("quit") end, { buffer = 0 })
		vim.keymap.set("n", "q", function() return do_action("closeFilterWindow") and do_action("quit") end, { buffer = 0 })
	end,
})


-- ###################################### --
--     ddu-ui-filer FileType autocmd      --
--          ddu-ui-filer keymap           --
-- ###################################### --
vim.api.nvim_create_autocmd("FileType", {
	pattern = {"ddu-filer"},
	callback = function()
		vim.keymap.set("n", "<Space>", function() return do_action("toggleSelectItem") end, { buffer = 0 })
		vim.keymap.set("n", "o", function() return do_action("expandItem", { mode = "toggle" }) end, { buffer = 0 })
		vim.keymap.set("n", "<ESC>", function() return do_action("quit") end, { buffer = 0 })
		vim.keymap.set("n", "q", function() return do_action("quit") end, { buffer = 0 })

		vim.keymap.set("n", "N", function() return do_action("itemAction", { name = "newFile" }) end, { buffer = 0 })
		vim.keymap.set("n", "K", function() return do_action("itemAction", { name = "newDirectory" }) end, { buffer = 0 })
		vim.keymap.set("n", "Y", function() return do_action("itemAction", { name = "copy" }) end, { buffer = 0 })
		vim.keymap.set("n", "P", function() return do_action("itemAction", { name = "paste" }) end, { buffer = 0 })
		vim.keymap.set("n", "R", function() return do_action("itemAction", { name = "rename" }) end, { buffer = 0 })
		vim.keymap.set("n", "D", function() return do_action("itemAction", { name = "delete" }) end, { buffer = 0 })

		vim.keymap.set("n", "<CR>", function ()
			if get_item().isTree then
				do_action("itemAction", { name = "narrow" })
			else
				do_action("itemAction", { name = "open" })
			end
		end, { buffer = 0 })
		vim.keymap.set("n", "l", function ()
			if get_item().isTree then
				do_action("itemAction", { name = "narrow" })
			else
				do_action("itemAction", { name = "open" })
			end
		end, { buffer = 0 })
		vim.keymap.set("n", "h", function ()
			return do_action("itemAction", { name = "narrow", params = { path = ".." } })
		end, { buffer = 0 })
		vim.keymap.set("n", "<C-h>", function() return do_action("collapseItem") end, { buffer = 0 })
		vim.keymap.set("n", "<C-l>", function() return do_action("expandItem", { mode = "toggle" }) end, { buffer = 0 })
	end,
})

vim.api.nvim_create_autocmd("FileType", {
	pattern = {"ddu-filer-filter"},
	callback = function()
		vim.keymap.set("i", "<CR>", "<ESC><cmd>call ddu#ui#close()<CR>", { buffer = 0 })
		vim.keymap.set("i", "<ESC>", function() return vim.fn["ddu#ui#close"]() and do_action("quit") end, { buffer = 0 })
		vim.keymap.set("n", "<ESC>", function() return vim.fn["ddu#ui#close"]() and do_action("quit") end, { buffer = 0 })
		vim.keymap.set("n", "q", function() return do_action("quit") end, { buffer = 0 })
	end,
})

-- ###################################### --
--            starting keymap             --
-- ###################################### --
vim.keymap.set("n", "ff", function() return tools.find_files() end)
vim.keymap.set("n", ";;", function() return tools.filer() end)
vim.keymap.set("n", "DU", function() return tools.dein_update() end)

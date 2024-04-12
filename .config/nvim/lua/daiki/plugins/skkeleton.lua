---------------------------
-- skkeleton config -------
---------------------------
local skk = {
	config = vim.fn["skkeleton#config"];
	keymap = vim.fn["skkeleton#register_keymap"];
	kanatable = vim.fn["register_kanatable"];
}

local skk_files = {}
table.insert(skk_files, vim.env.HOME .. "/.skk/SKK-JISYO.L")

vim.api.nvim_create_autocmd("User", {
	pattern = "skkeleton-initialize-pre",
	callback = function ()
		skk.config({
			eggLikeNewline = true,
			registerConvertResult = true,
			globalDictionaries = skk_files,
			usePopup = true,
			keepState = false,
			immediatelyCancel = true,
			selectCandidateKeys = "asdfjkl",
			setUndoPoint = true,
		})
	end,
})

skk.keymap("henkan", "<Space>", "zenkaku")

vim.keymap.set("i", "<C-j>", "<Plug>(skkeleton-enable)")
vim.keymap.set("c", "<C-j>", "<Plug>(skkeleton-enable)")

---------------------------
-- indicator config -------
---------------------------
require('skkeleton_indicator').setup({
	eijiHlName = "LineNr",
	hiraHlName = "String",
	kataHlName = "Todo",
	hankataHlName = "Special",
	zenkakuHlName = "LineNr",
	abbrevHlName = "Error",
	eijiText = "AZ",
	hiraText = "ひら",
	kataText = "カナ",
	hankataText = "ｶﾅ",
	zenkakuText = "ＡＺ",
	abbrevText = "abbr",
	border = "rounded",
	row = 0,
	col = 1,
	zindex = nil,
	alwaysShown = false,
	fadeOutMs = 3000,
	ignoreFt = {},
	bufFilter = function()
			return true
	end,
})

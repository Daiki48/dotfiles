# Setup skkeleton for Neovim

# 参考資料
- [Setup skkeleton as easy as ABC](https://zenn.dev/kawarimidoll/scraps/8e0774581a2825)
- [新しいVim用日本語入力プラグインを作った](https://zenn.dev/kuu/articles/vac2021-skkeleton#%E4%BD%BF%E3%81%84%E6%96%B9)

# 辞書

辞書をダウンロードする。

https://skk-dev.github.io/dict/

↓をダウンロードした。

- SKK-JISYO.L.gz

  - <https://skk-dev.github.io/dict/SKK-JISYO.L.gz> \[[md5](https://skk-dev.github.io/dict/SKK-JISYO.L.gz.md5)]
  - 最も大きな辞書です。ある程度の人名・地名や複合語までを含んでいます。 annotation を積極的に付けることが推奨

# 最小構成

## plugins/init.lua
lazy.nvimのプラグイン管理でこのように書く。

```lua
	-- skkeleton
	{
		'vim-skk/skkeleton',
		dependencies = {
			'vim-denops/denops.vim'
		},
		lazy = false,
		config = function ()
			require('daiki.plugins.skkeleton')
		end
	}
```

## plugins/skkeleton.lua

```lua
vim.keymap.set("i", "<C-j>", "<Plug>(skkeleton-enable)")
vim.keymap.set("c", "<C-j>", "<Plug>(skkeleton-enable)")

local skk = {
	skkeleton = vim.fn["skkeleton#config"];
}

vim.api.nvim_create_autocmd("User", {
	pattern = "skkeleton-initialize-pre",
	callback = function ()
		skk.skkeleton({
			eggLikeNewline = true,
			registerConvertResult = true,
			globalDictionaries = {{"~/.skk/SKK-JISYO.L", "EUC-JP"}},
		})
	end
})    
```

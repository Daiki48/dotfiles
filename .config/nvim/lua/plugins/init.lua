return {
	{
		"folke/snacks.nvim",
		priority = 1000,
		lazy = false,
		config = function()
			require("plugins.snacks")
		end,
	},
	{
		"Daiki48/sakurajima.nvim",
		lazy = false,
		branch = "main",
		-- branch = "develop",
		-- dev = true,
		config = function()
			vim.cmd([[colorscheme sakurajima]])
		end,
	},
	"editorconfig/editorconfig-vim",
	{
		"nvim-lualine/lualine.nvim",
		lazy = false,
		dependencies = { "nvim-tree/nvim-web-devicons" },
		config = function()
			require("plugins.lualine")
		end,
	},
	{
		"lewis6991/gitsigns.nvim",
		lazy = false,
		config = function()
			require("gitsigns").setup()
		end,
	},
	{
		"numToStr/Comment.nvim",
		lazy = false,
		config = function()
			require("Comment").setup()
			vim.api.nvim_set_keymap("n", "<C-_>", "gcc", {})
			vim.api.nvim_set_keymap("v", "<C-_>", "gc", {})
		end,
	},
	{
		"stevearc/oil.nvim",
		lazy = false,
		---@module 'oil'
		---@type oil.SetupOpts
		opts = {},
		-- Optional dependencies
		dependencies = { { "echasnovski/mini.icons", opts = {} } },
		dependencies = { "nvim-tree/nvim-web-devicons" }, -- use if prefer nvim-web-devicons
		config = function()
			require("plugins.oil")
		end,
	},
	{
		"SirZenith/oil-vcs-status",
		-- event = "VimEnter",
		lazy = false,
		dependencies = {
			"stevearc/oil.nvim",
		},
		config = function()
			require("plugins.oil-vcs-status")
		end,
		-- config = true,
	},
	-- {
	-- 	"nvim-tree/nvim-tree.lua",
	-- 	lazy = false,
	-- 	dependencies = {
	-- 		"nvim-tree/nvim-web-devicons",
	-- 	},
	-- 	config = function()
	-- 		require("plugins.nvim-tree")
	-- 	end,
	-- },
	{
		"windwp/nvim-autopairs",
		event = "InsertEnter",
		config = true,
	},
	{
		"dstein64/vim-startuptime",
		cmd = "StartupTime",
	},
	{
		"nvim-telescope/telescope.nvim",
		tag = "0.1.6",
		lazy = false,
		dependencies = {
			"nvim-lua/plenary.nvim",
		},
		config = function()
			require("plugins.telescope")
		end,
	},

	-- Git
	{
		"NeogitOrg/neogit",
		cmd = "Neogit",
		dependencies = {
			"nvim-lua/plenary.nvim",
			"sindrets/diffview.nvim",
			"nvim-telescope/telescope.nvim",
		},
		config = function()
			require("neogit").setup()
		end,
	},

	-- coc
	{
		"neoclide/coc.nvim",
		branch = "release",
		lazy = false,
		config = function()
			require("plugins.coc")
		end,
	},

	{
		"leafOfTree/vim-svelte-plugin",
		ft = "svelte",
		config = function()
			require("plugins.vim-svelte-plugin")
		end,
	},

	-- treesitter
	{
		"nvim-treesitter/nvim-treesitter",
		build = ":TSUpdate",
		lazy = false,
		config = function()
			require("plugins.treesitter")
		end,
	},
	{
		"windwp/nvim-ts-autotag",
		lazy = false,
		dependencies = {
			"nvim-treesitter/nvim-treesitter",
		},
		config = function()
			require("plugins.nvim-ts-autotag")
		end,
	},

	-- skkeleton
	-- {
	-- 	'vim-skk/skkeleton',
	-- 	dependencies = {
	-- 		'vim-denops/denops.vim',
	-- 		'Shougo/ddc.vim',
	-- 	},
	-- 	event = 'VimEnter',
	-- 	config = function ()
	-- 		require('plugins.skkeleton')
	-- 	end
	-- },
	-- 'delphinus/skkeleton_indicator.nvim',
	-- {
	-- 	'rinx/cmp-skkeleton',
	-- 	dependencies = {
	-- 		'hrsh7th/nvim-cmp',
	-- 		'vim-skk/skkeleton',
	-- 	},
	-- 	config = true
	-- },

	-- completion
	-- {
	-- 	'hrsh7th/nvim-cmp',
	-- 	event="InsertEnter",
	-- 	dependencies = {
	-- 		"hrsh7th/cmp-nvim-lsp",
	-- 		"hrsh7th/cmp-buffer",
	-- 		"hrsh7th/cmp-path",
	-- 		"hrsh7th/cmp-vsnip",
	-- 		"hrsh7th/vim-vsnip",
	-- 		-- "onsails/lspkind.nvim",
	-- 		"hrsh7th/cmp-nvim-lsp-signature-help",
	-- 		{ "hrsh7th/cmp-cmdline", event = { "CmdlineEnter" } },
	-- 		-- { "uga-rosa/cmp-skkeleton", dependencies = { "vim-skk/skkeleton" } },
	-- 	},
	-- 	config = function ()
	-- 		require('plugins.cmp')
	-- 	end
	-- },

	-- -- ddc
	-- {
	-- 	'Shougo/ddc.vim',
	-- 	event = "InsertEnter",
	-- 	dependencies = {
	-- 		'vim-denops/denops.vim',
	-- 		'vim-skk/skkeleton',
	-- 		'Shougo/pum.vim',
	-- 		'Shougo/ddc-ui-pum',
	-- 		'Shougo/ddc-source-around',
	-- 		'Shougo/ddc-source-lsp',
	-- 		'matsui54/ddc-buffer',
	-- 		'Shougo/ddc-source-cmdline',
	-- 		'Shougo/ddc-source-cmdline-history',
	-- 		'LumaKernel/ddc-source-file',
	-- 		'Shougo/ddc-source-input',
	-- 		'Shougo/ddc-source-nvim-lua',
	-- 		'Shougo/ddc-matcher_head',
	-- 		'Shougo/ddc-sorter_rank',
	-- 		'tani/ddc-fuzzy',
	-- 	},
	-- 	config = function ()
	-- 		require('plugins.ddc')
	-- 	end
	-- },
	--
	-- -- ddc UI
	-- 'Shougo/ddc-ui-pum',
	-- 'Shougo/pum.vim',
	--
	-- -- ddc source
	-- 'Shougo/ddc-source-around',
	-- {
	-- 	'Shougo/ddc-source-lsp',
	-- 	dependencies = {
	-- 		'neovim/nvim-lspconfig',
	-- 		'Shougo/ddc.vim'
	-- 	},
	-- },
	-- 'matsui54/ddc-buffer',
	-- 'Shougo/ddc-source-cmdline',
	-- 'Shougo/ddc-source-cmdline-history',
	-- 'LumaKernel/ddc-source-file',
	-- 'Shougo/ddc-source-input',
	-- 'Shougo/ddc-source-nvim-lua',
	--
	-- -- ddc filter
	-- 'Shougo/ddc-matcher_head',
	-- 'Shougo/ddc-sorter_rank',
	-- 'tani/ddc-fuzzy',

	-- lspconfig
	-- {
	-- 	'neovim/nvim-lspconfig',
	-- 	lazy = false,
	-- 	event = 'BufRead',
	-- 	config = function ()
	-- 		require('plugins.nvim-lspconfig')
	-- 	end
	-- },
	-- {
	-- 	'williamboman/mason.nvim',
	-- 	lazy = false,
	-- 	config = function ()
	-- 		require('plugins.mason')
	-- 	end
	-- },
	-- {
	-- 	'williamboman/mason-lspconfig.nvim',
	-- 	dependencies = {
	-- 		'williamboman/mason.nvim',
	-- 	}
	-- },

	-- codesnap
	{
		"mistricky/codesnap.nvim",
		build = "make",
		cmd = { "CodeSnap", "CodeSnapSave" },
		config = function()
			require("codesnap").setup({
				mac_window_bar = true,
				title = "",
				code_font_family = "CaskaydiaCove Nerd Font",
				watermark_font_family = "Pacifico",
				watermark = "",
				bg_theme = "bamboo",
				breadcrumbs_separator = "/",
				has_breadcrumbs = false,
				save_path = "~/codesnap/out.png",
			})
		end,
	},

	{
		"folke/flash.nvim",
		event = "VeryLazy",
		---@type Flash.Config
		opts = {},
		-- stylua: ignore
		keys = {
			{ "s", mode = { "n", "x", "o" }, function() require("flash").jump() end, desc = "Flash" },
			{ "S", mode = { "n", "x", "o" }, function() require("flash").treesitter() end, desc = "Flash Treesitter" },
			{ "r", mode = "o", function() require("flash").remote() end, desc = "Remote Flash" },
			{ "R", mode = { "o", "x" }, function() require("flash").treesitter_search() end, desc = "Treesitter Search" },
			{ "<c-s>", mode = { "c" }, function() require("flash").toggle() end, desc = "Toggle Flash Search" },
		},
		config = function()
			require("plugins.flash")
		end,
	},
	-- hop
	{
		"smoka7/hop.nvim",
		version = "*",
		event = "VimEnter",
		opts = {
			keys = "etvxqdygfblzhcksurn",
		},
		config = function()
			require("plugins.hop")
		end,
	},
	-- {
	-- 	"Daiki48/bootime.nvim",
	-- 	lazy = false,
	-- 	dev = true,
	-- 	cmd = "TestBuf",
	-- 	config = function()
	-- 		require("bootime").setup()
	-- 	end,
	-- },
}

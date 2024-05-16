return {
  -- colorsheme
  {
  'Daiki48/coolnessFlair.vim',
	lazy = false,
	branch = 'develop',
  config = function()
    vim.cmd([[colorscheme coolnessFlair]])
  end
  },
  'editorconfig/editorconfig-vim',
  {
    'nvim-lualine/lualine.nvim',
		lazy = false,
    dependencies = { 'nvim-tree/nvim-web-devicons' },
    config = function()
			require("daiki.plugins.lualine")
    end
  },
  {
    'lewis6991/gitsigns.nvim',
		lazy = false,
    config = function()
      require('gitsigns').setup()
    end
  },
  {
    'numToStr/Comment.nvim',
		lazy = false,
    config = function()
        require('Comment').setup()
        vim.api.nvim_set_keymap('n', '<C-_>', 'gcc', {})
        vim.api.nvim_set_keymap('v', '<C-_>', 'gc', {})
    end
  },
  {
    'nvim-tree/nvim-tree.lua',
		lazy = false,
    dependencies = {
      'nvim-tree/nvim-web-devicons',
    },
    config = function()
			require('daiki.plugins.nvim-tree')
     end
  },
	{
		'windwp/nvim-autopairs',
		event = 'InsertEnter',
		config = true
	},
	{
		'dstein64/vim-startuptime',
		cmd = 'StartupTime'
	},
	{
		'nvim-telescope/telescope.nvim', tag = '0.1.6',
		lazy = false,
		dependencies = {
			'nvim-lua/plenary.nvim',
		},
    config = function()
			require('daiki.plugins.telescope')
	  end
	},

	-- Git
	{
		'NeogitOrg/neogit',
		cmd = 'Neogit',
		dependencies = {
			'nvim-lua/plenary.nvim',
			'sindrets/diffview.nvim',
			'nvim-telescope/telescope.nvim',
		},
		config = function()
			require('neogit').setup()
		end
	},

	-- coc
	{
		'neoclide/coc.nvim',
		branch = 'release',
		lazy = false,
		config = function ()
			require('daiki.plugins.coc')
		end
	},

	{
		'leafOfTree/vim-svelte-plugin',
		ft = 'svelte',
		config = true
	},

	-- treesitter
	{
		'nvim-treesitter/nvim-treesitter',
		build = ':TSUpdate',
		lazy = false,
		config = function ()
			require('daiki.plugins.treesitter')
		end
	},
	{
		'windwp/nvim-ts-autotag',
		lazy = false,
		dependencies = {
			'nvim-treesitter/nvim-treesitter',
		}
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
	-- 		require('daiki.plugins.skkeleton')
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
	-- 		require('daiki.plugins.cmp')
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
	-- 		require('daiki.plugins.ddc')
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
	{
		'neovim/nvim-lspconfig',
		lazy = false,
		event = 'BufRead',
		config = function ()
			require('daiki.plugins.nvim-lspconfig')
		end
	},
	{
		'williamboman/mason.nvim',
		lazy = false,
		config = function ()
			require('daiki.plugins.mason')
		end
	},
	{
		'williamboman/mason-lspconfig.nvim',
		dependencies = {
			'williamboman/mason.nvim',
		}
	},

	-- codesnap
	{
		"mistricky/codesnap.nvim",
		build = "make",
		cmd = {'CodeSnap', 'CodeSnapSave'},
		config = function ()
			require("codesnap").setup({
				mac_window_bar = true,
				title = "",
				code_font_family = "CaskaydiaCove Nerd Font",
				watermark_font_family = "Pacifico",
				watermark = "",
				bg_theme = "bamboo",
				breadcrumbs_separator = "/",
				has_breadcrumbs = false,
				save_path = "~/codesnap/out.png"
			})
		end
	},
}

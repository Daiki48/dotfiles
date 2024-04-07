return {
  -- colorsheme
  {
  'Daiki48/coolnessFlair.vim',
  config = function()
    vim.cmd([[colorscheme coolnessFlair]])
  end
  },
  'editorconfig/editorconfig-vim',
  {
    'nvim-lualine/lualine.nvim',
    dependencies = { 'nvim-tree/nvim-web-devicons' },
    config = function()
			require("daiki.plugins.lualine")
    end
  },
  {
    'lewis6991/gitsigns.nvim',
    config = function()
      require('gitsigns').setup()
    end
  },
  {
    'numToStr/Comment.nvim',
    config = function()
        require('Comment').setup()
        vim.api.nvim_set_keymap('n', '<C-_>', 'gcc', {})
        vim.api.nvim_set_keymap('v', '<C-_>', 'gc', {})
    end
  },
  {
    'nvim-tree/nvim-tree.lua',
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
	}
}

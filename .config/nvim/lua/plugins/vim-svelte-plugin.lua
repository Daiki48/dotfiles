return {
  "leafOfTree/vim-svelte-plugin",
  ft = "svelte",
  config = function()
    vim.g.vim_svelte_plugin_load_full_syntax = 1
    vim.g.vim_svelte_plugin_use_typescript = 1
  end,
}

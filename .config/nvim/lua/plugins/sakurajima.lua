return {
  "Daiki48/sakurajima.nvim",
  priority = 1000,
  lazy = false,
  branch = "main",
  -- branch = "develop",
  -- dev = true,
  config = function()
    vim.cmd([[colorscheme sakurajima]])
  end,
}

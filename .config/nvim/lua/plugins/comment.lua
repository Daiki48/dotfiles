return {
  "numToStr/Comment.nvim",
  -- lazy = false,
  event = "BufRead",
  config = function()
    require("Comment").setup()
    vim.api.nvim_set_keymap("n", "<C-_>", "gcc", {})
    vim.api.nvim_set_keymap("v", "<C-_>", "gc", {})
  end,
}

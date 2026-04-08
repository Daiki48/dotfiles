return {
  "numToStr/Comment.nvim",
  -- lazy = false,
  event = "BufRead",
  config = function()
    require("Comment").setup()
    vim.keymap.set("n", "<C-_>", "gcc", { remap = true })
    vim.keymap.set("v", "<C-_>", "gc", { remap = true })
  end,
}

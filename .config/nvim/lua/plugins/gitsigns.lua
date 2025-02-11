return {

  "lewis6991/gitsigns.nvim",
  event = "BufRead",
  -- lazy = false,
  config = function()
    require("gitsigns").setup()
  end,
}

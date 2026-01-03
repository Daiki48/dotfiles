return {
  "rayliwell/tree-sitter-rstml",
  dependencies = { "nvim-treesitter/nvim-treesitter" },
  build = ":TSInstall! rust_with_rstml",
  ft = "rust",
  config = function()
    require("tree-sitter-rstml").setup()
  end,
}

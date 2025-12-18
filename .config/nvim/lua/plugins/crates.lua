return {
  "saecki/crates.nvim",
  tag = "stable",
  event = { "BufRead Cargo.toml" },
  config = function()
    require("crates").setup({
      lsp = {
        enabled = true,
        actions = true,
        completion = true,
        hover = true,
      },
    })
  end,
  keys = {
    {
      "<leader>ct",
      function()
        require("crates").toggle()
      end,
      desc = "Toggle crates",
      ft = "toml",
    },
    {
      "<leader>cu",
      function()
        require("crates").update_crate()
      end,
      desc = "Update crate",
      ft = "toml",
    },
    {
      "<leader>cU",
      function()
        require("crates").upgrade_crate()
      end,
      desc = "Upgrade crate",
      ft = "toml",
    },
    {
      "<leader>cv",
      function()
        require("crates").show_versions_popup()
      end,
      desc = "Show versions",
      ft = "toml",
    },
    {
      "<leader>cf",
      function()
        require("crates").show_features_popup()
      end,
      desc = "Show features",
      ft = "toml",
    },
    {
      "<leader>cd",
      function()
        require("crates").show_dependencies_popup()
      end,
      desc = "Show dependencies",
      ft = "toml",
    },
  },
}

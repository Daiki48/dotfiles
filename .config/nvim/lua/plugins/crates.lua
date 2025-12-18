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
    { ";ct", function() require("crates").toggle() end, desc = "Toggle crates", ft = "toml" },
    { ";cu", function() require("crates").update_crate() end, desc = "Update crate", ft = "toml" },
    { ";cU", function() require("crates").upgrade_crate() end, desc = "Upgrade crate", ft = "toml" },
    { ";cv", function() require("crates").show_versions_popup() end, desc = "Show versions", ft = "toml" },
    { ";cf", function() require("crates").show_features_popup() end, desc = "Show features", ft = "toml" },
    { ";cd", function() require("crates").show_dependencies_popup() end, desc = "Show dependencies", ft = "toml" },
  },
}

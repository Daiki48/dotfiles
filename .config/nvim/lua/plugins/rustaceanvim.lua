-- lua/plugins/rustaceanvim.lua
return {
  "mrcjkb/rustaceanvim",
  version = "^7",
  lazy = false,
  init = function()
    vim.g.rustaceanvim = {
      server = {
        default_settings = {
          ["rust-analyzer"] = {
            files = {
              watcherExclude = {
                ".git",
                ".github",
                "target",
                "book",
                "assets",
                "docker",
                "crates/protocol/registry/superchain-registry",
                "data",
                "monorepo",
              },
              excludeDirs = {
                ".git",
                ".github",
                "target",
                "book",
                "assets",
                "docker",
                "crates/protocol/registry/superchain-registry",
                "data",
                "monorepo",
              },
            },
            check = {
              command = "clippy",
            },
            imports = {
              granularity = {
                group = "module",
              },
              prefix = "self",
            },
            cargo = {
              buildScripts = {
                enable = true,
              },
            },
            procMacro = {
              enable = true,
            },
          },
        },
      },
    }
  end,
  keys = {
    {
      "<leader>rm",
      function()
        vim.cmd.RustLsp("expandMacro")
      end,
      desc = "Expand Macro",
      ft = "rust",
    },
    {
      "<leader>rc",
      function()
        vim.cmd.RustLsp("openCargo")
      end,
      desc = "Open Cargo.toml",
      ft = "rust",
    },
    {
      "<leader>rp",
      function()
        vim.cmd.RustLsp("parentModule")
      end,
      desc = "Parent Module",
      ft = "rust",
    },
    {
      "<leader>rd",
      function()
        vim.cmd.RustLsp("renderDiagnostic")
      end,
      desc = "Render Diagnostic",
      ft = "rust",
    },
    {
      "<leader>rr",
      function()
        vim.cmd.RustLsp("runnables")
      end,
      desc = "Runnables",
      ft = "rust",
    },
    {
      "<leader>rt",
      function()
        vim.cmd.RustLsp("testables")
      end,
      desc = "Testables",
      ft = "rust",
    },
    {
      "K",
      function()
        vim.cmd.RustLsp({ "hover", "actions" })
      end,
      desc = "Hover Actions",
      ft = "rust",
    },
    {
      "J",
      function()
        vim.cmd.RustLsp("joinLines")
      end,
      desc = "Join Lines",
      ft = "rust",
    },
  },
}

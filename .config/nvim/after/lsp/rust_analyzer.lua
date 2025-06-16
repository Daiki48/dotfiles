return {
  cmd = { "rust-analyzer" },
  filetypes = { "rust" },
  root_markers = { "Cargo.toml" },
  settings = {
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
}

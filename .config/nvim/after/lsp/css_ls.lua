return {
  cmd = {
    "vscode-css-language-server",
    "--stdio",
  },
  filetypes = {
    "css",
    "scss",
    "less",
  },
  settings = {
    ["cssls"] = {
      init_options = {
        provideFormatter = true,
      },
    },
  },
}

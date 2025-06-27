Snacks = Snacks

return {
  "folke/snacks.nvim",
  priority = 1000,
  lazy = false,
  opts = {
    dashboard = {
      enabled = true,
      preset = {
        header = [[
 ██╗  ██╗  █████╗  ██████╗  ██████╗  ██╗   ██╗     ██╗  ██╗  █████╗   ██████╗ ██╗  ██╗ ██╗ ███╗   ██╗  ██████╗
 ██║  ██║ ██╔══██╗ ██╔══██╗ ██╔══██╗ ╚██╗ ██╔╝     ██║  ██║ ██╔══██╗ ██╔════╝ ██║ ██╔╝ ██║ ████╗  ██║ ██╔════╝
 ███████║ ███████║ ██████╔╝ ██████╔╝  ╚████╔╝      ███████║ ███████║ ██║      █████╔╝  ██║ ██╔██╗ ██║ ██║  ███╗
 ██╔══██║ ██╔══██║ ██╔═══╝  ██╔═══╝    ╚██╔╝       ██╔══██║ ██╔══██║ ██║      ██╔═██╗  ██║ ██║╚██╗██║ ██║   ██║
 ██║  ██║ ██║  ██║ ██║      ██║         ██║        ██║  ██║ ██║  ██║ ╚██████╗ ██║  ██╗ ██║ ██║ ╚████║ ╚██████╔╝
 ╚═╝  ╚═╝ ╚═╝  ╚═╝ ╚═╝      ╚═╝         ╚═╝        ╚═╝  ╚═╝ ╚═╝  ╚═╝  ╚═════╝ ╚═╝  ╚═╝ ╚═╝ ╚═╝  ╚═══╝  ╚═════╝]],
      },
    },
    bigfile = { enabled = true },
    notifier = { enabled = true },
    quickfile = { enabled = true },
    statuscolumn = { enabled = true },
    words = { enabled = true },
    scroll = { enabled = false },
    styles = {
      notification = {
        wo = { wrap = true },
      },
    },
    picker = {
      enabled = true,
      explorer = {
        opts = {
          win = {
            list = {
              keys = {
                ["<c-]>"] = "explorer_cd",
              },
            },
          },
        },
      },
      previewers = {
        git = {
          native = true,
        },
      },
    },
    win = {
      -- input window
      input = {
        keys = {
          -- ["<Esc>"] = { "close", mode = { "n", "i" } },
          -- ["<S-k>"] = { "preview_scroll_up", mode = { "i", "n" } },
          -- ["<S-j>"] = { "preview_scroll_down", mode = { "i", "n" } },
          -- ["<C-k>"] = { "list_scroll_up", mode = { "i", "n" } },
          -- ["<C-j>"] = { "list_scroll_down", mode = { "i", "n" } },
        },
      },
    },
  },
  keys = {
    -- Using oil.nvim
    {
      ";se",
      function()
        Snacks.picker.explorer()
      end,
      desc = "explorer",
    },
    {
      ";sm",
      function()
        Snacks.picker.smart()
      end,
      desc = "smart files",
    },
    {
      ";gr",
      function()
        Snacks.picker.grep()
      end,
      desc = "grep",
    },
    {
      ";gw",
      function()
        Snacks.picker.grep_word()
      end,
      desc = "grep word",
    },
    {
      ";pl",
      function()
        Snacks.picker.projects()
      end,
      desc = "projects list",
    },

    {
      ";di",
      function()
        Snacks.picker.diagnostics()
      end,
      desc = "diagnostics",
    },

    {
      ";sf",
      function()
        Snacks.picker.files()
      end,
      desc = "files",
    },
    {
      ";bf",
      function()
        Snacks.picker.buffers()
      end,
      desc = "buffers",
    },
    {
      ";sh",
      function()
        Snacks.picker.recent()
      end,
      desc = "recent files",
    },
    {
      "<leader>sH",
      function()
        Snacks.picker.command_history()
      end,
      desc = "command history",
    },
    {
      ";ss",
      function()
        Snacks.picker.search_history()
      end,
      desc = "search history",
    },
    {
      ";sq",
      function()
        Snacks.picker.qflist()
      end,
      desc = "quickfix list",
    },
    {
      ";sc",
      function()
        Snacks.picker.colorschemes()
      end,
      desc = "color schemes",
    },
    {
      ";sd",
      function()
        Snacks.picker.files({ cwd = vim.fn.stdpath("config") })
      end,
      desc = "dotfiles",
    },

    {
      ";gf",
      function()
        Snacks.picker.git_files()
      end,
      desc = "git files",
    },
    {
      ";gs",
      function()
        Snacks.picker.git_status()
      end,
      desc = "git status",
    },
    {
      ";gd",
      function()
        Snacks.picker.git_diff()
      end,
      desc = "Git Diff (Hunks)",
    },
    {
      ";gl",
      function()
        Snacks.picker.git_log()
      end,
      desc = "log",
    },
    {
      ";gc",
      function()
        Snacks.picker.git_log_file()
      end,
      desc = "file commits log",
    },
    {
      ";gb",
      function()
        Snacks.picker.git_branches()
      end,
      desc = "git branch",
    },
    {
      ";ld",
      function()
        Snacks.picker.lsp_definitions()
      end,
      desc = "Goto Definition",
    },
    {
      ";lD",
      function()
        Snacks.picker.lsp_declarations()
      end,
      desc = "Goto Declaration",
    },
    {
      ";lr",
      function()
        Snacks.picker.lsp_references()
      end,
      nowait = true,
      desc = "References",
    },
    {
      ";li",
      function()
        Snacks.picker.lsp_implementations()
      end,
      desc = "Goto Implementation",
    },
    {
      ";lt",
      function()
        Snacks.picker.lsp_type_definitions()
      end,
      desc = "Goto T[y]pe Definition",
    },
    {
      ";ls",
      function()
        Snacks.picker.lsp_symbols()
      end,
      desc = "LSP Symbols",
    },
    {
      ";lw",
      function()
        Snacks.picker.lsp_workspace_symbols()
      end,
      desc = "LSP Workspace Symbols",
    },
  },
}

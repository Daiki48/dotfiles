Snacks = Snacks

return {
  "folke/snacks.nvim",
  priority = 1000,
  lazy = false,
  opts = {
    -- your configuration comes here
    -- or leave it empty to use the default settings
    -- refer to the configuration section below
    dashboard = {
      enabled = true,
      preset = {
        header = [[
　　　　＠　　　　　　　　　　　　　　　　　　　　＠　　　　　　　　　　　　
　　　　＠＠　　　　　　　　　　＠＠＠　　　　　　＠　　　＠　　　　　　　　
　　　　＠＠　　　　　　　　　＠＠　　　　　　　　＠　　　　　　　　　　　　
　　＠＠＠＠　＠＠＠＠＠＠　　＠＠＠　　＠＠＠　　＠　　　＠　　　　＠＠＠　
　＠　　＠＠　　＠　　　＠　　＠＠　　＠　　＠＠　＠　　　＠　　　＠　　＠＠
＠＠　　＠＠　　＠　　　＠　　＠＠　＠＠　　　＠　＠　　　＠　　＠＠　　　＠
＠＠　　＠＠　　＠　　　＠　　＠＠　＠＠　　　＠　＠　　　＠　　＠＠　　　＠
　＠　　＠＠　　＠　　　＠　　＠＠　　＠　　＠＠　＠　　　＠　　　＠　　＠＠
　＠＠＠＠＠　　＠＠　＠＠＠　＠＠　　　＠＠＠　　＠＠　　＠＠　　　＠＠＠　]],
      },
    },
    bigfile = { enabled = true },
    notifier = { enabled = true },
    quickfile = { enabled = true },
    statuscolumn = { enabled = true },
    words = { enabled = true },
    styles = {
      notification = {
        wo = { wrap = true }, -- Wrap notifications
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
          native = true, -- use native (terminal) or Neovim for previewing git diffs and commits
        },
      },
    },
    win = {
      -- input window
      input = {
        keys = {
          -- ["<Esc>"] = { "close", mode = { "n", "i" } },
          ["<c-u>"] = { "preview_scroll_up", mode = { "i", "n" } },
          ["<c-d>"] = { "preview_scroll_down", mode = { "i", "n" } },
          ["<c-b>"] = { "list_scroll_up", mode = { "i", "n" } },
          ["<c-f>"] = { "list_scroll_down", mode = { "i", "n" } },
        },
      },
    },
  },
  keys = {
    {
      "<C-e>",
      function()
        Snacks.picker.explorer()
      end,
      desc = "explorer",
    },
    {
      "<C-p>",
      function()
        Snacks.picker.smart()
      end,
      desc = "smart files",
    },
    {
      "<S-p>",
      function()
        Snacks.picker.grep()
      end,
      desc = "grep",
    },
  },
}

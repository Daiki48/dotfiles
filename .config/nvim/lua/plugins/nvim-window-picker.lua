return {
  "s1n7ax/nvim-window-picker",
  name = "window-picker",
  event = "VeryLazy",
  version = "2.*",
  keys = {
    {
      ";ww",
      function()
        local win_id = require("window-picker").pick_window()
        if win_id then
          vim.api.nvim_set_current_win(win_id)
        end
      end,
      desc = "Pick a window",
    },
  },
  config = function()
    require("window-picker").setup({
      filter_rules = {
        bo = {
          filetype = {},
          buftype = {},
        },
      },
      highlights = {
        statusline = {
          focused = {
            fg = "#1a1a1a",
            bg = "#98c379",
            bold = true,
          },
          unfocused = {
            fg = "#1a1a1a",
            bg = "#7fbf7f",
            bold = true,
          },
        },
      },
    })
  end,
}

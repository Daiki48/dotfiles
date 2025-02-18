return {
  "saghen/blink.cmp",
  dependencies = "rafamadriz/friendly-snippets",
  event = "InsertEnter",
  version = "*",
  opts = {
    keymap = {
      preset = "default",
      ["<C-u>"] = { "scroll_documentation_up", "fallback" },
      ["<C-d>"] = { "scroll_documentation_down", "fallback" },
      ["<S-Tab>"] = { "select_prev", "fallback" },
      ["<Tab>"] = { "select_next", "fallback" },
      ["<C-y>"] = { "select_and_accept" },
    },
    appearance = {
      use_nvim_cmp_as_default = false,
      nerd_font_variant = "mono",
    },
    sources = {
      default = { "lsp", "path", "snippets", "buffer" },
    },
    completion = {
      keyword = {
        range = "full",
      },
      list = {
        selection = {
          preselect = true,
          auto_insert = true,
        },
      },
      menu = {
        border = "single",
        auto_show = true,
        draw = {
          columns = { { "kind_icon" }, { "label", "label_description", gap = 1 }, { "kind_icon", "kind" } },
        },
      },
      documentation = {
        window = {
          border = "single",
        },
        auto_show = true,
        auto_show_delay_ms = 500,
      },
      ghost_text = { enabled = true },
    },
    signature = {
      window = {
        border = "single",
      },
    },
    cmdline = {
      enabled = false,
    },
    snippets = {
      preset = "default",
    },
  },
  opts_extend = { "sources.default" },
}

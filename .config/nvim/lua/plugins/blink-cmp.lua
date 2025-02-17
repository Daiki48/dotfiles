return {
  "saghen/blink.cmp",
  dependencies = "rafamadriz/friendly-snippets",
  event = "InsertEnter",
  version = "*",
  opts = {
    keymap = {
      preset = "default",
      ["<S-Tab>"] = { "select_prev", "fallback" },
      ["<Tab>"] = { "select_next", "fallback" },
    },
    appearance = {
      use_nvim_cmp_as_default = false,
      nerd_font_variant = "mono",
    },
    sources = {
      default = { "lsp", "path", "snippets", "buffer" },
    },
    completion = {
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
    },
    signature = {
      window = {
        border = "single",
      },
    },
    cmdline = {
      enabled = false,
    },
  },
  opts_extend = { "sources.default" },
}

local pum = {
  set_option = vim.fn["pum#set_option"],
}

pum.set_option({
  border = {
    "●",
    "─",
    "●",
    "│",
    "●",
    "─",
    "●",
    "│",
  },
  highlight_columns = {
    kind = "PreProc",
    abbr = "String",
    menu = "MoreMsg",
  },
  highlight_matches = "Search",
  item_orders = { "abbr", "space", "kind", "space", "space", "menu" },
  padding = true,
  scrollbar_char = "⇳",
  min_width = 40,
})

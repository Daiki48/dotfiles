local pum = {
	set_option = vim.fn["pum#set_option"];
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
	item_orders = {"abbr", "kind", "menu"},
	padding = true,
	scrollbar_char = "⇳",
})

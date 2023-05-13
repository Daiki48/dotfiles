local ddu = {
	patch_global = vim.fn["ddu#custom#patch_global"];
	patch_local = vim.fn["ddu#custom#patch_local"];
	alias = vim.fn["ddu#custom#alias"];
}

-- local lines = vim.o.lines
-- local win_height = math.floor(lines / 4)
-- local win_row = math.floor(lines - win_height)
-- local win_col = math.floor(vim.o.columns)
-- local win_between = 3
-- local win_width = math.floor((vim.o.columns - win_col * 2 - win_between) / 2)

ddu.alias("source", "directory_rec", "file_external")

ddu.patch_global({
	ui = "ff",
	uiParams = {
		ff = {
			displaySourceName = "no",
			prompt = "> ",
			split = "floating",
			floatingBorder = "rounded",
			previewFloating = true,
			previewFloatingBorder = "rounded",
			filterSplitDirection = "floating",
			filterFloatingPosition = "top",
			startFilter = true,
			highlights = {
				floating = "DduUiFfFloating",
				prompt = "DduUiFfPrompt",
				selected = "DduUiFfSelected",
			},
		},
		filer = {
			split = "floating",
			sort = "filename",
			sortTreesFirst = true,
			toggle = true,
			highlights = {
				floating = "DduUiFilerFloating",
				selected = "DduUiFilerSelected"
			},
		},
	},
	uiOptions = {
		filer = {
			toggle = true,
		}
	},
	sourceOptions = {
		['_'] = {
			matchers = {"matcher_substring"}
		},
		file = {
			columns = {"icon_filename"},
		},
	},
	kindOptions = {
		file = {
			defaultAction = "open",
		},
    help = {
      defaultAction = "open",
    },
    word = {
      defaultAction = "append",
    },
    action = {
      defaultAction = "do",
    },
    readme_viewer = {
      defaultAction = "open",
    },
	},
	actionOptions = {
		narrow = {
			quit = false,
		},
	},
	filterParams = {
		matcher_substring = {
			highlightMatched = "DduUiFfFilterMatch",
		}
	},
})

ddu.patch_local("dein_plugin_update", {
	uiParams = {
		ff = {
			startFilter = false,
			winCol = 16,
			winWidth = 70,
		},
	},
	sources = {
		{
			name = "dein_update",
		}
	},
	sourceOptions = {
		dein_update = {
			matchers = {"matcher_dein_update"},
		},
	},
	actionOptions = {
		echo = {
			quit = false,
		},
		echoDiff = {
			quit = false,
		},
	},
	kindOptions = {
		dein_update = {
			defaultAction = "viewDiff",
		},
	},
})

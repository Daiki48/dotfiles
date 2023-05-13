M = {}

local ddc = {
	global = vim.fn["ddc#custom#patch_global"];
	filetype = vim.fn["ddc#custom#patch_filetype"];
}

ddc.global({
	ui = "pum",
	-- completionMenu = "pum.vim",
	autoCompleteEvents = { 'InsertEnter', 'TextChangedI', 'TextChangedP' },
	sources = {
		"neosnippet",
		"around",
		"nvim-lsp",
		"buffer",
		"nvim-lua",
		"cmdline",
		"cmdline-history",
		"file",
		"input",
	},
	sourceOptions = {
		['_'] = {
			matchers = {"matcher_fuzzy"},
			sorters = {"sorter_fuzzy"},
			converters = {"converter_fuzzy"},
			ignoreCase = true,
			maxItems = 3,
		},
		['neosnippet'] = {
			mark = "Snip",
			isVolatile = true,
			maxItems = 4,
		},
		['around'] = {
			mark = "Around",
			isVolatile = true,
			maxItems = 2,
		},
		['nvim-lsp'] = {
			mark = "Lsp",
			isVolatile = true,
			forceCompletionPattern = "\\.\\w*|:\\w*|->\\\\w*",
			minAutoCompleteLength = 1,
			maxItems = 4,
		},
		['buffer'] = {
			mark = "Buffer",
			isVolatile = true,
		},
		['cmdline'] = {
			mark = "CmdLine",
			isVolatile = true,
		},
		['cmdline-history'] = {
			mark = "CL-History",
			isVolatile = true,
		},
		['file'] = {
			mark = "File",
			isVolatile = true,
			forceCompletionPattern = "\\S/\\S*",
		},
		['input'] = {
			mark = "Input",
			isVolatile = true,
		},
		['nvim-lua'] = {
			mark = "Lua",
			isVolatile = true,
		},
	},
	sourceParams = {
		['around'] = {
			maxSize = 500,
		},
		['nvim-lsp'] = {
			kindLabels = {
				Text          = ' ',
				Method        = ' ',
				Function      = ' ',
				Constructor   = ' ',
				Field         = 'ﰠ ',
				Variable      = ' ',
				Class         = ' ',
				Interface     = ' ',
				Module        = ' ',
				Property      = ' ',
				Unit          = ' ',
				Value         = ' ',
				Enum          = ' ',
				Keyword       = ' ',
				Snippet       = '﬌ ',
				Color         = ' ',
				File          = ' ',
				Reference     = ' ',
				Folder        = ' ',
				EnumMember    = ' ',
				Constant      = ' ',
				Struct        = ' ',
				Event         = ' ',
				Operator      = 'ﬦ ',
				TypeParameter = ' ',
			},
		},
		['buffer'] = {
			requireSameFiletype = false,
			limitBytes = 5000000,
			fromAltBuf = true,
			forceCollect = true,
		},
	},
	filterParams = {
		-- matcher_fuzzy = {
		-- 	splitMode = "word",
		-- },
		converter_fuzzy = {
			hlGroup = "Question",
		},
	},
	backspaceCompletion = true,
	cmdlineSources = {
		[':'] = { "cmdline", "cmdline-history", "around" },
	}
})

ddc.filetype({"ps1", "dosbatch", "autohotkey", "registry"}, {
	sourceOptions = {
		file = {
			forceCompletionPattern = "\\S\\\\S*",
		},
	},
	sourceParams = {
		file = {
			mode = "win32",
		},
	},
})

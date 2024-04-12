--------------------------------
--------- ddc config -----------
--------------------------------
local ddc = {
	global = vim.fn["ddc#custom#patch_global"];
}

ddc.global({
	ui = 'pum',
	autoCompleteEvents = {
		'InsertEnter',
		'TextChangedI',
		'TextChangedP',
	},
	sources = {
		'around',
		'lsp',
		'buffer',
		'nvim-lua',
		'cmdline',
		'cmdline-history',
		'file',
		'input',
		'skkeleton',
	},
	sourceOptions = {
		['_'] = {
			matchers = {'matcher_head'},
			sorters = {'sorter_rank'}
		},
		['around'] = {
			mark = 'Around',
			isVolatile = true,
			maxItems = 2,
		},
		['lsp'] = {
			mark = 'Lsp',
			isVolatile = true,
			maxItems = 4,
			forceCompletionPattern = "\\.\\w*\\s{2}|:\\s{2}\\w*|->\\s{2}\\\\w*",
			minAutoCompleteLength = 1,
			dup = 'keep',
			keywordPattern = '\\k+',
			sorters = { 'sorter_lsp-kind' },
		},
		['buffer'] = {
			mark = 'Buffer',
			isVolatile = true,
		},
		['cmdline'] = {
			mark = 'CmdLine',
			isVolatile = true,
		},
		['cmdline-history'] = {
			mark = 'CL-History',
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
		['skkeleton'] = {
			mark = 'skkeleton',
			matchers = { 'skkeleton' },
			sorters = {},
			converters = {},
			isVolatile = true,
			minAutoCompleteLength = 1,
		}
	},
	sourceParams = {
		['around'] = {
			maxSize = 500,
		},
		['lsp'] = {
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
			enableResolveItem = true,
			enableAdditionalTextEdit = true,
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


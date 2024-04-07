-- lualine
local status, lualine = pcall(require, 'lualine')
if (not status) then return end

-- vim_mode select function
local function vim_mode()
  local map = {
    ['n'] = 'N',
    ['no'] = 'O',
    ['nov'] = 'O',
    ['noV'] = 'O',
    ['no\22'] = 'O',
    ['niI'] = 'N',
    ['niR'] = 'N',
    ['niV'] = 'N',
    ['nt'] = 'N',
    ['v'] = 'V',
    ['vs'] = 'V',
    ['V'] = 'L',
    ['Vs'] = 'L',
    ['\22'] = 'V',
    ['\22s'] = 'VB',
    ['s'] = 'S',
    ['S'] = 'SL',
    ['\19'] = 'SB',
    ['i'] = 'I',
    ['ic'] = 'I',
    ['ix'] = 'I',
    ['R'] = 'R',
    ['Rc'] = 'R',
    ['Rx'] = 'R',
    ['Rv'] = 'VR',
    ['Rvc'] = 'VR',
    ['Rvx'] = 'VR',
    ['c'] = 'C',
    ['cv'] = 'X',
    ['ce'] = 'X',
    ['r'] = 'P',
    ['rm'] = 'M',
    ['r?'] = 'F',
    ['!'] = 'S',
    ['t'] = 'T',
  }
  local mode_code = vim.api.nvim_get_mode().mode
  local mode_sign = map[mode_code]
  if mode_sign == nil then
    return mode_code
  else
    return mode_sign
  end
end

lualine.setup {
  options = {
    icons_enabled = true,
--    theme = 'iceberg_dark',
    component_separators = {left = '', right = ''},
    section_separators = {left = '', right = ''},
    disabled_filetypes = {}
  },
  sections = {
    lualine_a = {
			{ vim_mode },
			'g:coc_status'
    },
    lualine_b = {
			'g:coc_git_status',
      'branch',
			{
				'diff',
				diff_color = {
					added = {
						fg = "#DA523A"
					},
					modified = {
						fg = "#E8C473"
					},
					removed = {
						fg = "#659AD2"
					},
				},
			},
			{
				'diagnostics',
				source = { coc },
				diagnostics_color = {
					error = {
						fg = "#8f3231"
					},
					warn = {
						fg = "#C7A252"
					},
					info = {
						fg = "#E6D2C9"
					},
					hint = {
						fg = "#717375"
					},
				},
			}
    },
    lualine_c = {
      {
        'filename',
      }
    },
    lualine_x = {'encoding', 'filetype'},
    lualine_y = {'progress'},
    lualine_z = {'location'}
  },
  inactive_sections = {
    lualine_a = {},
    lualine_b = {},
    lualine_c = {'filename'},
    lualine_x = {'location'},
    lualine_y = {},
    lualine_z = {}
  },
  tabline = {
    lualine_a = {'buffers'},
    lualine_b = {},
    lualine_c = {},
    lualine_x = {},
    lualine_y = {},
    lualine_z = {'tabs'}
  },
  extensions = {}
}

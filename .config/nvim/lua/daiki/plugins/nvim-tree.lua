-- recommended settings from nvim-tree documentation
vim.g.loaded_netrw = 1
vim.g.loaded_netrwPlugin = 1

require("nvim-tree").setup({
        view = {
          width = 35,
          relativenumber = true,
        },
        -- change folder arrow icons
        renderer = {
          indent_markers = {
            enable = true,
          },
          icons = {
            glyphs = {
              folder = {
                arrow_closed = "", -- arrow when folder is closed
                arrow_open = "", -- arrow when folder is open
              },
            },
          },
        },
        -- disable window_picker for
        -- explorer to work well with
        -- window splits
        actions = {
          open_file = {
            window_picker = {
              enable = false,
            },
          },
        },
        filters = {
          custom = { ".DS_Store" },
        },
        git = {
          ignore = false,
        },

})

-- set keymaps
local keymap = vim.keymap -- for conciseness

keymap.set("n", ";ee", "<cmd>NvimTreeToggle<CR>", { desc = "Toggle file explorer" }) -- toggle file explorer
keymap.set("n", ";ef", "<cmd>NvimTreeFindFileToggle<CR>", { desc = "Toggle file explorer on current file" }) -- toggle file explorer on current file
keymap.set("n", ";ec", "<cmd>NvimTreeCollapse<CR>", { desc = "Collapse file explorer" }) -- collapse file explorer
keymap.set("n", ";er", "<cmd>NvimTreeRefresh<CR>", { desc = "Refresh file explorer" }) -- refresh file explorer


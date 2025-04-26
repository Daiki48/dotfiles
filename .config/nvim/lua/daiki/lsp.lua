-- #############################################
-- LSP utils
-- #############################################
vim.diagnostic.config({
  virtual_text = false,
  virtual_lines = false,
  underline = true,
})

vim.api.nvim_create_autocmd("CursorHold", {
  callback = function()
    local diagnostics = vim.diagnostic.get(0, { cursor = true })
    if #diagnostics > 0 then
      vim.diagnostic.open_float({
        border = "rounded",
      })
    end
  end,
})

-- #############################################
-- LSP config
-- #############################################
-- Lua
vim.lsp.config.lua_ls = {
  cmd = { "lua-language-server" },
  filetypes = { "lua" },
  root_markers = { ".luarc.json" },
  settings = {
    Lua = {
      runtime = {
        version = "LuaJIT",
      },
      diagnostics = {
        globals = { "vim" },
      },
    },
  },
}
vim.lsp.enable("lua_ls")

-- Rust
vim.lsp.config["rust-analyzer"] = {
  cmd = { "rust-analyzer" },
  filetypes = { "rust" },
  root_markers = { "Cargo.toml" },
  settings = {
    ["rust-analyzer"] = {
      check = {
        command = "clippy",
      },
      imports = {
        granularity = {
          group = "module",
        },
        prefix = "self",
      },
      cargo = {
        buildScripts = {
          enable = true,
        },
      },
      procMacro = {
        enable = true,
      },
    },
  },
}
vim.lsp.enable("rust-analyzer")

-- C, C++
vim.lsp.config["clangd"] = {
  cmd = { "clangd", "--background-index" },
  root_markers = { "compile_commands.json", "compile_flags.txt" },
  filetypes = { "c", "cpp" },
}
vim.lsp.enable("clangd")

-- Go
vim.lsp.config["gopls"] = {
	cmd = { "gopls" },
	root_markers = { "go.work", "go.mod", ".git/" },
	filetypes = { "go", "gomod", "gowork", "gotmpl" },
}
vim.lsp.enable("gopls")

-- TypeScript (Node.js)
vim.lsp.config.ts_ls = {
  cmd = {
    "typescript-language-server",
    "--stdio",
  },
  filetypes = {
    "javascript",
    "javascriptreact",
    "javascript.jsx",
    "typescript",
    "typescriptreact",
    "typescript.tsx",
  },
  root_markers = { "package.json", "tsconfig.json" },
  single_file_support = true,
}
vim.lsp.enable("ts_ls")

-- TypeScript (Deno)
vim.lsp.config.deno_ls = {
  cmd = {},
  filetypes = {},
  root_markers = { "deno.json", "deno.jsonc" },
  single_file_support = true,
}
vim.lsp.enable("deno_ls")

-- Svelte
vim.lsp.config.svelte = {
  cmd = {
    "svelteserver",
    "--stdio",
  },
  filetypes = {
    "svelte",
  },
}
vim.lsp.enable("svelte")

-- HTML
vim.lsp.config.html_ls = {
  cmd = {
    "vscode-html-language-server",
    "--stdio",
  },
  filetypes = {
    "html",
  },
}
vim.lsp.enable("html_ls")

-- CSS
vim.lsp.config.css_ls = {
  cmd = {
    "vscode-css-language-server",
    "--stdio",
  },
  filetypes = {
    "css",
    "scss",
    "less",
  },
  settings = {
    ["cssls"] = {
      init_options = {
        provideFormatter = true,
      },
    },
  },
}
vim.lsp.enable("css_ls")

-- #############################################
-- LSP completion
-- NOTE: Using saghen/blink.cmp
-- #############################################
-- vim.api.nvim_create_autocmd("LspAttach", {
--   callback = function(ev)
--     local client = vim.lsp.get_client_by_id(ev.data.client_id)
--     if client:supports_method("textDocument/completion") then
--       vim.lsp.completion.enable(true, client.id, ev.buf, { autotrigger = true })
--     end
--   end,
-- })

-- #############################################
-- LSP keymap
-- #############################################
vim.keymap.set("n", "g]", "<cmd>lua vim.diagnostic.goto_next()<CR>")
vim.keymap.set("n", "g[", "<cmd>lua vim.diagnostic.goto_prev()<CR>")
vim.keymap.set("i", "<Tab>", [[pumvisible() ? "\<C-n>" : "\<Tab>"]], { expr = true })
vim.keymap.set("i", "<S-Tab>", [[pumvisible() ? "\<C-p>" : "\<S-Tab>"]], { expr = true })

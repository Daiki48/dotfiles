return {
  "neovim/nvim-lspconfig",
  dependencies = { "saghen/blink.cmp", "j-hui/fidget.nvim" },
  event = { "BufReadPre", "BufNewFile" },
  opts = {
    servers = {
      lua_ls = {
        settings = {
          Lua = {
            runtime = {
              version = "LuaJIT",
            },
            workspace = {
              checkThirdParty = false,
              library = {
                vim.env.VIMRUNTIME,
              },
            },
            hint = {
              enable = true,
            },
            diagnostics = {
              globals = { "vim" },
            },
          },
        },
      },
      rust_analyzer = {
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
      },
      taplo = {
        settings = {
          ["taplo"] = {
            cmd = {
              "taplo",
              "lsp",
              "stdio",
            },
            filetypes = {
              "toml",
            },
            single_file_support = true,
          },
        },
      },
      ts_ls = {
        settings = {
          ["ts_ls"] = {
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
            init_options = {
              hostInfo = "neovim",
            },
            single_file_support = true,
          },
        },
      },
      denols = {
        settings = {
          ["denols"] = {},
        },
      },
      svelte = {
        settings = {
          ["svelte"] = {
            cmd = {
              "svelteserver",
              "--stdio",
            },
            filetypes = {
              "svelte",
            },
          },
        },
      },
      html = {
        settings = {
          ["html"] = {
            cmd = {
              "vscode-html-language-server",
              "--stdio",
            },
            filetypes = {
              "html",
              "templ",
            },
          },
        },
      },
      cssls = {
        settings = {
          ["cssls"] = {
            cmd = {
              "vscode-css-language-server",
              "--stdio",
            },
            filetypes = {
              "css",
              "scss",
              "less",
            },
            init_options = {
              provideFormatter = true,
            },
          },
        },
      },
      gopls = {
        settings = {
          ["gopls"] = {
            cmd = {
              "gopls",
            },
            filetypes = {
              "go",
              "gomod",
              "gowork",
              "gotmpl",
            },
            single_file_support = true,
          },
        },
      },
      tailwindcss = {
        settings = {
          ["tailwindcss"] = {
            cmd = {
              "tailwindcss-language-server",
              "--stdio",
            },
            filetypes = {
              "javascript",
              "javascriptreact",
              "typescript",
              "typescriptreact",
              "vue",
              "svelte",
            },
            classAttributes = { "class", "className", "class:list", "classList", "ngClass" },
            includeLanguages = {
              eelixir = "html-eex",
              eruby = "erb",
              htmlangular = "html",
              templ = "html",
            },
            lint = {
              cssConflict = "warning",
              invalidApply = "error",
              invalidConfigPath = "error",
              invalidScreen = "error",
              invalidTailwindDirective = "error",
              invalidVariant = "error",
              recommendedVariantOrder = "warning",
            },
            validate = true,
          },
        },
      },
    },
  },
  config = function(_, opts)
    local lspconfig = require("lspconfig")
    local root_pattern = require("lspconfig").util.root_pattern

    for server, config in pairs(opts.servers) do
      config.capabilities = require("blink.cmp").get_lsp_capabilities(config.capabilities)

      local is_ts = root_pattern("package.json", "tsconfig.json")()
      local is_deno = root_pattern("deno.json", "deno.jsonc")()

      if server == "ts_ls" and is_ts then
        config.root_dir = root_pattern("package.json", "tsconfig.json")
        lspconfig[server].setup(config)
      elseif server == "denols" and is_deno then
        config.root_dir = root_pattern("deno.json", "deno.jsonc")
        lspconfig[server].setup(config)
      elseif server ~= "ts_ls" and server ~= "denols" then
        lspconfig[server].setup(config)
      end
    end
  end,
}

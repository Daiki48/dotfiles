return {
  "nvimtools/none-ls.nvim",
  dependencies = { "nvim-lua/plenary.nvim", "vim-test/vim-test" },
  event = { "BufReadPre", "BufNewFile" },
  config = function()
    local null_ls = require("null-ls")
    local formatting = null_ls.builtins.formatting

    local sql_formatter = {
      name = "sql-formatter",
      method = null_ls.methods.FORMATTING,
      filetypes = { "sql" },
      generator = null_ls.generator({
        command = "sql-formatter",
        args = { "--config", "{}", "--" },
        to_stdin = true,
      }),
    }

    null_ls.setup({
      sources = {
        formatting.stylua,
        formatting.prettier.with({
          filetypes = {
            "html",
            "json",
            "yaml",
            "typescriptreact",
            "typescript",
            "javascriptreact",
            "javascript",
            "svelte",
          },
        }),
        sql_formatter,
      },
      debug = true,
    })

    vim.api.nvim_create_user_command("Stylua", function()
      local success, result = pcall(vim.lsp.buf.format, {
        filter = function(client)
          return client.name == "null-ls"
        end,
        async = false,
      })

      if not success then
        if string.find(result, "Stylua is not executable") then
          vim.notify("Failed: Please cargo install stylua", vim.log.levels.ERROR, { title = "Stylua Format Error" })
        else
          vim.notify(
            "Error during Stylua Formatting: " .. result,
            vim.log.levels.ERROR,
            { title = "Stylua Format Error" }
          )
        end
      end
    end, { desc = "Format buffer with Stylua (via null-ls)" })

    vim.api.nvim_create_user_command("Prettier", function()
      local success, result = pcall(vim.lsp.buf.format, {
        filter = function(client)
          return client.name == "null-ls"
        end,
        async = false,
      })

      if not success then
        if string.find(result, "Prettier is not executable") then
          vim.notify(
            "Failed: Please npm install -g prettier",
            vim.log.levels.ERROR,
            { title = "Prettier Format Error" }
          )
        else
          vim.notify(
            "Error during Prettier Formatting: " .. result,
            vim.log.levels.ERROR,
            { title = "Prettier Format Error" }
          )
        end
      end
    end, { desc = "Format buffer with Prettier (via null-ls)" })

    vim.api.nvim_create_user_command("RustFmt", function()
      local success, result = pcall(vim.lsp.buf.format, {
        filter = function(client)
          return client.name == "null-ls"
        end,
        async = false,
      })

      if not success then
        if string.find(result, "RustFmt is not executable") then
          vim.notify(
            "Failed: Please rustup component add rustfmt",
            vim.log.levels.ERROR,
            { title = "RustFmt Format Error" }
          )
        else
          vim.notify(
            "Error during RustFmt Formatting: " .. result,
            vim.log.levels.ERROR,
            { title = "RustFmt Format Error" }
          )
        end
      end
    end, { desc = "Format buffer with RustFmt (via null-ls)" })

    vim.api.nvim_create_user_command("SqlFmt", function()
      local success, result = pcall(vim.lsp.buf.format, {
        filter = function(client)
          return client.name == "null-ls"
        end,
        async = false,
      })

      if not success then
        if string.find(result, "sql-formatter is not executable") then
          vim.notify(
            "Failed: Please npm install -g sql-formatter",
            vim.log.levels.ERROR,
            { title = "SqlFmt Format Error" }
          )
        else
          vim.notify(
            "Error during SqlFmt Formatting: " .. result,
            vim.log.levels.ERROR,
            { title = "SqlFmt Format Error" }
          )
        end
      end
    end, { desc = "Format buffer with sql-formatter (via null-ls)" })
  end,
}

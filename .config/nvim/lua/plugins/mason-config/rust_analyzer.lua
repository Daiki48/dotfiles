M = {}

M.setup = function(config)
  config.on_attach = function(client, bufnr)
    if client.server_capabilities.documentFormattingProvider then
      vim.api.nvim_buf_set_keymap(
        bufnr,
        "n",
        "<leader>f",
        ":lua vim.lsp.buf.format({ async = true })<CR>",
        { noremap = true, silent = true }
      )
    end
  end
end

return M

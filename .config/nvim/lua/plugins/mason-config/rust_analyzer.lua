M = {}

M.setup = function(config)
  config.on_attach = function(client, bufnr)
    if client.server_capabilities.documentFormattingProvider then
      vim.keymap.set("n", "<leader>f", ":lua vim.lsp.buf.format({ async = true })<CR>", {
        buffer = bufnr,
        noremap = true,
        silent = true,
      })
    end
  end
end

return M

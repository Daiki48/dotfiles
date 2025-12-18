vim.lsp.set_log_level("OFF")

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

vim.api.nvim_create_autocmd("BufWritePre", {
  pattern = { "*.tf", "*.tfvars" },
  callback = function()
    print("Formatting Terraform file...")
    vim.lsp.buf.format({ async = false })
  end,
})

-- #############################################
-- LSP enable
-- #############################################
local lsp_name = {
  "lua_ls",
  -- "rust_analyzer",
  "clangd",
  "gopls",
  "ts_ls",
  "deno_ls",
  "svelte_ls",
  "html_ls",
  "css_ls",
  "jsonls",
  "terraform_ls",
}
vim.lsp.enable(lsp_name)

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

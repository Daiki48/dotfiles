return {
  "sbdchd/neoformat",
  event = { "BufReadPre", "BufNewFile" },
  opts = {},
  config = function()
    vim.g.neoformat_run_all_formatters = 1
  end,
}

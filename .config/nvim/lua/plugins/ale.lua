return {
  "dense-analysis/ale",
  cmd = { "ALEFix" },
  config = function()
    -- Configuration goes here.
    local g = vim.g

    g.ale_ruby_rubocop_auto_correct_all = 1

    g.ale_linters = {
      ruby = { "rubocop", "ruby" },
      lua = { "lua_language_server" },
    }

    g.ale_fixers = {
      typescript = { "prettier" },
      typescriptreact = { "prettier" },
      javascript = { "prettier" },
      javascriptreact = { "prettier" },
      css = { "prettier" },
      html = { "prettier" },
    }

    g.ale_javascript_prettier_options = "--single-quote --trailing-comma all"
    g.ale_linters_explicit = 1
    g.ale_fix_on_save = 1
    g.ale_javascript_prettier_use_local_config = 1
  end,
}

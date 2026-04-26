local parsers = {
  "c",
  "cpp",
  "c_sharp",
  "lua",
  "vim",
  "vimdoc",
  "query",
  "rust",
  "typescript",
  "javascript",
  "svelte",
  "markdown",
  "markdown_inline",
  "go",
  "json",
  "toml",
  "vue",
  "yaml",
  "xml",
  "html",
  "css",
}

local filetypes = {
  "c",
  "cpp",
  "cs",
  "css",
  "go",
  "html",
  "javascript",
  "json",
  "lua",
  "markdown",
  "rust",
  "svelte",
  "toml",
  "typescript",
  "vim",
  "vue",
  "xml",
  "yaml",
}

return {
  "nvim-treesitter/nvim-treesitter",
  branch = "main",
  build = function()
    local ok, treesitter = pcall(require, "nvim-treesitter")
    if not ok or type(treesitter.update) ~= "function" then
      return
    end

    treesitter.update(parsers):wait(300000)
  end,
  lazy = false,
  config = function()
    local treesitter = require("nvim-treesitter")

    if type(treesitter.install) ~= "function" then
      vim.notify(
        "nvim-treesitter is still on the old master branch. Run :Lazy sync to switch it to main.",
        vim.log.levels.WARN
      )
      return
    end

    treesitter.setup()
    vim.treesitter.language.register("c_sharp", "cs")
    treesitter.install(parsers)

    vim.api.nvim_create_autocmd("FileType", {
      pattern = filetypes,
      callback = function(args)
        vim.schedule(function()
          if not vim.api.nvim_buf_is_valid(args.buf) then
            return
          end

          -- FileType で遅延読み込みされるカスタムパーサー登録後に開始する
          if not pcall(vim.treesitter.start, args.buf) then
            return
          end

          for _, win in ipairs(vim.fn.win_findbuf(args.buf)) do
            vim.wo[win].foldexpr = "v:lua.vim.treesitter.foldexpr()"
            vim.wo[win].foldmethod = "expr"
            vim.wo[win].foldlevel = 99
          end
        end)
      end,
    })
  end,
}

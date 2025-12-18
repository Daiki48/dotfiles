-- スニペットヘルプ表示モジュール
local M = {}

local SNIPPET_PATH = vim.fn.stdpath("config") .. "/snippets/rust.json"

-- カテゴリ判定
local function get_category(prefix)
  if prefix:match("^l") then
    return "Leptos"
  elseif prefix:match("^ax") then
    return "Axum"
  elseif prefix:match("^sql") then
    return "SQLx"
  else
    return "Other"
  end
end

-- スニペットボディを文字列に変換
local function body_to_string(body)
  if type(body) == "table" then
    return table.concat(body, "\n")
  end
  return body or ""
end

-- JSONから読み込み
local function load_snippets()
  local file = io.open(SNIPPET_PATH, "r")
  if not file then
    vim.notify("スニペットファイルが見つかりません", vim.log.levels.ERROR)
    return {}
  end
  local content = file:read("*a")
  file:close()

  local ok, data = pcall(vim.json.decode, content)
  if not ok then
    vim.notify("JSONパースエラー", vim.log.levels.ERROR)
    return {}
  end

  local items = {}
  for name, snippet in pairs(data) do
    local body_str = body_to_string(snippet.body)
    table.insert(items, {
      text = snippet.prefix .. " " .. name .. " " .. (snippet.description or ""),
      prefix = snippet.prefix,
      name = name,
      description = snippet.description or "",
      category = get_category(snippet.prefix),
      body = snippet.body,
      -- snacks.nvimのプレビュー用フィールド
      preview = {
        text = body_str,
        ft = "rust",
      },
    })
  end
  table.sort(items, function(a, b)
    return a.prefix < b.prefix
  end)
  return items
end

-- フォーマット
local function format_item(item)
  local icons = { Leptos = " ", Axum = " ", SQLx = " " }
  return {
    { icons[item.category] or " ", "SnacksPickerIcon" },
    { item.prefix, "SnacksPickerLabel" },
    { "  ", "SnacksPickerDelimiter" },
    { item.description, "SnacksPickerComment" },
  }
end

-- スニペットを挿入
local function insert_snippet(body)
  local body_str = body_to_string(body)
  -- Neovim 0.10+ のスニペットAPI
  vim.snippet.expand(body_str)
end

-- 表示
function M.show()
  local items = load_snippets()
  if #items == 0 then
    return
  end

  Snacks.picker({
    title = "Rust Snippets",
    items = items,
    format = format_item,
    preview = "preview", -- 組み込みプレビューを使用（item.previewを表示）
    layout = {
      preset = "default", -- プレビュー付きレイアウト
    },
    confirm = function(picker, item)
      picker:close()
      -- スニペットを挿入
      insert_snippet(item.body)
    end,
    win = {
      input = {
        keys = {
          -- Ctrl+y でクリップボードにコピー（挿入せずに）
          ["<C-y>"] = {
            function(self)
              local item = self:current()
              if item then
                vim.fn.setreg("+", item.prefix)
                vim.notify("Copied: " .. item.prefix, vim.log.levels.INFO)
              end
            end,
            mode = { "i", "n" },
            desc = "Copy prefix",
          },
        },
      },
    },
  })
end

return M

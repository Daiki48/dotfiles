return {
  "mistricky/codesnap.nvim",
  build = "make",
  cmd = { "CodeSnap", "CodeSnapSave" },
  config = function()
    require("codesnap").setup({
      mac_window_bar = true,
      title = "",
      code_font_family = "CaskaydiaCove Nerd Font",
      watermark_font_family = "Pacifico",
      watermark = "",
      bg_theme = "bamboo",
      breadcrumbs_separator = "/",
      has_breadcrumbs = false,
      save_path = "~/codesnap/out.png",
    })
  end,
}

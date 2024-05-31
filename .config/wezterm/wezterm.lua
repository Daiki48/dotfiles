local wezterm = require 'wezterm'
local config = {}

if wezterm.config_builder then
  config = wezterm.config_builder()
end

config.color_scheme = 'Whimsy'
config.window_padding = {
    left = 0,
    right = 0,
    top = 0,
    bottom = 0,
}
config.window_background_opacity = 0.9
config.use_ime = true
config.initial_cols = 140
config.initial_rows = 30
config.keys = {
  { key = 'c', mods = 'CTRL', action = wezterm.action.CopyTo 'ClipboardAndPrimarySelection' },
  { key = 'v', mods = 'CTRL', action = wezterm.action.PasteFrom 'Clipboard' },
  { key = 'v', mods = 'CTRL', action = wezterm.action.PasteFrom 'PrimarySelection' },
}

-- You can specify some parameters to influence the font selection;
-- for example, this selects a Bold, Italic font variant.
config.font = wezterm.font('JetBrains Mono', { weight = 'Bold', italic = false })
-- config.font = wezterm.font('Source Han Sans JP', { weight = 'Bold', italic = false })
-- config.font = wezterm.font_with_fallback {
--   'Noto Sans CJK JP',
--   'Source Han Sans JP',
-- }

-- Font problem
-- No fonts contain glyphs for these codepoints: \u{e6b4}.
-- Placeholder glyphs are being displayed instead.
-- You may wish to install additional fonts, or adjust
-- your configuration so that it can find them.
config.warn_about_missing_glyphs = false

config.set_environment_variables = {
	LANG = "en_US.UTF-8"
}

return config

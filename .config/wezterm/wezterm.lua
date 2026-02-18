local wezterm = require("wezterm")
local config = {}

if wezterm.config_builder then
	config = wezterm.config_builder()
end

config.color_scheme = "GitHub Dark"
config.window_padding = {
	left = 0,
	right = 0,
	top = 0,
	bottom = 0,
}
config.window_background_opacity = 0.9
config.use_ime = true
config.ime_preedit_rendering = "Builtin"
config.initial_cols = 140
config.initial_rows = 30
config.keys = {
	{
		key = "c",
		mods = "CTRL",
		action = wezterm.action_callback(function(window, pane)
			local has_selection = window:get_selection_text_for_pane(pane) ~= ""
			if has_selection then
				window:perform_action(wezterm.action.CopyTo("ClipboardAndPrimarySelection"), pane)
				window:perform_action(wezterm.action.ClearSelection, pane)
			else
				window:perform_action(wezterm.action.SendKey({ key = "c", mods = "CTRL" }), pane)
			end
		end),
	},
	{ key = "v", mods = "CTRL", action = wezterm.action.PasteFrom("Clipboard") },
	-- Ctrl+Shift+T: 新規タブで新しいtmuxセッションを起動
	{
		key = "t",
		mods = "CTRL|SHIFT",
		action = wezterm.action.SpawnCommandInNewTab({
			args = { "tmux", "new-session" },
		}),
	},
}

-- Ghosttyと同じフォント構成
config.font = wezterm.font_with_fallback({
	{ family = "JetBrainsMono Nerd Font", weight = "Medium" },
	{ family = "Noto Sans Mono CJK JP" },
})

config.font_size = 10

-- Font problem
-- No fonts contain glyphs for these codepoints: \u{e6b4}.
-- Placeholder glyphs are being displayed instead.
-- You may wish to install additional fonts, or adjust
-- your configuration so that it can find them.
config.warn_about_missing_glyphs = false

config.set_environment_variables = {
	LANG = "ja_JP.UTF-8",
	LC_ALL = "ja_JP.UTF-8",
}

-- tmux自動起動: mainセッションにアタッチ（存在しなければ作成）
config.default_prog = { "tmux", "new-session", "-A", "-s", "main" }

-- ランチメニュー: 名前付きtmuxセッションをすぐ起動可能
config.launch_menu = {
	{
		label = "tmux: main",
		args = { "tmux", "new-session", "-A", "-s", "main" },
	},
	{
		label = "tmux: code (コーディング)",
		args = { "tmux", "new-session", "-A", "-s", "code" },
	},
	{
		label = "tmux: test (テスト)",
		args = { "tmux", "new-session", "-A", "-s", "test" },
	},
	{
		label = "tmux: 新規セッション",
		args = { "tmux", "new-session" },
	},
}

return config

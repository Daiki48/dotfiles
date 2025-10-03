use nvim_oxi::Result;
use nvim_oxi::api::opts::SetKeymapOpts;

pub fn setup() -> Result<()> {
    let keymap = nvim_oxi::api::set_keymap;
    let opts = SetKeymapOpts::builder();

    keymap(
        nvim_oxi::api::types::Mode::Normal,
        "<C-l>",
        "<cmd>bnext<cr>",
        &opts.clone().noremap(true).silent(true).build(),
    )?;

    keymap(
        nvim_oxi::api::types::Mode::Normal,
        "<C-h>",
        "<cmd>bprevious<cr>",
        &opts.clone().noremap(true).silent(true).build(),
    )?;

    keymap(
        nvim_oxi::api::types::Mode::Normal,
        "<C-q>",
        "<cmd>bd<cr>",
        &opts.clone().noremap(true).build(),
    )?;

    keymap(
        nvim_oxi::api::types::Mode::Normal,
        "<C-a>",
        "<C-v>",
        &opts.clone().noremap(true).build(),
    )?;

    keymap(
        nvim_oxi::api::types::Mode::Visual,
        "<C-a>",
        "<C-v>",
        &opts.clone().noremap(true).build(),
    )?;

    Ok(())
}

use nvim_oxi::api::opts::{EchoOpts, ExecOpts};
use nvim_oxi::conversion::ToObject;
use nvim_oxi::{Array, Dictionary, Function, Object};
use nvim_oxi::{
    Result,
    api::{self, opts::OptionOpts},
};

pub fn setup() -> Result<()> {
    let opts = OptionOpts::builder().build();
    let exec_opts = ExecOpts::builder().build();

    api::exec2("autocmd!", &exec_opts)?;

    api::set_var("mapleader", "<Space>")?;

    api::set_option_value("encoding", "utf-8", &opts)?;
    api::set_option_value("fileencoding", "utf-8", &opts)?;
    api::set_option_value("number", true, &opts)?;
    api::set_option_value("numberwidth", 8, &opts)?;
    api::set_option_value("relativenumber", false, &opts)?;
    api::set_option_value("signcolumn", "yes", &opts)?;
    api::set_option_value("scrolloff", 10, &opts)?;
    api::set_option_value("cmdheight", 1, &opts)?;
    api::set_option_value("inccommand", "split", &opts)?;
    api::set_option_value("breakindent", true, &opts)?;
    api::set_option_value("helplang", "ja,en", &opts)?;
    api::set_option_value("ignorecase", true, &opts)?;
    api::set_option_value("smartcase", true, &opts)?;
    api::set_option_value("splitright", true, &opts)?;
    api::set_option_value("termguicolors", true, &opts)?;
    // Zellij + Neovimの組み合わせで描画抜けが発生するため無効化
    api::set_option_value("termsync", false, &opts)?;
    api::set_option_value("hidden", true, &opts)?;
    api::set_option_value("updatetime", 300, &opts)?;
    api::set_option_value("tabstop", 2, &opts)?;
    api::set_option_value("shiftwidth", 2, &opts)?;
    api::set_option_value("cursorline", true, &opts)?;
    api::set_option_value("pumblend", 20, &opts)?;
    api::set_option_value("clipboard", "unnamedplus", &opts)?;
    api::set_option_value("swapfile", false, &opts)?;
    api::set_option_value("backup", false, &opts)?;
    api::set_option_value("wrap", false, &opts)?;
    api::set_option_value("winborder", "rounded", &opts)?;
    api::set_option_value("completeopt", "menuone,noselect,popup", &opts)?;
    api::set_option_value("expandtab", true, &opts)?;
    api::set_option_value("autoindent", false, &opts)?;
    api::set_option_value("smartindent", false, &opts)?;
    api::set_option_value("autoread", true, &opts)?;
    api::set_option_value("background", "dark", &opts)?;

    api::exec2("set wildoptions=pum", &exec_opts)?;

    setup_wsl_clipboard()?;

    Ok(())
}

fn setup_wsl_clipboard() -> Result<()> {
    if api::call_function::<_, i64>("has", ("wsl",))? == 1 {
        if api::call_function::<_, i64>("executable", ("wl-copy",))? == 0 {
            let msg = "wl-clipboard not found, clipboard integration won't work";
            let chunks: [(String, Option<String>); 1] = [(msg.to_string(), None)];
            api::echo(chunks, true, &EchoOpts::builder().build())?;
        } else {
            let copy_plus = "wl-copy --foreground --type text/plain";
            let copy_star = "wl-copy --foreground --primary --type text/plain";
            let paste_plus: Function<(), Object> = Function::from_fn_mut(|()| -> Result<Object> {
                let cmd = "wl-paste --no-newline | sed -e 's/\\r$//'";
                let args = (cmd, "", 1);
                let result: Array = api::call_function("systemlist", args)?;
                Ok(result.to_object()?)
            });
            let paste_star: Function<(), Object> = Function::from_fn_mut(|()| -> Result<Object> {
                let cmd = "wl-paste --primary --no-newline | sed -e 's/\\r$//'";
                let args = (cmd, "", 1);
                let result: Array = api::call_function("systemlist", args)?;
                Ok(result.to_object()?)
            });
            let copy_dict = Dictionary::from_iter([
                (nvim_oxi::String::from("+"), ToObject::to_object(copy_plus)?),
                (nvim_oxi::String::from("*"), ToObject::to_object(copy_star)?),
            ]);
            let paste_dict = Dictionary::from_iter([
                (
                    nvim_oxi::String::from("+"),
                    ToObject::to_object(paste_plus)?,
                ),
                (
                    nvim_oxi::String::from("*"),
                    ToObject::to_object(paste_star)?,
                ),
            ]);

            let clipboard_config = Dictionary::from_iter([
                (
                    nvim_oxi::String::from("name"),
                    ToObject::to_object("wl-clipboard (wsl)")?,
                ),
                (
                    nvim_oxi::String::from("copy"),
                    ToObject::to_object(copy_dict)?,
                ),
                (
                    nvim_oxi::String::from("paste"),
                    ToObject::to_object(paste_dict)?,
                ),
                (
                    nvim_oxi::String::from("cache_enabled"),
                    ToObject::to_object(1)?,
                ),
            ]);
            api::set_var("clipboard", clipboard_config)?;
        }
    }
    Ok(())
}

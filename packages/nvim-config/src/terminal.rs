use nvim_oxi::Result;
use nvim_oxi::api::opts::CreateCommandOpts;
use nvim_oxi::api::opts::ExecOpts;
use nvim_oxi::api::types::CommandArgs;
use nvim_oxi::api::types::Mode;
use nvim_oxi::api::{self, opts::SetKeymapOpts};

pub fn setup() -> Result<()> {
    let keymap_opts = SetKeymapOpts::builder().noremap(true).silent(true).build();
    api::set_keymap(Mode::Terminal, "<ESC>", "<C-\\><C-n>", &keymap_opts)?;

    let command_callback = |args: CommandArgs| -> Result<()> {
        let exec_opts = ExecOpts::builder().build();
        api::exec2("vsplit", &exec_opts)?;
        api::exec2("wincmd l", &exec_opts)?;

        let command_args = args.args.unwrap_or_default();

        if command_args.is_empty() && api::call_function::<_, i64>("has", ("win64",))? == 1 {
            let cmd = r#"terminal "C:\\Program Files\\PowerShell\\7\\pwsh.exe""#;
            api::exec2(cmd, &exec_opts)?;
        } else {
            api::exec2("terminal", &exec_opts)?;
        }

        let cmd = format!("terminal {}", command_args);
        api::exec2(&cmd, &exec_opts)?;

        Ok(())
    };

    let command_opts = CreateCommandOpts::builder()
        .nargs(api::types::CommandNArgs::Any)
        .build();

    api::create_user_command("T", command_callback, &command_opts)?;
    Ok(())
}

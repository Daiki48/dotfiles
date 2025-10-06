use nvim_oxi::{Dictionary, Function, Result};

mod keymaps;
mod options;
mod terminal;

#[nvim_oxi::plugin]
fn nvim_config() -> Result<Dictionary> {
    let setup = |()| -> Result<()> {
        keymaps::setup()?;
        options::setup()?;
        terminal::setup()?;
        Ok(())
    };
    Ok(Dictionary::from_iter([(
        "setup",
        Function::<(), ()>::from_fn(setup),
    )]))
}

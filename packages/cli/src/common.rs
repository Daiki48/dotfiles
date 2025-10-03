use clap::ValueEnum;

#[derive(ValueEnum, Clone, Debug)]
#[clap(rename_all = "kebab_case")]
pub enum Distro {
    Ubuntu,
    Fedora,
}

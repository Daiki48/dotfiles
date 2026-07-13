use anyhow::{Context, Result, bail};
use std::fs;
use std::process::{Command, Stdio};

use crate::common::Distro;
use crate::utils::{create_symlink, run_command};

const JETBRAINS_MONO_NERD_FONT: &str = "JetBrainsMono Nerd Font";
const NOTO_SANS_MONO_CJK_JP: &str = "Noto Sans Mono CJK JP";
const JETBRAINS_MONO_ARCHIVE_URL: &str =
    "https://github.com/ryanoasis/nerd-fonts/releases/latest/download/JetBrainsMono.tar.xz";

/// Execute setup for Wezterm
pub fn setup(distro: &Distro) -> Result<()> {
    if !is_wezterm_installed() {
        println!("Wezterm is not found.");
        println!("Starting wezterm install...");
        wezterm_install(distro)?;
    } else {
        println!("Wezterm is already installed.");
    }

    wezterm_check()?;
    setup_fonts(distro)?;

    println!("\nSetting up symbolic link for Wezterm config...");
    create_symlink(".config/wezterm", ".config/wezterm")?;
    Ok(())
}

/// Checking Wezterm version
fn is_wezterm_installed() -> bool {
    Command::new("wezterm")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn wezterm_install(distro: &Distro) -> Result<()> {
    match distro {
        Distro::Ubuntu => {
            let mut update_cmd = Command::new("sudo");
            update_cmd.arg("apt").arg("update");
            run_command(update_cmd, "Failed to run apt update.")?;

            let mut install_cmd = Command::new("sudo");
            install_cmd
                .arg("apt")
                .arg("install")
                .arg("-y")
                .arg("wezterm");
            run_command(install_cmd, "Failed to install wezterm.")?;
        }
        Distro::Fedora => {
            let mut cmd = Command::new("sudo");
            cmd.arg("dnf").arg("install").arg("-y").arg("https://github.com/wezterm/wezterm/releases/download/20240203-110809-5046fc22/wezterm-20240203_110809_5046fc22-1.fedora42.x86_64.rpm");
            run_command(cmd, "Failed to install wezterm.")?;
        }
    }
    Ok(())
}

fn wezterm_check() -> Result<()> {
    println!("\n------ Current Wezterm Version ------");
    match Command::new("wezterm").arg("--version").output() {
        Ok(output) => {
            if output.status.success() {
                print!("{}", String::from_utf8_lossy(&output.stdout));
            } else {
                eprintln!("Failed to get Wezterm version info. Stderr:");
                eprint!("{}", String::from_utf8_lossy(&output.stderr));
            }
        }
        Err(e) => {
            eprintln!("Failed to execute 'wezterm --version': {}", e);
        }
    }
    println!("---------------------------------");
    Ok(())
}

/// WezTermの設定で使用するフォントを準備
fn setup_fonts(distro: &Distro) -> Result<()> {
    println!("\n------ Setting up WezTerm fonts ------");

    let mut installed = false;

    if is_font_face_available(JETBRAINS_MONO_NERD_FONT, "Medium") {
        println!("{JETBRAINS_MONO_NERD_FONT} is already installed.");
    } else {
        install_jetbrains_mono_nerd_font()?;
        installed = true;
    }

    if is_font_available(NOTO_SANS_MONO_CJK_JP) {
        println!("{NOTO_SANS_MONO_CJK_JP} is already installed.");
    } else {
        install_noto_sans_mono_cjk(distro)?;
        installed = true;
    }

    if installed {
        let mut cmd = Command::new("fc-cache");
        cmd.arg("-f");
        run_command(cmd, "Failed to refresh the font cache.")?;
    }

    if !is_font_face_available(JETBRAINS_MONO_NERD_FONT, "Medium") {
        bail!(
            "Font installation completed, but '{JETBRAINS_MONO_NERD_FONT} Medium' could not be found."
        );
    }
    if !is_font_available(NOTO_SANS_MONO_CJK_JP) {
        bail!("Font installation completed, but '{NOTO_SANS_MONO_CJK_JP}' could not be found.");
    }

    println!("---------------------------------------");
    Ok(())
}

/// fontconfigから指定したフォントファミリーを検索
fn is_font_available(family: &str) -> bool {
    let output = Command::new("fc-match")
        .args(["--format", "%{family}\n", family])
        .output();

    match output {
        Ok(output) if output.status.success() => {
            font_family_matches(&String::from_utf8_lossy(&output.stdout), family)
        }
        _ => false,
    }
}

fn font_family_matches(output: &str, family: &str) -> bool {
    output
        .lines()
        .flat_map(|line| line.split(','))
        .any(|candidate| candidate.trim() == family)
}

/// 指定したフォントファミリーとスタイルの組み合わせを検索
fn is_font_face_available(family: &str, style: &str) -> bool {
    let query = format!("{family}:style={style}");
    let output = Command::new("fc-match")
        .args(["--format", "%{family}\t%{style}\n", &query])
        .output();

    match output {
        Ok(output) if output.status.success() => {
            font_face_matches(&String::from_utf8_lossy(&output.stdout), family, style)
        }
        _ => false,
    }
}

fn font_face_matches(output: &str, family: &str, style: &str) -> bool {
    output.lines().any(|line| {
        let Some((families, styles)) = line.split_once('\t') else {
            return false;
        };
        font_family_matches(families, family)
            && styles.split(',').any(|candidate| candidate.trim() == style)
    })
}

/// Nerd Fonts公式リリースからJetBrains Monoをユーザー領域へ導入
fn install_jetbrains_mono_nerd_font() -> Result<()> {
    println!("Installing {JETBRAINS_MONO_NERD_FONT}...");

    let home_path = home::home_dir().context("Failed to get home directory")?;
    let font_dir = home_path
        .join(".local")
        .join("share")
        .join("fonts")
        .join("JetBrainsMonoNerdFont");
    fs::create_dir_all(&font_dir)
        .with_context(|| format!("Failed to create font directory: {}", font_dir.display()))?;

    // 一時ファイルを残さないよう、ダウンロード結果をtarへ直接渡す
    let script =
        format!("set -o pipefail; curl -fsSL '{JETBRAINS_MONO_ARCHIVE_URL}' | tar -xJ -C \"$1\"");
    let mut cmd = Command::new("bash");
    cmd.args(["-c", &script, "bash"]).arg(&font_dir);
    run_command(cmd, "Failed to install JetBrains Mono Nerd Font.")?;

    Ok(())
}

/// ディストリビューションの公式パッケージからNoto Sans Mono CJKを導入
fn install_noto_sans_mono_cjk(distro: &Distro) -> Result<()> {
    println!("Installing {NOTO_SANS_MONO_CJK_JP}...");

    let mut cmd = Command::new("sudo");
    match distro {
        Distro::Ubuntu => {
            cmd.arg("apt")
                .arg("install")
                .arg("-y")
                .arg("fonts-noto-cjk");
        }
        Distro::Fedora => {
            cmd.arg("dnf")
                .arg("install")
                .arg("-y")
                .arg("google-noto-sans-mono-cjk-vf-fonts");
        }
    }
    run_command(cmd, "Failed to install Noto Sans Mono CJK.")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{font_face_matches, font_family_matches};

    #[test]
    fn 複数名を持つフォントファミリーを検出できる() {
        assert!(font_family_matches(
            "JetBrainsMono Nerd Font,JetBrainsMono NF\n",
            "JetBrainsMono Nerd Font"
        ));
    }

    #[test]
    fn フォールバックした別フォントは検出しない() {
        assert!(!font_family_matches(
            "Noto Sans\n",
            "JetBrainsMono Nerd Font"
        ));
    }

    #[test]
    fn 指定したフォントウェイトを検出できる() {
        assert!(font_face_matches(
            "JetBrainsMono Nerd Font,JetBrainsMono NF\tMedium,Regular\n",
            "JetBrainsMono Nerd Font",
            "Medium"
        ));
    }

    #[test]
    fn 別のフォントウェイトは検出しない() {
        assert!(!font_face_matches(
            "JetBrainsMono Nerd Font,JetBrainsMono NF\tRegular\n",
            "JetBrainsMono Nerd Font",
            "Medium"
        ));
    }
}

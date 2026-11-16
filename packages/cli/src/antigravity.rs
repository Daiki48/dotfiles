use anyhow::{Context, Result, bail};
use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::process::Command;

const GLOBAL_RULES: &str = ".gemini/GEMINI.md";

fn link_rules(dotfiles: &Path, home: &Path) -> Result<()> {
    // 削除予定のworktreeをグローバル設定の参照先にしない。
    if dotfiles.join(".git").is_file() {
        bail!("統合後の通常のdotfiles checkoutから実行してください。");
    }
    let source = dotfiles.join(GLOBAL_RULES);
    if !source.is_file() {
        bail!("ルールが見つかりません。dotfiles直下で実行してください。");
    }
    let destination = home.join(GLOBAL_RULES);
    match fs::symlink_metadata(&destination) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() && fs::read_link(&destination)? == source {
                return Ok(());
            }
            bail!(
                "既存の設定を保持しました: {}。内容を確認し、退避してから再実行してください。",
                destination.display()
            );
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("既存のグローバルルールを確認できません"),
    }

    #[cfg(unix)]
    {
        fs::create_dir_all(destination.parent().context("設定先の親がありません")?)?;
        std::os::unix::fs::symlink(&source, &destination)
            .context("Antigravityのグローバルルールをリンクできません")?;
    }
    #[cfg(not(unix))]
    bail!("このセットアップはUnix環境に対応しています。");

    Ok(())
}

pub fn setup() -> Result<()> {
    let version = Command::new("agy").arg("--version").output().context(
        "agyを実行できません。公式手順でAntigravity CLIを導入してください: https://antigravity.google/docs/getting-started?tab=cli",
    )?;
    if !version.status.success() {
        bail!("agy --versionが失敗しました。設定は変更していません。");
    }
    let dotfiles = std::env::current_dir().context("現在のディレクトリを取得できません")?;
    let home = home::home_dir().context("homeディレクトリを取得できません")?;
    link_rules(&dotfiles, &home)?;
    println!(
        "Antigravity CLI {} のグローバルルールを設定しました。",
        String::from_utf8_lossy(&version.stdout).trim()
    );
    println!("新しいagyセッションを開始してください。");
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

    struct Fixture {
        root: PathBuf,
        dotfiles: PathBuf,
        home: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "antigravity-rules-{}-{}",
                std::process::id(),
                NEXT_ID.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            let dotfiles = root.join("dotfiles");
            let home = root.join("home");
            fs::create_dir_all(dotfiles.join(".gemini")).unwrap();
            fs::create_dir_all(home.join(".gemini")).unwrap();
            fs::write(dotfiles.join(GLOBAL_RULES), "確認できた事実だけを伝える").unwrap();
            Self {
                root,
                dotfiles,
                home,
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.root).unwrap();
        }
    }

    #[test]
    fn links_only_rules_and_can_be_repeated() {
        let fixture = Fixture::new();
        fs::write(fixture.home.join(".gemini/settings.json"), "local settings").unwrap();
        for _ in 0..2 {
            link_rules(&fixture.dotfiles, &fixture.home).unwrap();
        }
        assert_eq!(
            fs::read_link(fixture.home.join(GLOBAL_RULES)).unwrap(),
            fixture.dotfiles.join(GLOBAL_RULES)
        );
        assert_eq!(
            fs::read_to_string(fixture.home.join(".gemini/settings.json")).unwrap(),
            "local settings"
        );
        assert!(!fixture.home.join(".gemini/policies").exists());
    }

    #[test]
    fn refuses_existing_file_without_overwriting_it() {
        let fixture = Fixture::new();
        let destination = fixture.home.join(GLOBAL_RULES);
        fs::write(&destination, "local rules").unwrap();
        assert!(link_rules(&fixture.dotfiles, &fixture.home).is_err());
        assert_eq!(fs::read_to_string(destination).unwrap(), "local rules");
    }

    #[test]
    fn refuses_other_and_dangling_symlinks() {
        for target in ["other-rules.md", "missing-rules.md"] {
            let fixture = Fixture::new();
            if target == "other-rules.md" {
                fs::write(fixture.home.join(target), "other rules").unwrap();
            }
            let destination = fixture.home.join(GLOBAL_RULES);
            let other = fixture.home.join(target);
            std::os::unix::fs::symlink(&other, &destination).unwrap();
            assert!(link_rules(&fixture.dotfiles, &fixture.home).is_err());
            assert_eq!(fs::read_link(destination).unwrap(), other);
        }
    }

    #[test]
    fn refuses_missing_source_without_creating_a_link() {
        let fixture = Fixture::new();
        fs::remove_file(fixture.dotfiles.join(GLOBAL_RULES)).unwrap();
        assert!(link_rules(&fixture.dotfiles, &fixture.home).is_err());
        assert!(fs::symlink_metadata(fixture.home.join(GLOBAL_RULES)).is_err());
    }

    #[test]
    fn refuses_linked_worktree_without_creating_a_link() {
        let fixture = Fixture::new();
        fs::write(fixture.dotfiles.join(".git"), "gitdir: /example/repository").unwrap();
        assert!(link_rules(&fixture.dotfiles, &fixture.home).is_err());
        assert!(fs::symlink_metadata(fixture.home.join(GLOBAL_RULES)).is_err());
    }
}

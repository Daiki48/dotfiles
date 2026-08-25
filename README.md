# My Setup

## Font

- [JetBrains Mono](https://www.nerdfonts.com/font-downloads) for Programming
- [BIZ UDPGothic](https://fonts.google.com/specimen/BIZ+UDPGothic) for Japanese

## For Windows setup

```sh
.\setup_windows.ps1
```

## Setup script

### Prerequisites

#### Ubuntu (via WSL)

```sh
wsl.exe --install --no-distribution
```

Reboot PC.

```sh
wsl --install
```

Install [Rust](https://www.rust-lang.org/tools/install):

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

```sh
sudo apt install -y build-essential x11-apps wl-clipboard
```

### CLI Reference

This repository ships a Rust CLI under `packages/cli` that handles installation,
version checks, and configuration symlinking for every tool listed below.

```sh
cargo run -- [--distro <ubuntu|fedora>] <command> [command options]
```

- `--distro` is a global flag and defaults to `ubuntu`. Pass `fedora` on Fedora-based systems.
- A few commands ignore `--distro` because they install via curl or npm regardless of the host distro.
- Place `--distro` either before or after the subcommand — both are accepted by clap.

#### Commands

| Command | Description | Honors `--distro` |
|---------|-------------|:-----------------:|
| `zsh` | Install zsh (apt on Ubuntu / dnf on Fedora), symlink `.zshrc`, `.zsh/`, `.zprofile`, `.profile`, then run `chsh -s $(which zsh)` to change the default login shell. | yes |
| `neovim --tag <tag>` | Ensure `tree-sitter-cli >= 0.26.1` is installed, build Neovim from source at the given git tag (e.g. `v0.12.2`), and install it via `sudo make install`. Also symlinks `~/.config/nvim`. Ubuntu installs Tree-sitter via Cargo; Fedora uses dnf. If Neovim is already installed, its build is skipped. | yes |
| `neovim-update --tag <tag>` | Ensure `tree-sitter-cli >= 0.26.1` is installed, then update an existing Neovim install to the given tag. Runs `git fetch --depth 1` for the tag, `git checkout`, `make distclean`, `make CMAKE_BUILD_TYPE=Release`, then `sudo make install`. The tag is verified against the remote before any work begins. | yes |
| `build-nvim-config` | Build the `nvim-config` Rust library with `cargo build --release -p nvim-config` and copy `libnvim_config.so` (Linux) / `libnvim_config.dylib` (macOS) into `~/.config/nvim/lua/`. The file is **copied**, not symlinked. | no |
| `wezterm` | Install WezTerm and its configured fonts (`JetBrainsMono Nerd Font` and `Noto Sans Mono CJK JP`), then symlink `~/.config/wezterm`. JetBrains Mono is installed under `~/.local/share/fonts`; Noto Sans Mono CJK is installed from the distribution package. | yes |
| `alacritty` | Install Alacritty via apt/dnf and symlink `~/.config/alacritty`. | yes |
| `ghostty` | Install Ghostty. Ubuntu: community installer at `mkasberg/ghostty-ubuntu`. Fedora: `dnf install ghostty`. Symlinks `~/.config/ghostty`. | yes |
| `zellij` | Install Zellij. Ubuntu: `cargo install zellij`. Fedora: `dnf install zellij`. Symlinks `~/.config/zellij`. | yes |
| `tmux` | Install tmux via apt/dnf, clone [TPM](https://github.com/tmux-plugins/tpm) (Tmux Plugin Manager) into `~/.config/tmux/plugins/tpm`, and symlink `~/.config/tmux/tmux.conf`. After setup, press `Ctrl+g` then `I` (capital i) inside tmux to install plugins. | yes |
| `mise [TOOL@VERSION]...` | Install mise from its recommended apt/dnf repository. Optional tool arguments are installed and recorded in the global mise config; with no arguments, only mise itself is installed. Shell activation is provided by the managed `.zshrc`. | yes |
| `claude` | Install Claude Code via the official installer (`curl -fsSL https://claude.ai/install.sh \| bash`). Symlinks `CLAUDE.md`, `settings.json`, `settings.local.json`, `skills/`, and `agents/` under `~/.claude/`. | no |
| `codex` | Require a trusted Codex CLI at `~/.local/bin/codex`, symlink shared instructions, safety rules, and Skills, then release-build and atomically install the Rust Git/GitHub hook plus `codex-worktree` and `codex-delivery` multi-call helpers. The setup preflights every managed destination before mutation and idempotently migrates `~/.codex/config.toml`; it never executes `codex`, `cargo`, or `npm` from `PATH`. Machine-local trust and TUI settings are preserved. | no |
| `gemini` | Install Gemini CLI via `npm install -g @google/gemini-cli` and symlink `~/.gemini/settings.json`, `~/.gemini/GEMINI.md`, and `~/.gemini/policies/`. Requires `GEMINI_API_KEY` exported in your shell. | no |

#### AI CLI configuration policy

Codex uses a single `codex-autonomous` permission profile. It extends the built-in workspace
profile, explicitly grants Git metadata writes under each workspace root, and keeps the
workspace network behavior enabled. `on-request` approvals are routed through auto-review. A
PreToolUse hook validates Git and GitHub writes before they run, and deny rules block destructive
operations.
Implementation, fix, addition, and build requests use the managed `codex-worktree` command;
investigation, design, review, and diagnosis alone do not create a worktree. See
[Codex worktree運用ガイド](docs/codex-worktrees.ja.md) for the lifecycle, recovery, and safety
boundaries. The managed root follows the [OpenAI Git worktrees documentation](https://learn.chatgpt.com/docs/environments/git-worktrees).

Ordinary editing and testing run inside the sandbox. The explicit `.git` grant is paired with the
managed hook; it is not an independent authorization to bypass the Git policy. Hook-validated
normal commits, pushes to
the current non-protected work branch, Draft PR creation, and reversible Issue/PR lifecycle
updates are allowlisted so the workflow does not pause for approval. The hook validates the
current repository, explicit target IDs, canonical arguments, metadata, and detected secrets.
It continues to reject protected-branch or force/delete/tag pushes, destructive cleanup,
arbitrary GitHub API mutations, and repository mismatches. Draft PR後のreview receipt、Ready化、
merge、main同期、managed cleanupは専用[`codex-delivery` helper](docs/codex-delivery.ja.md)だけを
経路とし、直接の`gh pr merge`や`git worktree remove/prune`は行いません。失敗、timeout、pending、
dirty、stale、conflict、判定不能時はPR、branch、worktreeを保持します。任意の削除が必要な場合、
Codexは対象を`.codex-trash/<timestamp>/`へ移し、そのdirectoryをstageまたは自動削除しません。
managed root内でmerged、main到達性、clean、未pushなしを厳格に証明したcleanupだけが自律削除の例外です。
そのcleanupではreview済みSHAをleaseにしたremote task branch削除だけを限定的に許可します。
ignored artifactもdirtyとして停止し、stateをatomic保存して途中から再検証・再開します。
The guard checks direct Bash invocations; arbitrary programs, privileged concurrent PR/repository
mutations, and same-user races remain outside its security boundary.

The repository declares a required `required-ci` GitHub Actions job and the intended remote
`main` Ruleset in `.github/rulesets/main.json`. The declaration does not prove that the remote
Ruleset is active; apply it with explicit approval and verify its readback. See
[GitHub CI・Ruleset運用ガイド](docs/github-guardrails.ja.md) for the check mapping, application,
verification, and rollback procedure.

`codex-delivery` defaults to that strict Ruleset gate. A GitHub Free private repository may instead
explicitly use `--gate-mode github-free-private`; this lower-assurance mode binds the live private
repository identity, one successful exact-SHA `required-ci`, review state, and a high/critical
receipt. GitHub does not enforce direct-push, force-push, deletion, or helper-only merge constraints
server-side.

For a change, build, or fix request, Codex autonomously investigates, implements, and
verifies the requested scope. Plans, subagents, commits, pushes, Draft PRs, and the delivery
loop are used when the task or an explicit request warrants them. risk分類とDaikiの意思決定要否は
別々に判定します。全riskで固定head SHAの標準独立reviewを1つ行い、high/criticalだけ変更固有の
専門reviewを1つ追加します。actionable=0、未解決thread=0、required
checkの文字どおりの`success`、選択したremote gateを満たし、仕様・既存権限・rollback・検証を
Codexが確定できる場合に`codex-delivery`がReady、merge、mainのfetch後の`merge --ff-only`、
managed cleanupまで進めます。delivery安全境界、auth/secrets、production、不可逆migration、
breaking changeはhigh/criticalとしてreviewを強化しますが、riskだけで確認待ちにしません。
product decision、追加権限、費用、不可逆性、重大な残存リスク受容が必要な場合だけ
`approve-review`を使います。release、force push、protected-branch push、任意削除、material
scope expansionは引き続き手動です。詳しい条件は
[Codex delivery運用ガイド](docs/codex-delivery.ja.md)を参照してください。GitHub Free/private modeは
server-side強制がないためriskをhigh/criticalへ引き上げますが、decision assessmentがautonomousなら
確認待ちせずdeliveryできます。

`~/.codex/AGENTS.md`, `~/.codex/rules/default.rules`,
`~/.agents/skills`, and `~/.codex/agents` are symlinked from this repository.
The custom agents use Luna xhigh for non-writing exploration and scoped independent review,
while the root Sol high agent owns requirements, implementation, integration, and acceptance.
Subagents inherit the parent runtime permissions, so role-local sandbox settings are not treated
as a security boundary. The single-writer contract and explicit no-mutation instructions are the
operational controls against delegated writes, not hard sandbox enforcement. Automated tests,
lint, type checks, and builds provide exhaustive mechanical
assurance; review stays focused on the fixed diff, affected paths, risk boundaries, and gaps that
automation cannot prove.
`~/.codex/config.toml` remains local because it contains machine-specific
project trust, hook trust, and TUI state. `cargo run -- codex` backs up the local config
before migrating the shared top-level settings. If an older setup left `config.toml` as a
symlink, setup archives the link and writes a regular local config without modifying the
link target. A legacy profile config is backed up; deprecated profile selectors and
tables are removed while shared settings are merged without discarding project trust,
hook trust, TUI state, or custom agents. Retired teacher/autonomous profile files are
renamed to timestamped backups instead of being deleted.

After the first install or an update of the safety hook, restart Codex and use `/hooks` to
trust the displayed hook when Codex requests it. This is a one-time local trust decision;
the hook is not active until it is trusted.

Authentication, session history, pairing information, local databases, and credentials
must remain outside this public repository. Diff review is required before external writes.

`~/.gemini/policies/` is managed by this repository as a symlink and stores Gemini CLI
Policy Engine rules. Do not use deprecated `tools.allowed` in
`~/.gemini/settings.json` for persistent tool rules.

#### Examples

##### Ubuntu

```sh
cargo run -- zsh
cargo run -- neovim --tag v0.12.2
cargo run -- neovim-update --tag v0.12.2
cargo run -- build-nvim-config
cargo run -- tmux
cargo run -- claude
cargo run -- codex
cargo run -- gemini
```

`codex`の初回セットアップまたは更新後は、managed設定と2つのhelperを反映するため
Codexを再起動してください。

##### Fedora

```sh
cargo run -- --distro fedora zsh
cargo run -- --distro fedora neovim --tag v0.12.2
cargo run -- --distro fedora neovim-update --tag v0.12.2
cargo run -- --distro fedora wezterm
cargo run -- --distro fedora tmux
cargo run -- --distro fedora mise
cargo run -- --distro fedora mise node@lts python@latest deno@latest
```

The first mise example installs only mise. The second also installs Node.js,
Python, and Deno as user-wide defaults. For project-specific versions, run
`mise use node@lts python@latest deno@latest` in the project directory instead.

After running the `zsh` command, restart your terminal and verify the shell:

```sh
echo $SHELL
```

It should print `/usr/bin/zsh` (or `/bin/zsh`).

### .skk

[CorvusSKK](https://nathancorvussolis.github.io/)

```sh
winget install -h corvusskk -s winget
```

Dictionary.

```shell
git clone https://github.com/Daiki48/skk.git
```

Config for CorvusSKK.

[CorvusSKK config](https://github.com/Daiki48/dotfiles/blob/main/docs/corvusskk.ja.md)

## Setup Documentation

- [skk](https://github.com/Daiki48/dotfiles/blob/main/docs/setup-skk.md)
- [skkeleton](https://github.com/Daiki48/dotfiles/blob/main/docs/setup-skkeleton.md)
- [docker](https://github.com/Daiki48/dotfiles/blob/main/docs/setup-docker.md)
- [Codex worktree運用](docs/codex-worktrees.ja.md)
- [Codex delivery運用](docs/codex-delivery.ja.md)
- [Codex guardrailのRust実装](docs/codex-rust-guardrails.ja.md)

# Neovim setup

## LSP

- [lua-language-server](https://luals.github.io/#neovim-install)
  - [Scoop](https://scoop.sh): `scoop install lua-language-server`
- [rust-analyzer](https://rust-analyzer.github.io/)
  - [rustup](https://rust-analyzer.github.io/book/rust_analyzer_binary.html#rustup): `rustup component add rust-analyzer`
- [typescript-language-server](https://github.com/typescript-language-server/)
  - [npm](https://github.com/typescript-language-server/typescript-language-server?tab=readme-ov-file#installing): `npm install -g typescript-language-server typescript`
- [html](https://github.com/neovim/nvim-lspconfig/blob/master/doc/configs.md#html)
  - npm: `npm i -g vscode-langservers-extracted`
- [css](https://github.com/neovim/nvim-lspconfig/blob/master/doc/configs.md#cssls)
  - npm: `npm i -g vscode-langservers-extracted`
- [svelte](https://github.com/neovim/nvim-lspconfig/blob/master/doc/configs.md#svelte)
  - npm: `npm install -g svelte-language-server`
- [taplo](https://github.com/neovim/nvim-lspconfig/blob/master/doc/configs.md#taplo)
  - cargo: `cargo install --features lsp --locked taplo-cli`

## clipboard for Arch Linux

Enable the clipboard.

### Install wl-clipboard

```shell
$ sudo pacman -S wl-clipboard
[sudo] daiki のパスワード:
依存関係を解決しています...
衝突するパッケージがないか確認しています...

パッケージ (1) wl-clipboard-1:2.2.1-1

合計ダウンロード容量:  0.03 MiB
合計インストール容量:  0.12 MiB

:: インストールを行いますか？ [Y/n]
:: パッケージを取得します...
 wl-clipboard-1:2.2.1-1-x86_64                            32.3 KiB  33.6 KiB/s 00:01 [################################################] 100%
(1/1) キーリングのキーを確認                                                         [################################################] 100%
(1/1) パッケージの整合性をチェック                                                   [################################################] 100%
(1/1) パッケージファイルのロード                                                     [################################################] 100%
(1/1) ファイルの衝突をチェック                                                       [################################################] 100%
(1/1) 空き容量を確認                                                                 [################################################] 100%
:: パッケージの変更を処理しています...
(1/1) インストール wl-clipboard                                                      [################################################] 100%
wl-clipboard の提案パッケージ
    xdg-utils: for content type inference in wl-copy [インストール済み]
    mailcap: for type inference in wl-paste [インストール済み]
:: トランザクション後のフックを実行...
(1/2) Arming ConditionNeedsUpdate...
(2/2) Refreshing PackageKit...
```

### init.lua

```lua
vim.o.clipboard = 'unnamedplus'
```

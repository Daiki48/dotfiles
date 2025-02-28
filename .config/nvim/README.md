# Neovim setup

## LSP

Install the respective language servers.

### Installing a language server using `npm`

- [typescript-language-server](https://github.com/typescript-language-server/)
  - [npm](https://github.com/typescript-language-server/typescript-language-server?tab=readme-ov-file#installing): `npm install -g typescript-language-server typescript`
- [html](https://github.com/neovim/nvim-lspconfig/blob/master/doc/configs.md#html)
  - npm: `npm i -g vscode-langservers-extracted`
- [css](https://github.com/neovim/nvim-lspconfig/blob/master/doc/configs.md#cssls)
  - npm: `npm i -g vscode-langservers-extracted`
- [svelte](https://github.com/neovim/nvim-lspconfig/blob/master/doc/configs.md#svelte)
  - npm: `npm install -g svelte-language-server`
- [tailwindcss](https://github.com/neovim/nvim-lspconfig/blob/master/doc/configs.md#tailwindcss)
  - npm: `npm install -g @tailwindcss/language-server`

```sh
npm install -g prettier typescript typescript-language-server svelte-language-server @tailwindcss/language-server vscode-langservers-extracted
```

### Installing a language server using `scoop`

- [lua-language-server](https://luals.github.io/#neovim-install)
  - [Scoop](https://scoop.sh): `scoop install lua-language-server`

```sh
scoop install lua-language-server
```

### Installing a language server using `cargo`

- [taplo](https://github.com/neovim/nvim-lspconfig/blob/master/doc/configs.md#taplo)
  - cargo: `cargo install --features lsp --locked taplo-cli`

```sh
cargo install --features lsp --locked taplo-cli
```

### Installing a language server using `rustup`

- [rust-analyzer](https://rust-analyzer.github.io/)
  - [rustup](https://rust-analyzer.github.io/book/rust_analyzer_binary.html#rustup): `rustup component add rust-analyzer`

```sh
rustup component add rust-analyzer
```

## Arch Linux

Enable the clipboard.

### Install wl-clipboard

```shell
$ sudo pacman -S wl-clipboard
```

### init.lua

```lua
vim.o.clipboard = 'unnamedplus'
```

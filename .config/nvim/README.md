# Neovim setup

## clipboard

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

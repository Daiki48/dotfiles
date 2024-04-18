# Setup skk

If `fctix` is not installed, run from the `fctix5` installation.

## Update fctix5 from fctix

How to migrate to `fctix5-skk` If `fctix-mozc` is already isntalled.

### Uninstall fctix

```shell
$ sudo pacman -Rns fcitx fcitx-configtool fcitx-mozc fcitx-qt5 kcm-fcitx manjaro-asian-input-support-fcitx
```

PC reboot.

### Install fctix5 and fctix5-skk

```shell
$ sudo pacman -S fcitx5-im fcitx5-skk
:: 4 個のパッケージがグループ fcitx5-im に存在します:
:: リポジトリ extra
   1) fcitx5  2) fcitx5-configtool  3) fcitx5-gtk  4) fcitx5-qt

選択して下さい (デフォルト=all):
依存関係を解決しています...
衝突するパッケージがないか確認しています...

パッケージ (10) enchant-2.6.5-1  libskk-1.0.5-2  qt6-wayland-6.6.2-1  skk-jisyo-20231119-1  xcb-imdkit-1.0.7-1  fcitx5-5.1.8-1  fcitx5-configtool-5.1.3-1
                fcitx5-gtk-5.1.1-1  fcitx5-qt-5.1.4-5  fcitx5-skk-5.1.1-1

合計ダウンロード容量:  14.52 MiB
合計インストール容量:  42.53 MiB

:: インストールを行いますか？ [Y/n]
:: パッケージを取得します...
 fcitx5-qt-5.1.4-5-x86_64                                            443.5 KiB   550 KiB/s 00:01 [#########################################################] 100%
 qt6-wayland-6.6.2-1-x86_64                                         1115.3 KiB   480 KiB/s 00:02 [#########################################################] 100%
 fcitx5-configtool-5.1.3-1-x86_64                                    401.7 KiB   268 KiB/s 00:02 [#########################################################] 100%
 xcb-imdkit-1.0.7-1-x86_64                                           324.3 KiB   911 KiB/s 00:00 [#########################################################] 100%
 skk-jisyo-20231119-1-any                                              3.6 MiB  1308 KiB/s 00:03 [#########################################################] 100%
 libskk-1.0.5-2-x86_64                                               221.4 KiB   478 KiB/s 00:00 [#########################################################] 100%
 fcitx5-skk-5.1.1-1-x86_64                                            89.2 KiB   462 KiB/s 00:00 [#########################################################] 100%
 fcitx5-gtk-5.1.1-1-x86_64                                            82.5 KiB   427 KiB/s 00:00 [#########################################################] 100%
 enchant-2.6.5-1-x86_64                                               59.6 KiB   459 KiB/s 00:00 [#########################################################] 100%
 fcitx5-5.1.8-1-x86_64                                                 8.3 MiB  1468 KiB/s 00:06 [#########################################################] 100%
 合計 (10/10)                                                         14.5 MiB  2.49 MiB/s 00:06 [#########################################################] 100%
(10/10) キーリングのキーを確認                                                                   [#########################################################] 100%
(10/10) パッケージの整合性をチェック                                                             [#########################################################] 100%
(10/10) パッケージファイルのロード                                                               [#########################################################] 100%
(10/10) ファイルの衝突をチェック                                                                 [#########################################################] 100%
(10/10) 空き容量を確認                                                                           [#########################################################] 100%
:: パッケージの変更を処理しています...
( 1/10) インストール enchant                                                                     [#########################################################] 100%
enchant の提案パッケージ
    aspell: for aspell based spell checking support
    hunspell: for hunspell based spell checking support [インストール済み]
    libvoikko: for libvoikko based spell checking support
    hspell: for hspell based spell checking support
    nuspell: for nuspell based spell checking support
( 2/10) インストール xcb-imdkit                                                                  [#########################################################] 100%
( 3/10) インストール fcitx5                                                                      [#########################################################] 100%
( 4/10) インストール qt6-wayland                                                                 [#########################################################] 100%
( 5/10) インストール fcitx5-qt                                                                   [#########################################################] 100%
( 6/10) インストール fcitx5-configtool                                                           [#########################################################] 100%
fcitx5-configtool の提案パッケージ
    kdeclarative5: for KCM support [インストール済み]
    kirigami2: for KCM support [インストール済み]
    plasma-framework5: for fcitx5-plasma-theme-generator [インストール済み]
( 7/10) インストール fcitx5-gtk                                                                  [#########################################################] 100%
( 8/10) インストール libskk                                                                      [#########################################################] 100%
( 9/10) インストール skk-jisyo                                                                   [#########################################################] 100%
>>> If you want to merge dictionaries, use skktools
>>> For example, merging SKK-JISYO.L and SKK-JISYO.geo into SKK-JISYO.XL:
>>> % skkdic-expr2 SKK-JISYO.L + SKK-JISYO.geo > SKK-JISYO.XL
skk-jisyo の提案パッケージ
    skktools: Dictionary maintenance tools
(10/10) インストール fcitx5-skk                                                                  [#########################################################] 100%
:: トランザクション後のフックを実行...
(1/5) Arming ConditionNeedsUpdate...
(2/5) Refreshing PackageKit...
(3/5) Probing GTK3 input method modules...
(4/5) Updating icon theme caches...
(5/5) Updating the desktop file MIME type cache...
```
PC reboot.

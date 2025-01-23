# トラブルシューティング

## Windows

### `makers setup` 実行時に「PSReadline モジュールを読み込めません。コンソールは PSReadline なしで実行されています。」と表示されて実行出来ない

```sh
makers setup
[cargo-make] INFO - makers 0.37.23
[cargo-make] INFO - Build File: Makefile.toml
[cargo-make] INFO - Task: setup
[cargo-make] INFO - Profile: development
[cargo-make] INFO - Skipping Task: unix_setup
[cargo-make] INFO - Running Task: windows_setup
Windows PowerShell
Copyright (C) Microsoft Corporation. All rights reserved.

新機能と改善のために最新の PowerShell をインストールしてください!https://aka.ms/PSWindows

PSReadline モジュールを読み込めません。コンソールは PSReadline なしで実行されています。
```

`Powershell` を管理者で実行して以下のコマンドを実行します。

```sh
Install-Module -Name PSReadLine -Force
```

> [!WARNING]
> 管理者以外の権限では失敗します。
> ```sh
> Install-Module -Name PSReadLine -Force
> Install-Module : 'Install-Module' コマンドはモジュール 'PowerShellGet' で見つかりましたが、このモジュールを読み込むことができませんでした。詳細については、'Import-Module PowerShellGet' を実行してください。
> 発生場所 行:1 文字:1
> + Install-Module -Name PSReadLine -Force
> + ~~~~~~~~~~~~~~
>     + CategoryInfo          : ObjectNotFound: (Install-Module:String) [], CommandNotFoundException
>     + FullyQualifiedErrorId : CouldNotAutoloadMatchingModule
> ```

### モジュールの証明書確認

それでも実行出来ない場合は以下のコマンドで `status` が `Valid` となっているか確認します。

> [!NOTE]
> `NotTrusted` であれば信頼されていないようです。
> Status が NotTrusted の場合は、以下の手順で証明書を信頼します。
>
> 上記のコマンドの出力に表示されている `SignerCertificate` の情報を確認します。特に `Subject` と `Issuer` の情報をメモしておきます。
> certmgr.msc を実行して証明書マネージャーを開きます。
> 「信頼されたルート証明機関」→「証明書」を開きます。
> Issuer の情報に一致する証明書を探します。見つからない場合は、以下の手順で証明書をインポートします。
> Issuer の情報から証明書を取得します（通常は Microsoft の Web サイトからダウンロードできます）。
> 証明書マネージャーで、「信頼されたルート証明機関」を右クリックし、「すべてのタスク」→「インポート」を選択します。
> ウィザードに従って証明書をインポートします。

```sh
Get-AuthenticodeSignature -FilePath (Get-Module PSReadLine -ListAvailable | Select-Object -ExpandProperty Path)

    Directory: C:\Users\Owner\Documents\PowerShell\Modules\PSReadLine\2.3.6

SignerCertificate                         Status                                                            StatusMessage                                                    Path
-----------------                         ------                                                            -------------                                                    ----
C2048FB509F1C37A8C3E9EC6648118458AA01780  Valid                                                             Signature verified.                                              PSReadLine.psd1

    Directory: C:\program files\powershell\7\Modules\PSReadLine

SignerCertificate                         Status                                                            StatusMessage                                                    Path
-----------------                         ------                                                            -------------                                                    ----
C2048FB509F1C37A8C3E9EC6648118458AA01780  Valid                                                             Signature verified.                                              PSReadLine.psd1

    Directory: C:\Program Files\WindowsPowerShell\Modules\PSReadLine\2.0.0

SignerCertificate                         Status                                                            StatusMessage                                                    Path
-----------------                         ------                                                            -------------                                                    ----
71F53A26BB1625E466727183409A30D03D7923DF  Valid                                                             Signature verified.                                              PSReadLine.psd1
```



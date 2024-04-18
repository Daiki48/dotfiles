# Setup docker

Manjaro KDE

## Install

```shell
[daiki:/mnt/sabrent]
$ sudo pacman -S docker
[sudo] daiki のパスワード:
依存関係を解決しています...
衝突するパッケージがないか確認しています...

パッケージ (4) bridge-utils-1.7.1-2  containerd-1.7.13-1  runc-1.1.12-1  docker-1:25.0.3-1

合計ダウンロード容量:   56.84 MiB
合計インストール容量:  232.51 MiB

:: インストールを行いますか？ [Y/n]
:: パッケージを取得します...
 bridge-utils-1.7.1-2-x86_64                              16.2 KiB  6.45 KiB/s 00:03 [################################################] 100%
 runc-1.1.12-1-x86_64                                      3.0 MiB   702 KiB/s 00:04 [################################################] 100%
 docker-1:25.0.3-1-x86_64                                 27.5 MiB  1991 KiB/s 00:14 [################################################] 100%
 containerd-1.7.13-1-x86_64                               26.3 MiB  1880 KiB/s 00:14 [################################################] 100%
 合計 (4/4)                                               56.8 MiB  3.95 MiB/s 00:14 [################################################] 100%
(4/4) キーリングのキーを確認                                                         [################################################] 100%
(4/4) パッケージの整合性をチェック                                                   [################################################] 100%
(4/4) パッケージファイルのロード                                                     [################################################] 100%
(4/4) ファイルの衝突をチェック                                                       [################################################] 100%
(4/4) 空き容量を確認                                                                 [################################################] 100%
:: パッケージの変更を処理しています...
(1/4) インストール bridge-utils                                                      [################################################] 100%
(2/4) インストール runc                                                              [################################################] 100%
runc の提案パッケージ
    criu: checkpoint support
(3/4) インストール containerd                                                        [################################################] 100%
(4/4) インストール docker                                                            [################################################] 100%
docker の提案パッケージ
    btrfs-progs: btrfs backend support [インストール済み]
    pigz: parallel gzip compressor support
    docker-scan: vulnerability scanner
    docker-buildx: extended build capabilities
:: トランザクション後のフックを実行...
(1/5) Creating system user accounts...
Creating group 'docker' with GID 958.
(2/5) Reloading system manager configuration...
(3/5) Reloading device manager configuration...
(4/5) Arming ConditionNeedsUpdate...
(5/5) Refreshing PackageKit...
```

## Auto run setting

```shell
$ sudo systemctl enable --now docker.service
```

```shell
[daiki:/mnt/sabrent]
$ sudo systemctl enable --now docker.service
Created symlink /etc/systemd/system/multi-user.target.wants/docker.service → /usr/lib/systemd/system/docker.service.
```

```shell
$ sudo systemctl enable --now containerd.service
```

```shell
[daiki:/mnt/sabrent]
$ sudo systemctl enable --now containerd.service
Created symlink /etc/systemd/system/multi-user.target.wants/containerd.service → /usr/lib/systemd/system/containerd.service.
```

# Confirm docker status

```shell
[daiki:/mnt/sabrent]
$ systemctl status docker.service --no-pager
● docker.service - Docker Application Container Engine
     Loaded: loaded (/usr/lib/systemd/system/docker.service; enabled; preset: disabled)
     Active: active (running) since Mon 2024-04-15 22:59:40 JST; 2min 2s ago
TriggeredBy: ● docker.socket
       Docs: https://docs.docker.com
   Main PID: 18760 (dockerd)
      Tasks: 17
     Memory: 36.4M (peak: 46.6M)
        CPU: 316ms
     CGroup: /system.slice/docker.service
             └─18760 /usr/bin/dockerd -H fd:// --containerd=/run/containerd/containerd.sock

 4月 15 22:59:39 daiki systemd[1]: Starting Docker Application Container Engine...
 4月 15 22:59:39 daiki dockerd[18760]: time="2024-04-15T22:59:39.900535250+09:00" level=info msg="Starting up"
 4月 15 22:59:39 daiki dockerd[18760]: time="2024-04-15T22:59:39.972599629+09:00" level=info msg="Loading containers: start."
 4月 15 22:59:40 daiki dockerd[18760]: time="2024-04-15T22:59:40.217961362+09:00" level=info msg="Loading containers: done."
 4月 15 22:59:40 daiki dockerd[18760]: time="2024-04-15T22:59:40.354233856+09:00" level=warning msg="Not using native diff for o…r=overlay2
 4月 15 22:59:40 daiki dockerd[18760]: time="2024-04-15T22:59:40.354705700+09:00" level=info msg="Docker daemon" commit=f417435e…ion=25.0.3
 4月 15 22:59:40 daiki dockerd[18760]: time="2024-04-15T22:59:40.355003809+09:00" level=info msg="Daemon has completed initialization"
 4月 15 22:59:40 daiki dockerd[18760]: time="2024-04-15T22:59:40.406569628+09:00" level=info msg="API listen on /run/docker.sock"
 4月 15 22:59:40 daiki systemd[1]: Started Docker Application Container Engine.
Hint: Some lines were ellipsized, use -l to show in full.
```

# docker command
```shell
[daiki:/mnt/sabrent]
$ sudo groupadd -f docker
[daiki:/mnt/sabrent]
$ sudo usermod -aG docker $USER
[daiki:/mnt/sabrent]
$ newgrp docker
```

# Install docker-compose

```shell
[daiki:/mnt/sabrent]
$ sudo pacman -S docker-compose
[sudo] daiki のパスワード:
依存関係を解決しています...
衝突するパッケージがないか確認しています...

パッケージ (1) docker-compose-2.24.6-1

合計ダウンロード容量:  12.73 MiB
合計インストール容量:  56.55 MiB

:: インストールを行いますか？ [Y/n]
:: パッケージを取得します...
 docker-compose-2.24.6-1-x86_64                           12.7 MiB  1610 KiB/s 00:08 [################################################] 100%
(1/1) キーリングのキーを確認                                                         [################################################] 100%
(1/1) パッケージの整合性をチェック                                                   [################################################] 100%
(1/1) パッケージファイルのロード                                                     [################################################] 100%
(1/1) ファイルの衝突をチェック                                                       [################################################] 100%
(1/1) 空き容量を確認                                                                 [################################################] 100%
:: パッケージの変更を処理しています...
(1/1) インストール docker-compose                                                    [################################################] 100%
:: トランザクション後のフックを実行...
(1/2) Arming ConditionNeedsUpdate...
(2/2) Refreshing PackageKit...
```

# Confirm

```shell
[daiki:/mnt/sabrent]
$ docker-compose version
Docker Compose version 2.24.6
```

# Ref

- [Arch Linux に Docker をインストールする #Docker - Qiita](https://qiita.com/mnishiguchi/items/e5b61ec702d21165b079)
- [Arch Linux に Docker Compose をインストールする #Docker - Qiita](https://qiita.com/mnishiguchi/items/62744d09ce9a8a2d109c)
- [Docker - ArchWiki](https://wiki.archlinux.jp/index.php/Docker)

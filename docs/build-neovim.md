# Build Neovim from source code

Make Neovim available independent of package managers.
Build a Neovim environment in Ubuntu(WSL2).

## Information

```sh
cat /etc/os-release
PRETTY_NAME="Ubuntu 24.04.2 LTS"
NAME="Ubuntu"
VERSION_ID="24.04"
VERSION="24.04.2 LTS (Noble Numbat)"
VERSION_CODENAME=noble
ID=ubuntu
ID_LIKE=debian
HOME_URL="https://www.ubuntu.com/"
SUPPORT_URL="https://help.ubuntu.com/"
BUG_REPORT_URL="https://bugs.launchpad.net/ubuntu/"
PRIVACY_POLICY_URL="https://www.ubuntu.com/legal/terms-and-policies/privacy-policy"
UBUNTU_CODENAME=noble
LOGO=ubuntu-logo
```

## Uninstall from snap

If you have already installed Neovim from Snap, uninstall it.

```sh
sudo snap remove nvim
```

## Update dependencies

```sh
sudo apt update && sudo apt upgrade -y
```

## Preparation for build

See: [Build prerequisites | neovim GitHub](https://github.com/neovim/neovim/blob/master/BUILD.md#build-prerequisites)

```sh
sudo apt-get install ninja-build gettext cmake curl build-essential
```

## Clone from GitHub

> [!TIP]
> I cloned ito to `$HOME` .

```sh
git clone https://github.com/neovim/neovim.git
```

Move directory.

```sh
cd neovim
```

## Check branch

I build the release branch.
So, check the branch in the remote repository.

```sh
git branch -r
  origin/HEAD -> origin/master
  origin/master
  origin/release-0.10
  origin/release-0.11
  origin/release-0.3
  origin/release-0.4
  origin/release-0.5
  origin/release-0.6
  origin/release-0.7
  origin/release-0.8
  origin/release-0.9
```

Choice `origin/release-0.11` .

## Clone branch `origin/release-x.xx`

```sh
git checkout -b release-0.11 origin/release-0.11
```

## Let's building

See: 
- https://github.com/neovim/neovim/tree/release-0.11?tab=readme-ov-file#install-from-source
- https://github.com/neovim/neovim/blob/master/BUILD.md

Execute `make` command.

```sh
make CMAKE_BUILD_TYPE=RelWithDebInfo
```

## Install

After the build, let's installing!

```sh
sudo make install
```

## Check Neovim version

```sh
nvim -v
NVIM v0.11.1-dev-1+g5e4365b83d
Build type: RelWithDebInfo
LuaJIT 2.1.1741730670
Run "nvim -V1 -v" for more info
```

## Update

```sh
git pull origin release-0.11
remote: Enumerating objects: 41, done.
remote: Counting objects: 100% (32/32), done.
remote: Compressing objects: 100% (23/23), done.
remote: Total 41 (delta 17), reused 10 (delta 9), pack-reused 9 (from 2)
Unpacking objects: 100% (41/41), 177.58 KiB | 1.95 MiB/s, done.
From https://github.com/neovim/neovim
 * branch                  release-0.11 -> FETCH_HEAD
   5e4365b83d..6514e2c7ba  release-0.11 -> origin/release-0.11
Updating 5e4365b83d..6514e2c7ba
Fast-forward
 runtime/doc/provider.txt                |  2 +-
 src/gen/gen_keycodes.lua                | 33 ++++++++-------------------------
 src/nvim/drawscreen.c                   | 16 +++++++++++++---
 src/nvim/eval.c                         |  2 +-
 src/nvim/keycodes.c                     |  7 ++-----
 src/nvim/window.c                       | 10 +++++-----
 test/functional/plugin/health_spec.lua  |  4 ++--
 test/functional/ui/cmdline_spec.lua     |  6 ++++++
 test/functional/ui/decorations_spec.lua | 22 ++++++++++++++++++++++
 test/old/testdir/test_options.vim       | 13 ++++++++++---
 10 files changed, 70 insertions(+), 45 deletions(-)
```

## Latest version

```sh
git pull origin release-0.11
From https://github.com/neovim/neovim
 * branch                  release-0.11 -> FETCH_HEAD
Already up to date.
```

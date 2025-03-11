# Base options
setopt always_last_prompt
setopt auto_list
setopt auto_menu
setopt auto_cd
setopt auto_param_keys
setopt correct_all
setopt extended_glob
setopt list_types
setopt cdable_vars
setopt extended_history
setopt hist_ignore_dups
setopt hist_reduce_blanks
setopt hist_ignore_space
setopt auto_name_dirs
setopt auto_remove_slash
setopt ignore_eof
setopt no_beep
setopt multios
setopt numeric_glob_sort
setopt prompt_subst
setopt sh_word_split
setopt noclobber
zstyle ':completion:*' list-colors 'di=36' 'ex=31' 'ln=35'
zstyle ':completion:*:default' menu select=1
zstyle ':completion:*:*:make:*' tag-order 'targets'
export WORDCHARS='*?_.[];!#$%^{}<>'

# Add deno completions to search path
if [[ ":$FPATH:" != *":/home/daiki48/.zsh/completions:"* ]]; then export FPATH="/home/daiki48/.zsh/completions:$FPATH"; fi
setopt print_eight_bit

# git 
# curl -o .git-completion.sh https://raw.githubusercontent.com/git/git/master/contrib/completion/git-completion.bash
# curl -o .git-prompt.sh https://raw.githubusercontent.com/git/git/master/contrib/completion/git-prompt.sh
# source ~/.zsh/git-prompt.sh fpath=(~/.zsh $fpath)
# zstyle ':completion:*:*:git:*' script ~/.zsh/git-completion.zsh
# autoload -Uz compinit && compinit
# autoload -Uz vcs_info setopt prompt_subst
# zstyle ':vcs_info:git:*' check-for-changes true

source ~/.zsh/git-prompt.sh
fpath=(~/.zsh $fpath)
zstyle ':completion:*:*:git:*' script ~/.zsh/git-completion.zsh
autoload -Uz compinit && compinit
autoload -Uz vcs_info
setopt prompt_subst
zstyle ':vcs_info:git:*' check-for-changes true
zstyle ':vcs_info:git:*' stagedstr "%F{magenta}!"
zstyle ':vcs_info:git:*' unstagedstr "%F{yellow}+"
zstyle ':vcs_info:*' formats "%F{cyan}%c%u[%b]%f"
zstyle ':vcs_info:*' actionformats '[%b|%a]'
precmd () { vcs_info }
PROMPT='[%B%F{red}%n:%F{green}%~%f]%F{cyan}$vcs_info_msg_0_%f
%F{yellow}$%f '

# Add write, read permissions
umask 000

export NVM_DIR="$HOME/.nvm"
[ -s "$NVM_DIR/nvm.sh" ] && \. "$NVM_DIR/nvm.sh"  # This loads nvm
[ -s "$NVM_DIR/bash_completion" ] && \. "$NVM_DIR/bash_completion"  # This loads nvm bash_completion

. "/home/daiki48/.deno/env"

export PATH="$PATH:/snap/bin"

# python for WSL2 (Ubuntu)
export PYENV_ROOT="$HOME/.pyenv"
export PATH="$PYENV_ROOT/bin:$PATH"
eval "$(pyenv init --path)"

# pip3
source ~/.local/bin/pymyenv/bin/activate

# bun completions
[ -s "/home/daiki48/.bun/_bun" ] && source "/home/daiki48/.bun/_bun"

# bun
export BUN_INSTALL="$HOME/.bun"
export PATH="$BUN_INSTALL/bin:$PATH"

# Resolve "wl-clipboard not found, clipboard integration won't work"
# Check ".config/nvim/lua/daiki/options.lua"
# sudo apt install wl-clipboard
if [ ! -S "$XDG_RUNTIME_DIR/wayland-0" ]; then
  ln -s /mnt/wslg/runtime-dir/wayland-0* "$XDG_RUNTIME_DIR"
fi

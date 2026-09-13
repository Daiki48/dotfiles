"""人間用checkoutを変更せず、検証済みworktreeのローカル設定を有効化する。"""
import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import stat


def present(path):
    return path.exists() or path.is_symlink()


def private_directory(path):
    info = path.lstat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
        raise RuntimeError('unsafe_backup_directory')


def install(codex_home, expected_agents, source, apply=False):
    codex_home = codex_home.resolve(strict=True)
    source = source.resolve(strict=True)
    agents_source = source.parents[1] / 'AGENTS.md'
    for path in [agents_source, source / 'run.py', source / 'settings.config', source / 'roles.json']:
        if not path.is_file():
            raise RuntimeError('missing_source')
    if present(codex_home / 'AGENTS.override.md'):
        raise RuntimeError('global_override_requires_resolution')
    agents = codex_home / 'AGENTS.md'
    local = codex_home / 'local-model'
    if not agents.is_symlink():
        raise RuntimeError('agents_is_not_expected_symlink')
    old_target = os.readlink(agents)
    if agents.resolve(strict=True) not in {expected_agents.resolve(strict=True), agents_source}:
        raise RuntimeError('agents_target_changed')
    if present(local) and (not local.is_symlink() or local.resolve(strict=True) != source):
        raise RuntimeError('local_model_path_already_owned')
    if agents.resolve(strict=True) == agents_source and local.is_symlink():
        return {'status': 'already_active', 'source': str(source)}
    result = {'status': 'ready', 'source': str(source), 'previous_agents_target': old_target}
    if not apply:
        return result
    backup_root = codex_home / 'local-model-backups'
    backup_root.mkdir(mode=0o700, exist_ok=True)
    private_directory(backup_root)
    backup = backup_root / datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%S%fZ')
    backup.mkdir(mode=0o700)
    # 外部状態が変わっていたら上書きしない。
    if not agents.is_symlink() or os.readlink(agents) != old_target:
        raise RuntimeError('agents_changed_before_activation')
    if not present(local):
        local.symlink_to(source, target_is_directory=True)
    agents.rename(backup / 'AGENTS.md')
    try:
        agents.symlink_to(agents_source)
    except BaseException:
        if not present(agents):
            (backup / 'AGENTS.md').rename(agents)
        raise
    result.update(status='active', backup=str(backup))
    (backup / 'activation.json').write_text(json.dumps(result, ensure_ascii=False, indent=2) + '\n')
    return result


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--activate', action='store_true')
    args = parser.parse_args()
    print(json.dumps(install(Path.home() / '.codex', Path.home() / 'dotfiles/.codex/AGENTS.md',
                             Path(__file__).resolve().parent, args.activate), ensure_ascii=False))

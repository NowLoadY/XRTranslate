#!/usr/bin/env python3
"""Populate the installer's release-asset cache using GitHub's official CLI."""
import argparse
import json
from pathlib import Path
import subprocess
from urllib.parse import unquote, urlparse


def download(asset, destination):
    if destination.is_file() and destination.stat().st_size == asset['bytes']:
        return
    url = urlparse(asset['url'])
    parts = url.path.strip('/').split('/', 5)
    if url.hostname != 'github.com' or len(parts) != 6 or parts[2:4] != ['releases', 'download']:
        raise ValueError(f'Not a GitHub release asset: {asset["name"]}')
    owner, repository, _, _, tag, filename = map(unquote, parts)
    destination.parent.mkdir(parents=True, exist_ok=True)
    partial = destination.with_name(destination.name + '.part')
    subprocess.run(['gh', 'release', 'download', tag, '--repo', f'{owner}/{repository}',
                    '--pattern', filename, '--output', str(partial), '--clobber'], check=True)
    if partial.stat().st_size != asset['bytes']:
        raise RuntimeError(f'Unexpected resource size: {filename}')
    partial.replace(destination)
    print(f'Prepared {asset["name"]}', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--target', required=True)
    parser.add_argument('--cache', required=True, type=Path)
    parser.add_argument('--runtime-archive', required=True)
    args = parser.parse_args()
    config = json.loads((Path(__file__).resolve().parent.parent / 'config.json').read_text(encoding='utf-8'))
    manager = config['model_manager']
    runtime = manager['onnxruntime']
    matches = [a for a in runtime['downloads'] + runtime['cpu_downloads']
               if a['name'] == args.runtime_archive and a['target'] == args.target]
    if len(matches) != 1:
        parser.error(f'Expected one {args.target} runtime archive: {args.runtime_archive}')
    for asset in manager['bundled_models']:
        if asset.get('target') not in (None, args.target) or not asset.get('archive_path'):
            continue
        destination = args.cache / 'downloads' / (Path(asset['name']).name + '.zip')
        download(asset, destination)
    download(matches[0], args.cache / Path(matches[0]['name']).name)


if __name__ == '__main__':
    main()

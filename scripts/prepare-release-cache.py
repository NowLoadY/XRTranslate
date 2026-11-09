#!/usr/bin/env python3
"""Populate the installer's release-asset cache using GitHub's official CLI."""
import argparse
import json
from pathlib import Path
import subprocess
from urllib.parse import unquote, urlparse


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--target', required=True)
    parser.add_argument('--cache', required=True, type=Path)
    args = parser.parse_args()
    config = json.loads((Path(__file__).resolve().parent.parent / 'config.json').read_text(encoding='utf-8'))
    for asset in config['model_manager']['bundled_models']:
        if asset.get('target', args.target) != args.target or not asset.get('archive_path'):
            continue
        url = urlparse(asset['url'])
        parts = url.path.strip('/').split('/', 5)
        if url.hostname != 'github.com' or len(parts) != 6 or parts[2:4] != ['releases', 'download']:
            continue
        owner, repository, _, _, tag, filename = map(unquote, parts)
        destination = args.cache / (Path(asset['name']).name + '.zip')
        if destination.is_file() and destination.stat().st_size == asset['bytes']:
            continue
        destination.parent.mkdir(parents=True, exist_ok=True)
        partial = destination.with_suffix('.zip.part')
        subprocess.run(['gh', 'release', 'download', tag, '--repo', f'{owner}/{repository}',
                        '--pattern', filename, '--output', str(partial), '--clobber'], check=True)
        if partial.stat().st_size != asset['bytes']:
            raise RuntimeError(f'Unexpected resource size: {filename}')
        partial.replace(destination)
        print(f'Prepared {asset["label"]}', flush=True)


if __name__ == '__main__':
    main()

"""Verify and extract an exact prepared Linux upgrade pair.

The deb bytes arrive through the separately privileged fetch job. Their
identity and provenance come only from two manifests committed with the
controller checkout.
"""
import argparse
from datetime import datetime
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import subprocess
import sys
import tarfile


REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_EVIDENCE_DIR = 'docs/evidence/2026-09-15-linux-bootstrap'
ARCHITECTURES = {'x86_64': 'amd64', 'arm64': 'arm64'}
SHA40 = re.compile(r'[0-9a-f]{40}')
SHA256 = re.compile(r'[0-9a-f]{64}')
VERSION = re.compile(r'[0-9]+\.[0-9]+\.[0-9]+')
MANIFEST_KEYS = {
    'schemaVersion', 'kind', 'source', 'version', 'channel', 'iteration',
    'preparedAt', 'artifacts',
}
ARTIFACT_KEYS = {
    'architecture', 'sourceRevision', 'buildRevision', 'buildTree', 'version',
    'channel', 'iteration', 'label', 'fileName', 'sha256', 'sizeBytes',
    'reportSha256',
}


def _git(repo_root: Path, *args: str) -> bytes:
    try:
        return subprocess.run(
            ['git', *args], cwd=repo_root, check=True, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        ).stdout
    except subprocess.CalledProcessError as error:
        diagnostic = error.stderr.decode(errors='replace').strip()
        raise ValueError(f'evidence is not committed at HEAD: {diagnostic}') from None


def committed_manifest_paths(repo_root: Path, evidence_dir: str) -> tuple[Path, Path]:
    """Resolve a bounded, non-symlinked evidence directory committed at HEAD."""
    relative = PurePosixPath(evidence_dir)
    if (not evidence_dir or relative.is_absolute()
            or relative.parts[:2] != ('docs', 'evidence')
            or any(part in ('', '.', '..') for part in relative.parts)):
        raise ValueError('evidence directory must be beneath docs/evidence and repository-relative')

    current = repo_root
    for part in relative.parts:
        current = current / part
        if current.is_symlink():
            raise ValueError('evidence directory may not contain symlink components')
    try:
        resolved = current.resolve(strict=True)
        resolved.relative_to(repo_root.resolve(strict=True))
    except (FileNotFoundError, ValueError):
        raise ValueError('evidence directory is missing or escapes the repository') from None
    if not resolved.is_dir():
        raise ValueError('evidence path must name a directory')

    paths = tuple(resolved / side / 'manifest.json' for side in ('A', 'B'))
    for side, path in zip(('A', 'B'), paths):
        if path.parent.is_symlink() or path.is_symlink() or not path.is_file():
            raise ValueError(f'{side}/manifest.json must be a regular file, not a symlink')
        git_path = (relative / side / 'manifest.json').as_posix()
        mode = _git(repo_root, 'ls-tree', 'HEAD', '--', git_path).decode().split()
        if len(mode) < 4 or mode[0] not in ('100644', '100755') or mode[1] != 'blob':
            raise ValueError(f'evidence is not a committed regular file at HEAD: {git_path}')
        if path.read_bytes() != _git(repo_root, 'show', f'HEAD:{git_path}'):
            raise ValueError(f'evidence differs from committed HEAD: {git_path}')
    return paths[0], paths[1]


def _require_exact_keys(value: object, expected: set[str], label: str) -> dict:
    if not isinstance(value, dict) or set(value) != expected:
        raise ValueError(f'invalid {label} fields')
    return value


def read_manifest(path: Path) -> dict:
    try:
        manifest = _require_exact_keys(json.loads(path.read_text()), MANIFEST_KEYS, 'manifest')
    except (json.JSONDecodeError, UnicodeDecodeError) as error:
        raise ValueError(f'invalid manifest JSON: {path}') from error
    source = _require_exact_keys(manifest['source'], {'revision', 'tree'}, 'source')
    if (manifest['schemaVersion'] != 1 or isinstance(manifest['schemaVersion'], bool)
            or manifest['kind'] != 'linux-local-preparation'
            or manifest['channel'] != 'staging'
            or not isinstance(manifest['version'], str)
            or not VERSION.fullmatch(manifest['version'])
            or not isinstance(manifest['iteration'], int)
            or isinstance(manifest['iteration'], bool) or manifest['iteration'] < 1
            or not isinstance(manifest['preparedAt'], str)
            or not isinstance(source.get('revision'), str)
            or not SHA40.fullmatch(source['revision'])
            or not isinstance(source.get('tree'), str)
            or not SHA40.fullmatch(source['tree'])):
        raise ValueError(f'invalid Linux preparation identity: {path}')
    try:
        prepared_at = datetime.fromisoformat(manifest['preparedAt'].replace('Z', '+00:00'))
        if prepared_at.tzinfo is None:
            raise ValueError
    except ValueError:
        raise ValueError(f'invalid preparation timestamp: {path}') from None
    if not isinstance(manifest['artifacts'], list) or len(manifest['artifacts']) != 2:
        raise ValueError(f'manifest must contain exactly two architectures: {path}')

    seen = set()
    for value in manifest['artifacts']:
        artifact = _require_exact_keys(value, ARTIFACT_KEYS, 'artifact')
        architecture = artifact.get('architecture')
        if (not isinstance(architecture, str) or architecture not in ARCHITECTURES
                or architecture in seen):
            raise ValueError(f'manifest must contain x86_64 and arm64 exactly once: {path}')
        seen.add(architecture)
        expected_name = (
            f"kanna-staging_{manifest['version']}~staging.{manifest['iteration']}-1_"
            f"{ARCHITECTURES[architecture]}.deb"
        )
        if (artifact.get('sourceRevision') != source['revision']
                or artifact.get('buildRevision') != source['revision']
                or artifact.get('buildTree') != source['tree']
                or artifact.get('version') != manifest['version']
                or artifact.get('channel') != 'staging'
                or artifact.get('iteration') != manifest['iteration']
                or artifact.get('label') != f'//packaging/linux:deb_staging_{architecture}'
                or artifact.get('fileName') != expected_name
                or not isinstance(artifact.get('sha256'), str)
                or not SHA256.fullmatch(artifact['sha256'])
                or not isinstance(artifact.get('reportSha256'), str)
                or not SHA256.fullmatch(artifact['reportSha256'])
                or not isinstance(artifact.get('sizeBytes'), int)
                or isinstance(artifact.get('sizeBytes'), bool)
                or artifact['sizeBytes'] < 1):
            raise ValueError(f'incoherent Linux artifact/source provenance: {path}')
    return manifest


def load_pair(repo_root: Path, evidence_dir: str) -> tuple[dict, dict]:
    paths = committed_manifest_paths(repo_root, evidence_dir)
    old, new = (read_manifest(path) for path in paths)
    if (old['version'] != new['version'] or old['channel'] != new['channel']
            or old['iteration'] >= new['iteration']
            or old['source'] == new['source']):
        raise ValueError('prepared pair must be distinct ascending staging revisions of one version')
    return old, new


def verify_pair(archive: Path, destination: Path, evidence_dir: str,
                repo_root: Path = REPO_ROOT, selection_output: Path | None = None) -> None:
    old, new = load_pair(repo_root, evidence_dir)
    expected = {}
    selection = {'old': {}, 'new': {}}
    for role, manifest in (('old', old), ('new', new)):
        for artifact in manifest['artifacts']:
            name = artifact['fileName']
            if name in expected:
                raise ValueError('prepared pair contains duplicate artifact filenames')
            expected[name] = artifact
            selection[role][artifact['architecture']] = name

    with tarfile.open(archive, 'r:') as bundle:
        members = bundle.getmembers()
        if len(members) != 4 or {member.name for member in members} != set(expected):
            raise ValueError('bundle must contain exactly the four manifest-pinned deb filenames')
        destination.mkdir(parents=True, exist_ok=False)
        for member in members:
            artifact = expected[member.name]
            if not member.isfile() or member.size != artifact['sizeBytes']:
                raise ValueError(f'invalid type/size: {member.name}')
            source = bundle.extractfile(member)
            if source is None:
                raise ValueError(f'missing archive bytes: {member.name}')
            data = source.read()
            if hashlib.sha256(data).hexdigest() != artifact['sha256']:
                raise ValueError(f'checksum mismatch: {member.name}')
            (destination / member.name).write_bytes(data)
            print(json.dumps({'file': member.name, 'sha256': artifact['sha256'],
                              'source': artifact['sourceRevision'], 'tree': artifact['buildTree']}))
    if selection_output is not None:
        selection_output.parent.mkdir(parents=True, exist_ok=True)
        selection_output.write_text(json.dumps(selection, sort_keys=True) + '\n')


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument('archive', type=Path)
    parser.add_argument('destination', type=Path)
    parser.add_argument('--evidence-dir', default=DEFAULT_EVIDENCE_DIR)
    parser.add_argument('--selection-output', type=Path)
    args = parser.parse_args()
    verify_pair(args.archive, args.destination, args.evidence_dir,
                selection_output=args.selection_output)


if __name__ == '__main__':
    try:
        main()
    except Exception as error:
        print(str(error) if isinstance(error, ValueError) else type(error).__name__, file=sys.stderr)
        sys.exit(1)

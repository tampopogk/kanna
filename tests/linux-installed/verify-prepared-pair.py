"""Extract only the four exact collected debs; reject paths, links and substitutes."""
import hashlib
import json
from pathlib import Path
import sys
import tarfile


def verify_pair(archive: Path, destination: Path, evidence: Path) -> None:
    expected = {}
    for candidate in ('A', 'B'):
        manifest = json.loads((evidence / candidate / 'manifest.json').read_text())
        for artifact in manifest['artifacts']:
            expected[artifact['fileName']] = artifact
    with tarfile.open(archive, 'r:') as bundle:
        members = bundle.getmembers()
        if len(members) != len(expected) or {m.name for m in members} != set(expected):
            raise ValueError('bundle must contain exactly the four pinned deb filenames')
        destination.mkdir(parents=True, exist_ok=False)
        for member in members:
            artifact = expected[member.name]
            if not member.isfile() or member.size != artifact['sizeBytes']:
                raise ValueError(f'invalid type/size: {member.name}')
            data = bundle.extractfile(member).read()
            if hashlib.sha256(data).hexdigest() != artifact['sha256']:
                raise ValueError(f'checksum mismatch: {member.name}')
            (destination / member.name).write_bytes(data)
            print(json.dumps({'file': member.name, 'sha256': artifact['sha256'],
                              'source': artifact['sourceRevision'], 'tree': artifact['buildTree']}))


if __name__ == '__main__':
    verify_pair(Path(sys.argv[1]), Path(sys.argv[2]),
                Path(__file__).resolve().parents[2] / 'docs/evidence/2026-09-15-linux-bootstrap')

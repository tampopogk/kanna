import copy
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import unittest


spec = importlib.util.spec_from_file_location(
    'verify_pair', Path(__file__).with_name('verify-prepared-pair.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class PairFixture:
    def __init__(self, directory: str):
        self.root = Path(directory)
        self.relative = 'docs/evidence/dynamic-pair'
        self.evidence = self.root / self.relative
        self.archive = self.root / 'pair.tar'
        self.bytes = {}
        self.manifests = {
            'A': self._manifest('a' * 40, 'b' * 40, 7),
            'B': self._manifest('c' * 40, 'd' * 40, 9),
        }
        self.write_and_commit('pair fixture')
        with tarfile.open(self.archive, 'w:') as bundle:
            for manifest in self.manifests.values():
                for artifact in manifest['artifacts']:
                    data = self.bytes[artifact['fileName']]
                    member = tarfile.TarInfo(artifact['fileName'])
                    member.size = len(data)
                    bundle.addfile(member, io.BytesIO(data))

    def git(self, *args):
        return subprocess.run(
            ['git', *args], cwd=self.root, check=True, text=True,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        ).stdout.strip()

    def _manifest(self, revision, tree, iteration):
        artifacts = []
        for architecture, debian in module.ARCHITECTURES.items():
            name = f'kanna-staging_1.4.2~staging.{iteration}-1_{debian}.deb'
            data = f'exact {revision} {architecture}'.encode()
            self.bytes[name] = data
            artifacts.append({
                'architecture': architecture,
                'sourceRevision': revision,
                'buildRevision': revision,
                'buildTree': tree,
                'version': '1.4.2',
                'channel': 'staging',
                'iteration': iteration,
                'label': f'//packaging/linux:deb_staging_{architecture}',
                'fileName': name,
                'sha256': hashlib.sha256(data).hexdigest(),
                'sizeBytes': len(data),
                'reportSha256': hashlib.sha256(b'synthetic report').hexdigest(),
            })
        return {
            'schemaVersion': 1,
            'kind': 'linux-local-preparation',
            'source': {'revision': revision, 'tree': tree},
            'version': '1.4.2',
            'channel': 'staging',
            'iteration': iteration,
            'preparedAt': '2026-09-16T00:00:00.000Z',
            'artifacts': artifacts,
        }

    def write(self):
        for side, manifest in self.manifests.items():
            path = self.evidence / side / 'manifest.json'
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(json.dumps(manifest, indent=2) + '\n')

    def write_and_commit(self, message):
        if not (self.root / '.git').exists():
            self.git('init')
        self.write()
        self.git('add', 'docs/evidence')
        self.git('-c', 'user.name=Pair fixture', '-c', 'user.email=test@example.invalid',
                 '-c', 'commit.gpgsign=false', 'commit', '-m', message)


class VerifyPreparedPairTests(unittest.TestCase):
    def test_default_bootstrap_pair_remains_compatible(self):
        old, new = module.load_pair(module.REPO_ROOT, module.DEFAULT_EVIDENCE_DIR)
        self.assertEqual((old['iteration'], new['iteration']), (1, 2))

    def test_dynamic_committed_pair_selects_manifest_filenames(self):
        with tempfile.TemporaryDirectory() as directory:
            fixture = PairFixture(directory)
            destination = fixture.root / 'packages'
            selection = fixture.root / 'selection.json'
            module.verify_pair(fixture.archive, destination, fixture.relative,
                               fixture.root, selection)
            selected = json.loads(selection.read_text())
            self.assertEqual(
                selected['old']['x86_64'],
                'kanna-staging_1.4.2~staging.7-1_amd64.deb',
            )
            self.assertEqual(
                selected['new']['arm64'],
                'kanna-staging_1.4.2~staging.9-1_arm64.deb',
            )
            self.assertEqual(
                {path.name for path in destination.iterdir()}, set(fixture.bytes))

    def test_rejects_uncommitted_manifest_change(self):
        with tempfile.TemporaryDirectory() as directory:
            fixture = PairFixture(directory)
            fixture.manifests['B']['artifacts'][0]['sha256'] = 'f' * 64
            fixture.write()
            with self.assertRaisesRegex(ValueError, 'differs from committed HEAD'):
                module.verify_pair(fixture.archive, fixture.root / 'packages',
                                   fixture.relative, fixture.root)

    def test_rejects_committed_hash_tampering(self):
        with tempfile.TemporaryDirectory() as directory:
            fixture = PairFixture(directory)
            fixture.manifests['B']['artifacts'][0]['sha256'] = 'f' * 64
            fixture.write_and_commit('tamper hash')
            with self.assertRaisesRegex(ValueError, 'checksum mismatch'):
                module.verify_pair(fixture.archive, fixture.root / 'packages',
                                   fixture.relative, fixture.root)

    def test_rejects_committed_source_and_filename_mismatches(self):
        changes = (
            ('source', lambda manifest: manifest['artifacts'][0].update(
                sourceRevision='e' * 40)),
            ('filename', lambda manifest: manifest['artifacts'][0].update(
                fileName='../substitute.deb')),
        )
        for label, change in changes:
            with self.subTest(label=label), tempfile.TemporaryDirectory() as directory:
                fixture = PairFixture(directory)
                change(fixture.manifests['B'])
                fixture.write_and_commit(f'tamper {label}')
                with self.assertRaisesRegex(ValueError, 'incoherent Linux artifact/source provenance'):
                    module.verify_pair(fixture.archive, fixture.root / 'packages',
                                       fixture.relative, fixture.root)

    def test_rejects_unsafe_or_uncommitted_evidence_directories(self):
        with tempfile.TemporaryDirectory() as directory:
            fixture = PairFixture(directory)
            for evidence in ('/absolute/evidence', '../evidence', 'docs/other/evidence'):
                with self.subTest(evidence=evidence), self.assertRaisesRegex(
                        ValueError, 'beneath docs/evidence'):
                    module.load_pair(fixture.root, evidence)
            copied = fixture.root / 'docs/evidence/uncommitted'
            copied.mkdir()
            for side in ('A', 'B'):
                target = copied / side
                target.mkdir()
                (target / 'manifest.json').write_text(json.dumps(
                    copy.deepcopy(fixture.manifests[side])))
            with self.assertRaisesRegex(ValueError, 'not a committed regular file'):
                module.load_pair(fixture.root, 'docs/evidence/uncommitted')
            shutil.rmtree(fixture.evidence / 'B')
            os.symlink(fixture.evidence / 'A', fixture.evidence / 'B')
            with self.assertRaisesRegex(ValueError, 'regular file, not a symlink'):
                module.load_pair(fixture.root, fixture.relative)


if __name__ == '__main__':
    unittest.main()

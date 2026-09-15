import gzip
import io
import os
from pathlib import Path
import struct
import sys
import tarfile
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from artifact_tool import elf_facts, write_deb


class ArtifactToolTest(unittest.TestCase):
    def test_archive_is_deterministic_and_has_debian_layout(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp) / 'tree'
            (root / 'DEBIAN').mkdir(parents=True)
            (root / 'usr/bin').mkdir(parents=True)
            (root / 'DEBIAN/control').write_text('Package: fixture\nVersion: 1.0-1\nArchitecture: arm64\n')
            (root / 'usr/bin/program').write_bytes(b'executable')
            (root / 'usr/bin/program').chmod(0o755)
            (root / 'usr/bin/link').symlink_to('program')
            first, second = Path(temp) / 'a.deb', Path(temp) / 'b.deb'
            write_deb(root, first)
            for path in root.rglob('*'):
                os.utime(path, (1234567, 1234567), follow_symlinks=False)
            write_deb(root, second)
            self.assertEqual(first.read_bytes(), second.read_bytes())
            data = first.read_bytes()
            self.assertEqual(data[:8], b'!<arch>\n')
            members, pos = {}, 8
            while pos < len(data):
                header = data[pos:pos + 60]
                size = int(header[48:58])
                name = header[:16].decode().strip().rstrip('/')
                self.assertEqual(header[58:60], b'`\n')
                members[name] = data[pos + 60:pos + 60 + size]
                pos += 60 + size + size % 2
            self.assertEqual(list(members), ['debian-binary', 'control.tar.gz', 'data.tar.gz'])
            self.assertEqual(members['debian-binary'], b'2.0\n')
            with tarfile.open(fileobj=io.BytesIO(gzip.decompress(members['data.tar.gz']))) as tar:
                self.assertFalse(any('DEBIAN' in name for name in tar.getnames()))
                self.assertEqual(tar.getmember('./usr/bin/program').mode, 0o755)
                self.assertEqual(tar.getmember('./usr/bin/link').linkname, 'program')
                self.assertTrue(all(i.uid == i.gid == i.mtime == 0 for i in tar.getmembers()))

    def test_elf_dynamic_dependencies_versions_and_search_path(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / 'elf'
            data = bytearray(1024)
            data[:6] = b'\x7fELF\x02\x01'
            struct.pack_into('<HH', data, 16, 3, 183)
            struct.pack_into('<QQ', data, 32, 0, 64)
            struct.pack_into('<HHHH', data, 54, 56, 0, 64, 4)
            strings = b'\0libc.so.6\0GLIBC_2.39\0$ORIGIN\0'
            data[512:512 + len(strings)] = strings
            # Null, dynstr, dynamic, verneed sections.
            struct.pack_into('<IIQQQQIIQQ', data, 128, 0, 3, 0, 0, 512, len(strings), 0, 0, 1, 0)
            struct.pack_into('<IIQQQQIIQQ', data, 192, 0, 6, 0, 0, 600, 48, 1, 0, 8, 16)
            struct.pack_into('<IIQQQQIIQQ', data, 256, 0, 0x6ffffffe, 0, 0, 700, 32, 1, 0, 4, 0)
            struct.pack_into('<qQqQqQ', data, 600, 1, 1, 29, strings.index(b'$ORIGIN'), 0, 0)
            struct.pack_into('<HHIII', data, 700, 1, 1, 1, 16, 0)
            struct.pack_into('<IHHII', data, 716, 0, 0, 2, 11, 0)
            path.write_bytes(data)
            facts = elf_facts(str(path))
            self.assertEqual(facts['needed'], ['libc.so.6'])
            self.assertEqual(facts['versionRequirements'], {'GLIBC': ['2.39']})
            self.assertEqual(facts['runpaths'], ['$ORIGIN'])

    def test_elf_header_architecture_and_interpreter(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / 'elf'
            for machine, name, interpreter in [(183, 'AArch64', '/lib/ld-linux-aarch64.so.1'), (62, 'Advanced Micro Devices X86-64', '/lib64/ld-linux-x86-64.so.2')]:
                data = bytearray(256)
                data[:6] = b'\x7fELF\x02\x01'
                struct.pack_into('<HH', data, 16, 3, machine)
                struct.pack_into('<QQ', data, 32, 64, 0)
                struct.pack_into('<HHHH', data, 54, 56, 1, 64, 0)
                payload = interpreter.encode() + b'\0'
                struct.pack_into('<IIQQQQ', data, 64, 3, 0, 128, 0, 0, len(payload))
                data[128:128 + len(payload)] = payload
                path.write_bytes(data)
                facts = elf_facts(str(path))
                self.assertEqual(facts['machine'], name)
                self.assertEqual(facts['interpreter'], interpreter)
            path.write_bytes(b'not ELF')
            with self.assertRaises(ValueError):
                elf_facts(str(path))


if __name__ == '__main__':
    unittest.main()

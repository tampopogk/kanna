"""Execution-platform ELF inspection and deterministic Debian archive writer.

Uses only the pinned Python runtime. No target executable is run, and no host
readelf, ar, tar, compressor, or dpkg installation participates in assembly.
"""
import gzip
import io
import json
from pathlib import Path
import struct
import sys
import tarfile


def elf_facts(path):
    data = Path(path).read_bytes()
    if data[:6] != b'\x7fELF\x02\x01':
        raise ValueError(f'{path}: expected a little-endian ELF64 executable')

    def unpack(fmt, offset):
        return struct.unpack_from('<' + fmt, data, offset)

    def string(table, offset):
        end = table.index(b'\0', offset)
        return table[offset:end].decode('utf-8')

    machine = unpack('H', 18)[0]
    machines = {62: 'Advanced Micro Devices X86-64', 183: 'AArch64'}
    if machine not in machines or unpack('H', 16)[0] not in (2, 3):
        raise ValueError(f'{path}: unsupported executable machine/type')
    phoff, shoff = unpack('QQ', 32)
    phsize, phnum, shsize, shnum = unpack('HHHH', 54)
    interpreter = None
    for i in range(phnum):
        p = phoff + i * phsize
        kind, _, offset, _, _, size = unpack('IIQQQQ', p)
        if kind == 3:
            interpreter = data[offset:offset + size].rstrip(b'\0').decode()
    sections = [unpack('IIQQQQIIQQ', shoff + i * shsize) for i in range(shnum)]
    needed, runpaths, versions = [], [], {}
    for section in sections:
        _, kind, _, _, offset, size, link, _, _, entsize = section
        if kind not in (6, 0x6ffffffe):
            continue
        strings_section = sections[link]
        strings = data[strings_section[4]:strings_section[4] + strings_section[5]]
        if kind == 6:
            if entsize != 16:
                raise ValueError('invalid ELF64 dynamic entry size')
            for p in range(offset, offset + size, entsize):
                tag, value = unpack('qQ', p)
                if tag == 1:
                    needed.append(string(strings, value))
                elif tag in (15, 29):
                    runpaths.extend(string(strings, value).split(':'))
        else:
            p, end = offset, offset + size
            while p < end:
                _, count, _, aux, next_need = unpack('HHIII', p)
                a = p + aux
                for _ in range(count):
                    _, _, _, name, next_aux = unpack('IHHII', a)
                    value = string(strings, name)
                    family, _, version = value.rpartition('_')
                    if version and all(part.isdigit() for part in version.split('.')):
                        versions.setdefault(family, []).append(version)
                    a += next_aux
                if next_need == 0:
                    break
                p += next_need
    return dict(path=path, machine=machines[machine], interpreter=interpreter,
                needed=sorted(set(needed)), runpaths=runpaths,
                versionRequirements={k: sorted(set(v)) for k, v in versions.items()})


def tar_gz(root, control):
    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode='w', format=tarfile.USTAR_FORMAT) as tar:
        base = root / 'DEBIAN' if control else root
        paths = [base] + sorted(base.rglob('*'))
        for path in paths:
            relative = path.relative_to(base)
            if not control and relative.parts and relative.parts[0] == 'DEBIAN':
                continue
            name = './' if path == base else './' + relative.as_posix()
            info = tar.gettarinfo(str(path), arcname=name)
            info.uid = info.gid = info.mtime = 0
            info.uname = info.gname = 'root'
            if info.isfile():
                with path.open('rb') as f:
                    tar.addfile(info, f)
            else:
                tar.addfile(info)
    compressed = io.BytesIO()
    with gzip.GzipFile(fileobj=compressed, mode='wb', filename='', mtime=0, compresslevel=9) as f:
        f.write(raw.getvalue())
    return compressed.getvalue()


def write_deb(root, output):
    with Path(output).open('wb') as f:
        f.write(b'!<arch>\n')
        for name, data in [('debian-binary', b'2.0\n'),
                           ('control.tar.gz', tar_gz(Path(root), True)),
                           ('data.tar.gz', tar_gz(Path(root), False))]:
            header = f'{name + "/":<16}{0:<12}{0:<6}{0:<6}{"100644":<8}{len(data):<10}`\n'
            if len(header) != 60:
                raise ValueError('invalid ar header')
            f.write(header.encode('ascii'))
            f.write(data)
            if len(data) % 2:
                f.write(b'\n')


if __name__ == '__main__':
    if sys.argv[1] == 'elf':
        print(json.dumps([elf_facts(p) for p in sys.argv[2:]]))
    elif sys.argv[1] == 'deb' and len(sys.argv) == 4:
        write_deb(sys.argv[2], sys.argv[3])
    else:
        raise SystemExit('usage: artifact_tool elf <paths...> | deb <tree> <output>')

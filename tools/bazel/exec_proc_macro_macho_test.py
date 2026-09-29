"""Checks that exec-configuration proc macros are loadable Mach-O dylibs.

rustc's opt-mode `-Cstrip=debuginfo` runs its bundled rust-objcopy on Apple,
which can leave the LC_SYMTAB string table 4-byte aligned. macOS 27 dyld then
refuses to dlopen the dylib ("mis-aligned LINKEDIT string pool") and rustc
reports E0463 for the proc macro. MODULE.bazel keeps exec outputs unstripped.
"""

import ctypes
import os
import struct
import sys
import unittest
from pathlib import Path

MH_MAGIC_64 = 0xFEEDFACF
LC_SYMTAB = 0x2
N_STAB = 0xE0


def read_symtab(data: bytes) -> tuple[int, int, int]:
    magic, _, _, _, ncmds, _, _, _ = struct.unpack_from("<8I", data, 0)
    if magic != MH_MAGIC_64:
        raise ValueError(f"not a 64-bit Mach-O: magic {magic:#x}")
    offset = 32
    for _ in range(ncmds):
        cmd, cmdsize = struct.unpack_from("<2I", data, offset)
        if cmd == LC_SYMTAB:
            symoff, nsyms, stroff, _ = struct.unpack_from("<4I", data, offset + 8)
            return symoff, nsyms, stroff
        offset += cmdsize
    raise ValueError("no LC_SYMTAB load command")


def count_stabs(data: bytes, symoff: int, nsyms: int) -> int:
    # nlist_64: n_strx (u32), n_type (u8), n_sect (u8), n_desc (u16), n_value (u64)
    return sum(
        1 for i in range(nsyms) if data[symoff + i * 16 + 4] & N_STAB
    )


class ExecProcMacroMachoTest(unittest.TestCase):
    def setUp(self) -> None:
        self.dylib = Path(os.environ["EXEC_PROC_MACRO_DYLIB"]).resolve()
        self.data = self.dylib.read_bytes()
        self.symoff, self.nsyms, self.stroff = read_symtab(self.data)

    def test_exec_proc_macro_is_not_objcopy_stripped(self) -> None:
        self.assertGreater(
            count_stabs(self.data, self.symoff, self.nsyms),
            0,
            "exec proc macro was debuginfo-stripped; keep -Cstrip=none in "
            "rust.toolchain extra_exec_rustc_flags",
        )

    def test_string_table_is_pointer_aligned(self) -> None:
        self.assertEqual(self.stroff % 8, 0, f"stroff {self.stroff:#x}")

    def test_dyld_loads_exec_proc_macro(self) -> None:
        ctypes.CDLL(str(self.dylib))


if __name__ == "__main__":
    sys.exit(unittest.main())

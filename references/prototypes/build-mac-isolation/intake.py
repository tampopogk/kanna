import hashlib
import os
import unicodedata

MAX_FILE = 512 << 20
MAX_TOTAL = 2 << 30
MAX_FILES = 5000
MAX_COMPONENT = 255
MAX_PATH = 1024


class Rejected(ValueError):
    pass


def check_path(rel):
    if not isinstance(rel, str) or not rel:
        raise Rejected("path is not a non-empty string")
    if len(rel.encode()) > MAX_PATH:
        raise Rejected("path too long")
    if "\x00" in rel or "\\" in rel:
        raise Rejected("NUL or backslash in path")
    if unicodedata.normalize("NFC", rel) != rel:
        raise Rejected("path not NFC-normalised")
    if rel.startswith("/"):
        raise Rejected("absolute path")
    parts = rel.split("/")
    for part in parts:
        if part in ("", ".", ".."):
            raise Rejected("empty, dot or dot-dot component")
        if len(part.encode()) > MAX_COMPONENT:
            raise Rejected("component too long")
        if any(ord(c) < 32 for c in part):
            raise Rejected("control character in path")
    return parts


class Store:
    def __init__(self, root):
        os.mkdir(root, 0o700)
        self.root_fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        self.seen = set()
        self.total = 0
        self.manifest = []

    def close(self):
        os.close(self.root_fd)

    def put(self, rel, data, sha256):
        parts = check_path(rel)
        folded = "/".join(parts).casefold()
        if folded in self.seen or any(s.startswith(folded + "/") or folded.startswith(s + "/") for s in self.seen):
            raise Rejected("duplicate or file/directory collision (case-insensitive)")
        if len(self.seen) >= MAX_FILES:
            raise Rejected("too many files")
        if len(data) > MAX_FILE or self.total + len(data) > MAX_TOTAL:
            raise Rejected("size limit")
        if hashlib.sha256(data).hexdigest() != sha256:
            raise Rejected("checksum mismatch")
        fd = os.dup(self.root_fd)
        try:
            for part in parts[:-1]:
                try:
                    os.mkdir(part, 0o700, dir_fd=fd)
                except FileExistsError:
                    pass
                nfd = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd)
                os.close(fd)
                fd = nfd
            out = os.open(parts[-1], os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600, dir_fd=fd)
            try:
                view = memoryview(data)
                while view:
                    view = view[os.write(out, view):]
            finally:
                os.close(out)
        except OSError as error:
            raise Rejected("filesystem refused: %s" % error.strerror)
        finally:
            os.close(fd)
        self.seen.add(folded)
        self.total += len(data)
        self.manifest.append({"path": rel, "sha256": sha256, "bytes": len(data)})

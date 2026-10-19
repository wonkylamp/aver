#!/usr/bin/env python3
"""Real disk use of each directory given, in bytes: what removing it alone frees.

The same measure as crates/aver/src/disk/usage.rs `real_bytes`: per file the
APFS private size (ATTR_CMNEXT_PRIVATESIZE, blocks no clone shares), each inode
once, symlinks not followed; allocated blocks where the attribute is missing.
`aver doctor` prints the same number rounded to 0.1 GiB, too coarse here.

    python3 -I bench/private-bytes.py DIR...   # one "<bytes>\t<dir>" line each
"""
import ctypes
import os
import stat
import sys

ATTR_BIT_MAP_COUNT = 5
ATTR_CMNEXT_PRIVATESIZE = 0x8
FSOPT_NOFOLLOW = 0x1
FSOPT_ATTR_CMN_EXTENDED = 0x20


class AttrList(ctypes.Structure):
    _fields_ = [
        ("bitmapcount", ctypes.c_uint16),
        ("reserved", ctypes.c_uint16),
        ("commonattr", ctypes.c_uint32),
        ("volattr", ctypes.c_uint32),
        ("dirattr", ctypes.c_uint32),
        ("fileattr", ctypes.c_uint32),
        ("forkattr", ctypes.c_uint32),
    ]


class Reply(ctypes.Structure):
    _pack_ = 4
    _fields_ = [("length", ctypes.c_uint32), ("private", ctypes.c_int64)]


libc = ctypes.CDLL(None, use_errno=True)
getattrlist = getattr(libc, "getattrlist", None)


def private_bytes(path, st):
    if getattrlist is not None:
        attrs = AttrList(ATTR_BIT_MAP_COUNT, 0, 0, 0, 0, 0, ATTR_CMNEXT_PRIVATESIZE)
        reply = Reply()
        rc = getattrlist(
            os.fsencode(path),
            ctypes.byref(attrs),
            ctypes.byref(reply),
            ctypes.sizeof(reply),
            FSOPT_NOFOLLOW | FSOPT_ATTR_CMN_EXTENDED,
        )
        if rc == 0 and reply.length >= ctypes.sizeof(reply):
            return max(reply.private, 0)
    return st.st_blocks * 512


def real_bytes(root):
    seen = set()
    total = 0
    stack = [root]
    while stack:
        path = stack.pop()
        try:
            st = os.lstat(path)
        except OSError:
            continue
        if stat.S_ISDIR(st.st_mode):
            try:
                stack.extend(os.path.join(path, e) for e in os.listdir(path))
            except OSError:
                pass
        elif stat.S_ISREG(st.st_mode) and (st.st_dev, st.st_ino) not in seen:
            seen.add((st.st_dev, st.st_ino))
            total += private_bytes(path, st)
    return total


for d in sys.argv[1:]:
    print(f"{real_bytes(d)}\t{d}")

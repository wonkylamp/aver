//! Real disk use: the blocks a tree holds, not the sum of its files'
//! lengths. On APFS, copy-on-write clones (a workspace's checkout, a slot
//! cloned from where its dependencies were installed) share blocks, so
//! each file counts only what deleting it would free
//! (`ATTR_CMNEXT_PRIVATESIZE`): blocks two clones still share count in
//! neither.
//! Elsewhere each file counts its allocated blocks once per inode, which
//! is an upper bound where reflinks share extents.

use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::MetadataExt as _;
use std::path::Path;

/// The bytes on disk that removing `dir` would free, not following
/// symlinks. A missing directory is 0.
pub fn real_bytes(dir: &Path) -> u64 {
    let mut seen = HashSet::new();
    walk(dir, &mut seen)
}

fn walk(path: &Path, seen: &mut HashSet<(u64, u64)>) -> u64 {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return 0;
    };
    if meta.is_dir() {
        return fs::read_dir(path)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| walk(&e.path(), seen))
            .sum();
    }
    if !meta.is_file() || !seen.insert((meta.dev(), meta.ino())) {
        return 0;
    }
    private_bytes(path).unwrap_or(meta.blocks() * 512)
}

/// The bytes of the file at `path` no clone or snapshot shares.
#[cfg(target_os = "macos")]
fn private_bytes(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt as _;
    // <sys/attr.h>
    const ATTR_BIT_MAP_COUNT: u16 = 5;
    const ATTR_CMNEXT_PRIVATESIZE: u32 = 0x0000_0008;
    const FSOPT_NOFOLLOW: u32 = 0x0000_0001;
    const FSOPT_ATTR_CMN_EXTENDED: u32 = 0x0000_0020;
    #[repr(C)]
    struct AttrList {
        bitmapcount: u16,
        reserved: u16,
        commonattr: u32,
        volattr: u32,
        dirattr: u32,
        fileattr: u32,
        forkattr: u32,
    }
    #[repr(C, packed(4))]
    struct Reply {
        length: u32,
        private: i64,
    }
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut list = AttrList {
        bitmapcount: ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: 0,
        volattr: 0,
        dirattr: 0,
        fileattr: 0,
        forkattr: ATTR_CMNEXT_PRIVATESIZE,
    };
    let mut reply = Reply {
        length: 0,
        private: 0,
    };
    // SAFETY: `c` is NUL-terminated, `list` is a valid attribute list and
    // `reply` is writable for the size passed.
    let rc = unsafe {
        libc::getattrlist(
            c.as_ptr(),
            (&mut list as *mut AttrList).cast(),
            (&mut reply as *mut Reply).cast(),
            std::mem::size_of::<Reply>(),
            FSOPT_NOFOLLOW | FSOPT_ATTR_CMN_EXTENDED,
        )
    };
    let (length, private) = (reply.length, reply.private);
    (rc == 0 && length as usize >= std::mem::size_of::<Reply>())
        .then(|| u64::try_from(private).ok())
        .flatten()
}

#[cfg(not(target_os = "macos"))]
fn private_bytes(_: &Path) -> Option<u64> {
    None
}

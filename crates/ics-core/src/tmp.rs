//! Private scratch directories for external tool runs (ngspice, sign-off, Virtuoso, git).
//!
//! On a shared machine the system temporary folder is writable by everyone: a predictable
//! name created with `create_dir_all` would accept a folder another user made first, with
//! their `.spiceinit`, `.cdsinit` or symbolic links in it. These folders get an unguessable
//! name and are created exclusively, readable only by their owner.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// A new, empty directory `<temp>/<prefix>-<pid>-<random>`, mode 0700 on Unix.
pub fn private_dir(prefix: &str) -> io::Result<PathBuf> {
    static N: AtomicU64 = AtomicU64::new(0);
    for _ in 0..64 {
        // RandomState is keyed from the operating system's random source per process.
        let mut h = RandomState::new().build_hasher();
        h.write_u64(N.fetch_add(1, Ordering::Relaxed));
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
        let d = std::env::temp_dir().join(format!("{prefix}-{}-{:016x}", std::process::id(), h.finish()));
        #[cfg_attr(not(unix), allow(unused_mut))]
        let mut b = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            b.mode(0o700);
        }
        match b.create(&d) {
            Ok(()) => return Ok(d),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other(format!("cannot create a scratch directory for {prefix}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_and_private() {
        let (a, b) = (private_dir("ics-tmp-test").unwrap(), private_dir("ics-tmp-test").unwrap());
        assert_ne!(a, b);
        assert!(a.is_dir() && std::fs::read_dir(&a).unwrap().next().is_none());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&a).unwrap().permissions().mode() & 0o777, 0o700);
        }
        std::fs::remove_dir(a).unwrap();
        std::fs::remove_dir(b).unwrap();
    }
}

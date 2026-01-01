//! Local object cache for LFS objects.
//!
//! Stores LFS objects in `.git/lfs/objects/` to avoid re-downloading
//! and enable offline access.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::{Oid, Pointer, Result};

/// Local cache for LFS objects.
///
/// Objects are stored in the git-lfs standard layout:
/// `.git/lfs/objects/<oid[0:2]>/<oid[2:4]>/<oid>`
pub struct ObjectCache {
    base_path: PathBuf,
}

impl ObjectCache {
    /// Create a new object cache at the given base path.
    ///
    /// Typically this is `.git/lfs/objects` within a repository.
    pub fn new<P: AsRef<Path>>(base_path: P) -> Self {
        ObjectCache {
            base_path: base_path.as_ref().to_path_buf(),
        }
    }

    /// Create a cache for a repository's `.git/lfs/objects` directory.
    pub fn for_repo<P: AsRef<Path>>(git_dir: P) -> Self {
        let base_path = git_dir.as_ref().join("lfs").join("objects");
        ObjectCache { base_path }
    }

    /// Get the path where an object with the given OID would be stored.
    pub fn object_path(&self, oid: &Oid) -> PathBuf {
        let hex = oid.to_hex();
        self.base_path
            .join(&hex[0..2])
            .join(&hex[2..4])
            .join(&hex)
    }

    /// Check if an object exists in the cache.
    pub fn contains(&self, oid: &Oid) -> bool {
        self.object_path(oid).exists()
    }

    /// Check if an object exists and has the correct size.
    pub fn contains_valid(&self, pointer: &Pointer) -> bool {
        let path = self.object_path(pointer.oid());
        match fs::metadata(&path) {
            Ok(meta) => meta.len() == pointer.size(),
            Err(_) => false,
        }
    }

    /// Get an object from the cache.
    ///
    /// Returns `None` if the object is not cached.
    pub fn get(&self, oid: &Oid) -> Option<Vec<u8>> {
        let path = self.object_path(oid);
        fs::read(&path).ok()
    }

    /// Get an object and verify its hash.
    ///
    /// Returns `None` if not cached or hash doesn't match.
    pub fn get_verified(&self, pointer: &Pointer) -> Option<Vec<u8>> {
        let content = self.get(pointer.oid())?;

        // Verify size
        if content.len() as u64 != pointer.size() {
            return None;
        }

        // Verify hash
        let computed = Pointer::from_content(&content);
        if computed.oid() != pointer.oid() {
            return None;
        }

        Some(content)
    }

    /// Store an object in the cache.
    ///
    /// The object is stored atomically using a temp file + rename.
    pub fn put(&self, oid: &Oid, content: &[u8]) -> Result<()> {
        let path = self.object_path(oid);

        // Create parent directories
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(crate::Error::Io)?;
        }

        // Write to temp file first (atomic)
        let temp_path = path.with_extension("tmp");
        {
            let mut file = File::create(&temp_path).map_err(crate::Error::Io)?;
            file.write_all(content).map_err(crate::Error::Io)?;
            file.sync_all().map_err(crate::Error::Io)?;
        }

        // Rename to final path
        fs::rename(&temp_path, &path).map_err(crate::Error::Io)?;

        Ok(())
    }

    /// Store an object and verify the hash matches.
    pub fn put_verified(&self, pointer: &Pointer, content: &[u8]) -> Result<()> {
        // Verify content matches pointer
        let computed = Pointer::from_content(content);
        if computed.oid() != pointer.oid() {
            return Err(crate::Error::InvalidPointer(
                "content hash does not match pointer".into(),
            ));
        }
        if computed.size() != pointer.size() {
            return Err(crate::Error::InvalidPointer(
                "content size does not match pointer".into(),
            ));
        }

        self.put(pointer.oid(), content)
    }

    /// Remove an object from the cache.
    pub fn remove(&self, oid: &Oid) -> Result<bool> {
        let path = self.object_path(oid);
        match fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(crate::Error::Io(e)),
        }
    }

    /// Get the total size of cached objects in bytes.
    pub fn size(&self) -> u64 {
        self.iter_objects()
            .filter_map(|path| fs::metadata(&path).ok())
            .map(|meta| meta.len())
            .sum()
    }

    /// Get the number of cached objects.
    pub fn count(&self) -> usize {
        self.iter_objects().count()
    }

    /// Iterate over all cached object paths.
    fn iter_objects(&self) -> impl Iterator<Item = PathBuf> {
        let base = self.base_path.clone();

        walkdir(base)
    }

    /// Prune objects not referenced by any pointer.
    ///
    /// Takes an iterator of OIDs that should be kept.
    pub fn prune<'a>(&self, keep: impl Iterator<Item = &'a Oid>) -> Result<u64> {
        let keep_set: std::collections::HashSet<_> = keep.map(|o| o.to_hex()).collect();
        let mut removed = 0u64;

        for path in self.iter_objects() {
            if let Some(filename) = path.file_name().and_then(|s| s.to_str()) {
                if !keep_set.contains(filename) {
                    if let Ok(meta) = fs::metadata(&path) {
                        removed += meta.len();
                        let _ = fs::remove_file(&path);
                    }
                }
            }
        }

        Ok(removed)
    }

    /// Open a cached object for streaming read.
    pub fn open(&self, oid: &Oid) -> Option<File> {
        let path = self.object_path(oid);
        File::open(&path).ok()
    }

    /// Create a writer for storing an object.
    ///
    /// Returns a `CacheWriter` that will atomically store the object
    /// when finished.
    pub fn writer(&self, oid: &Oid) -> Result<CacheWriter> {
        let final_path = self.object_path(oid);

        // Create parent directories
        if let Some(parent) = final_path.parent() {
            fs::create_dir_all(parent).map_err(crate::Error::Io)?;
        }

        let temp_path = final_path.with_extension("tmp");
        let file = File::create(&temp_path).map_err(crate::Error::Io)?;

        Ok(CacheWriter {
            file,
            temp_path,
            final_path,
            finished: false,
        })
    }
}

/// Writer for streaming content into the cache.
pub struct CacheWriter {
    file: File,
    temp_path: PathBuf,
    final_path: PathBuf,
    finished: bool,
}

impl CacheWriter {
    /// Finish writing and atomically move to final location.
    pub fn finish(mut self) -> Result<()> {
        self.file.sync_all().map_err(crate::Error::Io)?;
        fs::rename(&self.temp_path, &self.final_path).map_err(crate::Error::Io)?;
        self.finished = true;
        Ok(())
    }
}

impl Write for CacheWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.file.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

impl Drop for CacheWriter {
    fn drop(&mut self) {
        if !self.finished {
            // Clean up temp file on error/drop
            let _ = fs::remove_file(&self.temp_path);
        }
    }
}

/// Walk a directory tree and return all file paths.
fn walkdir(base: PathBuf) -> impl Iterator<Item = PathBuf> {
    let mut stack = vec![base];

    std::iter::from_fn(move || {
        while let Some(path) = stack.pop() {
            if path.is_dir() {
                if let Ok(entries) = fs::read_dir(&path) {
                    for entry in entries.flatten() {
                        stack.push(entry.path());
                    }
                }
            } else if path.is_file() {
                return Some(path);
            }
        }
        None
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_object_path() {
        let cache = ObjectCache::new("/tmp/lfs/objects");
        let oid = Oid::from_hex("4d7a214614ab2935c943f9e0ff69d22eadbb8f32b1258daaa5e2ca24d17e2393").unwrap();

        let path = cache.object_path(&oid);
        assert!(path.to_string_lossy().contains("4d"));
        assert!(path.to_string_lossy().contains("7a"));
        assert!(path.to_string_lossy().ends_with("4d7a214614ab2935c943f9e0ff69d22eadbb8f32b1258daaa5e2ca24d17e2393"));
    }

    #[test]
    fn test_put_get() {
        let td = TempDir::new().unwrap();
        let cache = ObjectCache::new(td.path());

        let content = b"Hello, World!";
        let pointer = Pointer::from_content(content);

        // Initially not present
        assert!(!cache.contains(pointer.oid()));

        // Store it
        cache.put_verified(&pointer, content).unwrap();

        // Now present
        assert!(cache.contains(pointer.oid()));
        assert!(cache.contains_valid(&pointer));

        // Retrieve it
        let retrieved = cache.get_verified(&pointer).unwrap();
        assert_eq!(retrieved, content);
    }

    #[test]
    fn test_streaming_write() {
        let td = TempDir::new().unwrap();
        let cache = ObjectCache::new(td.path());

        let content = b"Streaming content";
        let pointer = Pointer::from_content(content);

        // Write via streaming
        let mut writer = cache.writer(pointer.oid()).unwrap();
        writer.write_all(content).unwrap();
        writer.finish().unwrap();

        // Verify
        let retrieved = cache.get(pointer.oid()).unwrap();
        assert_eq!(retrieved, content);
    }

    #[test]
    fn test_remove() {
        let td = TempDir::new().unwrap();
        let cache = ObjectCache::new(td.path());

        let content = b"To be removed";
        let pointer = Pointer::from_content(content);

        cache.put(pointer.oid(), content).unwrap();
        assert!(cache.contains(pointer.oid()));

        cache.remove(pointer.oid()).unwrap();
        assert!(!cache.contains(pointer.oid()));
    }

    #[test]
    fn test_count_and_size() {
        let td = TempDir::new().unwrap();
        let cache = ObjectCache::new(td.path());

        assert_eq!(cache.count(), 0);
        assert_eq!(cache.size(), 0);

        let content1 = b"First object";
        let content2 = b"Second object, longer";

        let p1 = Pointer::from_content(content1);
        let p2 = Pointer::from_content(content2);

        cache.put(p1.oid(), content1).unwrap();
        cache.put(p2.oid(), content2).unwrap();

        assert_eq!(cache.count(), 2);
        assert_eq!(cache.size(), (content1.len() + content2.len()) as u64);
    }
}

//! LFS pointer file format.
//!
//! LFS pointer files are small text files that replace large files in the Git repository.
//! They contain metadata about the actual file stored in LFS.

use std::io::Read;

use crate::{Error, Oid, Result};

/// LFS specification version.
pub const LFS_SPEC_V1: &str = "https://git-lfs.github.com/spec/v1";

/// Maximum size of an LFS pointer file (1KB).
pub const MAX_POINTER_SIZE: usize = 1024;

/// An LFS pointer representing a file stored in LFS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pointer {
    /// The SHA256 hash of the file content.
    oid: Oid,
    /// The size of the file in bytes.
    size: u64,
}

impl Pointer {
    /// Create a new pointer with the given OID and size.
    pub fn new(oid: Oid, size: u64) -> Self {
        Pointer { oid, size }
    }

    /// Create a pointer from file content.
    ///
    /// This computes the SHA256 hash of the content.
    pub fn from_content(content: &[u8]) -> Self {
        Pointer {
            oid: Oid::from_content(content),
            size: content.len() as u64,
        }
    }

    /// Create a pointer by streaming content from a reader.
    ///
    /// This computes the SHA256 hash while reading, avoiding loading
    /// the entire content into memory at once.
    pub fn from_reader<R: Read>(reader: R) -> std::io::Result<Self> {
        let (oid, size) = Oid::from_reader(reader)?;
        Ok(Pointer { oid, size })
    }

    /// Parse a pointer from its text representation.
    pub fn parse(content: &[u8]) -> Result<Self> {
        // Check size first
        if content.len() > MAX_POINTER_SIZE {
            return Err(Error::InvalidPointer(
                "content too large to be a pointer".into(),
            ));
        }

        let text = std::str::from_utf8(content)
            .map_err(|_| Error::InvalidPointer("invalid UTF-8".into()))?;

        let mut version_found = false;
        let mut oid: Option<Oid> = None;
        let mut size: Option<u64> = None;

        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            if line.starts_with("version ") {
                let ver = line.strip_prefix("version ").unwrap().trim();
                if ver == LFS_SPEC_V1 || ver == "https://hawser.github.com/spec/v1" {
                    version_found = true;
                } else {
                    return Err(Error::InvalidPointer(format!(
                        "unsupported version: {}",
                        ver
                    )));
                }
            } else if let Some(rest) = line.strip_prefix("oid sha256:") {
                oid = Some(Oid::from_hex(rest.trim())?);
            } else if let Some(rest) = line.strip_prefix("size ") {
                size = Some(
                    rest.trim()
                        .parse()
                        .map_err(|_| Error::InvalidPointer("invalid size".into()))?,
                );
            }
        }

        if !version_found {
            return Err(Error::InvalidPointer("missing version".into()));
        }

        match (oid, size) {
            (Some(oid), Some(size)) => Ok(Pointer { oid, size }),
            (None, _) => Err(Error::InvalidPointer("missing oid".into())),
            (_, None) => Err(Error::InvalidPointer("missing size".into())),
        }
    }

    /// Check if content looks like an LFS pointer.
    pub fn is_pointer(content: &[u8]) -> bool {
        if content.len() > MAX_POINTER_SIZE {
            return false;
        }
        content.starts_with(b"version https://git-lfs.github.com/spec/v1")
            || content.starts_with(b"version https://hawser.github.com/spec/v1")
    }

    /// Get the OID of this pointer.
    pub fn oid(&self) -> &Oid {
        &self.oid
    }

    /// Get the size of the file.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// Encode the pointer to its text representation.
    pub fn encode(&self) -> String {
        format!(
            "version {}\noid sha256:{}\nsize {}\n",
            LFS_SPEC_V1,
            self.oid.to_hex(),
            self.size
        )
    }

    /// Encode the pointer to bytes.
    pub fn encode_bytes(&self) -> Vec<u8> {
        self.encode().into_bytes()
    }
}

impl std::fmt::Display for Pointer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.encode())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pointer_from_content() {
        let content = b"Hello, World!";
        let pointer = Pointer::from_content(content);
        assert_eq!(pointer.size(), 13);
        assert_eq!(
            pointer.oid().to_hex(),
            "dffd6021bb2bd5b0af676290809ec3a53191dd81c7f70a4b28688a362182986f"
        );
    }

    #[test]
    fn test_pointer_encode() {
        let content = b"test";
        let pointer = Pointer::from_content(content);
        let encoded = pointer.encode();

        assert!(encoded.contains("version https://git-lfs.github.com/spec/v1"));
        assert!(encoded.contains("oid sha256:"));
        assert!(encoded.contains("size 4"));
    }

    #[test]
    fn test_pointer_roundtrip() {
        let content = b"test content for LFS";
        let pointer1 = Pointer::from_content(content);
        let encoded = pointer1.encode_bytes();
        let pointer2 = Pointer::parse(&encoded).unwrap();

        assert_eq!(pointer1, pointer2);
    }

    #[test]
    fn test_pointer_parse_valid() {
        let pointer_text = b"version https://git-lfs.github.com/spec/v1\n\
            oid sha256:4d7a214614ab2935c943f9e0ff69d22eadbb8f32b1258daaa5e2ca24d17e2393\n\
            size 12345\n";

        let pointer = Pointer::parse(pointer_text).unwrap();
        assert_eq!(pointer.size(), 12345);
        assert_eq!(
            pointer.oid().to_hex(),
            "4d7a214614ab2935c943f9e0ff69d22eadbb8f32b1258daaa5e2ca24d17e2393"
        );
    }

    #[test]
    fn test_pointer_parse_invalid() {
        // Not a pointer
        assert!(Pointer::parse(b"Hello, World!").is_err());

        // Missing oid
        assert!(Pointer::parse(b"version https://git-lfs.github.com/spec/v1\nsize 123\n").is_err());

        // Missing size
        assert!(
            Pointer::parse(b"version https://git-lfs.github.com/spec/v1\noid sha256:abc\n")
                .is_err()
        );
    }

    #[test]
    fn test_is_pointer() {
        let pointer = b"version https://git-lfs.github.com/spec/v1\noid sha256:abc\nsize 123\n";
        assert!(Pointer::is_pointer(pointer));

        let not_pointer = b"Hello, this is regular content";
        assert!(!Pointer::is_pointer(not_pointer));

        // Too large
        let large = vec![b'x'; 2000];
        assert!(!Pointer::is_pointer(&large));
    }
}

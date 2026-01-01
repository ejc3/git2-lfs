//! LFS Object ID (OID) - SHA256 content hash.

use sha2::{Digest, Sha256};
use std::fmt;
use std::io::{self, Read, Write};

use crate::{Error, Result};

/// LFS Object ID - a SHA256 hash of the file content.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Oid {
    bytes: [u8; 32],
}

impl Oid {
    /// Create an OID from raw bytes.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Oid { bytes }
    }

    /// Parse an OID from a hex string.
    pub fn from_hex(hex: &str) -> Result<Self> {
        let hex = hex.trim();
        if hex.len() != 64 {
            return Err(Error::InvalidOid(format!(
                "expected 64 hex chars, got {}",
                hex.len()
            )));
        }

        let bytes = hex::decode(hex).map_err(|e| Error::InvalidOid(e.to_string()))?;

        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Ok(Oid { bytes: arr })
    }

    /// Compute the OID (SHA256 hash) of content.
    pub fn from_content(content: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(content);
        let result = hasher.finalize();

        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&result);
        Oid { bytes }
    }

    /// Compute the OID (SHA256 hash) by reading from a stream.
    ///
    /// Returns (oid, size) tuple. Reads the entire stream.
    pub fn from_reader<R: Read>(mut reader: R) -> io::Result<(Self, u64)> {
        let mut hasher = Sha256::new();
        let mut buf = [0u8; 8192];
        let mut size = 0u64;

        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            size += n as u64;
        }

        let result = hasher.finalize();
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&result);

        Ok((Oid { bytes }, size))
    }

    /// Get the OID as a hex string.
    pub fn to_hex(&self) -> String {
        hex::encode(self.bytes)
    }

    /// Get the raw bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.bytes
    }
}

impl fmt::Display for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_hex())
    }
}

impl fmt::Debug for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Oid({})", self.to_hex())
    }
}

impl std::str::FromStr for Oid {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        Oid::from_hex(s)
    }
}

/// A writer that hashes data as it's written.
///
/// Wraps an inner writer and computes the SHA256 hash of all data written.
/// Call `finish()` to get the final OID and byte count.
pub struct HashingWriter<W: Write> {
    inner: W,
    hasher: Sha256,
    size: u64,
}

impl<W: Write> HashingWriter<W> {
    /// Create a new hashing writer wrapping the given writer.
    pub fn new(inner: W) -> Self {
        HashingWriter {
            inner,
            hasher: Sha256::new(),
            size: 0,
        }
    }

    /// Finish writing and return (oid, size, inner_writer).
    pub fn finish(self) -> (Oid, u64, W) {
        let result = self.hasher.finalize();
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&result);
        (Oid { bytes }, self.size, self.inner)
    }

    /// Get the current byte count.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// Get a reference to the inner writer.
    pub fn inner(&self) -> &W {
        &self.inner
    }
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        self.size += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_oid_from_content() {
        let content = b"Hello, World!";
        let oid = Oid::from_content(content);
        // SHA256 of "Hello, World!"
        assert_eq!(
            oid.to_hex(),
            "dffd6021bb2bd5b0af676290809ec3a53191dd81c7f70a4b28688a362182986f"
        );
    }

    #[test]
    fn test_oid_from_hex() {
        let hex = "dffd6021bb2bd5b0af676290809ec3a53191dd81c7f70a4b28688a362182986f";
        let oid = Oid::from_hex(hex).unwrap();
        assert_eq!(oid.to_hex(), hex);
    }

    #[test]
    fn test_oid_invalid_hex() {
        assert!(Oid::from_hex("not valid hex").is_err());
        assert!(Oid::from_hex("abc").is_err()); // Too short
    }

    #[test]
    fn test_oid_roundtrip() {
        let content = b"test content";
        let oid1 = Oid::from_content(content);
        let oid2 = Oid::from_hex(&oid1.to_hex()).unwrap();
        assert_eq!(oid1, oid2);
    }

    #[test]
    fn test_oid_from_reader() {
        let content = b"Hello, World!";
        let cursor = Cursor::new(content);
        let (oid, size) = Oid::from_reader(cursor).unwrap();

        assert_eq!(size, content.len() as u64);
        assert_eq!(
            oid.to_hex(),
            "dffd6021bb2bd5b0af676290809ec3a53191dd81c7f70a4b28688a362182986f"
        );
        // Should match from_content
        assert_eq!(oid, Oid::from_content(content));
    }

    #[test]
    fn test_hashing_writer() {
        let content = b"Hello, World!";
        let mut writer = HashingWriter::new(Vec::new());
        writer.write_all(content).unwrap();

        let (oid, size, output) = writer.finish();

        assert_eq!(size, content.len() as u64);
        assert_eq!(output, content);
        assert_eq!(
            oid.to_hex(),
            "dffd6021bb2bd5b0af676290809ec3a53191dd81c7f70a4b28688a362182986f"
        );
        // Should match from_content
        assert_eq!(oid, Oid::from_content(content));
    }
}

//! LFS Object ID (OID) - SHA256 content hash.

use sha2::{Digest, Sha256};
use std::fmt;

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

#[cfg(test)]
mod tests {
    use super::*;

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
}

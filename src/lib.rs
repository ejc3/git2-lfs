//! # git2-lfs
//!
//! Pure Rust implementation of Git LFS (Large File Storage) protocol.
//!
//! This crate provides:
//! - LFS pointer file parsing and generation
//! - LFS Batch API client for upload/download
//! - Content-addressed storage with SHA256
//! - Optional integration with git2's filter API
//!
//! ## Example
//!
//! ```no_run
//! use git2_lfs::{LfsClient, Pointer};
//!
//! // Generate a pointer for content
//! let content = b"Hello, this is a large file";
//! let pointer = Pointer::from_content(content);
//! println!("OID: {}", pointer.oid());
//! println!("Size: {}", pointer.size());
//!
//! // Create an LFS client
//! let client = LfsClient::new("https://github.com/owner/repo.git").unwrap();
//!
//! // Upload content
//! client.upload(&pointer, content).unwrap();
//!
//! // Download content
//! let downloaded = client.download(&pointer).unwrap();
//! ```

mod error;
mod oid;
mod pointer;
mod batch;
mod client;

#[cfg(feature = "git2-integration")]
mod filter;

pub use error::{Error, Result};
pub use oid::Oid;
pub use pointer::Pointer;
pub use batch::{BatchRequest, BatchResponse, BatchObject, Action, Operation};
pub use client::LfsClient;

#[cfg(feature = "git2-integration")]
pub use filter::LfsFilter;

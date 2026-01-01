//! LFS Batch API types.
//!
//! The Batch API is used to request upload/download URLs for LFS objects.
//! See: https://github.com/git-lfs/git-lfs/blob/main/docs/api/batch.md

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Operation type for batch requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Operation {
    /// Download objects from the server.
    Download,
    /// Upload objects to the server.
    Upload,
}

/// A batch request to the LFS server.
#[derive(Debug, Clone, Serialize)]
pub struct BatchRequest {
    /// The operation to perform.
    pub operation: Operation,
    /// The transfer adapters the client supports.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transfers: Option<Vec<String>>,
    /// Reference information (branch, etc).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#ref: Option<RefInfo>,
    /// The objects to operate on.
    pub objects: Vec<BatchRequestObject>,
}

/// Reference information for a batch request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefInfo {
    /// The reference name (e.g., "refs/heads/main").
    pub name: String,
}

/// An object in a batch request.
#[derive(Debug, Clone, Serialize)]
pub struct BatchRequestObject {
    /// The SHA256 OID of the object.
    pub oid: String,
    /// The size of the object in bytes.
    pub size: u64,
}

/// A batch response from the LFS server.
#[derive(Debug, Clone, Deserialize)]
pub struct BatchResponse {
    /// The transfer adapter to use (usually "basic").
    #[serde(default = "default_transfer")]
    pub transfer: String,
    /// The objects with their actions.
    pub objects: Vec<BatchObject>,
}

fn default_transfer() -> String {
    "basic".to_string()
}

/// An object in a batch response.
#[derive(Debug, Clone, Deserialize)]
pub struct BatchObject {
    /// The SHA256 OID of the object.
    pub oid: String,
    /// The size of the object in bytes.
    pub size: u64,
    /// Whether the object was authenticated.
    #[serde(default)]
    pub authenticated: Option<bool>,
    /// Actions available for this object.
    #[serde(default)]
    pub actions: Option<HashMap<String, Action>>,
    /// Error information if the object failed.
    #[serde(default)]
    pub error: Option<BatchError>,
}

/// An action (upload/download URL) for an object.
#[derive(Debug, Clone, Deserialize)]
pub struct Action {
    /// The URL for the action.
    pub href: String,
    /// HTTP headers to include in the request.
    #[serde(default)]
    pub header: HashMap<String, String>,
    /// Seconds until the action expires.
    #[serde(default)]
    pub expires_in: Option<u64>,
    /// Absolute expiration time (ISO 8601).
    #[serde(default)]
    pub expires_at: Option<String>,
}

/// Error information for a batch object.
#[derive(Debug, Clone, Deserialize)]
pub struct BatchError {
    /// HTTP status code.
    pub code: u16,
    /// Error message.
    pub message: String,
}

impl BatchRequest {
    /// Create a new batch request for downloading objects.
    pub fn download(objects: Vec<BatchRequestObject>) -> Self {
        BatchRequest {
            operation: Operation::Download,
            transfers: Some(vec!["basic".to_string()]),
            r#ref: None,
            objects,
        }
    }

    /// Create a new batch request for uploading objects.
    pub fn upload(objects: Vec<BatchRequestObject>) -> Self {
        BatchRequest {
            operation: Operation::Upload,
            transfers: Some(vec!["basic".to_string()]),
            r#ref: None,
            objects,
        }
    }

    /// Set the reference for this request.
    pub fn with_ref(mut self, name: &str) -> Self {
        self.r#ref = Some(RefInfo {
            name: name.to_string(),
        });
        self
    }
}

impl BatchRequestObject {
    /// Create a new batch request object.
    pub fn new(oid: &str, size: u64) -> Self {
        BatchRequestObject {
            oid: oid.to_string(),
            size,
        }
    }
}

impl BatchObject {
    /// Get the download action if available.
    pub fn download_action(&self) -> Option<&Action> {
        self.actions.as_ref()?.get("download")
    }

    /// Get the upload action if available.
    pub fn upload_action(&self) -> Option<&Action> {
        self.actions.as_ref()?.get("upload")
    }

    /// Get the verify action if available (for uploads).
    pub fn verify_action(&self) -> Option<&Action> {
        self.actions.as_ref()?.get("verify")
    }

    /// Check if this object has an error.
    pub fn has_error(&self) -> bool {
        self.error.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_batch_request_serialize() {
        let request = BatchRequest::upload(vec![
            BatchRequestObject::new("abc123", 1024),
        ]);

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"operation\":\"upload\""));
        assert!(json.contains("\"oid\":\"abc123\""));
        assert!(json.contains("\"size\":1024"));
    }

    #[test]
    fn test_batch_response_deserialize() {
        let json = r#"{
            "transfer": "basic",
            "objects": [
                {
                    "oid": "abc123",
                    "size": 1024,
                    "actions": {
                        "upload": {
                            "href": "https://example.com/upload",
                            "header": {
                                "Authorization": "Bearer token"
                            },
                            "expires_in": 3600
                        }
                    }
                }
            ]
        }"#;

        let response: BatchResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.transfer, "basic");
        assert_eq!(response.objects.len(), 1);
        assert_eq!(response.objects[0].oid, "abc123");

        let upload = response.objects[0].upload_action().unwrap();
        assert_eq!(upload.href, "https://example.com/upload");
        assert_eq!(upload.header.get("Authorization").unwrap(), "Bearer token");
    }

    #[test]
    fn test_batch_response_with_error() {
        let json = r#"{
            "objects": [
                {
                    "oid": "abc123",
                    "size": 1024,
                    "error": {
                        "code": 404,
                        "message": "Object not found"
                    }
                }
            ]
        }"#;

        let response: BatchResponse = serde_json::from_str(json).unwrap();
        assert!(response.objects[0].has_error());
        assert_eq!(response.objects[0].error.as_ref().unwrap().code, 404);
    }
}

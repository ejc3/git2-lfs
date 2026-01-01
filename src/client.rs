//! LFS HTTP client for upload/download operations.

use std::io::Read;
use url::Url;

use crate::batch::{BatchRequest, BatchRequestObject, BatchResponse};
use crate::{Error, Pointer, Result};

/// LFS client for communicating with an LFS server.
pub struct LfsClient {
    /// The LFS API endpoint URL.
    lfs_url: Url,
    /// Optional authentication (username, password).
    auth: Option<(String, String)>,
    /// HTTP agent for making requests.
    agent: ureq::Agent,
}

impl LfsClient {
    /// Create a new LFS client for a repository URL.
    ///
    /// The URL should be the Git remote URL (e.g., `https://github.com/owner/repo.git`).
    /// The LFS endpoint is derived by appending `/info/lfs` to the base URL.
    pub fn new(repo_url: &str) -> Result<Self> {
        let lfs_url = derive_lfs_url(repo_url)?;
        Ok(LfsClient {
            lfs_url,
            auth: None,
            agent: ureq::Agent::new(),
        })
    }

    /// Create a new LFS client with a specific LFS endpoint URL.
    pub fn with_url(lfs_url: Url) -> Self {
        LfsClient {
            lfs_url,
            auth: None,
            agent: ureq::Agent::new(),
        }
    }

    /// Set basic authentication credentials.
    pub fn with_auth(mut self, username: &str, password: &str) -> Self {
        self.auth = Some((username.to_string(), password.to_string()));
        self
    }

    /// Set authentication from a token (uses token as password with empty username).
    pub fn with_token(self, token: &str) -> Self {
        self.with_auth("", token)
    }

    /// Get the LFS endpoint URL.
    pub fn lfs_url(&self) -> &Url {
        &self.lfs_url
    }

    /// Send a batch request to the LFS server.
    pub fn batch(&self, request: &BatchRequest) -> Result<BatchResponse> {
        let url = self.lfs_url.join("objects/batch")?;

        let mut req = self.agent
            .post(url.as_str())
            .set("Accept", "application/vnd.git-lfs+json")
            .set("Content-Type", "application/vnd.git-lfs+json");

        if let Some((username, password)) = &self.auth {
            let credentials = format!("{}:{}", username, password);
            let encoded = base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                credentials.as_bytes(),
            );
            req = req.set("Authorization", &format!("Basic {}", encoded));
        }

        let response = req.send_json(request)?;
        let batch_response: BatchResponse = response.into_json()?;
        Ok(batch_response)
    }

    /// Upload content to the LFS server.
    ///
    /// Returns the pointer for the uploaded content.
    pub fn upload(&self, pointer: &Pointer, content: &[u8]) -> Result<()> {
        // Verify content matches pointer
        let computed = Pointer::from_content(content);
        if computed.oid() != pointer.oid() || computed.size() != pointer.size() {
            return Err(Error::InvalidPointer(
                "content does not match pointer".into(),
            ));
        }

        // Request upload URL
        let batch_req = BatchRequest::upload(vec![BatchRequestObject::new(
            &pointer.oid().to_hex(),
            pointer.size(),
        )]);

        let batch_resp = self.batch(&batch_req)?;

        if batch_resp.objects.is_empty() {
            return Err(Error::ServerError {
                code: 500,
                message: "no objects in batch response".into(),
            });
        }

        let obj = &batch_resp.objects[0];

        // Check for errors
        if let Some(err) = &obj.error {
            return Err(Error::ServerError {
                code: err.code,
                message: err.message.clone(),
            });
        }

        // Get upload action
        let upload_action = obj.upload_action().ok_or_else(|| {
            // No upload action means object already exists
            Error::Http("object already exists on server".into())
        });

        // If no upload action, object already exists - that's OK
        let action = match upload_action {
            Ok(a) => a,
            Err(_) => return Ok(()), // Already exists
        };

        // Upload the content
        let mut req = self.agent.put(&action.href);

        // Add headers from action
        for (key, value) in &action.header {
            req = req.set(key, value);
        }

        req = req.set("Content-Type", "application/octet-stream");
        req = req.set("Content-Length", &content.len().to_string());

        req.send_bytes(content)?;

        // Verify if required
        if let Some(verify_action) = obj.verify_action() {
            let verify_body = serde_json::json!({
                "oid": pointer.oid().to_hex(),
                "size": pointer.size()
            });

            let mut req = self.agent.post(&verify_action.href);
            for (key, value) in &verify_action.header {
                req = req.set(key, value);
            }
            req = req.set("Content-Type", "application/vnd.git-lfs+json");
            req.send_json(&verify_body)?;
        }

        Ok(())
    }

    /// Download content from the LFS server.
    pub fn download(&self, pointer: &Pointer) -> Result<Vec<u8>> {
        // Request download URL
        let batch_req = BatchRequest::download(vec![BatchRequestObject::new(
            &pointer.oid().to_hex(),
            pointer.size(),
        )]);

        let batch_resp = self.batch(&batch_req)?;

        if batch_resp.objects.is_empty() {
            return Err(Error::NotFound(pointer.oid().to_hex()));
        }

        let obj = &batch_resp.objects[0];

        // Check for errors
        if let Some(err) = &obj.error {
            return Err(Error::ServerError {
                code: err.code,
                message: err.message.clone(),
            });
        }

        // Get download action
        let action = obj.download_action().ok_or_else(|| {
            Error::NotFound(pointer.oid().to_hex())
        })?;

        // Download the content
        let mut req = self.agent.get(&action.href);

        // Add headers from action
        for (key, value) in &action.header {
            req = req.set(key, value);
        }

        let response = req.call()?;

        let mut content = Vec::with_capacity(pointer.size() as usize);
        response.into_reader().read_to_end(&mut content)?;

        // Verify content
        let computed = Pointer::from_content(&content);
        if computed.oid() != pointer.oid() {
            return Err(Error::InvalidPointer(
                "downloaded content hash mismatch".into(),
            ));
        }

        Ok(content)
    }

    /// Check if objects exist on the server.
    ///
    /// Returns a list of OIDs that exist.
    pub fn check_exists(&self, pointers: &[&Pointer]) -> Result<Vec<String>> {
        if pointers.is_empty() {
            return Ok(vec![]);
        }

        let objects: Vec<_> = pointers
            .iter()
            .map(|p| BatchRequestObject::new(&p.oid().to_hex(), p.size()))
            .collect();

        let batch_req = BatchRequest::download(objects);
        let batch_resp = self.batch(&batch_req)?;

        let existing: Vec<_> = batch_resp
            .objects
            .into_iter()
            .filter(|obj| obj.download_action().is_some())
            .map(|obj| obj.oid)
            .collect();

        Ok(existing)
    }
}

/// Derive the LFS endpoint URL from a Git remote URL.
fn derive_lfs_url(repo_url: &str) -> Result<Url> {
    let repo_url = repo_url.trim();

    // Handle SSH URLs (git@github.com:owner/repo.git)
    if repo_url.starts_with("git@") {
        let rest = repo_url.strip_prefix("git@").unwrap();
        if let Some((host, path)) = rest.split_once(':') {
            let path = path.strip_suffix(".git").unwrap_or(path);
            let url_str = format!("https://{}/{}/info/lfs", host, path);
            return Url::parse(&url_str).map_err(|e| Error::InvalidUrl(e.to_string()));
        }
    }

    // Handle HTTPS URLs
    let mut url = Url::parse(repo_url).map_err(|e| Error::InvalidUrl(e.to_string()))?;

    // Remove .git suffix if present
    let path = url.path().strip_suffix(".git").unwrap_or(url.path());
    let new_path = format!("{}/info/lfs", path);
    url.set_path(&new_path);

    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derive_lfs_url_https() {
        let url = derive_lfs_url("https://github.com/owner/repo.git").unwrap();
        assert_eq!(url.as_str(), "https://github.com/owner/repo/info/lfs");
    }

    #[test]
    fn test_derive_lfs_url_https_no_git() {
        let url = derive_lfs_url("https://github.com/owner/repo").unwrap();
        assert_eq!(url.as_str(), "https://github.com/owner/repo/info/lfs");
    }

    #[test]
    fn test_derive_lfs_url_ssh() {
        let url = derive_lfs_url("git@github.com:owner/repo.git").unwrap();
        assert_eq!(url.as_str(), "https://github.com/owner/repo/info/lfs");
    }

    #[test]
    fn test_client_new() {
        let client = LfsClient::new("https://github.com/owner/repo.git").unwrap();
        assert_eq!(
            client.lfs_url().as_str(),
            "https://github.com/owner/repo/info/lfs"
        );
    }

    #[test]
    fn test_client_with_auth() {
        let client = LfsClient::new("https://github.com/owner/repo.git")
            .unwrap()
            .with_auth("user", "pass");
        assert!(client.auth.is_some());
    }
}

//! LFS HTTP client for upload/download operations.

use std::io::Read;
use std::sync::Arc;
use url::Url;

use crate::batch::{BatchRequest, BatchRequestObject, BatchResponse};
use crate::{Error, Pointer, Result};

/// LFS client for communicating with an LFS server.
///
/// This type is cheaply cloneable - multiple clones share the same underlying
/// HTTP agent and configuration.
#[derive(Clone)]
pub struct LfsClient {
    inner: Arc<LfsClientInner>,
}

/// Authentication method for LFS requests.
#[derive(Clone)]
enum Auth {
    /// Bearer token (OAuth/PAT)
    Bearer(String),
    /// Basic auth (username, password)
    Basic(String, String),
}

struct LfsClientInner {
    /// The LFS API endpoint URL.
    lfs_url: Url,
    /// Optional authentication.
    auth: Option<Auth>,
    /// HTTP agent for making requests.
    agent: ureq::Agent,
    /// Optional ref name for batch requests (e.g., "refs/heads/main").
    ref_name: Option<String>,
}

impl LfsClient {
    /// Create a new LFS client for a repository URL.
    ///
    /// The URL should be the Git remote URL (e.g., `https://github.com/owner/repo.git`).
    /// The LFS endpoint is derived by appending `/info/lfs` to the base URL.
    pub fn new(repo_url: &str) -> Result<Self> {
        let lfs_url = derive_lfs_url(repo_url)?;
        Ok(LfsClient {
            inner: Arc::new(LfsClientInner {
                lfs_url,
                auth: None,
                agent: ureq::Agent::new(),
                ref_name: None,
            }),
        })
    }

    /// Create a new LFS client with a specific LFS endpoint URL.
    pub fn with_url(lfs_url: Url) -> Self {
        LfsClient {
            inner: Arc::new(LfsClientInner {
                lfs_url,
                auth: None,
                agent: ureq::Agent::new(),
                ref_name: None,
            }),
        }
    }

    /// Set basic authentication credentials.
    pub fn with_auth(self, username: &str, password: &str) -> Self {
        LfsClient {
            inner: Arc::new(LfsClientInner {
                lfs_url: self.inner.lfs_url.clone(),
                auth: Some(Auth::Basic(username.to_string(), password.to_string())),
                agent: ureq::Agent::new(),
                ref_name: self.inner.ref_name.clone(),
            }),
        }
    }

    /// Set authentication from a bearer token (OAuth/PAT).
    pub fn with_token(self, token: &str) -> Self {
        LfsClient {
            inner: Arc::new(LfsClientInner {
                lfs_url: self.inner.lfs_url.clone(),
                auth: Some(Auth::Bearer(token.to_string())),
                agent: ureq::Agent::new(),
                ref_name: self.inner.ref_name.clone(),
            }),
        }
    }

    /// Set the ref name for batch requests.
    ///
    /// The ref name is sent with batch requests to help servers with
    /// access control and locking decisions (e.g., "refs/heads/main").
    pub fn with_ref(self, ref_name: &str) -> Self {
        LfsClient {
            inner: Arc::new(LfsClientInner {
                lfs_url: self.inner.lfs_url.clone(),
                auth: self.inner.auth.clone(),
                agent: ureq::Agent::new(),
                ref_name: Some(ref_name.to_string()),
            }),
        }
    }

    /// Get the LFS endpoint URL.
    pub fn lfs_url(&self) -> &Url {
        &self.inner.lfs_url
    }

    /// Send a batch request to the LFS server.
    pub fn batch(&self, request: &BatchRequest) -> Result<BatchResponse> {
        let url = self.inner.lfs_url.join("objects/batch")?;

        let mut req = self
            .inner
            .agent
            .post(url.as_str())
            .set("Accept", "application/vnd.git-lfs+json")
            .set("Content-Type", "application/vnd.git-lfs+json")
            .set("User-Agent", "git2-lfs/0.1");

        if let Some(auth) = &self.inner.auth {
            req = match auth {
                Auth::Bearer(token) => req.set("Authorization", &format!("Bearer {}", token)),
                Auth::Basic(username, password) => {
                    let credentials = format!("{}:{}", username, password);
                    let encoded = base64::Engine::encode(
                        &base64::engine::general_purpose::STANDARD,
                        credentials.as_bytes(),
                    );
                    req.set("Authorization", &format!("Basic {}", encoded))
                }
            };
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
        let mut batch_req = BatchRequest::upload(vec![BatchRequestObject::new(
            &pointer.oid().to_hex(),
            pointer.size(),
        )]);
        if let Some(ref_name) = &self.inner.ref_name {
            batch_req = batch_req.with_ref(ref_name);
        }

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
        let mut req = self.inner.agent.put(&action.href);

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

            let mut req = self.inner.agent.post(&verify_action.href);
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
        let mut batch_req = BatchRequest::download(vec![BatchRequestObject::new(
            &pointer.oid().to_hex(),
            pointer.size(),
        )]);
        if let Some(ref_name) = &self.inner.ref_name {
            batch_req = batch_req.with_ref(ref_name);
        }

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
        let action = obj
            .download_action()
            .ok_or_else(|| Error::NotFound(pointer.oid().to_hex()))?;

        // Download the content
        let mut req = self.inner.agent.get(&action.href);

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

        let mut batch_req = BatchRequest::download(objects);
        if let Some(ref_name) = &self.inner.ref_name {
            batch_req = batch_req.with_ref(ref_name);
        }
        let batch_resp = self.batch(&batch_req)?;

        let existing: Vec<_> = batch_resp
            .objects
            .into_iter()
            .filter(|obj| obj.download_action().is_some())
            .map(|obj| obj.oid)
            .collect();

        Ok(existing)
    }

    /// Upload multiple objects in a single batch request.
    ///
    /// More efficient than calling `upload()` multiple times as it uses
    /// a single batch request to get all upload URLs.
    pub fn upload_batch(&self, items: &[(&Pointer, &[u8])]) -> Result<()> {
        if items.is_empty() {
            return Ok(());
        }

        // Verify all content matches pointers
        for (pointer, content) in items {
            let computed = Pointer::from_content(content);
            if computed.oid() != pointer.oid() || computed.size() != pointer.size() {
                return Err(Error::InvalidPointer(format!(
                    "content does not match pointer for oid {}",
                    pointer.oid().to_hex()
                )));
            }
        }

        // Request upload URLs for all objects
        let objects: Vec<_> = items
            .iter()
            .map(|(p, _)| BatchRequestObject::new(&p.oid().to_hex(), p.size()))
            .collect();

        let mut batch_req = BatchRequest::upload(objects);
        if let Some(ref_name) = &self.inner.ref_name {
            batch_req = batch_req.with_ref(ref_name);
        }
        let batch_resp = self.batch(&batch_req)?;

        // Create a map of oid -> content for lookup
        let content_map: std::collections::HashMap<_, _> = items
            .iter()
            .map(|(p, c)| (p.oid().to_hex(), *c))
            .collect();

        // Upload each object that has an upload action
        for obj in &batch_resp.objects {
            // Check for errors
            if let Some(err) = &obj.error {
                return Err(Error::ServerError {
                    code: err.code,
                    message: err.message.clone(),
                });
            }

            // Get upload action (no action means already exists)
            let action = match obj.upload_action() {
                Some(a) => a,
                None => continue, // Already exists on server
            };

            // Get content for this object
            let content = content_map.get(&obj.oid).ok_or_else(|| {
                Error::InvalidPointer(format!("no content for oid {}", obj.oid))
            })?;

            // Upload the content
            let mut req = self.inner.agent.put(&action.href);
            for (key, value) in &action.header {
                req = req.set(key, value);
            }
            req = req.set("Content-Type", "application/octet-stream");
            req = req.set("Content-Length", &content.len().to_string());
            req.send_bytes(content)?;

            // Verify if required
            if let Some(verify_action) = obj.verify_action() {
                let verify_body = serde_json::json!({
                    "oid": obj.oid,
                    "size": obj.size
                });

                let mut req = self.inner.agent.post(&verify_action.href);
                for (key, value) in &verify_action.header {
                    req = req.set(key, value);
                }
                req = req.set("Content-Type", "application/vnd.git-lfs+json");
                req.send_json(&verify_body)?;
            }
        }

        Ok(())
    }

    /// Download multiple objects in a single batch request.
    ///
    /// More efficient than calling `download()` multiple times as it uses
    /// a single batch request to get all download URLs.
    ///
    /// Returns a vector of (pointer, content) pairs in the same order as input.
    pub fn download_batch(&self, pointers: &[&Pointer]) -> Result<Vec<Vec<u8>>> {
        if pointers.is_empty() {
            return Ok(vec![]);
        }

        // Request download URLs for all objects
        let objects: Vec<_> = pointers
            .iter()
            .map(|p| BatchRequestObject::new(&p.oid().to_hex(), p.size()))
            .collect();

        let mut batch_req = BatchRequest::download(objects);
        if let Some(ref_name) = &self.inner.ref_name {
            batch_req = batch_req.with_ref(ref_name);
        }
        let batch_resp = self.batch(&batch_req)?;

        // Create a map of oid -> batch object for lookup
        let obj_map: std::collections::HashMap<_, _> = batch_resp
            .objects
            .into_iter()
            .map(|o| (o.oid.clone(), o))
            .collect();

        // Download each object in order
        let mut results = Vec::with_capacity(pointers.len());
        for pointer in pointers {
            let oid = pointer.oid().to_hex();
            let obj = obj_map
                .get(&oid)
                .ok_or_else(|| Error::NotFound(oid.clone()))?;

            // Check for errors
            if let Some(err) = &obj.error {
                return Err(Error::ServerError {
                    code: err.code,
                    message: err.message.clone(),
                });
            }

            // Get download action
            let action = obj
                .download_action()
                .ok_or_else(|| Error::NotFound(oid.clone()))?;

            // Download the content
            let mut req = self.inner.agent.get(&action.href);
            for (key, value) in &action.header {
                req = req.set(key, value);
            }
            let response = req.call()?;

            let mut content = Vec::with_capacity(pointer.size() as usize);
            response.into_reader().read_to_end(&mut content)?;

            // Verify content
            let computed = Pointer::from_content(&content);
            if computed.oid() != pointer.oid() {
                return Err(Error::InvalidPointer(format!(
                    "downloaded content hash mismatch for oid {}",
                    oid
                )));
            }

            results.push(content);
        }

        Ok(results)
    }
}

/// Derive the LFS endpoint URL from a Git remote URL.
fn derive_lfs_url(repo_url: &str) -> Result<Url> {
    let repo_url = repo_url.trim();

    // Handle SSH URLs (git@github.com:owner/repo.git)
    if repo_url.starts_with("git@") {
        let rest = repo_url.strip_prefix("git@").unwrap();
        if let Some((host, path)) = rest.split_once(':') {
            // Keep .git if present, add it if not - GitHub requires it
            let path = if path.ends_with(".git") {
                path.to_string()
            } else {
                format!("{}.git", path)
            };
            // Trailing slash needed for correct URL joining
            let url_str = format!("https://{}/{}/info/lfs/", host, path);
            return Url::parse(&url_str).map_err(|e| Error::InvalidUrl(e.to_string()));
        }
    }

    // Handle HTTPS URLs
    let mut url = Url::parse(repo_url).map_err(|e| Error::InvalidUrl(e.to_string()))?;

    // Keep .git if present, add it if not - GitHub requires it in the LFS path
    let path = url.path();
    let path = if path.ends_with(".git") {
        path.to_string()
    } else {
        format!("{}.git", path)
    };
    let new_path = format!("{}/info/lfs/", path);
    url.set_path(&new_path);

    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derive_lfs_url_https() {
        let url = derive_lfs_url("https://github.com/owner/repo.git").unwrap();
        assert_eq!(url.as_str(), "https://github.com/owner/repo.git/info/lfs/");
    }

    #[test]
    fn test_derive_lfs_url_https_no_git() {
        // URLs without .git get it added - GitHub requires it
        let url = derive_lfs_url("https://github.com/owner/repo").unwrap();
        assert_eq!(url.as_str(), "https://github.com/owner/repo.git/info/lfs/");
    }

    #[test]
    fn test_derive_lfs_url_ssh() {
        let url = derive_lfs_url("git@github.com:owner/repo.git").unwrap();
        assert_eq!(url.as_str(), "https://github.com/owner/repo.git/info/lfs/");
    }

    #[test]
    fn test_client_new() {
        let client = LfsClient::new("https://github.com/owner/repo.git").unwrap();
        assert_eq!(
            client.lfs_url().as_str(),
            "https://github.com/owner/repo.git/info/lfs/"
        );
    }

    #[test]
    fn test_client_with_auth() {
        let client = LfsClient::new("https://github.com/owner/repo.git")
            .unwrap()
            .with_auth("user", "pass");
        assert!(client.inner.auth.is_some());
    }

    #[test]
    fn test_client_clone() {
        let client1 = LfsClient::new("https://github.com/owner/repo.git").unwrap();
        let client2 = client1.clone();

        // Both should point to the same URL
        assert_eq!(client1.lfs_url(), client2.lfs_url());

        // Arc should be shared
        assert!(Arc::ptr_eq(&client1.inner, &client2.inner));
    }
}

use crate::error::{EgressError, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HttpRequestSnapshot {
    pub method: String,
    pub url: String,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HttpResponseSnapshot {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CassetteEntry {
    pub request: HttpRequestSnapshot,
    pub response: HttpResponseSnapshot,
    pub body_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cassette {
    pub name: String,
    pub entries: Vec<CassetteEntry>,
}

pub struct VcrEngine {
    cassette_dir: PathBuf,
}

impl VcrEngine {
    pub fn new(cassette_dir: impl Into<PathBuf>) -> Self {
        Self {
            cassette_dir: cassette_dir.into(),
        }
    }

    fn cassette_path(&self, name: &str) -> PathBuf {
        self.cassette_dir.join(format!("{}.json", name))
    }

    pub fn sanitize_headers(headers: &HashMap<String, String>) -> HashMap<String, String> {
        let sensitive_keys = [
            "authorization",
            "x-api-key",
            "api-key",
            "cookie",
            "set-cookie",
            "token",
            "secret",
        ];
        headers
            .iter()
            .map(|(k, v)| {
                let lower = k.to_lowercase();
                if sensitive_keys.iter().any(|s| lower.contains(s)) {
                    (k.clone(), "***REDACTED***".to_string())
                } else {
                    (k.clone(), v.clone())
                }
            })
            .collect()
    }

    pub fn hash_body(body: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(body);
        format!("{:x}", hasher.finalize())
    }

    pub fn load_cassette(&self, name: &str) -> Result<Option<Cassette>> {
        let path = self.cassette_path(name);
        if !path.exists() {
            return Ok(None);
        }
        let data = fs::read(&path).map_err(|e| EgressError::io(&path, e))?;
        let cassette: Cassette = serde_json::from_slice(&data).map_err(|e| EgressError::CassetteCorrupt {
            path,
            msg: e.to_string(),
        })?;
        Ok(Some(cassette))
    }

    pub fn record(
        &self,
        name: &str,
        request: &HttpRequestSnapshot,
        response: &HttpResponseSnapshot,
    ) -> Result<()> {
        fs::create_dir_all(&self.cassette_dir).map_err(|e| EgressError::io(&self.cassette_dir, e))?;

        let mut cassette = self.load_cassette(name)?.unwrap_or_else(|| Cassette {
            name: name.to_string(),
            entries: Vec::new(),
        });

        let sanitized_req = HttpRequestSnapshot {
            method: request.method.to_uppercase(),
            url: request.url.clone(),
            headers: Self::sanitize_headers(&request.headers),
            body: request.body.clone(),
        };

        let sanitized_resp = HttpResponseSnapshot {
            status: response.status,
            headers: Self::sanitize_headers(&response.headers),
            body: response.body.clone(),
        };

        let entry = CassetteEntry {
            request: sanitized_req,
            response: sanitized_resp,
            body_hash: Self::hash_body(&request.body),
        };

        cassette.entries.push(entry);

        let path = self.cassette_path(name);
        let json = serde_json::to_vec_pretty(&cassette)?;
        fs::write(&path, json).map_err(|e| EgressError::io(&path, e))?;

        Ok(())
    }

    pub fn replay(
        &self,
        name: &str,
        request: &HttpRequestSnapshot,
    ) -> Result<Option<HttpResponseSnapshot>> {
        let Some(cassette) = self.load_cassette(name)? else {
            return Ok(None);
        };

        let target_method = request.method.to_uppercase();
        let target_hash = Self::hash_body(&request.body);

        for entry in &cassette.entries {
            if entry.request.method == target_method
                && entry.request.url == request.url
                && entry.body_hash == target_hash
            {
                return Ok(Some(entry.response.clone()));
            }
        }

        Ok(None)
    }
}

//! `.sp-state.yaml` — declares which external stores are captured with every checkpoint.

use crate::error::{Result, StateError};
use serde::Deserialize;
use std::path::{Path, PathBuf};

pub const CONFIG_FILES: [&str; 2] = [".sp-state.yaml", ".sp-state.yml"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    Sqlite,
    Postgres,
    Mysql,
    Redis,
    Memcached,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderSpec {
    pub name: String,
    pub kind: ProviderKind,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub path: Option<PathBuf>,
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default = "default_true")]
    pub terminate_connections: bool,
    #[serde(default = "default_true")]
    pub consistent: bool,
    #[serde(default = "default_lock_timeout")]
    pub lock_timeout_secs: u64,
    #[serde(default)]
    pub maintenance_db: Option<String>,
}

fn default_true() -> bool {
    true
}
fn default_lock_timeout() -> u64 {
    30
}

#[derive(Debug, Clone)]
pub struct StateConfig {
    pub providers: Vec<ProviderSpec>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default)]
    providers: Vec<ProviderSpec>,
}

impl StateConfig {
    /// Loads `.sp-state.yaml` (or `.yml`) from the workspace root, interpolating
    /// `${VAR}` from the process environment. `Ok(None)` when absent or empty.
    pub fn load(workspace: &Path) -> Result<Option<StateConfig>> {
        for name in CONFIG_FILES {
            let path = workspace.join(name);
            if path.is_file() {
                let text = std::fs::read_to_string(&path).map_err(|e| StateError::io(&path, e))?;
                let cfg = Self::parse(&text, &path, &|k| std::env::var(k).ok())?;
                return Ok((!cfg.providers.is_empty()).then_some(cfg));
            }
        }
        Ok(None)
    }

    pub fn parse(
        text: &str,
        origin: &Path,
        env: &dyn Fn(&str) -> Option<String>,
    ) -> Result<StateConfig> {
        let raw: RawConfig =
            serde_yaml::from_str(text).map_err(|e| config_err(origin, e.to_string()))?;
        let mut providers = raw.providers;
        let mut seen = std::collections::HashSet::new();
        for p in &mut providers {
            if !valid_name(&p.name) {
                return Err(config_err(
                    origin,
                    format!(
                        "provider name {:?} is invalid: use 1-64 chars of [A-Za-z0-9_-], starting alphanumeric",
                        p.name
                    ),
                ));
            }
            if !seen.insert(p.name.clone()) {
                return Err(config_err(origin, format!("duplicate provider name {:?}", p.name)));
            }
            let ctx = |m: String| config_err(origin, format!("provider {:?}: {m}", p.name));
            if let Some(u) = &p.url {
                p.url = Some(interpolate(u, env).map_err(ctx)?);
            }
            if let Some(path) = &p.path {
                let s = path.to_string_lossy().into_owned();
                p.path = Some(PathBuf::from(interpolate(&s, env).map_err(ctx)?));
            }
            validate(p).map_err(ctx)?;
        }
        Ok(StateConfig { providers })
    }
}

fn valid_name(name: &str) -> bool {
    let b = name.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && b[0].is_ascii_alphanumeric()
        && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'-')
}

fn validate(p: &ProviderSpec) -> std::result::Result<(), String> {
    let schemes: &[&str] = match p.kind {
        ProviderKind::Sqlite => &[],
        ProviderKind::Postgres => &["postgres", "postgresql"],
        ProviderKind::Mysql => &["mysql", "mariadb"],
        ProviderKind::Redis => &["redis", "rediss", "valkey", "valkeys"],
        ProviderKind::Memcached => &["memcached"],
    };
    match p.kind {
        ProviderKind::Sqlite => {
            if p.path.is_none() {
                return Err("kind sqlite requires `path`".into());
            }
            if p.url.is_some() {
                return Err("kind sqlite takes `path`, not `url`".into());
            }
        }
        _ => {
            let url = p
                .url
                .as_deref()
                .ok_or_else(|| format!("kind {:?} requires `url`", p.kind))?;
            if p.path.is_some() {
                return Err(format!("kind {:?} takes `url`, not `path`", p.kind));
            }
            let scheme = url.split("://").next().unwrap_or("").to_ascii_lowercase();
            if !url.contains("://") || !schemes.contains(&scheme.as_str()) {
                return Err(format!(
                    "url scheme {scheme:?} does not match kind {:?} (expected one of {schemes:?})",
                    p.kind
                ));
            }
        }
    }
    let keyed = matches!(p.kind, ProviderKind::Redis | ProviderKind::Memcached);
    if !keyed && !p.include.is_empty() {
        return Err("`include` is only supported for redis and memcached".into());
    }
    if !keyed && !p.exclude.is_empty() {
        return Err("`exclude` is only supported for redis and memcached".into());
    }
    if p.maintenance_db.is_some() && p.kind != ProviderKind::Postgres {
        return Err("`maintenance_db` is only supported for postgres".into());
    }
    if p.lock_timeout_secs == 0 {
        return Err("`lock_timeout_secs` must be >= 1".into());
    }
    Ok(())
}

/// `${VAR}`, `${VAR:-default}` (default also used when VAR is empty), `$$` -> `$`.
fn interpolate(s: &str, env: &dyn Fn(&str) -> Option<String>) -> std::result::Result<String, String> {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('$') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        if let Some(stripped) = after.strip_prefix('$') {
            out.push('$');
            rest = stripped;
        } else if let Some(body) = after.strip_prefix('{') {
            let end = body
                .find('}')
                .ok_or_else(|| format!("unterminated `${{` in {s:?}"))?;
            let expr = &body[..end];
            let (var, default) = match expr.split_once(":-") {
                Some((v, d)) => (v, Some(d)),
                None => (expr, None),
            };
            match (env(var).filter(|v| !v.is_empty() || default.is_none()), default) {
                (Some(v), _) => out.push_str(&v),
                (None, Some(d)) => out.push_str(d),
                (None, None) => {
                    return Err(format!(
                        "environment variable {var} is not set (referenced as ${{{var}}})"
                    ))
                }
            }
            rest = &body[end + 1..];
        } else {
            out.push('$');
            rest = after;
        }
    }
    out.push_str(rest);
    Ok(out)
}

/// Replaces the password component of a URL with `***`.
pub fn redact_url(url: &str) -> String {
    let Some(scheme_end) = url.find("://") else {
        return url.to_string();
    };
    let auth_start = scheme_end + 3;
    let rest = &url[auth_start..];
    let host_start = rest.find(['/', '?']).unwrap_or(rest.len());
    let Some(at) = rest[..host_start].rfind('@') else {
        return url.to_string();
    };
    let userinfo = &rest[..at];
    match userinfo.find(':') {
        Some(colon) => format!(
            "{}{}:***{}",
            &url[..auth_start],
            &userinfo[..colon],
            &rest[at..]
        ),
        None => url.to_string(),
    }
}

impl std::fmt::Debug for ProviderSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderSpec")
            .field("name", &self.name)
            .field("kind", &self.kind)
            .field("url", &self.url.as_deref().map(redact_url))
            .field("path", &self.path)
            .field("include", &self.include)
            .field("exclude", &self.exclude)
            .finish_non_exhaustive()
    }
}

fn config_err(path: &Path, msg: impl Into<String>) -> StateError {
    StateError::Config {
        path: path.to_path_buf(),
        msg: msg.into(),
    }
}

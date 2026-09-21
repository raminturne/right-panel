use serde::{Deserialize, Serialize};
use std::path::{Component, Path};

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Permission {
    #[serde(rename = "storage")]
    Storage,
    #[serde(rename = "clipboard.read")]
    ClipboardRead,
    #[serde(rename = "clipboard.write")]
    ClipboardWrite,
    #[serde(rename = "system.openUrl")]
    SystemOpenUrl,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Setting {
    pub key: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub label: String,
    #[serde(default)]
    pub default: serde_json::Value,
    #[serde(default)]
    pub options: Vec<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub manifest_version: u32,
    #[serde(default = "api_one")]
    pub api_version: u32,
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub homepage: String,
    #[serde(default)]
    pub icon: String,
    pub entry: String,
    #[serde(default)]
    pub permissions: Vec<Permission>,
    #[serde(default)]
    pub settings: Vec<Setting>,
}
fn api_one() -> u32 {
    1
}
impl Manifest {
    pub fn validate(&self, root: &Path) -> Result<(), String> {
        if self.manifest_version != 1 || self.api_version != 1 {
            return Err("unsupported manifest or API version".into());
        }
        if !valid_id(&self.id) {
            return Err("invalid plugin id".into());
        }
        if self.name.trim().is_empty()
            || self.name.len() > 100
            || self.description.len() > 1000
            || self.author.len() > 100
        {
            return Err("invalid manifest text field".into());
        }
        if !semverish(&self.version) {
            return Err("version must look like SemVer".into());
        }
        for p in [&self.entry, &self.icon] {
            if !p.is_empty() && (!safe(p) || !root.join(p).is_file()) {
                return Err("entry/icon must be an existing safe relative path".into());
            }
        }
        if self.entry.is_empty() {
            return Err("entry is required".into());
        }
        if self.permissions.len() > 8 || self.settings.len() > 32 {
            return Err("too many permissions or settings".into());
        }
        for s in &self.settings {
            if s.key.is_empty()
                || s.key.len() > 64
                || !matches!(s.kind.as_str(), "boolean" | "text" | "number" | "select")
                || (s.kind == "select" && s.options.is_empty())
            {
                return Err("invalid settings schema".into());
            }
        }
        Ok(())
    }
}
fn safe(s: &str) -> bool {
    let p = Path::new(s);
    !p.is_absolute()
        && !s.contains(':')
        && p.components().all(|c| matches!(c, Component::Normal(_)))
}
pub fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && s.bytes().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'.' | b'_' | b'-')
        })
}
fn semverish(s: &str) -> bool {
    let p: Vec<_> = s.split('.').collect();
    p.len() == 3
        && p.iter()
            .all(|x| !x.is_empty() && x.bytes().all(|c| c.is_ascii_digit()))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ids() {
        assert!(valid_id("hello-world.2"));
        for s in ["", "../x", "hello/world", "UPPER"] {
            assert!(!valid_id(s));
        }
    }
}

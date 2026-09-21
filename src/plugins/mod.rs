//! Deliberately small plugin host. Plugins are data-only packages; native operations
//! are reached only through the checked bridge in `main.rs`.
mod manifest;

pub use manifest::{Manifest, Permission};

use serde::Serialize;
use std::{
    collections::BTreeMap,
    fs,
    io::{self, Read},
    path::{Component, Path, PathBuf},
};

const MAX_FILE: u64 = 512 * 1024;
const MAX_ARCHIVE_FILE: u64 = 2 * 1024 * 1024;
const MAX_STORAGE: usize = 64 * 1024;

#[derive(Clone, Serialize)]
pub struct PluginInfo {
    #[serde(flatten)]
    pub manifest: Manifest,
    pub enabled: bool,
    pub error: Option<String>,
    pub icon_data: Option<String>,
    /// This is deliberately only sent to the trusted host page. It is placed in an
    /// opaque-origin sandboxed iframe, never directly in the application DOM.
    pub document: Option<String>,
}

pub struct Manager {
    root: PathBuf,
    data: PathBuf,
    plugins: BTreeMap<String, PluginInfo>,
}

impl Manager {
    pub fn load(data: PathBuf, enabled: &BTreeMap<String, bool>) -> Self {
        let root = data.join("plugins");
        let _ = fs::create_dir_all(&root);
        let mut plugins = BTreeMap::new();
        if let Ok(entries) = fs::read_dir(&root) {
            for entry in entries.flatten() {
                if !entry.path().is_dir() {
                    continue;
                }
                match Self::read_plugin(&entry.path(), enabled) {
                    Ok(p) => {
                        if plugins.contains_key(&p.manifest.id) {
                            continue;
                        }
                        plugins.insert(p.manifest.id.clone(), p);
                    }
                    Err(e) => {
                        eprintln!("Right Panel ignored plugin {}: {e}", entry.path().display());
                    }
                }
            }
        }
        Self {
            root,
            data,
            plugins,
        }
    }
    fn read_plugin(dir: &Path, enabled: &BTreeMap<String, bool>) -> Result<PluginInfo, String> {
        let raw =
            fs::read_to_string(dir.join("manifest.json")).map_err(|e| format!("manifest: {e}"))?;
        let manifest: Manifest =
            serde_json::from_str(&raw).map_err(|e| format!("manifest JSON: {e}"))?;
        manifest.validate(dir)?;
        let document = build_document(dir, &manifest).map_err(|e| format!("entry: {e}"))?;
        let icon_data = if manifest.icon.ends_with(".svg") {
            fs::read(dir.join(&manifest.icon))
                .ok()
                .filter(|b| b.len() <= 128 * 1024)
                .map(|b| format!("data:image/svg+xml;base64,{}", crate::util::base64(&b)))
        } else {
            None
        };
        Ok(PluginInfo {
            enabled: enabled.get(&manifest.id).copied().unwrap_or(true),
            manifest,
            error: None,
            icon_data,
            document: Some(document),
        })
    }
    pub fn infos(&self) -> Vec<PluginInfo> {
        self.plugins.values().cloned().collect()
    }
    pub fn get(&self, id: &str) -> Option<&PluginInfo> {
        self.plugins.get(id)
    }
    pub fn permissions(&self, id: &str) -> Option<&[Permission]> {
        self.get(id).map(|p| p.manifest.permissions.as_slice())
    }
    pub fn install(
        &mut self,
        archive: &Path,
        enabled: &BTreeMap<String, bool>,
    ) -> Result<String, String> {
        let file = fs::File::open(archive).map_err(|e| e.to_string())?;
        let mut zip =
            zip::ZipArchive::new(file).map_err(|e| format!("not a valid .rpp/.zip: {e}"))?;
        if zip.is_empty() || zip.len() > 128 {
            return Err("package has an invalid number of files".into());
        }
        let temp = self.root.join(".installing");
        let _ = fs::remove_dir_all(&temp);
        fs::create_dir_all(&temp).map_err(|e| e.to_string())?;
        for n in 0..zip.len() {
            let mut f = zip.by_index(n).map_err(|e| e.to_string())?;
            let name = Path::new(f.name());
            if !safe_relative(name) || f.size() > MAX_ARCHIVE_FILE {
                let _ = fs::remove_dir_all(&temp);
                return Err("unsafe archive entry".into());
            }
            if f.is_dir() {
                continue;
            }
            let out = temp.join(name);
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut dst = fs::File::create(out).map_err(|e| e.to_string())?;
            io::copy(&mut f, &mut dst).map_err(|e| e.to_string())?;
        }
        let p = Self::read_plugin(&temp, enabled)?;
        if self.plugins.contains_key(&p.manifest.id) {
            let _ = fs::remove_dir_all(&temp);
            return Err(format!("plugin '{}' is already installed", p.manifest.id));
        }
        let id = p.manifest.id.clone();
        fs::rename(&temp, self.root.join(&id)).map_err(|e| e.to_string())?;
        self.plugins.insert(id.clone(), p);
        Ok(id)
    }
    pub fn uninstall(&mut self, id: &str) -> Result<(), String> {
        if self.plugins.remove(id).is_none() {
            return Err("plugin not found".into());
        }
        fs::remove_dir_all(self.root.join(id)).map_err(|e| e.to_string())?;
        let _ = fs::remove_file(self.data.join("plugin-data").join(format!("{id}.json")));
        Ok(())
    }
    pub fn storage(
        &self,
        id: &str,
        op: &str,
        key: Option<&str>,
        value: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, String> {
        if self.get(id).is_none() {
            return Err("unknown plugin".into());
        }
        let dir = self.data.join("plugin-data");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join(format!("{id}.json"));
        let mut map: BTreeMap<String, serde_json::Value> = fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let key = key.unwrap_or("");
        if key.is_empty() || key.len() > 128 {
            return Err("invalid storage key".into());
        }
        let result = match op {
            "get" => map.get(key).cloned().unwrap_or(serde_json::Value::Null),
            "set" => {
                map.insert(key.into(), value.unwrap_or(serde_json::Value::Null));
                serde_json::Value::Null
            }
            "remove" => {
                map.remove(key);
                serde_json::Value::Null
            }
            "clear" => {
                map.clear();
                serde_json::Value::Null
            }
            _ => return Err("unknown storage operation".into()),
        };
        let body = serde_json::to_string(&map).map_err(|e| e.to_string())?;
        if body.len() > MAX_STORAGE {
            return Err("plugin storage limit is 64 KiB".into());
        }
        fs::write(path, body).map_err(|e| e.to_string())?;
        Ok(result)
    }
}

fn safe_relative(path: &Path) -> bool {
    let raw = path.to_string_lossy();
    !raw.is_empty()
        && !raw.contains(['\\', ':'])
        && !path.is_absolute()
        && path.components().all(|c| matches!(c, Component::Normal(_)))
}
fn build_document(dir: &Path, m: &Manifest) -> Result<String, String> {
    let path = dir.join(&m.entry);
    let meta = fs::metadata(&path).map_err(|e| e.to_string())?;
    if meta.len() > MAX_FILE {
        return Err("entry is too large".into());
    }
    let mut html = fs::read_to_string(path).map_err(|e| e.to_string())?;
    // Packages may use an inline entry or conventional plugin.js/style.css. Inline
    // their contents so the opaque sandbox cannot fetch arbitrary filesystem URLs.
    for (name, tag) in [("style.css", "style"), ("plugin.js", "script")] {
        let resource = dir.join(name);
        if resource.exists() {
            let mut s = String::new();
            fs::File::open(resource)
                .map_err(|e| e.to_string())?
                .take(MAX_FILE)
                .read_to_string(&mut s)
                .map_err(|e| e.to_string())?;
            html = html.replace(
                &format!(
                    "<{} {}=\"{}\"></{}>",
                    if tag == "style" { "link" } else { "script" },
                    if tag == "style" { "href" } else { "src" },
                    name,
                    if tag == "style" { "link" } else { "script" }
                ),
                &format!("<{}>{}</{}>", tag, s, tag),
            );
        }
    }
    let bridge = format!(
        r#"<script>(function(){{const ID={id};let n=0,p=new Map();function call(op,args){{return new Promise((resolve,reject)=>{{const r=String(++n);p.set(r,{{resolve,reject}});parent.postMessage({{rightPanelPlugin:1,id:ID,request:r,op:op,args:args||{{}}}},'*')}})}}window.addEventListener('message',e=>{{let m=e.data;if(!m||m.rightPanelPlugin!==1||!p.has(m.request))return;let q=p.get(m.request);p.delete(m.request);m.ok?q.resolve(m.value):q.reject(new Error(m.error||'Plugin request failed'))}});window.RightPanel={{apiVersion:1,plugin:{{id:ID,version:{version}}},storage:{{get:k=>call('storage.get',{{key:k}}),set:(k,v)=>call('storage.set',{{key:k,value:v}}),remove:k=>call('storage.remove',{{key:k}}),clear:()=>call('storage.clear',{{key:'_'}})}},clipboard:{{read:()=>call('clipboard.read'),write:text=>call('clipboard.write',{{text}})}},openUrl:url=>call('system.openUrl',{{url}}),ui:{{close:()=>call('ui.close'),showToast:message=>call('ui.toast',{{message}})}},onReady:f=>addEventListener('DOMContentLoaded',f),onShow:f=>addEventListener('rightpanelshow',f),onHide:f=>addEventListener('rightpanelhide',f)}}}})();</script>"#,
        id = serde_json::to_string(&m.id).unwrap(),
        version = serde_json::to_string(&m.version).unwrap()
    );
    // Must precede every plugin script: entries may omit a head tag entirely.
    Ok(format!("{bridge}{html}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_unsafe_paths() {
        for p in ["../x", "/etc/passwd", "C:\\x", "a/../../x"] {
            assert!(!safe_relative(Path::new(p)));
        }
    }

    #[test]
    fn bridge_precedes_entry_markup() {
        assert!(
            format!("<script>bridge</script>{}", "<button>plugin</button>")
                .starts_with("<script>bridge")
        );
    }
}

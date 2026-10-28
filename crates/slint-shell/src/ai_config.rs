// ai_config.rs — AI 配置持久化(~/.config/office-rs2/ai_profiles.json,权限 600)
// 不依赖 serde derive,手工读写 serde_json::Value。

use serde_json::{json, Value};
use std::io::Write;
use std::path::PathBuf;

#[derive(Clone, Debug, Default)]
pub struct Profile {
    pub name: String,
    pub provider: i32, // 0 Claude 1 Gemini 2 GPT 3 DeepSeek 4 其他
    pub endpoint: String,
    pub key: String,
    pub model: String,
}

#[derive(Clone, Debug)]
pub struct AiConfig {
    pub profiles: Vec<Profile>,
    pub active: i32,
    pub hf_cache_dir: String,
    pub language: i32,
    pub theme: i32,
}

impl Default for AiConfig {
    fn default() -> Self {
        AiConfig { profiles: vec![], active: -1, hf_cache_dir: String::new(), language: -1, theme: 0 }
    }
}

pub fn config_path() -> Option<PathBuf> {
    let base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => PathBuf::from(std::env::var_os("HOME")?).join(".config"),
    };
    Some(base.join("office-rs2").join("ai_profiles.json"))
}

/// 界面语言下标：>=0 用用户选的；<0（从没选过）跟随系统：zh 开头=0 简体中文，其余=1 English
pub fn effective_language(saved: i32) -> i32 {
    if saved >= 0 { return saved; }
    for k in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(v) = std::env::var(k) {
            let v = v.trim().to_lowercase();
            if v.is_empty() || v == "c" || v == "posix" { continue; }
            if v.starts_with("zh") {
                let t = v.replace('-', "_");
                return if t.starts_with("zh_tw") || t.starts_with("zh_hk") || t.starts_with("zh_mo") || t.contains("hant") { 7 } else { 0 };
            }
            return 1;
        }
    }
    0
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}
fn n(v: &Value, k: &str) -> i32 {
    v.get(k).and_then(|x| x.as_i64()).unwrap_or(0) as i32
}

impl AiConfig {
    pub fn load() -> Self {
        let path = match config_path() { Some(p) => p, None => return Self::default() };
        let text = match std::fs::read_to_string(&path) { Ok(t) => t, Err(_) => return Self::default() };
        let root: Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[AI-CONFIG] 解析失败,使用空配置: {}", e);
                return Self::default();
            }
        };
        let profiles: Vec<Profile> = root
            .get("profiles")
            .and_then(|x| x.as_array())
            .map(|arr| {
                arr.iter()
                    .map(|p| Profile {
                        name: s(p, "name"),
                        provider: n(p, "provider"),
                        endpoint: s(p, "endpoint"),
                        key: s(p, "key"),
                        model: s(p, "model"),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut active = root.get("active").and_then(|x| x.as_i64()).unwrap_or(-1) as i32;
        if active < 0 || active as usize >= profiles.len() { active = -1; }
        AiConfig {
            profiles,
            active,
            hf_cache_dir: s(&root, "hf_cache_dir"),
            language: root.get("language").and_then(|x| x.as_i64()).map(|v| v as i32).unwrap_or(-1),
            theme: n(&root, "theme"),
        }
    }

    /// 激活项转 Agent 配置;未配置完整时返回 None
    pub fn to_agent_config(&self) -> Option<agent_bridge::AgentConfig> {
        if !self.is_configured() { return None; }
        let p = self.profiles.get(self.active as usize)?;
        Some(agent_bridge::AgentConfig {
            provider: p.provider,
            endpoint: p.endpoint.clone(),
            key: p.key.clone(),
            model: p.model.clone(),
        })
    }

    /// 有激活项,且 Key、模型名都非空
    pub fn is_configured(&self) -> bool {
        if self.active < 0 { return false; }
        match self.profiles.get(self.active as usize) {
            Some(p) => !p.key.trim().is_empty() && !p.model.trim().is_empty(),
            None => false,
        }
    }

    pub fn save(&self) -> Result<(), String> {
        let path = config_path().ok_or("找不到配置目录(HOME 未设置)")?;
        let dir = path.parent().ok_or("配置路径异常")?;
        std::fs::create_dir_all(dir).map_err(|e| format!("创建配置目录失败: {}", e))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
        let profs: Vec<Value> = self
            .profiles
            .iter()
            .map(|p| json!({
                "name": p.name, "provider": p.provider, "endpoint": p.endpoint,
                "key": p.key, "model": p.model,
            }))
            .collect();
        let root = json!({
            "version": 1,
            "active": self.active,
            "hf_cache_dir": self.hf_cache_dir,
            "language": self.language,
            "theme": self.theme,
            "profiles": profs,
        });
        let text = serde_json::to_string_pretty(&root).map_err(|e| e.to_string())?;

        // 先写临时文件(创建即 600),再原子改名
        let tmp = path.with_extension("json.tmp");
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp).map_err(|e| format!("写入失败: {}", e))?;
        f.write_all(text.as_bytes()).map_err(|e| format!("写入失败: {}", e))?;
        f.sync_all().ok();
        drop(f);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&tmp, &path).map_err(|e| format!("保存失败: {}", e))?;
        Ok(())
    }
}

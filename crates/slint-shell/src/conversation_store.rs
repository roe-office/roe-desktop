//! conversation_store.rs — Agent 对话历史持久化(一对话一个 JSON 文件)
//!
//! 窄边界:本模块只做 (Conversation <-> JSON <-> 磁盘),不碰 Slint、不碰 agent-bridge。
//! 目录由调用方传入。第一版只存 user/assistant 可见消息,不存工具历史。

use std::path::{Path, PathBuf};

pub const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq)]
pub struct StoredMessage {
    /// "user" | "assistant"
    pub role: String,
    pub text: String,
    /// Unix 秒
    pub ts: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Conversation {
    pub version: u32,
    pub id: String,
    pub title: String,
    /// 用户手改过标题后为 true,此后 build_snapshot 不再重推标题
    pub title_custom: bool,
    pub created: u64,
    pub updated: u64,
    pub messages: Vec<StoredMessage>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConversationMeta {
    pub id: String,
    pub title: String,
    pub updated: u64,
}

/// 只有 user / assistant 值得持久化(script 的 shadow_handle 跨会话无效,
/// thinking 是过程态,notice 是界面反馈)。
pub fn is_persistable_kind(kind: &str) -> bool {
    kind == "user" || kind == "assistant"
}

const TITLE_MAX_CHARS: usize = 50;

/// 标题 = 第一条非空 user 消息的第一句话(遇换行、。!?!? 截断,英文句点不算);
/// 超过 50 个字符取前 49 个加 "…"(合计 50);没有或第一句为空则 "新对话"。
pub fn derive_title(messages: &[StoredMessage]) -> String {
    let first = messages
        .iter()
        .find(|m| m.role == "user" && !m.text.trim().is_empty())
        .map(|m| m.text.trim())
        .and_then(|t| {
            let end = t
                .find(|c: char| matches!(c, '\n' | '\r' | '。' | '！' | '？' | '!' | '?'))
                .unwrap_or(t.len());
            let sentence = t[..end].trim();
            if sentence.is_empty() { None } else { Some(sentence) }
        });
    match first {
        None => "新对话".to_string(),
        Some(sen) => {
            if sen.chars().count() <= TITLE_MAX_CHARS {
                sen.to_string()
            } else {
                let mut out: String = sen.chars().take(TITLE_MAX_CHARS - 1).collect();
                out.push('…');
                out
            }
        }
    }
}

pub fn to_json(c: &Conversation) -> String {
    let msgs: Vec<serde_json::Value> = c
        .messages
        .iter()
        .map(|m| serde_json::json!({ "role": m.role, "text": m.text, "ts": m.ts }))
        .collect();
    let v = serde_json::json!({
        "version": c.version,
        "id": c.id,
        "title": c.title,
        "title_custom": c.title_custom,
        "created": c.created,
        "updated": c.updated,
        "messages": msgs,
    });
    serde_json::to_string_pretty(&v).unwrap_or_default()
}

pub fn from_json(s: &str) -> Result<Conversation, String> {
    let v: serde_json::Value = serde_json::from_str(s).map_err(|e| format!("JSON 解析失败: {e}"))?;
    let version = v
        .get("version")
        .and_then(|x| x.as_u64())
        .ok_or_else(|| "缺少 version 字段".to_string())? as u32;
    if version != FORMAT_VERSION {
        return Err(format!("不支持的对话文件版本: {version}"));
    }
    let get_s = |k: &str| -> Result<String, String> {
        v.get(k)
            .and_then(|x| x.as_str())
            .map(|x| x.to_string())
            .ok_or_else(|| format!("缺少字段: {k}"))
    };
    let get_n = |k: &str| -> Result<u64, String> {
        v.get(k).and_then(|x| x.as_u64()).ok_or_else(|| format!("缺少字段: {k}"))
    };
    let arr = v
        .get("messages")
        .and_then(|x| x.as_array())
        .ok_or_else(|| "缺少字段: messages".to_string())?;
    let mut messages = Vec::with_capacity(arr.len());
    for m in arr {
        let role = m.get("role").and_then(|x| x.as_str()).ok_or("消息缺少 role")?.to_string();
        let text = m.get("text").and_then(|x| x.as_str()).ok_or("消息缺少 text")?.to_string();
        let ts = m.get("ts").and_then(|x| x.as_u64()).ok_or("消息缺少 ts")?;
        messages.push(StoredMessage { role, text, ts });
    }
    Ok(Conversation {
        version,
        id: get_s("id")?,
        title: get_s("title")?,
        title_custom: v.get("title_custom").and_then(|x| x.as_bool()).unwrap_or(false),
        created: get_n("created")?,
        updated: get_n("updated")?,
        messages,
    })
}

fn check_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.contains('/') || id.contains('\\') || id.contains("..") {
        return Err(format!("非法对话 id: {id:?}"));
    }
    Ok(())
}

fn file_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.json"))
}

/// 写 `<dir>/<id>.json`;目录不存在则创建;先写临时文件再 rename。
pub fn save(dir: &Path, c: &Conversation) -> Result<(), String> {
    check_id(&c.id)?;
    std::fs::create_dir_all(dir).map_err(|e| format!("创建目录失败 {}: {e}", dir.display()))?;
    let final_path = file_path(dir, &c.id);
    let tmp_path = dir.join(format!("{}.json.tmp", c.id));
    std::fs::write(&tmp_path, to_json(c)).map_err(|e| format!("写临时文件失败: {e}"))?;
    std::fs::rename(&tmp_path, &final_path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp_path);
        format!("改名失败: {e}")
    })
}

/// id 含路径分隔符或 ".." 时返回 Err。
pub fn load(dir: &Path, id: &str) -> Result<Conversation, String> {
    check_id(id)?;
    let p = file_path(dir, id);
    let s = std::fs::read_to_string(&p).map_err(|e| format!("读取失败 {}: {e}", p.display()))?;
    from_json(&s)
}

/// 按 updated 倒序;非 .json 文件与解析失败的文件跳过;目录不存在返回空。
pub fn list(dir: &Path) -> Vec<ConversationMeta> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<ConversationMeta> = Vec::new();
    for e in rd.flatten() {
        let path = e.path();
        if path.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let Ok(s) = std::fs::read_to_string(&path) else { continue };
        let Ok(c) = from_json(&s) else { continue };
        out.push(ConversationMeta { id: c.id, title: c.title, updated: c.updated });
    }
    out.sort_by(|a, b| b.updated.cmp(&a.updated));
    out
}

/// 由界面消息重建要落盘的对话。items 是 (role, text),调用方已过滤为 user/assistant。
/// - created 沿用 existing,否则 now;updated = now;title = derive_title。
/// - 第 i 条消息:existing 同下标且 role 相同则沿用其 ts,否则用 now。
pub fn build_snapshot(
    existing: Option<&Conversation>,
    id: &str,
    now: u64,
    items: &[(String, String)],
) -> Conversation {
    let messages: Vec<StoredMessage> = items
        .iter()
        .enumerate()
        .map(|(i, (role, text))| {
            let ts = existing
                .and_then(|c| c.messages.get(i))
                .filter(|m| &m.role == role)
                .map(|m| m.ts)
                .unwrap_or(now);
            StoredMessage { role: role.clone(), text: text.clone(), ts }
        })
        .collect();
    Conversation {
        version: FORMAT_VERSION,
        id: id.to_string(),
        title: match existing {
            Some(c) if c.title_custom => c.title.clone(),
            _ => derive_title(&messages),
        },
        title_custom: existing.map(|c| c.title_custom).unwrap_or(false),
        created: existing.map(|c| c.created).unwrap_or(now),
        updated: now,
        messages,
    }
}

/// 改标题:读、改 title、置 title_custom、写回;不改 updated(列表顺序不变)。
/// 标题 trim 后为空返回 Err。
pub fn rename(dir: &Path, id: &str, title: &str) -> Result<(), String> {
    let t = title.trim();
    if t.is_empty() {
        return Err("标题不能为空".to_string());
    }
    let mut c = load(dir, id)?;
    c.title = t.to_string();
    c.title_custom = true;
    save(dir, &c)
}

/// 删除 `<dir>/<id>.json`。id 非法返回 Err;文件不存在视为成功(幂等),
/// 这样列表与磁盘不一致时点一次删除也能把列表刷干净。
pub fn delete(dir: &Path, id: &str) -> Result<(), String> {
    check_id(id)?;
    let p = file_path(dir, id);
    match std::fs::remove_file(&p) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("删除失败 {}: {e}", p.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("roe_conv_test_{}_{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    fn msg(role: &str, text: &str, ts: u64) -> StoredMessage {
        StoredMessage { role: role.into(), text: text.into(), ts }
    }

    fn conv(id: &str, updated: u64) -> Conversation {
        Conversation {
            version: FORMAT_VERSION,
            id: id.into(),
            title: format!("标题{id}"),
            title_custom: false,
            created: 1,
            updated,
            messages: vec![msg("user", "你好", 10), msg("assistant", "您好,有什么可以帮您?", 11)],
        }
    }

    #[test]
    fn persistable_kinds() {
        assert!(is_persistable_kind("user"));
        assert!(is_persistable_kind("assistant"));
        assert!(!is_persistable_kind("thinking"));
        assert!(!is_persistable_kind("script"));
        assert!(!is_persistable_kind("notice"));
    }

    #[test]
    fn title_from_first_user_message_truncated_by_chars() {
        let long = "一二三四五六七八九十".repeat(6); // 60 个字符,无句末标点
        let msgs = vec![msg("assistant", "先说话", 1), msg("user", &long, 2)];
        let t = derive_title(&msgs);
        assert_eq!(t.chars().count(), 50);
        assert!(t.ends_with('…'));
        assert!(t.starts_with("一二三"));
    }

    #[test]
    fn title_exactly_50_chars_has_no_ellipsis() {
        let exact = "一二三四五六七八九十".repeat(5); // 恰好 50
        let t = derive_title(&[msg("user", &exact, 1)]);
        assert_eq!(t, exact);
    }

    #[test]
    fn title_first_sentence_cuts_at_cjk_terminators() {
        assert_eq!(derive_title(&[msg("user", "你好。请帮我写表格", 1)]), "你好");
        assert_eq!(derive_title(&[msg("user", "怎么做?先说步骤", 1)]), "怎么做");
        assert_eq!(derive_title(&[msg("user", "太好了!继续", 1)]), "太好了");
        assert_eq!(derive_title(&[msg("user", "为什么?", 1)]), "为什么");
        assert_eq!(derive_title(&[msg("user", "真的吗？再说一遍", 1)]), "真的吗");
        assert_eq!(derive_title(&[msg("user", "太好了！继续", 1)]), "太好了");
    }

    #[test]
    fn title_first_sentence_cuts_at_newline() {
        assert_eq!(derive_title(&[msg("user", "第一行\n第二行", 1)]), "第一行");
        assert_eq!(derive_title(&[msg("user", "A行\r\nB行", 1)]), "A行");
    }

    #[test]
    fn title_keeps_ascii_period() {
        assert_eq!(derive_title(&[msg("user", "把 3.14 写进 a.xlsx", 1)]), "把 3.14 写进 a.xlsx");
    }

    #[test]
    fn title_skips_blank_user_and_falls_back_when_first_sentence_empty() {
        let msgs = vec![msg("user", "   ", 1), msg("user", "有内容", 2)];
        assert_eq!(derive_title(&msgs), "有内容");
        assert_eq!(derive_title(&[msg("user", "。。。", 1)]), "新对话");
    }

    #[test]
    fn title_fallback_when_no_user_message() {
        assert_eq!(derive_title(&[]), "新对话");
        assert_eq!(derive_title(&[msg("assistant", "hi", 1)]), "新对话");
    }

    #[test]
    fn json_roundtrip() {
        let c = conv("a1", 100);
        let back = from_json(&to_json(&c)).expect("应能解析");
        assert_eq!(back, c);
    }

    #[test]
    fn from_json_rejects_unknown_version() {
        let mut c = conv("a1", 100);
        c.version = 999;
        assert!(from_json(&to_json(&c)).is_err());
    }

    #[test]
    fn from_json_rejects_malformed() {
        assert!(from_json("{oops").is_err());
        assert!(from_json("{}").is_err());
    }

    #[test]
    fn save_then_load_roundtrip_and_creates_dir() {
        let dir = tmp("roundtrip").join("nested");
        let c = conv("c1", 100);
        save(&dir, &c).expect("save");
        assert!(dir.join("c1.json").is_file());
        assert_eq!(load(&dir, "c1").expect("load"), c);
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }

    #[test]
    fn save_leaves_no_tmp_file() {
        let dir = tmp("notmp");
        save(&dir, &conv("c1", 100)).expect("save");
        let names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["c1.json".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_rejects_path_traversal_ids() {
        let dir = tmp("badid");
        assert!(load(&dir, "../evil").is_err());
        assert!(load(&dir, "a/b").is_err());
        assert!(load(&dir, "").is_err());
    }

    #[test]
    fn list_sorted_desc_and_skips_junk() {
        let dir = tmp("list");
        save(&dir, &conv("c1", 100)).unwrap();
        save(&dir, &conv("c2", 300)).unwrap();
        save(&dir, &conv("c3", 200)).unwrap();
        std::fs::write(dir.join("notes.txt"), "x").unwrap();
        std::fs::write(dir.join("bad.json"), "{oops").unwrap();
        let ids: Vec<String> = list(&dir).into_iter().map(|m| m.id).collect();
        assert_eq!(ids, vec!["c2", "c3", "c1"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_missing_dir_is_empty() {
        assert!(list(&tmp("missing")).is_empty());
    }

    fn items(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter().map(|(r, t)| (r.to_string(), t.to_string())).collect()
    }

    #[test]
    fn snapshot_fresh_uses_now_everywhere() {
        let c = build_snapshot(None, "n1", 500, &items(&[("user", "你好"), ("assistant", "您好")]));
        assert_eq!(c.version, FORMAT_VERSION);
        assert_eq!(c.id, "n1");
        assert_eq!((c.created, c.updated), (500, 500));
        assert_eq!(c.title, "你好");
        assert_eq!(c.messages.len(), 2);
        assert!(c.messages.iter().all(|m| m.ts == 500));
    }

    #[test]
    fn snapshot_preserves_existing_ts_and_created() {
        let old = conv("n1", 100); // created=1, ts=10/11
        let c = build_snapshot(
            Some(&old),
            "n1",
            900,
            &items(&[("user", "你好"), ("assistant", "您好"), ("user", "再问一个")]),
        );
        assert_eq!(c.created, 1);
        assert_eq!(c.updated, 900);
        assert_eq!(c.messages[0].ts, 10);
        assert_eq!(c.messages[1].ts, 11);
        assert_eq!(c.messages[2].ts, 900);
    }

    #[test]
    fn snapshot_keeps_ts_when_text_grew_but_not_when_role_differs() {
        let old = conv("n1", 100);
        let c = build_snapshot(
            Some(&old),
            "n1",
            900,
            &items(&[("assistant", "不同角色"), ("assistant", "您好,又多了一段")]),
        );
        assert_eq!(c.messages[0].ts, 900); // 下标 0 角色由 user 变 assistant
        assert_eq!(c.messages[1].ts, 11); // 角色相同,文本变长仍沿用
    }

    #[test]
    fn snapshot_empty_items() {
        let c = build_snapshot(None, "n1", 500, &[]);
        assert!(c.messages.is_empty());
        assert_eq!(c.title, "新对话");
    }

    #[test]
    fn json_roundtrip_keeps_title_custom() {
        let mut c = conv("a1", 100);
        c.title_custom = true;
        assert_eq!(from_json(&to_json(&c)).unwrap(), c);
    }

    #[test]
    fn snapshot_keeps_custom_title() {
        let mut old = conv("n1", 100);
        old.title = "自定义".into();
        old.title_custom = true;
        let c = build_snapshot(Some(&old), "n1", 900, &items(&[("user", "你好")]));
        assert_eq!(c.title, "自定义");
        assert!(c.title_custom);
    }

    #[test]
    fn rename_sets_title_and_flag_keeps_updated() {
        let dir = tmp("rename");
        save(&dir, &conv("c1", 100)).unwrap();
        rename(&dir, "c1", "  新名字 ").unwrap();
        let c = load(&dir, "c1").unwrap();
        assert_eq!(c.title, "新名字");
        assert!(c.title_custom);
        assert_eq!(c.updated, 100);
        assert_eq!(c.messages.len(), 2);
        assert_eq!(list(&dir)[0].title, "新名字");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rename_rejects_empty_title() {
        let dir = tmp("rename_empty");
        save(&dir, &conv("c1", 100)).unwrap();
        assert!(rename(&dir, "c1", "   ").is_err());
        assert_eq!(load(&dir, "c1").unwrap().title, "标题c1");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rename_missing_or_bad_id_is_err() {
        let dir = tmp("rename_missing");
        assert!(rename(&dir, "nope", "x").is_err());
        assert!(rename(&dir, "../evil", "x").is_err());
    }

    #[test]
    fn delete_removes_only_target() {
        let dir = tmp("delete_target");
        save(&dir, &conv("c1", 100)).unwrap();
        save(&dir, &conv("c2", 200)).unwrap();
        delete(&dir, "c1").unwrap();
        assert!(load(&dir, "c1").is_err());
        assert!(!file_path(&dir, "c1").exists());
        let metas = list(&dir);
        assert_eq!(metas.len(), 1);
        assert_eq!(metas[0].id, "c2");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_missing_is_ok() {
        let dir = tmp("delete_missing");
        assert!(delete(&dir, "nope").is_ok());
        save(&dir, &conv("c1", 100)).unwrap();
        assert!(delete(&dir, "nope").is_ok());
        assert_eq!(list(&dir).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_rejects_bad_id_and_keeps_outside_file() {
        let dir = tmp("delete_bad");
        save(&dir, &conv("c1", 100)).unwrap();
        let victim_name = format!("roe_conv_victim_{}", std::process::id());
        let victim = dir.parent().unwrap().join(format!("{victim_name}.json"));
        std::fs::write(&victim, "x").unwrap();
        assert!(delete(&dir, &format!("../{victim_name}")).is_err());
        assert!(victim.exists());
        assert!(delete(&dir, "").is_err());
        assert!(delete(&dir, "a/b").is_err());
        assert!(delete(&dir, "a\\b").is_err());
        assert_eq!(list(&dir).len(), 1);
        let _ = std::fs::remove_file(&victim);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

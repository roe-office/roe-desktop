//! "+"号上传:把用户选的本地文件复制进 Agent 工作区的 inputs/。
//! 宿主机路径只出现在 UI 与本函数里,模型只会看到虚拟路径。

use std::path::Path;

/// 单个上传文件大小上限(与图片函数的 20MB 上限一致)。
pub const MAX_UPLOAD_BYTES: u64 = 20 * 1024 * 1024;

/// 允许上传的扩展名(小写)。odf、md/txt 暂不支持。
pub const ALLOWED_EXTS: [&str; 7] = ["xlsx", "docx", "pptx", "csv", "png", "jpg", "jpeg"];

/// 校验并复制 `src` 到 `workspace_root` 的 inputs/,返回最终落地的文件名(重名会带序号)。
pub fn upload_to_inputs(workspace_root: &Path, src: &Path) -> Result<String, String> {
    let name = src
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| "文件名无效".to_string())?;
    let ext = src
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    if !ALLOWED_EXTS.contains(&ext.as_str()) {
        return Err(format!("不支持的文件类型:{name}(支持 xlsx / docx / pptx / csv / png / jpg)"));
    }
    let meta = std::fs::metadata(src).map_err(|e| format!("无法读取 {name}: {e}"))?;
    if !meta.is_file() {
        return Err(format!("{name} 不是文件"));
    }
    if meta.len() > MAX_UPLOAD_BYTES {
        return Err(format!("{name} 超过 20MB 上限"));
    }
    let bytes = std::fs::read(src).map_err(|e| format!("无法读取 {name}: {e}"))?;
    let ws = roe_agent::workspace::WorkspaceContext::open_or_create(workspace_root)
        .map_err(|e| format!("打开工作区失败: {e}"))?;
    let vpath = ws
        .write_file("inputs", name, &bytes)
        .map_err(|e| format!("写入 inputs 失败: {e}"))?;
    Ok(vpath.rsplit('/').next().unwrap_or(name).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn setup() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let ws = d.path().join("ws");
        let src = d.path().join("src");
        fs::create_dir_all(&src).unwrap();
        (d, ws, src)
    }

    #[test]
    fn uploads_allowed_file_into_inputs_keeping_name() {
        let (_d, ws, src) = setup();
        let f = src.join("logo.png");
        fs::write(&f, b"abc").unwrap();
        let name = upload_to_inputs(&ws, &f).expect("应上传成功");
        assert_eq!(name, "logo.png");
        let ws_real = ws.join("inputs").join("logo.png");
        assert_eq!(fs::read(ws_real).unwrap(), b"abc");
    }

    #[test]
    fn extension_check_is_case_insensitive() {
        let (_d, ws, src) = setup();
        let f = src.join("Report.XLSX");
        fs::write(&f, b"x").unwrap();
        assert_eq!(upload_to_inputs(&ws, &f).unwrap(), "Report.XLSX");
    }

    #[test]
    fn same_name_twice_gets_serial() {
        let (_d, ws, src) = setup();
        let f = src.join("a.png");
        fs::write(&f, b"1").unwrap();
        assert_eq!(upload_to_inputs(&ws, &f).unwrap(), "a.png");
        assert_eq!(upload_to_inputs(&ws, &f).unwrap(), "a (1).png");
    }

    #[test]
    fn rejects_disallowed_extension() {
        let (_d, ws, src) = setup();
        for n in ["a.txt", "b.odp", "c.ods", "d", "e.xls"] {
            let f = src.join(n);
            fs::write(&f, b"x").unwrap();
            assert!(upload_to_inputs(&ws, &f).is_err(), "{n} 不应被接受");
        }
    }

    #[test]
    fn rejects_oversize_file() {
        let (_d, ws, src) = setup();
        let f = src.join("big.csv");
        let h = fs::File::create(&f).unwrap();
        h.set_len(MAX_UPLOAD_BYTES + 1).unwrap();
        drop(h);
        assert!(upload_to_inputs(&ws, &f).is_err());
    }

    #[test]
    fn rejects_missing_source() {
        let (_d, ws, src) = setup();
        assert!(upload_to_inputs(&ws, &src.join("nope.png")).is_err());
    }
}

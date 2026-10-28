//! 把模型输出的 markdown 清洗成可读纯文本(Slint 的 Text 不渲染 markdown)。
//! 纯函数,流式场景下由调用方对"累计原文"整串调用,不要对单个增量调用。

pub fn strip_markdown(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut in_fence = false;
    for line in text.split('\n') {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            out.push(line.to_string());
            continue;
        }
        if let Some(l) = convert_line(line) {
            out.push(l);
        }
    }
    out.join("\n")
}

/// 单行转换;返回 None 表示整行丢弃(表格分隔行)。
fn convert_line(line: &str) -> Option<String> {
    let t = line.trim_start();
    let tt = t.trim_end();

    // 表格行
    if tt.len() > 1 && tt.starts_with('|') && tt.ends_with('|') {
        let inner = &tt[1..tt.len() - 1];
        if inner.contains('-') && inner.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ')) {
            return None;
        }
        let cells: Vec<String> = inner.split('|').map(|c| inline(c.trim())).collect();
        return Some(cells.join("  |  "));
    }

    // 引用
    if let Some(rest) = t.strip_prefix("> ") {
        return Some(inline(rest));
    }

    // 标题
    let hashes = t.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&hashes) && t[hashes..].starts_with(' ') {
        return Some(inline(t[hashes..].trim_start()));
    }

    // 列表
    if let Some(rest) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
        let leading = &line[..line.len() - t.len()];
        return Some(format!("{leading}• {}", inline(rest)));
    }

    Some(inline(line))
}

/// 行内:去 ** 和反引号,链接改成"文字 (url)"。
fn inline(s: &str) -> String {
    strip_links(&s.replace("**", "").replace('`', ""))
}

fn strip_links(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('[') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        if let Some(j) = after.find("](") {
            let text = &after[..j];
            let tail = &after[j + 2..];
            if !text.contains('[') {
                if let Some(k) = tail.find(')') {
                    out.push_str(&format!("{} ({})", text, &tail[..k]));
                    rest = &tail[k + 1..];
                    continue;
                }
            }
        }
        out.push('[');
        rest = after;
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::strip_markdown as s;

    #[test]
    fn bold_markers_removed() {
        assert_eq!(s("这是**重点**内容"), "这是重点内容");
    }

    #[test]
    fn heading_prefix_removed() {
        assert_eq!(s("## 标题\n正文"), "标题\n正文");
    }

    #[test]
    fn bullets_become_dots() {
        assert_eq!(s("- a\n* b"), "• a\n• b");
    }

    #[test]
    fn code_fence_and_inline_code() {
        assert_eq!(s("用 `x` 和\n```rust\nlet a=1;\n```\n结束"), "用 x 和\nlet a=1;\n结束");
    }

    #[test]
    fn table_rows_flattened() {
        assert_eq!(
            s("| 名称 | 价格 |\n|---|---|\n| 猪肘 | 20 |"),
            "名称  |  价格\n猪肘  |  20"
        );
    }

    #[test]
    fn link_keeps_text_and_url() {
        assert_eq!(s("[官网](https://a.com)"), "官网 (https://a.com)");
    }

    #[test]
    fn quote_prefix_removed() {
        assert_eq!(s("> 引用一\n> 引用二\n正文"), "引用一\n引用二\n正文");
    }

    #[test]
    fn plain_text_untouched() {
        assert_eq!(s("2*3=6,a * b"), "2*3=6,a * b");
        assert_eq!(s("普通一句话。"), "普通一句话。");
    }
}

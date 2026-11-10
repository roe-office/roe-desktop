//! 流式错误的用户友好文案:把 rig 的 MaxTurnsError 英文技术话换成带「回复继续」的中文提示。
//! 纯函数,单测覆盖;runtime.rs 的 Err 分支只做调用。

/// 输入为错误的 Display 文本;输出为发给 UI 的完整文案(含 [错误] 前缀或友好提示)。
pub(crate) fn friendly_stream_error(err: &str) -> String {
    if err.contains("MaxTurnsError") {
        return "本次任务已达到单次调用上限,已完成的部分已保留(若已保存,可在项目目录查看)。\
                回复「继续」可接着完成。"
            .to_string();
    }
    format!("[错误] {err}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_turns_is_friendly() {
        let s = friendly_stream_error("MaxTurnsError: reached max turns limit: 24");
        assert!(s.contains("回复「继续」"));
        assert!(!s.contains("MaxTurnsError"));
    }

    #[test]
    fn max_turns_with_wrapper_prefix() {
        let s = friendly_stream_error("PromptError: MaxTurnsError: reached max turns limit: 8");
        assert!(s.contains("回复「继续」"));
    }

    #[test]
    fn other_errors_keep_original() {
        let s = friendly_stream_error("HttpError: 400 Bad Request");
        assert_eq!(s, "[错误] HttpError: 400 Bad Request");
    }

    #[test]
    fn friendly_text_mentions_saved_progress() {
        let s = friendly_stream_error("MaxTurnsError: reached max turns limit: 24");
        assert!(s.contains("上限"));
    }
}

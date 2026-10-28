//! 剩余轮数提示 hook:在工具结果末尾追加一个 Text 块,告诉模型还能调用几次模型。
//! max_turns 是模型调用总预算;第 N 轮工具结果之后,模型还剩 max-N 次调用(含最终回复)。
//! 纯函数 budget_note / append_note 单测覆盖,hook 只做薄壳。

use rig_agent::agent::{AgentHook, HookContext, ToolResultAction, ToolResultEvent};
use rig_core::tool::ToolOutput;

/// 按剩余调用次数生成提示;剩余过多或为 0(模型看不到)时返回 None。
pub(crate) fn budget_note(turn: usize, max_turns: usize) -> Option<String> {
    let remaining = max_turns.saturating_sub(turn);
    match remaining {
        0 => None,
        1..=3 => Some(format!(
            "[系统提示] 本次任务还可调用模型 {remaining} 次(含最终回复)。\
             请立即停止新的探测:若已有文档内容,先调用 save_document 留下产物,\
             然后直接汇报已完成和未完成的部分,不要声称未执行过的步骤已成功。若任务未完成,告知用户回复「继续」即可接着完成。"
        )),
        4..=8 => Some(format!(
            "[系统提示] 本次任务还可调用模型 {remaining} 次(含最终回复)。\
             请把剩余工作拆成更小的步骤,每次只推进一个可验证的目标,不要再写长循环或大脚本。"
        )),
        _ => None,
    }
}

/// 在原输出的所有块之后追加一个 Text 块;重建失败返回 None(调用方保持原样)。
pub(crate) fn append_note(presentation: &ToolOutput, note: &str) -> Option<ToolOutput> {
    let mut blocks = presentation.as_content().to_vec();
    blocks.extend(ToolOutput::text(note).into_content());
    ToolOutput::content(blocks).ok()
}

pub struct TurnBudgetHook {
    max_turns: usize,
}

impl TurnBudgetHook {
    pub fn new(max_turns: usize) -> Self {
        Self { max_turns }
    }
}

impl AgentHook for TurnBudgetHook {
    async fn on_tool_result(
        &self,
        ctx: &HookContext,
        event: ToolResultEvent<'_>,
    ) -> ToolResultAction {
        let Some(note) = budget_note(ctx.turn(), self.max_turns) else {
            return ToolResultAction::Keep;
        };
        match append_note(event.presentation, &note) {
            Some(out) => ToolResultAction::rewrite_output(out),
            None => ToolResultAction::Keep,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plenty_left_no_note() {
        assert!(budget_note(15, 24).is_none()); // 剩 9
        assert!(budget_note(1, 24).is_none());
    }

    #[test]
    fn mid_budget_suggests_small_steps() {
        let n = budget_note(16, 24).expect("剩 8 应有提示"); // 剩 8
        assert!(n.contains('8'));
        assert!(!n.contains("save_document"));
        assert!(budget_note(20, 24).is_some()); // 剩 4
    }

    #[test]
    fn low_budget_demands_save_and_report() {
        let n = budget_note(21, 24).expect("剩 3 应有提示"); // 剩 3
        assert!(n.contains('3'));
        assert!(n.contains("save_document"));
        assert!(n.contains("继续"));
        assert!(budget_note(23, 24).expect("剩 1").contains('1'));
    }

    #[test]
    fn exhausted_or_overrun_no_note() {
        assert!(budget_note(24, 24).is_none());
        assert!(budget_note(30, 24).is_none());
    }

    #[test]
    fn append_keeps_json_block_and_adds_text() {
        let out = ToolOutput::json(serde_json::json!({"a": 1}));
        let new = append_note(&out, "提示").expect("应能重建");
        let blocks = new.as_content();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0], out.as_content()[0]);
        assert!(new.render().contains("提示"));
    }
}

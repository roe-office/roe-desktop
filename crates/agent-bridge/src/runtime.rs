//! tokio runtime 生命周期管理 + mpsc channel 建立 +
//! slint::invoke_from_event_loop 事件泵封装。
//!
//! 本文件是 agent-bridge 中唯一依赖 slint/tokio/roe-agent 三方的
//! "脏"文件，故意保持小而集中，不与 context.rs / event.rs 的纯
//! 数据定义混在一起。
//!
//! ============================================================
//! 本版本说明（正式接入 roe-agent，取代此前的骨架验证版）
//! ============================================================
//! 骨架验证阶段已确认 UI -> mpsc -> 后台 tokio task ->
//! invoke_from_event_loop -> UI 回环本身可用（真机验证通过，见
//! roe-ext_progress_v202.md）。本版本在此基础上接入真实的
//! roe-agent/rig 调用逻辑。
//!
//! 关键设计点（对应 v202 会话"二节"与用户/Gemini 讨论结论）：
//!
//! 1. handle 缺失拦截：UserQuery 到达时第一时间检查
//!    agent_context.get_active_doc()，无活动文档直接回提示，不发给
//!    LLM（省一次 token/耗时，零延迟反馈）。v1 阶段简化处理：无脑
//!    要求必须有文档，不做"通用提问是否需要文档上下文"的启发式
//!    判断。
//!
//! 2. client/registry/tool/query_tool/history 的生命周期：在
//!    spawn() 内部、async move 块顶部（接收循环之前）构造一次，
//!    贯穿整个后台 task 生命周期。history 用普通 Vec（无需
//!    Arc<Mutex<...>>，只有这一个 task 访问）。对应设计稿六节
//!    "Session 与 handle 同生共死"。
//!
//! 3. handle 如何传给 Tool：Tool 自己持有 AgentContextProvider，
//!    call() 内部动态查询，LLM 完全不需要知道/传递 handle（已在
//!    roe-agent 侧落地，见 roe-agent/src/tool.rs）。因此 preamble
//!    不再嵌入任何 handle 值。
//!
//! 4. stream_prompt 的 .history(history.clone())：v1 阶段直接
//!    clone，不优化，办公场景对话轮数有限，拷贝成本可忽略。
//!
//! GUI 场景与 CLI 场景的关键差异——挂起确认的处理方式：
//! CLI 版本用阻塞式 loop 等待用户在终端输入 y/n。GUI 场景没有
//! "阻塞等待"这个概念，改为：
//! - 每轮 stream_prompt 处理完毕后，检查 registry.has_pending()，
//!   有新的挂起项则发送 ToolExecutionPending 事件，本轮到此为止，
//!   等待 UI 侧后续的 AgentAction::UserConfirm/UserDiscard。
//! - 如果收到 UserQuery 时已经存在挂起项（用户没有先处理完预览
//!   就又发了新消息），直接拒绝、回一条 ExecutionError 提示用户
//!   先处理当前挂起的预览，不混入新一轮对话——语义对应 CLI 版本
//!   "循环顶部检查 has_pending()，有则不进入正常对话分支"。
//! - UserConfirm/UserDiscard 处理完毕后，只回一条 ExecutionSuccess/
//!   ExecutionError 告知结果，不主动续接对话（与 CLI 版本一致：
//!   处理完 y/n 后 continue 回主循环顶部，不把"用户确认了"这件事
//!   继续讲给 LLM 听）。

use std::sync::Arc;

use futures::StreamExt;
use rig_agent::agent::MultiTurnStreamItem;
use rig_agent::prelude::*;
use rig_agent::streaming::StreamedAssistantContent;
use rig_core::providers::deepseek;

use roe_agent::context::AgentContextProvider;
use roe_agent::executor::ScriptExecutor;
use roe_agent::tool::{ExecuteScriptTool, QueryScriptTool};
use roe_agent::tool_doc::{
    CloseDocumentTool, ListWorkspaceTool, NewDocumentTool, OpenDocumentTool, SaveDocumentTool,
};
use roe_agent::worker::DocWorker;
use roe_agent::workspace::WorkspaceContext;

use crate::config::AgentConfig;
use crate::event::{AgentAction, AgentUiEvent};
use crate::saved_hook::SavedHook;
use crate::turn_budget_hook::TurnBudgetHook;

/// 临时方案（明确知道会被替换）：DEEPSEEK_API_KEY 目前硬编码从
/// roe 仓库根目录的 .env 读取。
///
/// 前台（office-rs2/slint-shell）本身不持有、不接触 key 的值——
/// dotenvy 把环境变量加载进当前进程后，key 只会被 roe-agent 内部
/// 的 deepseek::Client::from_env() 读取，slint-shell 侧代码全程
/// 不经手这个值。
///
/// 待办（用户已确认，本次不做）：后续会做前台配置 key 的功能，
/// key 存在配置文件里，届时这个硬编码路径推导要整体替换掉。
///
/// 路径推导使用 CARGO_MANIFEST_DIR（编译期常量，不受运行时可执行
/// 文件位置影响，详见 v201 会话记录"四节·4.2"的返工教训）：
///   CARGO_MANIFEST_DIR = .../office-rs2/crates/agent-bridge
///     -> parent() = .../office-rs2/crates
///     -> parent() = .../office-rs2
///     -> parent() = .../Projects
///   Projects/roe/.env
fn locate_and_load_dotenv() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));

    let projects_dir = manifest_dir
        .parent() // crates/
        .and_then(|p| p.parent()) // office-rs2/
        .and_then(|p| p.parent()); // Projects/

    let Some(projects_dir) = projects_dir else {
        eprintln!(
            "[agent-bridge] CARGO_MANIFEST_DIR 路径层级不足3层，无法推导 .env 位置: {}",
            manifest_dir.display()
        );
        return;
    };

    let env_path = projects_dir.join("roe").join(".env");

    match dotenvy::from_path(&env_path) {
        Ok(()) => {
            eprintln!("[agent-bridge] 已加载 .env: {}", env_path.display());
        }
        Err(e) => {
            eprintln!(
                "[agent-bridge] 加载 .env 失败（{}），DEEPSEEK_API_KEY 可能不可用: {e}",
                env_path.display()
            );
        }
    }
}

/// 持有 UI 线程与 Agent 后台 task 之间通信所需的句柄。
///
/// 字段：
/// - `sender`：UI 线程通过它把 AgentAction 发给后台 task。
/// - `_runtime`：独立的 tokio 多线程 Runtime，必须被 AgentRuntimeHandle
///   持有以防止被 drop（一旦 drop，spawn 出去的后台 task 会被终止）。
///   命名加下划线前缀是因为调用方不直接读它，只是让它活着。
#[derive(Clone)]
pub struct AgentRuntimeHandle {
    sender: mpsc::UnboundedSender<AgentAction>,
    _runtime: Arc<tokio::runtime::Runtime>,
}

use tokio::sync::mpsc;

impl AgentRuntimeHandle {
    /// 创建独立 tokio runtime，建立 channel，spawn 后台 task，
    /// 内部真正接入 roe-agent（deepseek client、两个 Tool、
    /// ShadowRegistry、多轮对话历史）。
    ///
    /// `on_event`：后台 task 每次要更新 UI 时，通过
    /// slint::invoke_from_event_loop 在 UI 线程调用这个闭包。
    /// 要求 `Send + Sync + 'static`；内部用 Arc 包裹以支持在循环里
    /// 反复 clone 后传给 invoke_from_event_loop（后者签名要求
    /// FnOnce）。
    ///
    /// `agent_context`：GUI 侧（main.rs）创建、在 on_open_file_clicked
    /// 等处调用 set_active_doc 维护的 AgentContextProvider。
    /// AgentRuntimeHandle 只是把这个 Provider 的 Clone 交给两个
    /// Tool，本身不负责维护其内容。
    pub fn spawn<F>(
        on_event: F,
        agent_context: AgentContextProvider,
        executor: Arc<dyn ScriptExecutor>,
    ) -> Self
    where
        F: Fn(AgentUiEvent) + Send + Sync + 'static,
    {
        Self::spawn_with(on_event, agent_context, None, executor)
    }

    /// 同 spawn，但可传入初始配置；None 时走 .env 兜底。
    pub fn spawn_with<F>(
        on_event: F,
        agent_context: AgentContextProvider,
        initial: Option<AgentConfig>,
        executor: Arc<dyn ScriptExecutor>,
    ) -> Self
    where
        F: Fn(AgentUiEvent) + Send + Sync + 'static,
    {
        Self::spawn_impl(on_event, agent_context, initial, executor, false)
    }

    /// Agent 页专用:不依赖前台文档,自己持有 DocWorker 新建/打开/保存文档(与 CLI 同一套工具)。
    pub fn spawn_headless<F>(on_event: F, initial: Option<AgentConfig>) -> Self
    where
        F: Fn(AgentUiEvent) + Send + Sync + 'static,
    {
        // headless 下 agent_context 与 executor 不会被用到,仅为满足内部签名
        Self::spawn_impl(
            on_event,
            AgentContextProvider::default(),
            initial,
            Arc::new(crate::ui_executor::UiExecutor::new()),
            true,
        )
    }

    fn spawn_impl<F>(
        on_event: F,
        agent_context: AgentContextProvider,
        initial: Option<AgentConfig>,
        executor: Arc<dyn ScriptExecutor>,
        headless: bool,
    ) -> Self
    where
        F: Fn(AgentUiEvent) + Send + Sync + 'static,
    {
        // .env 仅在无初始配置时兜底（见后台 task）

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("[agent-bridge] 创建 tokio runtime 失败");

        let (sender, mut receiver) = mpsc::unbounded_channel::<AgentAction>();
        let on_event = Arc::new(on_event);

        runtime.spawn(async move {
            eprintln!("[agent-bridge] 后台 task 已启动，等待 AgentAction...");

            // 以下对象在 async 块顶部构造一次，生命周期贯穿整个后台
            // task（对应 v202 会话"二节·2.2"决策）。

            // Agent workspace（ROE_Agent_Workspace_设计文档_v1.md 三、3.4）：
            // 后台 task 生命周期内构造一次，全程复用。physical_root 用
            // 用户主目录下固定路径，不绑定具体文档（这个 task 独立于
            // "当前打开哪个文档"的生命周期）。构造失败按本函数已有的
            // deepseek::Client::from_env() 降级先例处理：上报 UI + 提前
            // return，不阻塞、不 panic 主程序，agent 后台能力整体不可用。
            let workspace_root = crate::workspace_path::agent_workspace_root();

            let workspace = match WorkspaceContext::open_or_create(&workspace_root) {
                Ok(ctx) => Arc::new(ctx),
                Err(e) => {
                    eprintln!(
                        "[agent-bridge] 打开 agent workspace 失败（路径: {}）: {e}",
                        workspace_root.display()
                    );
                    emit_to_ui(
                        on_event.clone(),
                        AgentUiEvent::ExecutionError(format!(
                            "[agent-bridge] 初始化失败，无法打开 agent workspace: {e}"
                        )),
                    );
                    return;
                }
            };

            // headless:DocWorker 在后台 task 里只建一次(Reconfigure 重建 agent 时复用,打开的文档不丢)
            let worker: Option<Arc<DocWorker>> = if headless {
                match DocWorker::spawn(workspace.clone()) {
                    Ok(w) => Some(Arc::new(w)),
                    Err(e) => {
                        eprintln!("[agent-bridge] 启动文档工作线程失败: {e}");
                        emit_to_ui(
                            on_event.clone(),
                            AgentUiEvent::ExecutionError(format!("启动文档工作线程失败: {e}")),
                        );
                        return;
                    }
                }
            } else {
                None
            };

            // agent 构造：cfg=Some 用设置页配置；None 走 .env 兜底
            // （DEEPSEEK_API_KEY + DEEPSEEK_V4_FLASH）。当前仅接入 DeepSeek。
            let build_agent = |cfg: Option<&AgentConfig>| -> Result<_, String> {
                let (client, model) = match cfg {
                    Some(c) => {
                        if c.provider != 3 {
                            return Err("当前 Agent 仅接入 DeepSeek 服务商，其他服务商后续支持".to_string());
                        }
                        let key = c.key.trim().to_string();
                        let model = c.model.trim().to_string();
                        if key.is_empty() {
                            return Err("API Key 为空".to_string());
                        }
                        if model.is_empty() {
                            return Err("模型名为空".to_string());
                        }
                        if model.chars().any(|ch| ch.is_whitespace()) {
                            return Err(format!(
                                "模型名含空格：{model}（DeepSeek 模型名形如 deepseek-v4-pro）"
                            ));
                        }
                        let mut builder = deepseek::Client::builder().api_key(&key);
                        let ep = c.endpoint.trim();
                        if !ep.is_empty() {
                            builder = builder.base_url(ep);
                        }
                        let client = builder
                            .build()
                            .map_err(|e| format!("创建客户端失败: {e}"))?;
                        (client, model)
                    }
                    None => {
                        let client = deepseek::Client::from_env()
                            .map_err(|e| format!("创建客户端失败（.env 兜底）: {e}"))?;
                        (client, deepseek::DEEPSEEK_V4_FLASH.to_string())
                    }
                };
                if let Some(worker) = worker.as_ref() {
                    // 无头 Agent:7 个工具(不含 ingest_media_asset,宿主机文件只能经"+"号上传),文档由模型自己新建/打开/保存
                    return Ok(client
                        .agent(model.as_str())
                        .preamble(&roe_agent::prompt::headless_preamble())
                        .tool(ExecuteScriptTool::new(worker.clone()))
                        .tool(QueryScriptTool::new(worker.clone()))
                        .tool(NewDocumentTool::new(worker.clone()))
                        .tool(OpenDocumentTool::new(worker.clone(), workspace.clone()))
                        .tool(SaveDocumentTool::new(worker.clone(), workspace.clone()))
                        .tool(CloseDocumentTool::new(worker.clone()))
                        .tool(ListWorkspaceTool::new(worker.clone(), workspace.clone()))
                        .build());
                }
                let tool = ExecuteScriptTool::new(executor.clone());
                let query_tool = QueryScriptTool::new(executor.clone());
                Ok(client
                    .agent(model.as_str())
                    // 提示词与 CLI 共用 roe_agent::prompt(规则只写一份);GUI 版不告诉模型文档句柄,
                    // 句柄由 Tool 内部经 AgentContextProvider 获取。
                    .preamble(&roe_agent::prompt::gui_preamble())
                    .tool(tool)
                    .tool(query_tool)
                    .build())
            };

            let mut agent = match initial {
                Some(cfg) => match build_agent(Some(&cfg)) {
                    Ok(a) => {
                        eprintln!("[agent-bridge] agent 已构造: {cfg:?}");
                        Some(a)
                    }
                    Err(e) => {
                        eprintln!("[agent-bridge] agent 构造失败: {e}");
                        // 启动时配置不完整不算错误（红灯已表达）；完整但构造失败才提示
                        if !(cfg.key.trim().is_empty() || cfg.model.trim().is_empty()) {
                            emit_to_ui(
                                on_event.clone(),
                                AgentUiEvent::ConfigError(format!("AI 配置无法使用：{e}")),
                            );
                        }
                        None
                    }
                },
                None => {
                    locate_and_load_dotenv();
                    match build_agent(None) {
                        Ok(a) => Some(a),
                        Err(e) => {
                            eprintln!("[agent-bridge] .env 兜底构造失败: {e}");
                            None
                        }
                    }
                }
            };

            let mut history: Vec<rig_core::message::Message> = Vec::new();

            while let Some(action) = receiver.recv().await {
                match action {
                    AgentAction::UserQuery(text) => {
                        eprintln!("[agent-bridge] 收到 UserQuery: {text}");

                        // 第一时间拦截：无活动文档直接回提示，不发给
                        // LLM（对应 v202 会话"二节·2.1"）。
                        if !headless
                            && agent_context
                            .get_active_doc()
                            .map(|s| s.raw_handle == 0)
                            .unwrap_or(true)
                        {
                            emit_to_ui(
                                on_event.clone(),
                                AgentUiEvent::ExecutionError(
                                    "当前未打开文档，请先打开或新建文档。".to_string(),
                                ),
                            );
                            continue;
                        }

                        let Some(active_agent) = agent.as_ref() else {
                            emit_to_ui(
                                on_event.clone(),
                                AgentUiEvent::ExecutionError(
                                    "尚未配置可用的 API Key 或模型，请点击右上角「去设置」添加。"
                                        .to_string(),
                                ),
                            );
                            continue;
                        };

                        emit_to_ui(on_event.clone(), AgentUiEvent::ThinkingStatus(true));

                        let mut response_stream = active_agent
                            .stream_prompt(text.clone())
                            .history(history.clone())
                            .max_turns(GUI_AGENT_MAX_TURNS)
                            .add_hook(TurnBudgetHook::new(GUI_AGENT_MAX_TURNS))
                            .add_hook(SavedHook::new({
                                let on_event = on_event.clone();
                                move |path| {
                                    emit_to_ui(on_event.clone(), AgentUiEvent::FileSaved(path))
                                }
                            }))
                            .await;

                        let mut acc = String::new();
                        let mut new_messages = None;

                        loop {
                            let Some(chunk) = response_stream.next().await else {
                                break;
                            };
                            match chunk {
                                Ok(MultiTurnStreamItem::StreamAssistantItem(
                                    StreamedAssistantContent::Text(text_item),
                                )) => {
                                    acc.push_str(&text_item.text);
                                    emit_to_ui(
                                        on_event.clone(),
                                        AgentUiEvent::TextDelta(text_item.text),
                                    );
                                }
                                Ok(MultiTurnStreamItem::FinalResponse(final_response)) => {
                                    new_messages = final_response.messages().map(|h| h.to_vec());
                                }
                                Err(e) => {
                                    eprintln!("[agent-bridge] stream_prompt 错误: {e}");
                                    emit_to_ui(
                                        on_event.clone(),
                                        AgentUiEvent::ExecutionError(format!("[错误] {e}")),
                                    );
                                    break;
                                }
                                _ => continue,
                            }
                        }

                        emit_to_ui(on_event.clone(), AgentUiEvent::ThinkingStatus(false));

                        if let Some(messages) = new_messages {
                            history.extend(messages);
                        } else if !acc.is_empty() {
                            history.push(rig_core::message::Message::user(text));
                            history.push(rig_core::message::Message::assistant(acc));
                        }

                    }
                    AgentAction::Reconfigure(cfg) => {
                        eprintln!("[agent-bridge] 收到 Reconfigure: {cfg:?}");
                        match build_agent(Some(&cfg)) {
                            Ok(a) => {
                                agent = Some(a);
                                eprintln!("[agent-bridge] agent 已重建（对话历史保留）");
                            }
                            Err(e) => {
                                // 配置不完整 = 用户主动清除，回到未配置；
                                // 配置完整但构造失败 = 误操作，保留旧 agent。
                                let cleared = cfg.key.trim().is_empty()
                                    || cfg.model.trim().is_empty();
                                let keep_old = !cleared && agent.is_some();
                                if !keep_old {
                                    agent = None;
                                }
                                if cleared {
                                    // 主动清除/未配置完整不是错误，只打日志，红灯已表达状态
                                    eprintln!("[agent-bridge] 配置不完整，agent 已清除: {e}");
                                } else {
                                    eprintln!(
                                        "[agent-bridge] agent 重建失败（保留旧 agent: {keep_old}）: {e}"
                                    );
                                    let msg = if keep_old {
                                        format!("AI 配置无法使用，仍在使用之前的配置：{e}")
                                    } else {
                                        format!("AI 配置无法使用：{e}")
                                    };
                                    emit_to_ui(on_event.clone(), AgentUiEvent::ConfigError(msg));
                                }
                            }
                        }
                    }
                    // 确认流程已移除(脚本直接生效,可撤销);变体暂留,UI 清理后删除。
                    AgentAction::UserConfirm(_) | AgentAction::UserDiscard(_) => {}
                    AgentAction::ResetHistory => {
                        history.clear();
                    }
                }
            }
            eprintln!("[agent-bridge] channel 已关闭，后台 task 退出");
        });

        Self {
            sender,
            _runtime: Arc::new(runtime),
        }
    }

    /// UI 线程调用：把用户动作发给后台 task。
    /// channel 已关闭（后台 task 异常退出）时只打印错误，不 panic，
    /// 避免因为 agent 后台的问题打断整个 UI 程序。
    pub fn send_action(&self, action: AgentAction) {
        if let Err(e) = self.sender.send(action) {
            eprintln!("[agent-bridge] send_action 失败（后台 task 可能已退出）: {e}");
        }
    }
}

/// 把一个 AgentUiEvent 通过 invoke_from_event_loop 送回 UI 线程。
/// 接收 Arc<F> 而非 &F：invoke_from_event_loop 要求传入的闭包是
/// 'static + Send，必须拥有独立所有权，不能借用调用方栈上的引用；
/// 用 Arc clone 满足这一点，同时允许 on_event 本体在后台 task
/// 循环里被多次复用。
fn emit_to_ui<F>(on_event: Arc<F>, event: AgentUiEvent)
where
    F: Fn(AgentUiEvent) + Send + Sync + 'static,
{
    let result = slint::invoke_from_event_loop(move || {
        on_event(event);
    });
    if let Err(e) = result {
        eprintln!("[agent-bridge] invoke_from_event_loop 失败（UI 事件循环可能已退出）: {e}");
    }
}


/// GUI Agent 单次对话的最大工具调用轮数。
/// Impress 没有只读通道,探测(list_functions/inspect_presentation)与修改都占轮次,
/// 原值 12 会在多步任务里触发 MaxTurnsError;CLI(roe-agent/src/main.rs)仍为 5,互不影响。
pub(crate) const GUI_AGENT_MAX_TURNS: usize = 24;

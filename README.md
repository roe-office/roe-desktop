# roe-desktop

Office 套件 UI 前台工程（第二代技术方案）。

基于 slint + winit 构建 UI 交互层，通过路径依赖消费同机的
[`roe`](../roe) 无头内核获取文档模型、公式计算、渲染等核心能力。

> 注：仓库内还有一个 `office-rs`（无 `2` 后缀）项目，是基于
> LibreOfficeKit + Qt6 + QML 的第一代技术方案，已归档，不再活跃。

## 技术栈

### roe-desktop 自身
- **slint** (1.16.1) — UI 渲染框架，跨平台、Rust 原生
- **winit**（经 `i-slint-backend-winit`）— 窗口/事件后端
- **polars** — 透视表数据处理（`crates/pivot-engine`）
- **calamine** / **rust_xlsxwriter** — xlsx 读/写
- **rfd** — 原生文件对话框
- **lru** — tile 缓存淘汰策略（`crates/tile-cache`）

### 依赖 roe 内核提供的能力（间接依赖，见 [`roe`](../roe)）
- **formualizer** — Excel/OpenFormula 公式解析与求值
- **petgraph** — 公式依赖图（Calc FormulaDAG）
- **rstar** — 空间索引（Impress viewport culling）
- **parley** / **taffy** / **skrifa** / **swash** — 文字排版、图文混排、字体
- **ratex**（RaTeX）— LaTeX 公式排版
- **charts-rs** — 图表渲染
- **tiny-skia** / **resvg** — 2D 光栅渲染
- **quick-xml** — OOXML 解析

roe 内核还提供 Agent 相关能力（基于 **rig**，LLM agent 框架，见
`roe/roe-agent`），本仓库暂未接入。

## 仓库结构
```
roe-desktop/
├── Cargo.toml          # workspace
├── crates/
│   ├── doc-core/        # 文档会话/状态管理，依赖 roe-api
│   ├── pivot-engine/     # 透视表引擎（Polars），亦被 roe-polars-bridge 复用
│   ├── slint-shell/      # 前台 UI 主体（ui/*.slint + src/main.rs）
│   └── tile-cache/       # 画布 tile 缓存
├── assets/
├── build.rs
└── docs/
```

## 编译
待补充

## 已知问题
见 `docs/` 下的反馈记录，以及历史踩坑记录（如有）。

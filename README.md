<div align="center">

# roe-desktop

**Modern, Native Graphical Desktop Shell for the ROE Office Suite**  
*Built with Rust & Slint UI • Ultra-Lightweight • Tile-Cached Canvas • Multi-Language*

[![License: AGPL v3](https://img.shields.io/badge/License-AGPLv3-blue.svg)](https://www.gnu.org/licenses/agpl-3.0)
[![Built with Rust](https://img.shields.io/badge/Rust-1.80%2B-orange.svg?logo=rust)](https://www.rust-lang.org/)
[![UI Framework](https://img.shields.io/badge/UI-Slint%201.16-purple.svg)](https://slint.dev/)
[![Engine Core](https://img.shields.io/badge/Engine-roe--core-green.svg)](https://github.com/roe-office/roe-core)

</div>

---

## 📖 Overview

`roe-desktop` is the official cross-platform desktop frontend for **ROE Office (Rust Office Engine)**. 

Built natively with **Rust** and the modern **Slint** GUI toolkit, it delivers a fluid, instant-startup user experience with a minimal footprint (~150MB baseline RAM). It completely decouples UI rendering and user interactions from document layout computations by consuming the headless [`roe-core`](https://github.com/roe-office/roe-core) kernel.

> **Historical Note**: This represents the second-generation (V2) architecture. The legacy V1 prototype (based on LibreOfficeKit + Qt6 + QML) has been permanently retired and archived under [`office-lok`](https://github.com/roe-office/office-lok).

---

## ⚡ Highlights & Key Features

* **🎨 Pure Slint + Winit Shell**: Declarative, lightweight UI markup powered by modern GPU/software rendering pipelines.
* **🖼️ Smooth Tile-Cached Viewport**: Implements an LRU-based tile canvas cache (`crates/tile-cache`) for smooth 60fps panning and scrolling across large documents and multi-slide decks.
* **📊 In-Memory Polars Pivot Engine**: Interactive spreadsheet pivot tables powered by the columnar `polars` engine (`crates/pivot-engine`).
* **🌐 Multilingual by Default**: Native out-of-the-box UI localization across **7 languages** (Simplified Chinese, Traditional Chinese, English, Japanese, Korean, French, German, and Spanish).
* **🪟 Native System Integration**: Seamless platform file dialogs via `rfd` and native window lifecycle management.

---

## 🏗️ Repository Architecture

The workspace is structured into dedicated modular crates:

```text
roe-desktop/
├── Cargo.toml            # Workspace orchestration
├── crates/
│   ├── slint-shell/      # Main application binary entry (Slint markup + runtime state)
│   ├── doc-core/         # Document session state machine (bridges to roe-api / roe-core)
│   ├── pivot-engine/     # Standalone Polars-based DataFrame pivot computation
│   └── tile-cache/       # High-performance LRU canvas tile cache
├── assets/               # Bundled application icons and font glyphs
└── scripts/              # Asset packaging and build helpers
```

## 🛠️ Technology Stack
Layer	Component	Description
UI Presentation	slint (1.16)	Next-generation declarative GUI framework
Window Backend	winit	Cross-platform event-loop and windowing
Data Engine	polars	Fast columnar analytics for spreadsheet aggregations
File I/O	rfd, calamine, rust_xlsxwriter	Native file dialogs & spreadsheet interchange
Kernel Engine	roe-core	External headless kernel (layout, typography, formula DAG, math)
🚀 Building & Running from Source
Prerequisites

 -   Recent stable Rust toolchain (1.80+ recommended).

 -  Platform dependencies (Linux: fontconfig, libx11 / wayland-client dev headers).

 -   Ensure the sibling kernel roe-core repository is present if developing via local path dependencies.

## Quick Start

```bash
# 1. Check workspace compilation
cargo check --workspace

# 2. Run the desktop application
cargo run -p slint-shell
```

## 🎯 Current Status (Preview 0.1)

roe-desktop is in active preview development. Core editing for documents, spreadsheets, and slide decks is functional, but advanced edge-case styling, complex dialog settings, and extended keyboard shortcuts are being refined.

If you encounter layout glitches or visual inconsistencies, please feel free to submit an issue via GitHub Issues.

## 📜 Licensing

roe-desktop is licensed under the GNU Affero General Public License v3.0 (AGPLv3).
For special entity commercial exemptions (Anthropic, Alphabet/Google, DeepSeek, and Public Good NGOs), please refer to the organization-level Licensing Declaration.

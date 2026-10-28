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

#!/usr/bin/env python3
"""从 roe 的 oo_icon_paths.rs 生成:
  crates/slint-shell/ui/IconSetGlyphs.slint   (IconGlyph 矢量图标 + CfIconSetPicker 20 项选择器)
  crates/slint-shell/src/ribbon/cf_icon_styles.rs (ICON_STYLES,顺序与 Slint 格子下标一致)
用法: python3 scripts/gen_icon_glyphs.py [oo_icon_paths.rs 路径]
roe 改了图标数据后重跑即可。生成文件请勿手改。"""
import re, sys, pathlib

ROOT = pathlib.Path(__file__).resolve().parent.parent
SRC = pathlib.Path(sys.argv[1]) if len(sys.argv) > 1 else pathlib.Path.home() / "Projects/roe/roe-core/src/oo_icon_paths.rs"
OUT_SLINT = ROOT / "crates/slint-shell/ui/IconSetGlyphs.slint"
OUT_RS = ROOT / "crates/slint-shell/src/ribbon/cf_icon_styles.rs"

# Excel 菜单布局:(组名, [(左, 右或None), ...]),下标 = 行优先、先左后右
GROUPS = [
    ("方向", [("3Arrows", "3ArrowsGray"), ("3Triangles", "4ArrowsGray"), ("4Arrows", "5ArrowsGray"), ("5Arrows", None)]),
    ("形状", [("3TrafficLights1", "3TrafficLights2"), ("3Signs", "4TrafficLights"), ("4RedToBlack", None)]),
    ("标记", [("3Symbols", "3Symbols2"), ("3Flags", None)]),
    ("等级", [("3Stars", "4Rating"), ("5Quarters", "5Rating"), ("5Boxes", None)]),
]

src = SRC.read_text(encoding="utf-8")

# ---- 图标层 ----
layer_re = re.compile(
    r'\(\s*"([^"]*)"\s*,\s*(None|Some\("([^"]*)"\))\s*,\s*(None|Some\(\(\s*"([^"]*)"\s*,\s*([0-9.]+)_f32\s*\)\))\s*\)')
icons = {}
for m in re.finditer(r'pub const ICON_(\d+): &\[IconPath\] = &\[(.*?)\];\n', src, re.S):
    layers = []
    for lm in layer_re.finditer(m.group(2)):
        d, fill, stroke = lm.group(1), lm.group(3), None
        if lm.group(5):
            stroke = (lm.group(5), float(lm.group(6)))
        layers.append((d, fill, stroke))
    assert layers, f"ICON_{m.group(1)} 解析不到任何层"
    icons[int(m.group(1))] = layers
assert len(icons) == 50, f"应解析到 50 个图标,实际 {len(icons)}"

# ---- 预设 → 图标 id(低档在前)----
tail = src[src.index("pub fn preset_icon_ids"):]
presets = {}
for m in re.finditer(r'"(\w+)"\s*=>\s*&\[([\d,\s]+)\]', tail):
    presets[m.group(1)] = [int(x) for x in m.group(2).split(",") if x.strip()]

def col(c, alpha=1.0):
    named = {"white": "#ffffff", "black": "#000000"}
    c = named.get(c.lower(), c.lower())
    assert re.fullmatch(r"#[0-9a-f]{6}", c), f"不认识的颜色 {c}"
    if alpha < 1.0:
        c += "%02x" % round(alpha * 255)
    return c

styles = []
used = set()
for _, rows in GROUPS:
    for left, right in rows:
        for s in (left, right):
            if s:
                assert s in presets, f"预设 {s} 不在 preset_icon_ids"
                styles.append(s)
                used.update(presets[s])
assert len(styles) == 20, len(styles)

L = []
A = L.append
A("// IconSetGlyphs.slint — 由 scripts/gen_icon_glyphs.py 生成,请勿手改。")
A("// 图标数据来自 roe-core/src/oo_icon_paths.rs(viewBox 16x16)。")
A("")

# ---- 选择器布局计算 ----
TITLE_H, ROW_H, GROUP_GAP, COL_W = 22, 28, 6, 190
y = 0
cells = []   # (idx, x, y, ids_high_first)
titles = []  # (name, y)
idx = 0
for gi, (gname, rows) in enumerate(GROUPS):
    titles.append((gname, y))
    y += TITLE_H + 2
    for left, right in rows:
        for ci, s in enumerate((left, right)):
            if s:
                cells.append((idx, ci * COL_W, y, list(reversed(presets[s]))))
                idx += 1
        y += ROW_H
    y += GROUP_GAP
H = y - GROUP_GAP

A("export global CfIconSetMetrics {")
A(f"    out property <length> picker-height: {H}px;")
A("}")
A("")
A("export component IconGlyph inherits Rectangle {")
A("    in property <int> icon-id: 0;")
A("    in property <length> glyph-size: 16px;")
A("    width: root.glyph-size;")
A("    height: root.glyph-size;")
A("    background: transparent;")
for i in sorted(used):
    for d, fill, stroke in icons[i]:
        parts = ["x: 0px; y: 0px; width: parent.width; height: parent.height; viewbox-width: 16; viewbox-height: 16;",
                 f'commands: "{d}";']
        if fill:
            parts.append(f"fill: {col(fill)};")
        if stroke:
            parts.append(f"stroke: {col(stroke[0], stroke[1])}; stroke-width: root.glyph-size / 16;")
        A(f"    if root.icon-id == {i} : Path {{ " + " ".join(parts) + " }")
A("}")
A("")
A("""component IconSetCell inherits Rectangle {
    in property <[int]> ids: [];
    in property <bool> selected: false;
    callback clicked();
    width: 190px;
    height: 28px;
    border-radius: 3px;
    border-width: 1px;
    border-color: root.selected ? #217346 : transparent;
    background: root.selected ? #e3f1e8 : (cell-touch.has-hover ? #f0f0f0 : transparent);
    for id[i] in root.ids : IconGlyph {
        x: 10px + i * 21px;
        y: (parent.height - self.height) / 2;
        icon-id: id;
        glyph-size: 16px;
    }
    cell-touch := TouchArea { clicked => { root.clicked(); } }
}
""")
A("export component CfIconSetPicker inherits Rectangle {")
A("    in property <int> selected-index: -1;")
A("    callback picked(int);")
A(f"    width: {2 * COL_W}px;")
A(f"    height: {H}px;")
A("    background: transparent;")
for name, ty in titles:
    A(f'    Rectangle {{ x: 0px; y: {ty}px; width: {2 * COL_W}px; height: {TITLE_H}px; background: #eef0f3;')
    A(f'        Text {{ x: 10px; height: parent.height; text: "{name}"; font-size: 12px; color: #444444; vertical-alignment: center; }} }}')
for i, x, cy, ids in cells:
    A(f"    IconSetCell {{ x: {x}px; y: {cy}px; ids: {ids}; selected: root.selected-index == {i}; clicked => {{ root.picked({i}); }} }}")
A("}")
OUT_SLINT.write_text("\n".join(L) + "\n", encoding="utf-8")

rs = ["// 由 scripts/gen_icon_glyphs.py 生成,请勿手改。顺序与 CfIconSetPicker 格子下标一致。",
      "pub const ICON_STYLES: [&str; 20] = ["]
rs += [f'    "{s}",' for s in styles]
rs.append("];")
OUT_RS.write_text("\n".join(rs) + "\n", encoding="utf-8")
print(f"ok: {len(used)} 个图标, 选择器高度 {H}px")
for i, s in enumerate(styles):
    print(i, s)

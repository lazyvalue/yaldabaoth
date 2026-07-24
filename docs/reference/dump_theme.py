#!/usr/bin/env python3
import re, sys

SRC = "/Users/scott/ws/yaldabaoth/src/theme.rs"
OUT = "/Users/scott/ws/yaldabaoth/docs/reference/colorscheme.md"

lines = open(SRC).read().splitlines()

TARGET_IMPLS = {"Theme", "AgentTheme", "OverlayTheme"}
THEME_ORDER = ["dracula", "nightfox", "solarized_light", "solarized_dark",
               "gruvbox_dark", "financial_times", "financial_times_dark", "folio"]
THEME_TITLE = {
    "dracula": "Dracula (default dark)",
    "nightfox": "Nightfox",
    "solarized_light": "Solarized Light",
    "solarized_dark": "Solarized Dark",
    "gruvbox_dark": "Gruvbox Dark",
    "financial_times": "Financial Times (light)",
    "financial_times_dark": "Financial Times Dark",
    "folio": "Folio",
}
IMPL_TITLE = {
    "Theme": "Editor / document",
    "AgentTheme": "Agent (Claude chat surface)",
    "OverlayTheme": "Overlay / menus",
}

color_re = re.compile(r'(?:\.(fg|bg)\()?Color::Rgb\(\s*(\w+)\s*,\s*(\w+)\s*,\s*(\w+)\s*\)')
field_re = re.compile(r'^\s*([a-zA-Z_]\w*):\s')
impl_re = re.compile(r'^impl\s+(\w+)')
fn_re = re.compile(r'^\s*pub fn (\w+)\(\)\s*->\s*Self')

def hexof(a, b, c):
    r, g, bl = int(a, 0), int(b, 0), int(c, 0)
    return f"#{r:02X}{g:02X}{bl:02X}"

# data[theme][impl] = list of (field, role, hex)
data = {t: {i: [] for i in TARGET_IMPLS} for t in THEME_ORDER}

cur_impl = None
in_fn = None
depth = 0
cur_field = None

for ln in lines:
    m = impl_re.match(ln)
    if m:
        cur_impl = m.group(1)
        in_fn = None
        continue
    if in_fn is None:
        fm = fn_re.match(ln)
        if fm and cur_impl in TARGET_IMPLS and fm.group(1) in THEME_ORDER:
            in_fn = fm.group(1)
            depth = ln.count("{") - ln.count("}")
            cur_field = None
        continue
    # inside a target fn body
    fld = field_re.match(ln)
    if fld:
        cur_field = fld.group(1)
    for cm in color_re.finditer(ln):
        role = cm.group(1) or ""
        h = hexof(cm.group(2), cm.group(3), cm.group(4))
        data[in_fn][cur_impl].append((cur_field, role, h))
    depth += ln.count("{") - ln.count("}")
    if depth <= 0:
        in_fn = None

# ---- emit ----
out = []
out.append("# Yalda color scheme — full dump\n")
out.append("Every color literal in `src/theme.rs`, per theme, extracted straight "
           "from source (so it stays exact). Regenerate with `/tmp/dump_theme.py` "
           "if the themes change.\n")
out.append("Three layers per theme: **Editor/document** (`Theme` — markdown "
           "render + chrome, colors decimal in source), **Agent** (`AgentTheme` — "
           "the Claude chat surface), **Overlay** (`OverlayTheme` — menus/pickers). "
           "`fg`/`bg` marks the role within a `Style`; a blank role means the field "
           "*is* that color.\n")
out.append(f"Themes: {', '.join(THEME_TITLE[t] for t in THEME_ORDER)}.\n")

for t in THEME_ORDER:
    out.append(f"\n## {THEME_TITLE[t]}\n")
    for impl in ("Theme", "AgentTheme", "OverlayTheme"):
        rows = data[t][impl]
        if not rows:
            continue
        out.append(f"\n### {IMPL_TITLE[impl]}\n")
        out.append("| Field | Role | Color |")
        out.append("|-------|------|-------|")
        # index repeated fields (heading[0..5])
        seen = {}
        for field, role, h in rows:
            key = field
            seen[field] = seen.get(field, -1) + 1
            label = field
            # only index fields that repeat (heading array)
            cnt = sum(1 for f, _, _ in rows if f == field)
            if cnt > 1:
                label = f"{field}[{seen[field]}]"
            out.append(f"| `{label}` | {role} | `{h}` |")
        out.append("")

open(OUT, "w").write("\n".join(out) + "\n")
print(f"wrote {OUT}")
print(f"themes={len(THEME_ORDER)} "
      f"colors={sum(len(data[t][i]) for t in THEME_ORDER for i in TARGET_IMPLS)}")

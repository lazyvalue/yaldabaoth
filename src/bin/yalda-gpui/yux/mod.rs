//! # yux — yalda's reusable UX component layer over GPUI
//!
//! Every UX surface in `yalda-gpui` is built from yux. It owns two things:
//!
//! 1. **The render-skip infrastructure** (`cached`) — `cached_child`, the
//!    `record_render`/`record_notify` perf counters, and `MissReason`. This is
//!    the one lever that keeps typing latency O(changed), not O(whole tree).
//! 2. **Reusable view primitives** (`detail`) — `DetailStyle` + the
//!    domain-free building blocks (`multiline_text`, `kv_row`,
//!    `section_heading`, `compact_tab`, `compact_count_indicator`,
//!    `compact_list_group_heading`,
//!    `single_line_ellipsis`, `compact_status_mark`, `compact_bounded_group`,
//!    `context_menu_item`,
//!    `picker_option_row`, `picker_option_row_detailed`, `note_block`,
//!    `fmt_iso_datetime`) that any surface composes from.
//! 3. **Virtualized scroll surfaces** (`list`) — `ScrollAnchoredList`, the one
//!    place the "splice the changed range, never `reset()`" reconcile lives, so
//!    no scroll surface re-derives it (or re-introduces the jump-to-top bug) —
//!    and `list_rows_overlay`, an uncached element painted over a span of a
//!    uniform-row list's rows (inline inputs inside a cached list).
//! 4. **Display text** (`display_text`) — the one document-line → rendered
//!    string projection (newline-trimmed, tab-expanded) and the raw→display
//!    column mapper every caret/selection painter must go through.
//! 5. **Single-line text input** (`line_input`) — `LineInput` (the lib-crate
//!    model: text + caret + one key policy) and `LINE_INPUT_CARET`. Every
//!    query / filter / rename field is one; never hand-roll `push`/`pop`.
//!
//! Read `yux/CLAUDE.md` before adding to it: it states the rules (state
//! encapsulation, the never-notify-in-render law, the render-count test) and
//! the contribution mandate — **all UX work lives here or is built from here.**

mod cached;
mod detail;
mod display_text;
mod line_input;
mod list;

pub(crate) use cached::*;
pub(crate) use detail::*;
pub(crate) use display_text::*;
pub(crate) use line_input::*;
pub(crate) use list::*;

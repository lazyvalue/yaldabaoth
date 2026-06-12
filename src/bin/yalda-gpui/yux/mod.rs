//! # yux — yalda's reusable UX component layer over GPUI
//!
//! Every UX surface in `yalda-gpui` is built from yux. It owns two things:
//!
//! 1. **The render-skip infrastructure** (`cached`) — `cached_child`, the
//!    `record_render`/`record_notify` perf counters, and `MissReason`. This is
//!    the one lever that keeps typing latency O(changed), not O(whole tree).
//! 2. **Reusable view primitives** (`detail`) — `DetailStyle` + the
//!    domain-free building blocks (`multiline_text`, `kv_row`,
//!    `section_heading`, `note_block`, `fmt_iso_datetime`) that any read-only
//!    detail panel composes from.
//!
//! Read `yux/CLAUDE.md` before adding to it: it states the rules (state
//! encapsulation, the never-notify-in-render law, the render-count test) and
//! the contribution mandate — **all UX work lives here or is built from here.**

mod cached;
mod detail;

pub(crate) use cached::*;
pub(crate) use detail::*;

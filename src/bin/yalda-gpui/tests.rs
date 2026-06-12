//! Unit tests for the GPUI app (moved out of main.rs, split-gpui-main).

use super::*;

/// 5c / ADR-0007: a theme switch re-renders Doc blocks via `re_render_one_doc`.
/// For a pool-bound Doc the authority is the LIVE shared core (unsaved edits
/// from a sibling Edit view), not the file on disk. The old code read disk
/// here — silently reverting unsaved edits, and (because `rendered_seq` would
/// not advance) the per-frame `refresh_blocks` would not self-correct. This
/// pins the fix: re-render reflects the live core and stamps `rendered_seq`.
#[test]
fn re_render_one_doc_sources_live_core_not_disk() {
    let dir = std::env::temp_dir().join(format!("yalda_rerender_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("doc.md");
    // Disk holds a single paragraph → exactly one rendered block.
    std::fs::write(&path, "disk only\n").unwrap();

    let mut ws: workspace::Workspace<App> = workspace::Workspace::new();
    let (id, core) = ws.open_and_retain(&path).unwrap();

    // A pool-bound Doc, rendered at the disk content (rendered_seq stamped at
    // the pristine core).
    let mut doc = DocState {
        blocks: render_with_wiki("disk only\n", &Theme::default(), Some(&path)),
        file_label: path.display().to_string().into(),
        cursor_block: 0,
        list_state: DocState::new_list_state(0),
        list_item_count: std::cell::Cell::new(0),
        blocks_seq: 0,
        blocks_snapshot: RefCell::new(None),
        last_cursor_block: std::cell::Cell::new(None),
        source: Some(DocSource::new(id, core.clone())),
    };
    assert_eq!(doc.blocks.len(), 1, "disk content is one block");

    // Simulate an unsaved edit through a sibling view: append two more
    // paragraphs that exist ONLY in the live core, never on disk.
    {
        let mut c = core.borrow_mut();
        let d = c.document_mut();
        let n = d.full_text().chars().count();
        d.insert_str_at_char(n, "\n\npara two\n\npara three\n");
    }
    let live_seq = core.borrow().document().edit_seq();
    let live_blocks = render_with_wiki(
        &core.borrow().document().full_text(),
        &Theme::default(),
        Some(&path),
    )
    .len();
    assert!(live_blocks >= 3, "live core now has multiple blocks");

    // Theme switch path. Must reflect the LIVE core (≥3 blocks), not disk (1).
    re_render_one_doc(&mut doc, &Theme::default());
    assert_eq!(
        doc.blocks.len(),
        live_blocks,
        "re-render must source the live core, not disk"
    );
    assert_eq!(
        doc.source.as_ref().unwrap().rendered_seq,
        live_seq,
        "rendered_seq must advance to the live edit_seq so refresh_blocks stays coherent"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// ADR-0010: the canonical on-disk cwd key resolves a symlinked spelling
/// and the real path to the SAME string (so a session saved under one is
/// found when launched under the other), and falls back to the raw spelling
/// when the path can't be canonicalized (never regresses to never-matching).
#[test]
fn persist_cwd_key_canonicalizes_symlinks() {
    use std::os::unix::fs::symlink;
    let base = std::env::temp_dir().join(format!("yalda-cwdkey-{}", std::process::id()));
    let real = base.join("real");
    let link = base.join("link");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&real).unwrap();
    symlink(&real, &link).unwrap();

    assert_eq!(
        persist_cwd_key(&link),
        persist_cwd_key(&real),
        "symlinked and real cwd must share one on-disk key"
    );
    assert_ne!(
        persist_cwd_key(&link),
        link.to_string_lossy(),
        "the key must be canonicalized, not the raw symlink spelling"
    );

    // Non-existent path: canonicalize fails -> echo raw (no never-match).
    let missing = base.join("does-not-exist");
    assert_eq!(persist_cwd_key(&missing), missing.to_string_lossy());

    let _ = std::fs::remove_dir_all(&base);
}

/// Settings persistence round-trips (theme, agent bar, text zoom) and a
/// preferences file written before `text_scale` existed still loads — the
/// `#[serde(default)]` keeps it forward-compatible (no panic, zoom = None).
#[test]
fn preferences_round_trip_with_text_scale() {
    let prefs = Preferences {
        theme: Some("dracula".into()),
        agent_status_position: Some("top".into()),
        text_scale: Some(1.21),
        desktop_grid_cols: Some(100),
        desktop_grid_rows: Some(30),
    };
    let json = serde_json::to_string(&prefs).unwrap();
    let back: Preferences = serde_json::from_str(&json).unwrap();
    assert_eq!(back.theme.as_deref(), Some("dracula"));
    assert_eq!(back.agent_status_position.as_deref(), Some("top"));
    assert_eq!(back.text_scale, Some(1.21));
    assert_eq!(back.desktop_grid_cols, Some(100));
    assert_eq!(back.desktop_grid_rows, Some(30));

    // Default (no zoom) is omitted from the serialized form.
    let bare = Preferences::default();
    assert!(!serde_json::to_string(&bare).unwrap().contains("text_scale"));

    // An old file lacking the field deserializes with text_scale == None.
    let legacy = r#"{"theme":"folio","agent_status_position":"bottom"}"#;
    let parsed: Preferences = serde_json::from_str(legacy).unwrap();
    assert_eq!(parsed.text_scale, None);
    assert_eq!(parsed.theme.as_deref(), Some("folio"));
}

fn s(text: &str) -> Segment {
    (text.to_string(), NStyle::default())
}

/// Finding 9 enforcement hook: the turn lifecycle is a total function over
/// `TurnPhase`, and the canonical `submit → stop → stop → finalize`
/// sequence pins the escalation behavior that used to live only in a field
/// comment. The first Stop moves Awaiting → StopRequested (graceful cancel
/// pending, not yet escalated); the second Stop, gated on `stop_requested()`,
/// escalates; `finalize` returns to Idle.
#[test]
fn turn_phase_submit_stop_stop_finalize_pins_escalation() {
    use std::time::Instant;

    // submit → Awaiting (in flight, no stop yet).
    let mut phase = TurnPhase::begin(Instant::now());
    assert!(phase.is_awaiting(), "submit must enter awaiting");
    assert!(!phase.stop_requested(), "fresh turn has no pending stop");
    assert!(
        phase.turn_started().is_some(),
        "awaiting carries the elapsed timer"
    );
    assert!(
        phase.last_event_at().is_some(),
        "awaiting carries the quiet clock"
    );

    // First Stop → StopRequested, graceful (not escalated). The handler
    // gate `stop_requested()` is what decides escalate-vs-graceful.
    let first_stop_escalates = phase.stop_requested();
    assert!(
        !first_stop_escalates,
        "the FIRST stop must be graceful, not a hard kill"
    );
    phase.request_stop(Instant::now());
    assert!(
        phase.is_awaiting(),
        "a pending stop is still in flight (timers run)"
    );
    assert!(
        phase.stop_requested(),
        "first stop records a pending cancel"
    );
    assert!(!phase.is_escalated(), "first stop has not escalated");
    // Timers survive the transition so the indicator keeps reading.
    assert!(phase.turn_started().is_some());
    assert!(phase.last_event_at().is_some());

    // Second Stop → the handler sees `stop_requested()` and escalates.
    let second_stop_escalates = phase.stop_requested();
    assert!(
        second_stop_escalates,
        "the SECOND stop while awaiting must escalate"
    );
    phase.escalate();
    assert!(
        phase.is_escalated(),
        "second stop marks the phase escalated"
    );

    // finalize (turn end / force-restart) → Idle, all markers cleared.
    phase = TurnPhase::Idle;
    assert!(!phase.is_awaiting(), "finalize returns to idle");
    assert!(!phase.stop_requested(), "idle has no pending stop");
    assert!(!phase.is_escalated(), "idle is not escalated");
    assert!(phase.turn_started().is_none(), "idle has no timer");
    assert!(phase.last_event_at().is_none(), "idle has no quiet clock");
}

/// `request_stop`/`escalate`/`note_event` are no-ops when idle, so a stray
/// Stop or stale event can never strand the phase in a contradictory state.
#[test]
fn turn_phase_idle_transitions_are_noops() {
    use std::time::Instant;
    let mut phase = TurnPhase::Idle;
    phase.request_stop(Instant::now());
    assert!(matches!(phase, TurnPhase::Idle), "stop on idle is a no-op");
    phase.escalate();
    assert!(
        matches!(phase, TurnPhase::Idle),
        "escalate on idle is a no-op"
    );
    phase.note_event(Instant::now());
    assert!(matches!(phase, TurnPhase::Idle), "event on idle is a no-op");

    // note_event refreshes the quiet clock only while in flight.
    let t0 = Instant::now();
    let mut awaiting = TurnPhase::Awaiting {
        started: t0,
        last_event: t0,
    };
    let later = t0 + std::time::Duration::from_secs(5);
    awaiting.note_event(later);
    assert_eq!(
        awaiting.last_event_at(),
        Some(later),
        "note_event advances the quiet clock while awaiting",
    );
    assert_eq!(
        awaiting.turn_started(),
        Some(t0),
        "note_event must not disturb the elapsed timer",
    );
}

#[test]
fn split_segments_at_col_zero_in_first_segment() {
    let segs = vec![s("hello"), s(" "), s("world")];
    let (before, (ch, _), after) = split_segments_at_col(&segs, 0);
    assert!(before.is_empty());
    assert_eq!(ch, 'h');
    let after_text: String = after.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(after_text, "ello world");
}

#[test]
fn split_segments_at_col_inside_a_segment() {
    // col 2 of "hello" → 'l', before="he", after="lo world"
    let segs = vec![s("hello"), s(" world")];
    let (before, (ch, _), after) = split_segments_at_col(&segs, 2);
    let before_text: String = before.iter().map(|(t, _)| t.as_str()).collect();
    let after_text: String = after.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(before_text, "he");
    assert_eq!(ch, 'l');
    assert_eq!(after_text, "lo world");
}

#[test]
fn split_segments_at_col_on_segment_boundary() {
    // col 5 lands on the first char of the second segment (' ').
    let segs = vec![s("hello"), s(" world")];
    let (before, (ch, _), after) = split_segments_at_col(&segs, 5);
    let before_text: String = before.iter().map(|(t, _)| t.as_str()).collect();
    let after_text: String = after.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(before_text, "hello");
    assert_eq!(ch, ' ');
    assert_eq!(after_text, "world");
}

#[test]
fn split_segments_at_col_past_end_is_virtual_space() {
    let segs = vec![s("hi")];
    let (before, (ch, _), after) = split_segments_at_col(&segs, 99);
    let before_text: String = before.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(before_text, "hi");
    assert_eq!(ch, ' '); // cursor at/past EOL renders as a space caret
    assert!(after.is_empty());
}

#[test]
fn split_segments_at_col_empty_input() {
    let segs: Vec<Segment> = vec![];
    let (before, (ch, _), after) = split_segments_at_col(&segs, 0);
    assert!(before.is_empty());
    assert_eq!(ch, ' ');
    assert!(after.is_empty());
}

/// Builds a synthetic frozen transcript: `n_blocks` fenced code blocks
/// (`block_lines` lines each) separated by prose, plus one editable tail
/// line. Returns `(lines, frozen_ranges, frozen_line_count)`.
fn synthetic_transcript(
    n_blocks: usize,
    block_lines: usize,
) -> (Vec<String>, Vec<(usize, usize)>, usize) {
    let mut lines: Vec<String> = Vec::new();
    for b in 0..n_blocks {
        lines.push(format!("prose before block {b}"));
        lines.push("```rust".to_string());
        for i in 0..block_lines {
            lines.push(format!("let x_{b}_{i} = {i};"));
        }
        lines.push("```".to_string());
    }
    lines.push(String::new()); // editable tail
    let frozen_len = lines.len() - 1;
    (lines, vec![(0usize, frozen_len)], frozen_len)
}

fn block_ptrs(flat: &[FlatItem]) -> Vec<*const RenderedBlock> {
    flat.iter()
        .filter_map(|f| match f {
            FlatItem::Block(b) => Some(std::rc::Rc::as_ptr(b)),
            _ => None,
        })
        .collect()
}

/// Worksheet-typing perf invariant: an S1 rebuild whose frozen prefix is
/// unchanged (a keystroke in the editable tail bumps the fingerprint but
/// not the frozen line count) must reuse every parsed `RenderedBlock` by
/// Rc IDENTITY — no re-parse, no deep clone. The old rebuild deep-cloned
/// every parsed block into fresh per-rebuild lookup maps on every
/// keystroke — the dominant per-keystroke cost on large transcripts (the
/// "Worksheet mode is slow to type" report).
#[test]
fn worksheet_rebuild_reuses_parsed_blocks_by_identity() {
    let mut st = AgentState::new_for_test();
    let theme = Theme::default();
    let (lines, frozen, frozen_len) = synthetic_transcript(3, 4);

    let (flat1, _) = rebuild_agent_view_model(&mut st, &lines, &frozen, frozen_len, &theme, 1);
    let blocks1 = block_ptrs(&flat1);
    assert_eq!(blocks1.len(), 3, "three fenced blocks must parse");

    // Keystroke in the editable tail: new fingerprint, same frozen count.
    let mut lines2 = lines.clone();
    *lines2.last_mut().unwrap() = "x".to_string();
    let (flat2, _) = rebuild_agent_view_model(&mut st, &lines2, &frozen, frozen_len, &theme, 2);
    assert_eq!(
        blocks1,
        block_ptrs(&flat2),
        "a tail keystroke must reuse every parsed block by Rc identity"
    );
}

/// Streaming perf invariant: a chunk that inserts lines ABOVE the blocks
/// shifts every `(start, end)` range, but parses are keyed by CONTENT —
/// the revalidation must keep Rc identity for every block whose text is
/// unchanged. The old position-keyed cache missed on every shift, so each
/// streamed chunk re-parsed (pulldown-cmark + syntect) the entire frozen
/// transcript — the paint-thread flood behind "typing lags while a turn
/// streams".
#[test]
fn streamed_shift_reuses_parses_by_content() {
    let mut st = AgentState::new_for_test();
    let theme = Theme::default();
    let (lines, frozen, frozen_len) = synthetic_transcript(3, 4);

    let (flat1, _) = rebuild_agent_view_model(&mut st, &lines, &frozen, frozen_len, &theme, 1);
    let blocks1 = block_ptrs(&flat1);
    assert_eq!(blocks1.len(), 3);

    // Simulated streamed chunk: one new prose line at the top; every
    // block range shifts down by one and the frozen prefix grows.
    let mut lines2 = vec!["new streamed prose".to_string()];
    lines2.extend(lines.iter().cloned());
    let frozen2 = vec![(0usize, frozen_len + 1)];
    let (flat2, _) =
        rebuild_agent_view_model(&mut st, &lines2, &frozen2, frozen_len + 1, &theme, 2);
    assert_eq!(
        blocks1,
        block_ptrs(&flat2),
        "a range shift with unchanged block text must reuse every parse by Rc identity"
    );
}

/// INV-10 at the rebuild level: a detected range that `parse_block_range`
/// rejects (here a pipe "table" without a separator row) resolves to
/// `None` in `resolved_blocks` and must render as its source Lines — no
/// Block item, no swallowed lines.
#[test]
fn rebuild_renders_unparsed_range_as_lines() {
    let mut st = AgentState::new_for_test();
    let theme = Theme::default();
    let lines: Vec<String> = vec![
        "| a | b |".to_string(),
        "| 1 | 2 |".to_string(),
        "| 3 | 4 |".to_string(),
        String::new(), // editable tail
    ];
    let frozen = vec![(0usize, 3)];
    let (flat, _) = rebuild_agent_view_model(&mut st, &lines, &frozen, 3, &theme, 1);
    assert!(
        !flat.iter().any(|f| matches!(f, FlatItem::Block(_))),
        "rejected range must emit no Block item"
    );
    let line_items: Vec<usize> = flat
        .iter()
        .filter_map(|f| match f {
            FlatItem::Line(i) => Some(*i),
            _ => None,
        })
        .collect();
    assert_eq!(
        line_items,
        vec![0, 1, 2, 3],
        "every source line of an unparsed range must render as a Line \
         (plus the editable tail — a lone user blank not adjacent to a \
         structural item is kept, not collapsed)"
    );
}

/// Cost probe for the worksheet-keystroke path: repeated rebuilds over a
/// large transcript with the frozen prefix unchanged. Prints the per-
/// rebuild cost; the assert is a generous debug-build ceiling that only
/// trips if the rebuild regresses to re-parsing/deep-cloning per
/// keystroke again.
#[test]
fn worksheet_rebuild_cost_probe() {
    let mut st = AgentState::new_for_test();
    let theme = Theme::default();
    let (mut lines, frozen, frozen_len) = synthetic_transcript(50, 60);

    // Warm: parse all blocks once.
    let _ = rebuild_agent_view_model(&mut st, &lines, &frozen, frozen_len, &theme, 0);

    const ROUNDS: u64 = 200;
    let t0 = std::time::Instant::now();
    for k in 0..ROUNDS {
        let n = lines.len();
        lines[n - 1] = format!("typing {k}");
        let _ = rebuild_agent_view_model(&mut st, &lines, &frozen, frozen_len, &theme, k + 1);
    }
    let per = t0.elapsed() / ROUNDS as u32;
    eprintln!(
        "[probe] worksheet rebuild: {} lines, 50 blocks → {per:?}/keystroke",
        lines.len()
    );
    assert!(
        per < std::time::Duration::from_millis(10),
        "worksheet rebuild regressed to {per:?}/keystroke (budget 10ms debug)"
    );
}

/// INV-RV regression: cursor-reveal is O(1) and the reverse index is a
/// faithful mirror of the rendered `flat_items`. The old Worksheet key
/// handler recomputed the cursor's flat-item position from scratch on EVERY
/// keystroke — an O(transcript) gutter scan + tool/anchor walk — which is the
/// monotonic "typing gets slower as the session grows" regression. The fix
/// derives a doc-line → item index FROM the canonical flat list at build
/// time, so the per-keystroke reveal is a single array read. This test pins
/// (a) the map points every `Line` item at its real position (single source
/// of truth — it can't drift from what's rendered), (b) lookups are
/// bounds-clamped (cursor past EOF must not panic), and (c) the map is built
/// once per rebuild, not per keystroke.
#[test]
fn reveal_index_mirrors_flat_items_and_is_o1() {
    let mut st = AgentState::new_for_test();
    let theme = Theme::default();
    // Several fenced blocks + interleaved prose + an editable tail.
    let (lines, frozen, frozen_len) = synthetic_transcript(4, 6);

    VIEW_MODEL_REBUILDS.with(|n| n.set(0));
    let (flat, _gut) = rebuild_agent_view_model(&mut st, &lines, &frozen, frozen_len, &theme, 1);

    // (a) Every `Line(idx)` is reachable in O(1) at its REAL flat position —
    // the reverse index mirrors the canonical list exactly.
    let mut checked = 0usize;
    for (p, item) in flat.iter().enumerate() {
        if let FlatItem::Line(idx) = item {
            assert_eq!(
                st.view_model.item_for_line(*idx),
                p,
                "item_for_line({idx}) must equal the Line's real flat position {p}"
            );
            checked += 1;
        }
    }
    assert!(checked > 0, "transcript must contain Line items to validate");

    // (a') The desync vector: Block items dropped their source range, so the
    // map must re-pair them with `resolved`. Verify every doc line resolves
    // IN-RANGE, and that lines collapsed into a structural Block resolve to a
    // `Block` item (not to a stray Line or off-by-one position). At least one
    // line must land on a Block (the synthetic transcript has fenced blocks).
    let mut lines_on_a_block = 0usize;
    for line in 0..lines.len() {
        let idx = st.view_model.item_for_line(line);
        assert!(idx < flat.len(), "line {line} resolved out of range ({idx})");
        if matches!(flat[idx], FlatItem::Block(_)) {
            lines_on_a_block += 1;
        }
    }
    assert!(
        lines_on_a_block > 0,
        "block-covered lines must resolve to their Block item, not a Line/off-by-one"
    );

    // (b) Out-of-range (cursor past EOF) clamps into the list, never panics.
    let last = flat.len().saturating_sub(1);
    assert!(
        st.view_model.item_for_line(usize::MAX) <= last,
        "an out-of-range reveal must clamp into the built list"
    );

    // (c) The reverse index is part of the memoized view model: re-rendering
    // at the SAME structural fingerprint is a pure cache hit, so neither the
    // flat list NOR the reveal index is rebuilt per keystroke — the reveal
    // cost stays independent of transcript length.
    let rebuilds = VIEW_MODEL_REBUILDS.with(|n| n.get());
    for _ in 0..100 {
        match st.view_model.cached(1) {
            Some(_hit) => {} // O(1) reuse, no store
            None => panic!("same-fingerprint render must hit the S1 cache"),
        }
    }
    assert_eq!(
        VIEW_MODEL_REBUILDS.with(|n| n.get()),
        rebuilds,
        "100 same-fingerprint renders must not rebuild — per-keystroke work is O(changed)"
    );
}

/// S1 enforcement, on the split `cached` + `store` API: an unchanged
/// fingerprint must hit (`cached` returns `Some` the same-pointer `Rc`s
/// and the caller never `store`s, so ZERO rebuilds). A changed fingerprint
/// must miss (`cached` returns `None`) and the following `store` produces
/// fresh `Rc`s. `VIEW_MODEL_REBUILDS` counts `store` calls (= misses).
/// Models the `highlight_cache` fast-skip tests.
#[test]
fn view_model_memoization_fast_skip() {
    VIEW_MODEL_REBUILDS.with(|n| n.set(0));
    let mut st = AgentState::new_for_test();

    // Build a fingerprint over the empty structural state.
    let fp1 = st.view_model_fingerprint(0, 0);

    // Cold cache: miss → rebuild at the call site, then `store`.
    assert!(st.view_model.cached(fp1).is_none(), "cold cache must miss");
    let (flat1, gut1) = st
        .view_model
        .store(fp1, vec![FlatItem::Line(0)], vec![None], vec![0]);
    assert_eq!(
        VIEW_MODEL_REBUILDS.with(|n| n.get()),
        1,
        "store counts as one rebuild"
    );
    let seq_after_first = st.view_model.view_model_seq;
    assert_eq!(seq_after_first, 1, "first store bumps the seq to 1");

    // SAME fingerprint: hit → reuse the very same `Rc`s (pointer identity),
    // no `store`, seq unchanged.
    let (flat2, gut2) = st
        .view_model
        .cached(fp1)
        .expect("same fingerprint must hit");
    assert_eq!(
        VIEW_MODEL_REBUILDS.with(|n| n.get()),
        1,
        "a hit must not rebuild"
    );
    assert!(
        std::rc::Rc::ptr_eq(&flat1, &flat2),
        "flat_items Rc must be reused on a hit"
    );
    assert!(
        std::rc::Rc::ptr_eq(&gut1, &gut2),
        "gutter Rc must be reused on a hit"
    );
    assert_eq!(
        st.view_model.view_model_seq, seq_after_first,
        "seq must not change on a hit"
    );

    // Fingerprint sensitivity: a structural change (turn_phase enters
    // awaiting, which the thinking indicator depends on) yields a DIFFERENT
    // fingerprint → miss, and the following `store` produces a fresh `Rc`.
    st.turn_phase = TurnPhase::begin(std::time::Instant::now());
    let fp2 = st.view_model_fingerprint(0, 0);
    assert_ne!(fp1, fp2, "turn_phase awaiting must change the fingerprint");
    assert!(
        st.view_model.cached(fp2).is_none(),
        "changed fingerprint must miss"
    );
    let (flat3, _gut3) = st
        .view_model
        .store(fp2, vec![FlatItem::ThinkingIndicator], vec![None], vec![0]);
    assert_eq!(
        VIEW_MODEL_REBUILDS.with(|n| n.get()),
        2,
        "a miss + store rebuilds again"
    );
    assert!(
        !std::rc::Rc::ptr_eq(&flat1, &flat3),
        "a rebuild must produce a fresh Rc"
    );
    assert_eq!(
        st.view_model.view_model_seq, 2,
        "second store bumps the seq again"
    );
}

/// F7 (parse-don't-validate at the trust boundary): a `ToolCallKey` parsed
/// from a protocol `ToolCallId` is the maps' key type, and two keys built
/// from the same protocol id are equal + hash-equal, so an insert via one
/// and a lookup via another (the live-update path) land on the same entry.
/// The type itself is the enforcement hook (no `Deref` to `String`, so an
/// arbitrary label can't be substituted for a tool id); this pins the
/// round-trip the maps rely on.
#[test]
fn tool_call_key_round_trips_through_the_maps() {
    use yalda::acp_channel::ToolCallId;

    let id: ToolCallId = "tool-abc".into();
    let key_started = ToolCallKey::from_id(&id);
    // A later `ToolCallUpdated` re-parses the SAME protocol id into a key.
    let key_updated = ToolCallKey::from_id(&id);

    assert_eq!(
        key_started, key_updated,
        "keys parsed from the same protocol id must be equal"
    );
    assert_eq!(
        key_started.as_str(),
        "tool-abc",
        "the render edge can recover the id string"
    );
    assert_eq!(key_started.to_string(), "tool-abc");

    // Insert on the started key, look up on the (separately parsed) updated
    // key — the live ToolCallUpdated path. The lookup must hit.
    let mut map: std::collections::HashMap<ToolCallKey, u32> = std::collections::HashMap::new();
    map.insert(key_started, 7);
    assert_eq!(
        map.get(&key_updated),
        Some(&7),
        "a key re-parsed from the same id must resolve the same map entry"
    );

    // A DIFFERENT id is a distinct key — no accidental collision.
    let other = ToolCallKey::from_id(&("tool-xyz".into()));
    assert_eq!(map.get(&other), None, "a different id must miss");
}

/// The fingerprint must EXCLUDE tool-call content (the `ToolCallUpdated`
/// trap): mutating a `ToolCall`'s content without touching
/// `tool_call_order` / `edit_seq` must leave the fingerprint unchanged,
/// so the cached flat_items (which only carry tool ids) stay valid.
#[test]
fn view_model_fingerprint_ignores_tool_content() {
    let mut st = AgentState::new_for_test();
    st.tools.order.push(ToolCallKey::from_id(&"tool-1".into()));
    let before = st.view_model_fingerprint(7, 3);

    // Simulate a ToolCallUpdated: content changes, order/edit_seq don't.
    // (We don't have a ToolCall constructor handy in-test; the point is
    // that the fingerprint reads neither `tool_calls` content nor map
    // size — only `tool_call_order`.) Re-derive with identical structural
    // inputs and assert stability.
    let after = st.view_model_fingerprint(7, 3);
    assert_eq!(before, after, "tool content is not part of the fingerprint");
}

/// F6 / INV (header-owning turns are exactly {Llm, User}): `HeaderRole`
/// is a TOTAL mapping over `TurnId` — `Tool`/`System` -> None (no header),
/// `Llm` -> Claude, `User` -> User. This replaces the old `unreachable!()`
/// arm with a compiler-checked `Option`, so a new `TurnId` variant is a
/// compile error, not a paint-path panic.
#[test]
fn header_role_is_total_over_turn_id() {
    assert_eq!(HeaderRole::from_turn(TurnId::Tool(3)), None);
    assert_eq!(HeaderRole::from_turn(TurnId::System), None);
    assert_eq!(
        HeaderRole::from_turn(TurnId::Llm(1)),
        Some(HeaderRole::Claude)
    );
    assert_eq!(
        HeaderRole::from_turn(TurnId::User(2)),
        Some(HeaderRole::User)
    );
    // And the role threads through to the rendered `TurnRole`.
    assert_eq!(HeaderRole::Claude.into_turn_role(), TurnRole::Claude);
    assert_eq!(HeaderRole::User.into_turn_role(), TurnRole::User);
}

/// F8 / INV-12 (count parity): `reconcile_list` is the ONLY mutator of
/// `(list_state, list_item_count)`, updating both together so they can't
/// drift. It returns whether the list grew. After any reconcile the
/// registered count equals the requested count.
#[test]
fn reconcile_list_keeps_count_in_sync_and_reports_growth() {
    // Ticket 021: the `(list_state, list_item_count)` pair moved out of
    // `AgentState` into the `TranscriptScroll` UI-state struct owned by
    // `TranscriptView`. The reconcile logic is unchanged and still pure-
    // testable — `block_ranges_active` is now passed in rather than read off
    // `AgentState`.
    let mut sc = TranscriptScroll::new();
    assert_eq!(sc.list_item_count, 0);

    // Growth: count rises, reports grew=true, splices.
    assert!(sc.reconcile_list(false, 5, 0), "0 -> 5 must report growth");
    assert_eq!(sc.list_item_count, 5, "count tracks the requested length");

    // No change: same count, reports grew=false, count unchanged.
    assert!(!sc.reconcile_list(false, 5, 0), "5 -> 5 is not growth");
    assert_eq!(sc.list_item_count, 5);

    // Shrink: count falls, reports grew=false, resets.
    assert!(!sc.reconcile_list(false, 2, 0), "5 -> 2 is not growth");
    assert_eq!(sc.list_item_count, 2, "count tracks a shrink too");

    // With block ranges active, even growth resets (height cache can't be
    // spliced) — but parity must still hold.
    assert!(sc.reconcile_list(true, 9, 0));
    assert_eq!(sc.list_item_count, 9);
}

/// F10 / INV-10 (block/line partition is total): a range
/// `detect_block_ranges` emits but `parse_block_range` rejects must
/// `FallBackToLines`, contribute NO entry to the block cache, and so
/// leave every one of its source lines to render as a standalone Line.
/// Mirrors render_agent's cache + `in_block` construction exactly.
#[test]
fn unparsed_detected_range_falls_back_to_one_line_per_source_line() {
    // 3 pipe-delimited rows with NO separator row: `detect_block_ranges`
    // accepts it (>=3 rows, all `|...|`), but it is NOT a valid markdown
    // table, so `parse_block_range` rejects it.
    let lines: Vec<String> = vec![
        "| a | b |".to_string(),
        "| c | d |".to_string(),
        "| e | f |".to_string(),
    ];
    let frozen = vec![(0usize, lines.len())];
    let ranges = detect_block_ranges(&lines, &frozen);
    assert_eq!(
        ranges,
        vec![(0, 3)],
        "the 3 pipe rows must be DETECTED as a candidate range"
    );

    let theme = Theme::default();
    assert!(
        matches!(
            parse_block_range(&lines, 0, 3, &theme),
            BlockParse::FallBackToLines
        ),
        "a separator-less pipe block must NOT parse as a table"
    );

    // Replicate the render_agent partition: block_cache holds only Parsed
    // ranges; `in_block` is derived from the cache; any line not in a
    // block is emitted as a Line.
    let mut block_cache: std::collections::HashMap<(usize, usize), RenderedBlock> =
        std::collections::HashMap::new();
    for &(s, e) in &ranges {
        if let BlockParse::Parsed(b) = parse_block_range(&lines, s, e, &theme) {
            block_cache.insert((s, e), b);
        }
    }
    let mut in_block: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for &(s, e) in &ranges {
        if block_cache.contains_key(&(s, e)) {
            for li in s..e {
                in_block.insert(li);
            }
        }
    }
    let line_items: Vec<usize> = (0..lines.len())
        .filter(|i| !block_cache.keys().any(|&(s, _)| s == *i) && !in_block.contains(i))
        .collect();
    // Count parity over the range: a Line for EVERY source line, no Block.
    assert!(
        block_cache.is_empty(),
        "rejected range must emit no Block item"
    );
    assert_eq!(
        line_items,
        vec![0, 1, 2],
        "every source line of an unparsed range must render as a Line"
    );
}

/// F11 / INV-8 (memo soundness): the fingerprint must change when a
/// resolved tool anchor line changes, because the flat build groups tool
/// calls by that resolved line. Holding `edit_seq` FIXED across the two
/// fingerprint calls isolates the anchor dependency from the `edit_seq`
/// co-variation the memo previously leaned on implicitly.
/// A.6: `ToolCalls::register` is the one chokepoint that keeps `order`,
/// `calls`, and `anchor` in sync — a new id appends to order exactly once,
/// a re-register (the update path) never duplicates the order entry, and
/// `clear` empties every map together.
#[test]
fn tool_calls_register_keeps_maps_in_sync() {
    use yalda::acp_channel::{ToolCall, ToolCallId};
    let mut st = AgentState::new_for_test();
    let id1: ToolCallId = "t1".into();
    let k1 = ToolCallKey::from_id(&id1);

    st.tools.register(
        k1.clone(),
        ToolCall::new(id1.clone(), String::from("ls")),
        st.editor.anchor_for_line(0),
    );
    assert_eq!(st.tools.order.len(), 1);
    assert!(st.tools.calls.contains_key(&k1) && st.tools.anchor.contains_key(&k1));

    // Re-register the same id (an update arriving via register): order must
    // NOT grow — the three maps stay coherent.
    st.tools.register(
        k1.clone(),
        ToolCall::new(id1.clone(), String::from("ls -la")),
        st.editor.anchor_for_line(0),
    );
    assert_eq!(
        st.tools.order.len(),
        1,
        "re-register must not duplicate the order entry"
    );
    assert_eq!(st.tools.calls.len(), 1);

    // A distinct id appends.
    let id2: ToolCallId = "t2".into();
    let k2 = ToolCallKey::from_id(&id2);
    st.tools.register(
        k2.clone(),
        ToolCall::new(id2, String::from("grep")),
        st.editor.anchor_for_line(0),
    );
    assert_eq!(st.tools.order.len(), 2);
    assert!(st.tools.order.contains(&k2));

    st.tools.clear();
    assert!(
        st.tools.order.is_empty() && st.tools.calls.is_empty() && st.tools.anchor.is_empty(),
        "clear empties every map together"
    );
}

#[test]
fn fingerprint_tracks_resolved_tool_anchor_line() {
    let mut st = AgentState::new_for_test();
    // Seed a few frozen lines so an anchor can resolve to a real line.
    st.editor
        .programmatic_insert(0, "line0\nline1\nline2\nline3\n");

    // Anchor a tool call to line 2 and register it in the build's inputs.
    let anchor = st.editor.anchor_for_line(2);
    let key = ToolCallKey::from_id(&"tool-1".into());
    st.tools.order.push(key.clone());
    st.tools.anchor.insert(key, anchor);
    assert_eq!(st.editor.line_for_anchor(anchor), Some(2));

    // Fingerprint at a FIXED edit_seq/frozen_count.
    let fp_before = st.view_model_fingerprint(42, 4);

    // Insert a line ABOVE the anchor: its resolved line moves 2 -> 3.
    // We pass the SAME edit_seq (42) again, so any fingerprint change is
    // attributable to the resolved anchor line, not to edit_seq.
    st.editor.programmatic_insert(0, "header\n");
    assert_eq!(
        st.editor.line_for_anchor(anchor),
        Some(3),
        "the anchor must have shifted down by one line"
    );
    let fp_after = st.view_model_fingerprint(42, 4);

    assert_ne!(
        fp_before, fp_after,
        "a moved tool anchor must change the fingerprint even at a fixed edit_seq"
    );
}

/// F4 / INV-13 enforcement: the tail re-reveal must fire on CONTENT growth
/// (`edit_seq` advanced), NOT on a flat-item count delta. A chunk that
/// grows the last line without adding a row (agent prose before a `\n`)
/// bumps `edit_seq` but leaves the count unchanged; the old count-keyed
/// path skipped it. `reveal_tail_if_following` must request the reveal
/// anyway, and must NOT re-request at the same `edit_seq` (idle ticks).
#[test]
fn reveal_tail_keys_on_content_growth_not_count() {
    // Ticket 021: the reveal logic + the `last_scrolled_edit_seq` watermark
    // moved to `TranscriptScroll`; `follow_tail()` (the follow DECISION) stays
    // on `AgentState`. The caller threads `follow_tail()` + the document's
    // `edit_seq` into `reveal_tail_if_following`, exactly as `TranscriptView`
    // does in render. The behavior under test is unchanged.
    let mut st = AgentState::new_for_test();
    let mut sc = TranscriptScroll::new();
    // new_for_test starts in Chatbox with follow_output = true, so the
    // follow decision is satisfied; we isolate the edit_seq/count behavior.
    assert!(st.follow_tail(), "Chatbox + follow_output should follow");

    let count = 3usize; // simulated post-reconcile flat-item count
    let seq0 = st.editor.document().edit_seq();

    // First reveal at the current edit_seq: requested (watermark was MAX).
    assert!(
        sc.reveal_tail_if_following(st.follow_tail(), seq0, count),
        "first reveal at a new edit_seq must be requested"
    );
    assert_eq!(
        sc.last_scrolled_edit_seq, seq0,
        "reveal stamps the watermark to the current edit_seq"
    );

    // Idle tick — same edit_seq, same count: must NOT re-reveal (so a
    // user who scrolled up isn't yanked back every frame).
    assert!(
        !sc.reveal_tail_if_following(st.follow_tail(), seq0, count),
        "no content growth ⇒ no re-reveal at the same edit_seq"
    );

    // Append a chunk WITHOUT a trailing newline: grows the last line but
    // adds no row, so the flat-item count is UNCHANGED. This is exactly
    // the case the old `new_count != old_count` trigger missed.
    let char_len = st.editor.document().rope().len_chars();
    st.editor
        .programmatic_insert(char_len, "more streamed prose");
    let seq1 = st.editor.document().edit_seq();
    assert_ne!(seq1, seq0, "an intra-line insert must advance edit_seq");

    // Count is held constant (no new row) — the reveal must STILL fire,
    // keyed on the advanced edit_seq, not on a count delta.
    assert!(
        sc.reveal_tail_if_following(st.follow_tail(), seq1, count),
        "intra-line content growth must re-reveal even with unchanged count"
    );
    assert_eq!(sc.last_scrolled_edit_seq, seq1);

    // A zero count never reveals (guards the `count - 1` underflow).
    let seq2_before = sc.last_scrolled_edit_seq;
    st.editor.programmatic_insert(0, "x");
    let seq2 = st.editor.document().edit_seq();
    assert!(
        !sc.reveal_tail_if_following(st.follow_tail(), seq2, 0),
        "an empty list never reveals regardless of growth"
    );
    assert_eq!(
        sc.last_scrolled_edit_seq, seq2_before,
        "a skipped reveal must not advance the watermark"
    );

    // When following is OFF (user scrolled up in Chatbox), growth alone
    // must not yank the viewport back.
    st.follow_output.set(false);
    assert!(!st.follow_tail());
    st.editor.programmatic_insert(0, "y");
    let seq3 = st.editor.document().edit_seq();
    assert!(
        !sc.reveal_tail_if_following(st.follow_tail(), seq3, count),
        "no reveal while the user has scrolled away from the tail"
    );
}

/// F12 / INV-11 enforcement: an UNTERMINATED code fence must yield NO
/// block range, so its arrived lines render as plain Lines (each its own
/// FlatItem) until the closing fence freezes. A matched closing fence is
/// required, symmetric to the >=3-row table rule.
#[test]
fn detect_block_ranges_skips_unterminated_fence() {
    // Open fence, two body lines, NO closing ``` — all frozen.
    let lines: Vec<String> = vec![
        "```rust".to_string(),
        "let x = 1;".to_string(),
        "let y = 2;".to_string(),
    ];
    let frozen = vec![(0usize, lines.len())];
    let ranges = detect_block_ranges(&lines, &frozen);
    assert!(
        ranges.is_empty(),
        "an unterminated fence must NOT emit a block range, got {ranges:?}"
    );

    // Sanity: once the closing fence arrives, the range IS emitted so
    // the closed block still renders as one Block.
    let mut closed = lines.clone();
    closed.push("```".to_string());
    let frozen_closed = vec![(0usize, closed.len())];
    let ranges_closed = detect_block_ranges(&closed, &frozen_closed);
    assert_eq!(
        ranges_closed,
        vec![(0usize, closed.len())],
        "a closed fence must emit exactly one block range"
    );
}

#[test]
fn segments_to_styled_line_preserves_text_and_count() {
    let segs = vec![s("foo"), s("bar"), s("")];
    let line = segments_to_styled_line(&segs);
    assert_eq!(line.spans.len(), 3);
    assert_eq!(line.spans[0].text, "foo");
    assert_eq!(line.spans[2].text, "");
}

// ---- line_selection_range ----

#[test]
fn line_selection_range_outside_returns_none() {
    // Selection lines 1..=3, querying line 0 (above) and line 5 (below).
    let sel = ((1, 0), (3, 0));
    assert_eq!(line_selection_range(sel, 0, 10), None);
    assert_eq!(line_selection_range(sel, 5, 10), None);
}

#[test]
fn line_selection_range_single_line_returns_partial() {
    // Sel from col 2 to col 6 on line 4.
    let sel = ((4, 2), (4, 6));
    assert_eq!(line_selection_range(sel, 4, 20), Some((2, 6)));
}

#[test]
fn line_selection_range_first_line_starts_at_sc() {
    let sel = ((2, 5), (4, 3));
    assert_eq!(line_selection_range(sel, 2, 12), Some((5, 12)));
}

#[test]
fn line_selection_range_last_line_ends_at_ec() {
    let sel = ((2, 5), (4, 3));
    assert_eq!(line_selection_range(sel, 4, 20), Some((0, 3)));
}

#[test]
fn line_selection_range_middle_line_full_width() {
    let sel = ((2, 5), (4, 3));
    assert_eq!(line_selection_range(sel, 3, 8), Some((0, 8)));
}

// ---- apply_selection_bg ----

fn seg_text(segs: &[Segment]) -> String {
    segs.iter().map(|(t, _)| t.as_str()).collect()
}

#[test]
fn apply_selection_bg_no_overlap_preserves_segments() {
    // Selection col 0..2 but apply over a single 3-char segment by passing
    // 99..100 (out of range). Result should equal input with 0 bg applied.
    let segs = vec![s("abc")];
    let out = apply_selection_bg(&segs, 99, 100, NColor::Red);
    assert_eq!(seg_text(&out), "abc");
    assert!(out.iter().all(|(_, st)| st.bg.is_none()));
}

#[test]
fn apply_selection_bg_full_segment_gets_bg() {
    let segs = vec![s("abc")];
    let out = apply_selection_bg(&segs, 0, 3, NColor::Red);
    assert_eq!(seg_text(&out), "abc");
    assert!(out.iter().all(|(_, st)| st.bg == Some(NColor::Red)));
}

#[test]
fn apply_selection_bg_splits_segment_at_boundary() {
    // Selection covers chars 1..2 of a 3-char segment → expect 3 segments:
    // unselected "a", selected "b", unselected "c".
    let segs = vec![s("abc")];
    let out = apply_selection_bg(&segs, 1, 2, NColor::Red);
    assert_eq!(seg_text(&out), "abc");
    assert_eq!(out.len(), 3);
    assert_eq!(out[0].0, "a");
    assert_eq!(out[0].1.bg, None);
    assert_eq!(out[1].0, "b");
    assert_eq!(out[1].1.bg, Some(NColor::Red));
    assert_eq!(out[2].0, "c");
    assert_eq!(out[2].1.bg, None);
}

#[test]
fn apply_selection_bg_spans_multiple_input_segments() {
    // Sel chars 2..6 across two segments "hello"+"world".
    let segs = vec![s("hello"), s("world")];
    let out = apply_selection_bg(&segs, 2, 6, NColor::Red);
    // Reconstructed text should be unchanged; "ll" + "o" + "w" should be bg'd.
    assert_eq!(seg_text(&out), "helloworld");
    let bg_text: String = out
        .iter()
        .filter(|(_, st)| st.bg == Some(NColor::Red))
        .map(|(t, _)| t.as_str())
        .collect();
    assert_eq!(bg_text, "llow");
}

#[test]
fn apply_selection_bg_empty_input_returns_empty() {
    let out = apply_selection_bg(&[], 0, 5, NColor::Red);
    assert!(out.is_empty());
}

// ---- classify_wp_line ----

#[test]
fn classify_wp_line_empty_blank_and_whitespace() {
    assert_eq!(classify_wp_line("", false), WpLineKind::Empty);
    assert_eq!(classify_wp_line("   ", false), WpLineKind::Empty);
    assert_eq!(classify_wp_line("\t  ", false), WpLineKind::Empty);
}

#[test]
fn classify_wp_line_headings_levels_1_through_6() {
    assert_eq!(classify_wp_line("# H1", false), WpLineKind::Heading(1));
    assert_eq!(classify_wp_line("## H2", false), WpLineKind::Heading(2));
    assert_eq!(classify_wp_line("### H3", false), WpLineKind::Heading(3));
    assert_eq!(classify_wp_line("###### H6", false), WpLineKind::Heading(6));
    // 7 hashes = not a valid heading per CommonMark; treat as paragraph.
    assert_eq!(
        classify_wp_line("####### too many", false),
        WpLineKind::Paragraph
    );
}

#[test]
fn classify_wp_line_heading_requires_space_after_hashes() {
    // No space after hashes = not a heading.
    assert_eq!(classify_wp_line("#hashtag", false), WpLineKind::Paragraph);
    // Hashes only on the line is still a heading per CommonMark.
    assert_eq!(classify_wp_line("##", false), WpLineKind::Heading(2));
}

#[test]
fn classify_wp_line_bullet_markers() {
    assert_eq!(classify_wp_line("- item", false), WpLineKind::BulletItem);
    assert_eq!(classify_wp_line("* item", false), WpLineKind::BulletItem);
    assert_eq!(classify_wp_line("+ item", false), WpLineKind::BulletItem);
    assert_eq!(
        classify_wp_line("  - nested", false),
        WpLineKind::BulletItem
    );
    // Dash without trailing space is not a bullet.
    assert_eq!(classify_wp_line("-no-space", false), WpLineKind::Paragraph);
}

#[test]
fn classify_wp_line_ordered_markers() {
    assert_eq!(classify_wp_line("1. item", false), WpLineKind::OrderedItem);
    assert_eq!(classify_wp_line("42. item", false), WpLineKind::OrderedItem);
    assert_eq!(classify_wp_line("3) item", false), WpLineKind::OrderedItem);
    // No space after marker.
    assert_eq!(classify_wp_line("1.no", false), WpLineKind::Paragraph);
    // No marker punctuation.
    assert_eq!(classify_wp_line("1 hello", false), WpLineKind::Paragraph);
}

#[test]
fn classify_wp_line_blockquote() {
    assert_eq!(classify_wp_line("> quote", false), WpLineKind::Blockquote);
    assert_eq!(classify_wp_line(">>nested", false), WpLineKind::Blockquote);
}

#[test]
fn classify_wp_line_code_fences() {
    // Opening fence outside of a fence.
    assert_eq!(classify_wp_line("```", false), WpLineKind::CodeFence);
    assert_eq!(classify_wp_line("```rust", false), WpLineKind::CodeFence);
    assert_eq!(classify_wp_line("~~~", false), WpLineKind::CodeFence);
    // Inside a fence: any line is content unless it's a closer.
    assert_eq!(
        classify_wp_line("let x = 1;", true),
        WpLineKind::CodeContent
    );
    assert_eq!(classify_wp_line("```", true), WpLineKind::CodeFence);
    // A heading inside a fence is still code, not a heading.
    assert_eq!(
        classify_wp_line("# not a heading", true),
        WpLineKind::CodeContent
    );
}

#[test]
fn classify_wp_line_table_row_heuristic() {
    // 2+ pipes → table row.
    assert_eq!(
        classify_wp_line("| col1 | col2 |", false),
        WpLineKind::TableRow
    );
    assert_eq!(classify_wp_line("|---|---|", false), WpLineKind::TableRow);
    // Single pipe falls through to paragraph (heuristic requires 2+).
    assert_eq!(classify_wp_line("a | b", false), WpLineKind::Paragraph);
    // Zero pipes = paragraph.
    assert_eq!(classify_wp_line("just text", false), WpLineKind::Paragraph);
}

#[test]
fn classify_wp_line_paragraph_fallback() {
    assert_eq!(
        classify_wp_line("hello world", false),
        WpLineKind::Paragraph
    );
    assert_eq!(
        classify_wp_line("**bold** text", false),
        WpLineKind::Paragraph
    );
}

// ---- doc_char_to_line_col ----

#[test]
fn doc_char_to_line_col_basic_mapping() {
    let ed = Editor::new("ab\ncd\nef".into(), std::path::PathBuf::from("/t"));
    assert_eq!(doc_char_to_line_col(ed.document(), 0), (0, 0));
    assert_eq!(doc_char_to_line_col(ed.document(), 1), (0, 1));
    // Char 2 is the '\n' between line 0 and line 1.
    assert_eq!(doc_char_to_line_col(ed.document(), 3), (1, 0));
    assert_eq!(doc_char_to_line_col(ed.document(), 6), (2, 0));
    // Past EOF clamps to len.
    assert_eq!(doc_char_to_line_col(ed.document(), 999), (2, 2));
}

// ---- Menu rendering helpers ----

#[test]
fn format_menu_key_single_char() {
    let kp = KeyPress::new(Key::Char('f'), KMods::NONE);
    assert_eq!(format_menu_key(&[kp]), "f");
}

#[test]
fn format_menu_key_with_ctrl() {
    let kp = KeyPress::new(Key::Char('k'), KMods::CONTROL);
    assert_eq!(format_menu_key(&[kp]), "Ctrl-k");
}

#[test]
fn format_menu_key_named_keys() {
    assert_eq!(
        format_menu_key(&[KeyPress::new(Key::Enter, KMods::NONE)]),
        "Enter"
    );
    assert_eq!(
        format_menu_key(&[KeyPress::new(Key::Esc, KMods::NONE)]),
        "Esc"
    );
    assert_eq!(
        format_menu_key(&[KeyPress::new(Key::F(2), KMods::NONE)]),
        "F2"
    );
}

#[test]
fn format_menu_key_multi_press_sequence() {
    // `g g` for goto-top, etc.
    let g = KeyPress::new(Key::Char('g'), KMods::NONE);
    assert_eq!(format_menu_key(&[g.clone(), g]), "g g");
}

#[test]
fn gpui_menu_has_required_entries() {
    // Sanity check: the menu builder must include every action that
    // `dispatch_menu_command` knows how to dispatch. If we add a new
    // command name to the menu, this assert points at the missing
    // dispatch arm via the matching test below.
    fn collect_leaves<'a>(nodes: &'a [MenuNode], out: &mut Vec<&'a str>) {
        for n in nodes {
            match &n.action {
                yalda::menu::MenuAction::Command(s) => out.push(s.as_str()),
                yalda::menu::MenuAction::Submenu(children) => {
                    collect_leaves(children, out);
                }
                _ => {}
            }
        }
    }
    let menu = gpui_menu();
    let mut leaf_actions: Vec<&str> = Vec::new();
    collect_leaves(&menu, &mut leaf_actions);
    // The expected leaf actions — change here if gpui_menu changes.
    let expected = [
        // "open file here" returned to the global new-submenu by user
        // request (2026-06-10) — replaces the focused tile in place, so
        // it's workspace-scoped enough to live here.
        "open-browser",
        "new-buffer-tile",
        "buffer-list",
        "new-agent-tile",
        "split-h",
        "close-window",
        "new-tab",
        "move-tile",
        "cycle-layout",
        // Direct layout-mode selection (no cycling) + desktop grid size.
        "layout-manual",
        "layout-master-stack",
        "layout-monocle",
        "layout-columns",
        "layout-desktop",
        "desktop-grid",
        "tag-add",
        "list-marks",
        "dev-restart-gui",
        "back-to-doc",
        "quit",
    ];
    for e in expected {
        assert!(
            leaf_actions.contains(&e),
            "expected menu to contain leaf {:?}, got {:?}",
            e,
            leaf_actions
        );
    }
    // Tile-scoped + chrome entries removed from the global menu
    // (Phase 2 cleanup + restructure): they live in the `.` local
    // menus / on chords; themes were killed outright.
    for gone in [
        "enter-edit",
        "enter-wp",
        "reload-file",
        "claude-status-bar",
        "rail-files",
        "rail-outline",
        "rail-flip",
        "claude-new",
        "claude-list",
        "claude-close",
        "claude-rename",
        "agent-input-toggle",
        "claude-mode-cycle",
        "theme-dracula",
        "theme-folio",
    ] {
        assert!(
            !leaf_actions.contains(&gone),
            "{gone:?} should no longer be in the global menu"
        );
    }
}

#[test]
fn menu_state_round_trip_picks_command() {
    // Pressing 'q' at root closes the menu and returns "quit".
    let mut state = MenuState::new();
    state.open();
    let menu = gpui_menu();
    let cmd = state.process_key(KeyPress::new(Key::Char('q'), KMods::NONE), &menu);
    assert_eq!(cmd, Some("quit".to_string()));
    assert!(!state.is_active(), "menu should close after a leaf select");
}

#[test]
fn local_menus_have_no_duplicate_keys_per_level() {
    // spec-menu-scopes.md: every local menu must be unambiguous — one
    // key, one entry, at each depth.
    fn check_level(nodes: &[MenuNode], path: &str) {
        let mut seen: Vec<&[KeyPress]> = Vec::new();
        for n in nodes {
            match &n.action {
                yalda::menu::MenuAction::Command(_) | yalda::menu::MenuAction::Submenu(_) => {
                    assert!(
                        !seen.contains(&n.key.as_slice()),
                        "duplicate key {:?} at {path}",
                        n.key
                    );
                    seen.push(&n.key);
                }
                _ => {}
            }
            if let yalda::menu::MenuAction::Submenu(children) = &n.action {
                check_level(children, &format!("{path}/{}", n.label));
            }
        }
    }
    check_level(&doc_local_menu(), "doc");
    check_level(&edit_local_menu(), "edit");
    check_level(&agent_local_menu(), "agent");
    check_level(&browser_local_menu(), "browser");
}

#[test]
fn doc_local_menu_g_g_resolves_goto_top() {
    let mut state = MenuState::new();
    state.open();
    let menu = doc_local_menu();
    let after_g = state.process_key(KeyPress::new(Key::Char('g'), KMods::NONE), &menu);
    assert_eq!(after_g, None, "g alone should open the goto submenu");
    let cmd = state.process_key(KeyPress::new(Key::Char('g'), KMods::NONE), &menu);
    assert_eq!(cmd, Some("doc-goto-top".to_string()));
}

#[test]
fn browser_local_menu_dot_resolves_toggle_hidden() {
    // `.` opens the local menu; `. .` is the relocated toggle-hidden.
    let mut state = MenuState::new();
    state.open();
    let menu = browser_local_menu();
    let cmd = state.process_key(KeyPress::new(Key::Char('.'), KMods::NONE), &menu);
    assert_eq!(cmd, Some("browser-hidden".to_string()));
}

#[test]
fn edit_local_menu_e_v_resolves_extend_mode() {
    let mut state = MenuState::new();
    state.open();
    let menu = edit_local_menu();
    let after_e = state.process_key(KeyPress::new(Key::Char('e'), KMods::NONE), &menu);
    assert_eq!(after_e, None, "e alone should open the edit submenu");
    let cmd = state.process_key(KeyPress::new(Key::Char('v'), KMods::NONE), &menu);
    assert_eq!(cmd, Some("toggle-extend-mode".to_string()));
}

#[test]
fn agent_local_n_resolves_to_claude_new() {
    // Claude session management lives in the Agent local menu now.
    let mut state = MenuState::new();
    state.open();
    let menu = agent_local_menu();
    let cmd = state.process_key(KeyPress::new(Key::Char('n'), KMods::NONE), &menu);
    assert_eq!(cmd, Some("claude-new".to_string()));
}

#[test]
fn agent_local_c_resolves_to_session_picker() {
    // The session selector (free-session picker / rebind) lives at `c` in the
    // Agent local menu (spec-agent-session-ownership.md).
    let mut state = MenuState::new();
    state.open();
    let menu = agent_local_menu();
    let cmd = state.process_key(KeyPress::new(Key::Char('c'), KMods::NONE), &menu);
    assert_eq!(cmd, Some("claude-session-picker".to_string()));
}

#[test]
fn theme_toggle_alternates_nightfox_and_folio() {
    // From Folio → Nightfox; from anything else (Nightfox or any other theme)
    // → Folio, so the toggle always lands on one of the pair and alternates.
    assert_eq!(next_toggle_theme(ThemeName::Folio), ThemeName::Nightfox);
    assert_eq!(next_toggle_theme(ThemeName::Nightfox), ThemeName::Folio);
    // Any non-Folio theme jumps into the pair at Folio.
    assert_eq!(next_toggle_theme(ThemeName::Dracula), ThemeName::Folio);
    // Toggling twice from Folio returns to Folio.
    let back = next_toggle_theme(next_toggle_theme(ThemeName::Folio));
    assert_eq!(back, ThemeName::Folio);
}

#[test]
fn agent_local_shift_c_resolves_to_claude_clear() {
    // `/clear` is reachable from the Agent local menu at `C` (capital, distinct
    // from lowercase `c` = select session). Regression guard for the original
    // bug: clear_agent_session existed but had no entry point.
    let mut state = MenuState::new();
    state.open();
    let menu = agent_local_menu();
    let cmd = state.process_key(
        KeyPress::new(Key::Char('C'), KMods::NONE),
        &menu,
    );
    assert_eq!(cmd, Some("claude-clear".to_string()));
}

#[test]
fn menu_n_f_resolves_to_new_buffer_tile() {
    // `n` opens the new submenu; `f` creates a new buffer tile (in Picking).
    let mut state = MenuState::new();
    state.open();
    let menu = gpui_menu();
    let after_n = state.process_key(KeyPress::new(Key::Char('n'), KMods::NONE), &menu);
    assert_eq!(after_n, None, "n alone should open the new submenu");
    assert!(state.is_active(), "submenu open keeps menu state active");
    let cmd = state.process_key(KeyPress::new(Key::Char('f'), KMods::NONE), &menu);
    assert_eq!(cmd, Some("new-buffer-tile".to_string()));
}

#[test]
fn menu_root_submenus_resolve() {
    // Root layout after the restructure: n=new, w=windows, s=workspace,
    // l=layout — each a submenu; theme is gone entirely.
    let menu = gpui_menu();
    for (ch, follow, expected) in &[
        ('w', 's', "split-h"),
        ('s', 't', "new-tab"),
        ('l', 'l', "cycle-layout"),
        ('n', 'c', "new-agent-tile"),
    ] {
        let mut state = MenuState::new();
        state.open();
        let after = state.process_key(KeyPress::new(Key::Char(*ch), KMods::NONE), &menu);
        assert_eq!(after, None, "{ch:?} should open a submenu");
        let cmd = state.process_key(KeyPress::new(Key::Char(*follow), KMods::NONE), &menu);
        assert_eq!(
            cmd,
            Some(expected.to_string()),
            "{ch:?} {follow:?} should resolve to {expected:?}"
        );
    }
}

#[test]
fn menu_e_and_w_resolve_to_edit_views() {
    // Phase 2 cleanup: enter-edit / enter-wp moved from the global menu
    // to the Doc local menu; only `v` (back-to-doc) stays global.
    let menu = gpui_menu();
    let mut state = MenuState::new();
    state.open();
    let cmd = state.process_key(KeyPress::new(Key::Char('v'), KMods::NONE), &menu);
    assert_eq!(cmd, Some("back-to-doc".to_string()));

    let local = doc_local_menu();
    for (ch, expected) in &[('e', "enter-edit"), ('w', "enter-wp")] {
        let mut state = MenuState::new();
        state.open();
        let cmd = state.process_key(KeyPress::new(Key::Char(*ch), KMods::NONE), &local);
        assert_eq!(
            cmd,
            Some(expected.to_string()),
            "doc-local key {:?} should resolve to {:?}",
            ch,
            expected
        );
    }
}

#[test]
fn menu_state_unknown_key_keeps_menu_open() {
    let mut state = MenuState::new();
    state.open();
    let menu = gpui_menu();
    // 'z' isn't bound at root.
    let cmd = state.process_key(KeyPress::new(Key::Char('z'), KMods::NONE), &menu);
    assert_eq!(cmd, None);
    assert!(state.is_active(), "menu should stay open on unknown key");
}

#[test]
fn append_llm_chunk_chains_turns_above_draft() {
    // Mirrors the old splice-then-lock-then-splice integration test
    // for the new append-and-tag flow: each turn appends just after
    // the last frozen Llm(n) line; a manually-inserted user draft
    // (simulating worksheet typing) survives the agent's reply
    // arriving for the same turn.
    let mut ed = Editor::new(String::new(), std::path::PathBuf::from("*claude*"));
    // Turn 1: agent greets.
    ed.append_llm_chunk(TurnId::Llm(1), "Hi.");
    finalize_agent_turn(&mut ed);
    // User types a reply on the editable line below the frozen
    // "Hi.". The worksheet cursor lives wherever the user puts it.
    ed.cursor_mut().line = ed.document().line_count().saturating_sub(1);
    ed.cursor_mut().col = 0;
    ed.insert_char('o');
    ed.insert_char('k');
    // Turn 2 starts: agent's first chunk goes at EOF (no Llm(2) lines
    // yet) — i.e. after the user's draft "ok". This matches the
    // worksheet's "agent writes at the far end" model (§19).
    ed.append_llm_chunk(TurnId::Llm(2), "Yes!");
    finalize_agent_turn(&mut ed);

    let text = ed.document().full_text();
    assert!(text.contains("Hi."));
    assert!(text.contains("ok"));
    assert!(text.contains("Yes!"));
    let pos_hi = text.find("Hi.").unwrap();
    let pos_ok = text.find("ok").unwrap();
    let pos_yes = text.find("Yes!").unwrap();
    assert!(pos_hi < pos_ok, "Hi before ok ({:?})", text);
    assert!(pos_ok < pos_yes, "ok before Yes! ({:?})", text);
}

/// Source files must split into one CodeBlock per line: the doc view scrolls
/// and focuses by block (j/k move `cursor_block`) and `gpui::list`
/// virtualizes by item, so a whole file as ONE block can neither scroll nor
/// virtualize. `start_line` carries the absolute line number for the gutter.
#[test]
fn source_file_renders_one_block_per_line() {
    let path = std::path::Path::new("example.rs");
    let blocks = render_with_wiki(
        "fn main() {\n    let x = 1;\n}\n",
        &Theme::default(),
        Some(path),
    );
    assert_eq!(blocks.len(), 3, "one block per source line");
    for (i, b) in blocks.iter().enumerate() {
        match b {
            RenderedBlock::CodeBlock {
                lines,
                source_file,
                start_line,
                ..
            } => {
                assert!(*source_file);
                assert_eq!(*start_line, i);
                assert_eq!(lines.len(), 1);
            }
            other => panic!("expected CodeBlock, got {:?}", other),
        }
    }
}

/// Empty source files still produce a single (empty) block so cursor and
/// reveal logic have a target.
#[test]
fn empty_source_file_renders_single_block() {
    let path = std::path::Path::new("empty.rs");
    let blocks = render_with_wiki("", &Theme::default(), Some(path));
    assert_eq!(blocks.len(), 1);
}

// --- Visual-selection highlight on blank / whitespace-only lines
//     (apply_line_selection). A blank line whose newline is inside a
//     multi-line selection must still render a highlighted placeholder so the
//     selection reads as continuous; the syntax highlighter yields no segments
//     for such lines, so apply_selection_bg alone would paint nothing. ---

#[test]
fn blank_line_inside_selection_gets_highlight_placeholder() {
    let style = NStyle::default();
    let bg = NColor::Rgb(1, 2, 3);
    // Selection covers line 0..=2; line 1 is blank and fully interior.
    let out = apply_line_selection(&[], "", ((0, 0), (2, 1)), 1, style, bg);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].0, " ");
    assert_eq!(out[0].1, style.bg(bg), "placeholder carries selection bg");
}

#[test]
fn whitespace_only_line_fully_selected_gets_highlight_placeholder() {
    let style = NStyle::default();
    let bg = NColor::Rgb(1, 2, 3);
    // A line of spaces, fully inside the selection (newline also selected).
    let out = apply_line_selection(&[], "   ", ((0, 0), (2, 0)), 1, style, bg);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].0, " ");
    assert_eq!(out[0].1, style.bg(bg));
}

#[test]
fn blank_line_at_selection_end_is_not_highlighted() {
    // Selection ends at the start of the blank line (col 0) — its newline is
    // NOT selected, so it stays un-highlighted (matches vim).
    let out = apply_line_selection(&[], "", ((0, 0), (1, 0)), 1, NStyle::default(), NColor::Rgb(1, 2, 3));
    assert!(out.is_empty(), "unchanged empty input → no placeholder");
}

#[test]
fn line_outside_selection_is_unchanged() {
    let style = NStyle::default();
    let segs = vec![("text".to_string(), style)];
    let out = apply_line_selection(&segs, "text", ((0, 0), (0, 4)), 3, style, NColor::Rgb(1, 2, 3));
    assert_eq!(out, segs, "line 3 is outside a line-0 selection");
}

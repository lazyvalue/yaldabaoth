# bug-0013: tool-call-still-splits-agent-sentence

**Status:** FIXED
**First seen:** 2026-07-21 (evidence: `~/ws/fulcrum/tmp/Screenshot 2026-07-21 at 11.33.00 AM.png`)
**Component:** `docs/components/agent-tile/transcript.md` — violates `UXI-AgentTile-8`

## Symptom

"Sometimes tool use still breaks up agent text." A tool-call row lands in the middle
of a streamed agent sentence and the prose is cut around it. From the 11:33 screenshot,
two live instances in one turn:

- `Verification running. To close the loop on your question: the fix for` | `2 cd, 1 tail` |
  ` it on my side is to stop delegating long test runs to subagents…` — one sentence,
  guillotined after "for".
- A transcript line containing nothing but `.` — the sentence's terminating period
  arrived as the post-tool chunk and was stranded on its own line.

Expected: the tool row appears after the completed sentence; the prose reads as the
model wrote it.

## Context / root cause

`UXI-AgentTile-8` and its fix (`dbe67be`, 2026-07-08) already cover the
**word-cut-in-half** case (`` `m `` | tool | `ode=max`). That fix,
`Editor::midtoken_rejoin_point` (`src/editor.rs`), is deliberately narrow: it rejoins
the continuation onto the open tail line ONLY when **both** the open line's last
content char AND the incoming chunk's first char are `is_alphanumeric()`.

Both symptoms above fall outside that window, so the guard returns `None` and
`find_llm_insertion_point`'s "tail ends with `\n` → different turn on the next line →
splice at EOF" branch does the splitting:

| case | open line's last char | chunk head | old rule fires? |
|------|----------------------|-----------|-----------------|
| `` `m `` \| tool \| `ode=max` | `m` alnum | `o` alnum | yes (fixed 07-08) |
| `…the fix for` \| tool \| `_it on my side` | `r` alnum | **space** | no → split |
| `…runn` \| tool \| `.` | alnum | **`.`** | no → stranded `.` |

The conservative rule was chosen to avoid mis-fusing an ambiguous punctuation split.
But the real invariant the user wants is coarser than "don't cut a word": **a tool row
may only break agent prose at a sentence boundary.** Everything else is a streaming
artifact of a `ReplyEvent` tool notification arriving between two text deltas of one
content block.

## Planned solution

Widen the guard from "mid-word" to "mid-sentence" — rejoin unless the open run ended at
a genuine sentence boundary. New rule for the (renamed) rejoin point:

1. Tail still OPEN and tagged this turn — unchanged.
2. Chunk head is `\n`/`\r` ⇒ `None` (the model itself ended the line; a real block
   break is a legitimate `text → tool → text` interleave).
3. Otherwise take the open line's content, **trim trailing whitespace**, strip a run of
   closing markup (`*_`~)]}"'»”’`) so `*…done.*` still reads as terminated, and check
   the resulting last char: one of `.!?:` ⇒ `None` (legitimate interleave, tool stays
   between the statements). Anything else ⇒ rejoin at end-of-content.

This is strictly WIDER than the old rule (last-char-alnum implies not `.!?:`), so the
existing `dbe67be` guard test keeps passing unchanged. The trailing-whitespace trim is
what preserves the complementary invariant pinned by
`tests.rs::floored_tools_and_text_stay_in_order_above_draft` (chunks ending `". "` must
STAY interleaved with their tools) — without it, that trailing space would read as a
non-terminator and wrongly fuse.

**How this differs from the 07-08 attempt:** that one asked "was a *word* cut?" and
answered with a symmetric alphanumeric test on both sides of the break. This asks "did
the model finish a *sentence* before the tool arrived?" — a property of the pre-tool
text alone. The chunk-head test is dropped entirely except for the newline case.

## Approaches already tried (do NOT repeat)

- **Symmetric alphanumeric mid-word test** (`midtoken_rejoin_point`, `dbe67be`,
  2026-07-08, `UXI-AgentTile-8`) — correct as far as it goes and still shipping, but
  only covers the both-sides-alnum window. It does NOT hold for a continuation starting
  with whitespace or punctuation, which is this bug. Do not re-narrow it to an
  alphanumeric test.

---

## Log

### 2026-07-21 — widened the rejoin rule from mid-word to mid-sentence

**Localized first.** `verify_harness.rs::tool_call_midsentence_does_not_split_agent_sentence`
drives the REAL reducer (`apply_server_batch` → `append_llm_chunk_floored`) with the
screenshot's two shapes and reproduced the buffer exactly before any fix:

```
"To close the loop on your question: the fix for\n\n it on my side is to stop delegating long test runs to subagents\n\n.\n"
```

**Changed** (`src/editor.rs`): `midtoken_rejoin_point` → `continuation_rejoin_point`,
with the symmetric alphanumeric test replaced by the three gates in Planned solution
(open tail, chunk-head-is-newline ⇒ interleave, pre-tool text must not end on `.!?:`
after trailing-whitespace trim + `SENTENCE_CLOSERS` strip). New module const
`SENTENCE_CLOSERS`. Call site in `append_llm_chunk_floored` renamed; no other callers.

**Verified.** New guard green; `tool_call_midtoken_does_not_split_agent_text_run`
(the 07-08 mid-word case) still green UNCHANGED, confirming the new rule is a strict
superset; `tests.rs::floored_tools_and_text_stay_in_order_above_draft` still green,
confirming a chunk ending `". "` still legitimately interleaves with its tool (this is
the case the trailing-whitespace trim exists to protect). Suites: 400 bin + 156 lib.

**Negative control observed RED.** Re-inserted the old pair of `is_alphanumeric()`
gates into `continuation_rejoin_point`: the new test failed on "a tool must not cut a
sentence at a non-word boundary" while the old mid-word test stayed green — i.e. the
guard fails for exactly the right reason and the two tests cover distinct windows.
Restored, re-ran green.

**Unverified / caveats.** (1) Runtime: not yet seen in the live app — the user runs
`main` via `./dev-gui.sh`, so this needs a rebuild + restart before it reaches them
(anti-circling rule 5). (2) `:` is treated as a sentence terminator so a lead-in like
"three levers, ranked:" still interleaves ahead of its list; if a tool ever lands
inside a time like "2:15" that split would survive. (3) The rule reads only the tail
LINE, so a tool arriving after the model already emitted `\n` is untouched by design.

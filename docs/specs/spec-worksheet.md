# Worksheet — the inline editing model (behavioral contract)

**Status:** AUTHORITATIVE (2026-06-28). This is the user's behavioral contract for
worksheet mode. Where it contradicts the worksheet/chatbox sections of
`spec-agent-window.md` (§4–§20) or the *UX* of ADR-0024 (Model C), **this spec
wins** and those are SUPERSEDED. It does **not** override Model C's *implementation
durability* (see "Durability" below).

## The point

The worksheet is an **editable conversation buffer**. You read the whole
conversation, put your cursor wherever you want, and type a reply *in place* —
the way it worked before the Model C UX regression turned it into a read-only
transcript with a separate always-on compose box. The behavior below is the spec;
the implementation underneath stays Model C.

## Model C is the substrate, not the UX

Model C (ADR-0024) is kept as the **durable implementation** because it is what
killed the ordering-corruption bug class (a turn streaming/replaying into the
middle of the document, a draft stranded mid-history, "fixed" 15+ times). Its
properties that MUST continue to hold:

- The transcript is the **single ordered source of truth**; agent content only
  ever **appends/streams at a clean EOF** of the live region.
- **Only text** authored by the user enters the transcript — never a smuggled
  position that a later reconcile can mis-place.
- Replay rebuilds **committed** turns in event order.

These survive because of the behavioral constraints below — specifically: **the
user can only edit while the agent is idle**, so streaming never lands in a buffer
that is being edited. The one thing Model C's *UX* forbade — an editable region
inside the transcript — is re-allowed, but **bounded** (one block, latest turn
only, idle only) so the corruption case stays unrepresentable.

## The rules (the contract — verbatim intent)

### Worksheet — agent idle

1. **Free navigation.** In **Normal** mode the cursor moves freely over the entire
   transcript. Nothing is editable until you enter Insert; this is pure read
   navigation.

2. **Insert opens a You-block at the cursor.** Entering **Insert** mode at an
   insertable point creates a **You-block**: a `You` delimiter at the cursor point
   with an editable region. Your typed text lives inside that block.

3. **Empty insert is a no-op.** If you leave Insert having typed **no
   non-whitespace text**, the You-block — delimiter and all — **disappears**. The
   transcript is byte-identical to before you entered Insert. (No phantom "You"
   turns; the no-empty-turn rule, INV-UX-4.)

4. **Non-empty You-block persists and is sent.** If you typed non-whitespace text,
   the You-block **persists in place** as your pending reply. The next **Submit**
   (`Ctrl-Enter`) sends that text to the agent and **freezes** the block as a
   committed user turn.

5. **You can only insert into the most recent agent turn.** A You-block may be
   opened **only within the agent's most-recent turn**, and **only after an agent
   newline** (at a line boundary — never mid-line). Frozen content (earlier agent
   turns, already-committed user turns) is **not** editable; trying to enter Insert
   there does nothing (status hint), it does not open a block.

6. **One You-block at a time.** There is at most one pending You-block — "the
   editable set is exactly what you've typed since the last Submit." This is the
   constraint that keeps the worksheet a stateful prompt board, not a chat log with
   floating annotations (it also bounds the durability surface to a single region).

### Chatbox — agent mid-turn

7. **Mid-turn input goes to the chatbox; the chatbox exists only mid-turn.** While
   the agent is **writing (mid-turn)** the transcript is fully read-only (no
   You-block can be opened) and a **chatbox appears pinned at the bottom of the
   tile**; everything you type goes there. Submitting from the chatbox steers /
   queues per INV-UX-7 (turn steering). When the turn ends the chatbox is **not
   visible** (it hides when empty) and inline worksheet editing is available again.

## Frozen-text rules (the guard rails behind rule 5/7)

- **Insert only after an agent newline.** The caret must sit at a line boundary
  inside the editable region; you cannot split an agent line mid-word.
- **Insert only amidst the most recent agent turn.** Lines belonging to older
  turns are frozen; the editable window is the latest turn's lines plus its tail.
- A blank trailing editable line at EOF is always a legal insertion point (so you
  can always reply at the end).

## Durability (why rules 5–7 make this safe on Model C)

- **Idle-only editing** (rule 7): a You-block exists only when nothing is
  streaming, so the agent never appends into a region the user is editing. During a
  turn the transcript is append-only exactly as Model C requires (the EOF-floor
  property holds whenever it matters).
- **Commit-on-submit** (rule 4): a You-block becomes a normal committed user turn
  before the next turn streams; the agent's following output appends after it.
- **Replay** sees only committed turns; an uncommitted (or empty, rule 3)
  You-block never existed, so resume rebuilds a clean ordered transcript.

## Cursor / viewport (inherited invariants)

- **INV-UX-1** holds: the caret is always visible and the viewport tracks it. The
  worksheet is **cursor-anchored** — streaming output elsewhere does not yank the
  viewport away from where you are editing (sticky-bottom only when the caret is at
  EOF). See `spec-agent-window.md` §19.
- **INV-UX-2** holds: the editable region word-wraps.

## Default & the chatbox toggle

New sessions **default to Worksheet** (stage 3). The chatbox is primarily the
**mid-turn input surface** (rule 7) — it auto-appears while the agent is writing
and is hidden when idle. It is **also** available as an optional *persistent*
placement via `Ctrl-Alt-Enter` (a pinned bottom box for users who prefer a plain
message box); that toggle is not required for normal use and the worksheet is the
canonical mode.

## What this supersedes

- `spec-agent-window.md` §4–§7 (two **co-equal** input modes, defaulting to
  Chatbox): the worksheet is now the **default and canonical** mode; the chatbox
  is the mid-turn surface (auto, idle-hidden) plus an optional persistent
  placement (above).
- The Model C *UX* (read-only transcript + always-present separate compose,
  ADR-0024 "Consequences" → UX bullets). The Model C *data architecture* stays.

## Enforcement

`docs/ux-invariants.md` **INV-UX-9** states this as an invariant and names the
headless guards in `verify_harness.rs`.

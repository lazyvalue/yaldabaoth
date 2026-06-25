# PRD — Agent Worksheet Mode (redesign)

Status: **Draft** · Owner: Scott · Surface: `yalda-gpui` agent tile

## 0. Why this document exists

The agent tile supports two input surfaces: **Message Box** (a pinned compose
box; conventional chat) and **Worksheet** (input composed directly inside the
transcript). Worksheet is currently shipped but **fundamentally broken** — the
behaviors below are the *intended* product, not the present reality. This PRD
defines the target. It does **not** describe the current implementation, which
is being replaced.

Non-negotiable constraint: **Message Box and all other agent-tile behavior must
be preserved unchanged.** Worksheet is the only thing being redesigned. Message
Box is the safety net the worksheet redesign must never regress.

## 1. The bet

The product premise is that **a coding-agent conversation is better modeled as a
shared, editable document than as a chat log.** Message Box is the conventional
fallback: a pinned compose box, transcript read-only above it, send-and-it's-
gone. Worksheet's entire reason to exist is everything Message Box *can't* do.
If the document-as-conversation premise is wrong, this feature should not exist —
so every requirement below serves that premise.

## 2. Product requirements

### PR-1 — Compose in place, interleaved with output
The user types their next prompt **directly in the transcript**, not in an
exiled box at the bottom. Input and agent output share one surface. The user can
place their draft where it is contextually relevant (e.g. immediately under the
agent output it responds to), not only at the tail.

### PR-2 — The transcript is a workspace, not a receipt
The user can:
- stage multiple lines / thoughts before sending,
- edit a draft across several distinct edits over time,
- see their in-progress prompt alongside the agent's prior output,
without any of it being sent until they choose to submit.

### PR-3 — Authorship and turn structure stay legible
Because input and output are interleaved in one document, the UI must keep
**who wrote what** unambiguous. Every line carries a turn attribution shown in a
gutter:
- agent output → its turn number,
- user-sent input → `U<n>` (the turn it was sent as part of),
- tool activity → `T<n>`,
- local/system notices → blank, excluded from turn counting,
- **un-submitted draft lines → visually distinct from everything sent** (this is
  the load-bearing distinction; the user must always be able to tell, at a
  glance, what is a live draft versus a frozen record).

### PR-4 — Sent history is immutable; drafts are live
- Once submitted, the user's lines **freeze**: they become an immutable, faithful
  record of exactly what was sent. They cannot be edited, split, or merged.
- Anything not yet submitted stays **fully editable** as ordinary text.
- Agent output and tool blocks are never user-editable.

### PR-5 — Submit semantics are predictable
- A single, discoverable gesture submits the pending draft.
- Submit sends **the editable (un-frozen) user content**, in document order,
  as one prompt.
- On a successful send, exactly those lines freeze as one new `U<n>` turn.
- On a failed send, **nothing is lost** — the draft stays editable so the user
  can retry. The failure is surfaced, never silent.
- Submitting drives no duplicate rendering: the user's frozen lines and the
  server's echo of the same turn must resolve to a single rendered turn.

### PR-6 — Editing a document with immutable regions feels natural
Navigating and editing a transcript that mixes frozen and editable lines must
not fight the user:
- the cursor moves freely across frozen and editable lines,
- selection and copy span frozen regions normally,
- typing, newline, and delete are only blocked where they would mutate a frozen
  line — and that block is quiet (a no-op), not an error,
- the user can always start a fresh draft line adjacent to frozen content.

### PR-7 — Switching surfaces never loses work
Toggling Worksheet ⇄ Message Box preserves the user's in-progress draft. Moving a
draft between surfaces does not drop text or silently freeze it.

### PR-8 — Performance is invisible
Typing latency does not grow with transcript length. The worksheet must stay
responsive in a long session — see the responsiveness invariants in the tech
design; this is a product requirement because a laggy compose surface defeats
the entire premise.

## 3. Explicit non-goals

- Rich-text / WYSIWYG editing of prompts. Worksheet is plain text.
- Editing or "rewinding" already-sent turns by mutating frozen lines.
- Replacing Message Box. The two modes coexist; Message Box is unchanged.
- Multi-user / collaborative editing of the same transcript.

## 4. Success criteria

1. Every behavior in §2 is demonstrable at runtime, and the broken behaviors that
   triggered this redesign are gone.
2. Message Box behavior is provably unchanged (regression tests + runtime check).
3. Agent-buffer invariants (frozen immutability, turn attribution, no double-
   render, O(changed) typing) are pinned by unit/functional tests so the class of
   bugs that made worksheet "fundamentally broken" cannot silently return.

## 5. Open questions (resolved in the tech design)

- The exact data model for draft-vs-frozen and per-line turn identity.
- The component boundary between transcript, input surface, and turn/freeze
  bookkeeping.
- The submit/freeze/reconcile pipeline and where it lives.
- The concrete failure modes of the current implementation (to be enumerated as
  the design's regression-test corpus).

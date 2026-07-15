# ADR-0026: Make impossible states unrepresentable — use types to a neurotic level

**Status:** Accepted
**Date:** 2026-07-14
**Related:** ADR-0025 (identity-based session restore), ADR-0023 (WorkspaceCwd typed
field), `docs/bugs/bug-0001-created-session-not-persisted.md`,
`docs/components/agent-tile/session-binding.md`, CLAUDE.md § The anti-circling rules

## Context

The auto-resume-on-restart feature (UXI-AgentTile-18/19) was "fixed" **three times**,
each with a green, negative-controlled test, each shipping a still-broken app:

1. **Identity via `resume_id` / `channel.session_id()`.** The guard set the tile's
   cached id BY HAND, so it never exercised the real save-side resolution.
2. **Resolve the id via the store's `sid_of`.** The guard asserted the *in-memory*
   snapshot, not the `workspace.json` FILE restore reads.
3. **Sync `workspace.json` on save.** The two persistence files had drifted on disk.

Every one of these was a **desynchronised-state bug**: two things that were supposed
to agree (an in-memory cache vs the store; `acp_sessions.json` vs `workspace.json`;
a tile's `bound` vs its `picker`) were allowed by the types to disagree. Rust's whole
value proposition — *make illegal states unrepresentable, and the compiler carries the
burden manual testing otherwise carries* — was being thrown away. We were using Rust
like a dynamically-typed language with extra ceremony: a bag of `Option`s and bare
`String`s, with the real invariants living only in prose comments and my fallible
attention. That is the root cause, not any individual off-by-one.

### Case study: three type smells, three failures

**Smell 1 — `AgentTile` is Option-soup, not a state machine.**
```rust
struct AgentTile {
    bound: Option<SessionId>,
    pending_open_token: Option<u64>,
    picker: Option<SessionPicker>,
    resume_sid: Option<String>,
    unavailable: Option<SharedString>,
}
```
Four orthogonal `Option`s encode ~4 real states (Selecting / Opening / Bound /
Unavailable) → 16 representable combinations, ~4 legal. `bound` *and* `picker` both
`Some`, all-`None`, `unavailable`-while-`bound` are all constructible; the render
disambiguates with ad-hoc `if let … else`. Adding the Unavailable state was fragile
precisely because nothing forced the other paths to account for it.

**Smell 2 — the server session id is stringly-typed.** There are two id-spaces: a
typed local `SessionId` handle and a server session id that is a bare `String`
everywhere (`resume_id: Option<String>`, store `sid: Option<String>`,
`bind_sid(id, "…".into())`). Nothing at the type level distinguishes them or prevents
mixing them.

**Smell 3 — a duplicated source of truth (`resume_sid`).** The server sid already
lives authoritatively in the store's `by_sid` map. `resume_sid` is a *copy* on the
tile, lazily stamped by `save_agent_ring`. A cache of an authoritative value with no
type-level link to its source is a drift waiting to happen — and it drifted, which
is bug-0001. Likewise the session id is persisted into BOTH `workspace.json` and
`acp_sessions.json` by two different methods at two different times; the types let
the two files disagree.

If `AgentTile` had been an enum and the sid had one typed owner, **each of the three
bugs would have been a compile error**, not a runtime surprise a hand-written test
failed to reproduce.

## Decision

**Impossible states must be unrepresentable. We use types aggressively — to a
neurotic, almost psychotic level — to push invariants from prose+attention into the
compiler.** Concretely, the standing rules for this codebase:

1. **A thing with N mutually-exclusive states is an `enum`, never a struct of
   `Option`s.** If two fields are "one xor the other," they are one sum type. Prefer
   data-carrying variants so a state's data can only exist in that state.
2. **Distinct id-spaces / units get newtypes, not primitives.** A server session id
   is a `ServerSid(String)`, not a `String`. Mixing it with a `SessionId` must not
   compile.
3. **One source of truth. No caches of authoritative values** unless the cache is
   provably derived at a single chokepoint and can't be observed stale. Resolve from
   the owner (the store) at use time; if the access is awkward (no `cx`), fix the
   access — do not add a field that can lie.
4. **Data that must agree lives in one place** and everything else *derives* it. Two
   files/fields that "should match" are a bug latent in the type design; collapse
   them or make one the sole writer.
5. **Make the guard check the real artifact, and let the types make the illegal case
   uncompilable.** A green test over a proxy state is worthless (the whole
   anti-circling saga). The strongest test is one you cannot even write the bug past —
   an exhaustive `match` the compiler forces.

### The refactor this mandates (session/tile state)

- `AgentTile` → an enum state machine. **The real transitions must be mapped first** —
  a naive `Selecting | Bound | Unavailable` DROPS a live state: a tile can be bound to
  its old session *and* have an open/reopen round-trip in flight
  (`pending_open_token`), which `reconcile_session_closed` treats as "respawning, do
  not drop to the picker." Encoding that as a modifier, not losing it:
  ```rust
  enum AgentTile {
      Selecting(SessionPicker),
      // Bound; `reopening` carries the token while a change-cwd/reopen round-trip is
      // in flight (the old "bound + pending_open_token" respawn state).
      Bound { session: SessionId, reopening: Option<OpenToken> },
      // First open from the picker: a create is in flight, nothing bound yet.
      Opening { token: OpenToken },
      Unavailable { remembered: ServerSid, lost: SharedString },
  }
  ```
  Discovering this state BEFORE coding — instead of shipping the naive enum with green
  tests — is itself the ADR working: the type design forced the question "what states
  actually exist?", which the Option-soup never did.
- `ServerSid(String)` newtype for the server session id across the store, persistence,
  and channel.
- Delete `resume_sid`: `Bound` resolves its sid from the store (`sid_of`);
  `Unavailable` carries `remembered`; `Selecting` has none. No cached copy.
- Persistence derives the per-tile id from the tile enum at save time (single write),
  so `workspace.json` and the side-channel can't drift.

### What Rust actually offers (and where the wishlist doesn't map)

Be honest about the tool. Rust is **not** Haskell/Idris — it has **no native
higher-kinded types, no dependent types, no type families, no GADTs**. Precisely:
- No HKT: you cannot abstract over a type constructor (`trait Functor<F<_>>`). **GATs**
  (generic associated types, stable 1.65) encode a *restricted* slice of it via the
  type-family/defunctionalization trick, but that is an encoding, not `F<_>`, and does
  not help here. **HRTBs** (`for<'a>`) are higher-*ranked* over lifetimes, not
  higher-*kinded* over types — adjacent name, different feature.
- No dependent types: values don't appear in types, except **const generics** (a
  narrow "values in types" sliver for array sizes) — the closest, and appropriately
  small.

Reaching for those by name is a category error; claiming to use them would be the
same overclaiming that caused this ADR. What Rust DOES give us is more than enough to
make our bug uncompilable:

- **Sum types (`enum`)** — the workhorse. Illegal combinations of states simply have
  no representation. This alone kills the `AgentTile` Option-soup bug.
- **Newtypes + parse-don't-validate** — `ServerSid(String)` with a private field and a
  smart constructor: an invalid or wrong-space id can't be *constructed*, so it can't
  exist downstream. Distinct id-spaces stop being confusable.
- **Typestate (phantom types / `PhantomData`)** — encode a state machine in the type
  so invalid transitions don't compile (methods consume `self`, return the next state
  type). The nearest Rust has to "the type tracks the state."
  **Limit that matters here:** typestate needs monomorphic sites. A *collection* of
  tiles in different states (our `Layout<App>` tree) can't hold N distinct state
  types — so the **enum** (runtime tag + compiler-enforced exhaustive `match`) is the
  correct tool for `AgentTile`, not typestate. Use typestate for *linear* lifecycles
  (a builder, a single connection), enums for *heterogeneous collections*.
- **Sealed traits** — closed sets of implementors (the GADT/type-family use-case we'd
  reach for) modeled as a trait no downstream crate can implement.
- **Const generics** — the one sliver of "values in types" Rust has (array sizes,
  small dimension checks); the closest thing to dependent typing, and appropriately
  narrow.
- **`#[non_exhaustive]`, `#[must_use]`, ownership/lifetimes, `!Send`** — more
  invariants the compiler carries for free.

The rule is therefore: **push every invariant we can into the type with the strongest
tool that FITS — enum first, newtype second, typestate where linear, sealed trait for
closed sets** — and be honest that the ceiling is Rust's, not Haskell's. Where a
guarantee genuinely can't be typed (e.g. the live daemon's session actually exists —
that's I/O, not a value we own), it stays a runtime check, named as such, never
pretended into the type system.

## Alternatives rejected

- **Keep patching + more tests.** Three green tests already coexisted with a broken
  app; the failure mode is *unreproduced state*, and more example-tests don't close
  a representable-but-illegal state. The fix is to make the state unrepresentable.
- **Runtime asserts / `debug_assert!` on the invariants.** Moves the check to run
  time on the paths that happen to execute — the exact gap the saga exploited. A
  `match` arm the compiler demands is strictly stronger and free.
- **Leave the id stringly-typed "for convenience."** The convenience is what let the
  two id-spaces and the two persistence files silently disagree.

## Status of the refactor

- **`AgentTile` enum — DONE** (this ADR's commit). ~95 field-access sites + 24
  constructors converted to `match`/transition methods; the `resume_sid` cache is
  **deleted**; persistence resolves the sid from the store via a `SidResolver`
  threaded through `snapshot_content`/`snapshot_layout`/`save_persisted_workspace`
  (single source of truth — `sid_of` is cx-free, so no cache is needed). 369 tests
  green incl. the state-machine fuzzer + oracle. The bug-0001 guards
  (`agent_tile_persists_session_identity_not_index`,
  `save_agent_ring_persists_session_id_to_workspace_json`,
  `created_server_session_persists_its_id_for_restore`) now drive the enum.
- **`ServerSid` newtype — PENDING** (separate pass). The server sid is still `String`
  across the store / persist / wire; newtyping it kills the id-space smell but is a
  large mechanical ripple that crosses the `session_proto` wire boundary and has not
  itself caused a bug — lower leverage than the enum, tracked as follow-up.

## Consequences

- A real refactor across the render, save, and restore paths (every `tile.bound` /
  `tile.picker` / `tile.unavailable` / `tile.resume_sid` / `tile.pending_open_token`
  access). Each becomes a `match`; the compiler enumerates the sites.
- New states/transitions (e.g. a future "detached" or "branching" tile) are added as
  variants — every consumer is then forced to handle them or fail to build.
- This ADR is the general principle, not a one-off: it applies to any future
  state-bag in the codebase (compose modes, turn phases, layout kinds). ADR-0023
  (`WorkspaceCwd`) was an early instance of the same move; this generalises it.

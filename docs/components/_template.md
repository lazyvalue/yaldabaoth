# Component: <Name>

**Status:** <draft | living>
**Component token:** `<Component>` (⇒ invariants are `UXI-<Component>-N`)

## Description

<Prose. What this component is, its role in the app, its states/modes. Enough that a
reader understands it without reading code. Name the primary code home
(file/struct).>

## References

- `docs/components/common/<shared>.md` — <why this component depends on it>
- `docs/specs/spec-<...>.md` — <deeper design doc, if any>
- ADR-<NNNN> — <decision that shaped this, if any>

## UX invariants

### UXI-<Component>-1 — <short title>

**Statement.** <Declarative present, testable. What must always be true.>

**Applies to.** <Surfaces + real code symbols: files, functions, structs.>

**Why.** <The problem this prevents.>

**Status.** `implemented` | `partial` | `not implemented`

**Enforcement.** <`verify_harness.rs::<test>` / `tests.rs::<test>`, or the named
human runtime check for a genuine paint/subprocess/timing gap. Neither ⇒ a gap.>

<!-- Repeat UXI-<Component>-N for each behavior / visual element / sub-component.
     Ids are stable + append-only; never renumber. -->

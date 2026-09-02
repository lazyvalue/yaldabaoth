# bug-0065: Cog agent names still show three-letter ids

**Status:** FIXED  
**First seen:** 2026-09-01  
**Component:** `docs/components/cog.md` (`UXI-Cog-19`)  
**Cog:** `m72`

## Symptom

The Cog window continued to show terse routing ids such as `ncz` across Chat
metadata and graph history even after communication entry authors had been made
readable.

## Root cause

The original repair introduced a directory-backed formatter only inside
`communication_card`. Other semantic agent fields independently rendered their
raw strings, so the fix covered senders but not identities throughout the Cog
surface.

## Fix

`CogView` now owns one address-directory-backed formatter for individual and
list identities. Chat creators/members, Note and mail participants/authors,
agent loading chrome, graph transition actors, and graph-note authors all use
it. Registered names lead and stable ids remain secondary; unknown and empty
fallbacks are preserved. Routing/object ids and arbitrary JSON remain literal.

## Attempts

### 2026-08-30 — communication authors only

- Resolved Note, Chat, inbox, and agent-thread entry senders.
- Missed semantic identities outside the shared communication card.

### 2026-09-01 — unify every semantic identity projection

- Added label-keyed GPUI paint probes through the real Home, Chat, graph, node,
  and agent-mail reducer/click paths.
- Observed the guard RED on raw Chat creator `ncz`, then GREEN after using the
  shared formatter everywhere.
- Negative control bypassed directory lookup and failed on bare `ncz` exactly as
  intended.

export const meta = {
  name: 'refactor-review',
  description: 'Multi-lens refactor review over a resolved code scope: each lens finds design/quality improvements, a skeptic verifies each finding, and a synthesizer emits one ranked, implementation-ready report.',
  phases: [
    { title: 'Review', detail: 'one finder per selected lens' },
    { title: 'Verify', detail: 'a skeptic refutes each finding as its lens lands' },
    { title: 'Synthesize', detail: 'dedup across lenses, rank, write the report' },
  ],
}

// ---------------------------------------------------------------------------
// args, supplied by the /refactor skill after it resolves the scope inline:
//   {
//     scopeLabel:      string,    // human description, e.g. "ingest" or "db writes in queue handlers"
//     files:           string[],  // resolved work-list, repo-relative paths
//     lenses:          string[],  // selected lens keys (subset of LENS keys below)
//     migrationActive: boolean,   // is this scope in play for the Python -> Rust migration?
//     notes:           string,    // one-paragraph map of what's in the scope (from inline resolution)
//   }
// Returns: the full markdown report body as a string.
// ---------------------------------------------------------------------------

// The runtime delivers `args` as a JSON string; parse it. Tolerate object or absent.
const A = typeof args === 'string' ? JSON.parse(args) : (args || {})
const FILES = A.files || []
const MIGRATION = !!A.migrationActive
const SCOPE = A.scopeLabel || '(unspecified scope)'
const NOTES = A.notes || ''

// --- Shared philosophy preamble: grounds every agent in Fulcrum's rules ----
const PHILOSOPHY = `You are reviewing EXISTING code to find refactors that make the system simpler and more provably correct. You do not edit code; you report findings. Fulcrum's design philosophy is the standard you measure against:

- Functional by default (ADR-005): plain functions for stateless ops; classes only for genuine instance state; factories are functions.
- Types make invalid states unrepresentable. Invariants and decisions are encoded in types, not in comments or scattered runtime checks. Untrusted data is PARSED into a validated domain type once at the trust boundary (queue payload, connector data, API body, DB row) and never flows inward as a raw dict/string. Functions are total over their input type, or the input type is narrowed until they are.
- Powerful abstractions make simple systems, and simple systems are easier to prove correct. Prefer a few sharp abstractions over many shallow ones. An abstraction that leaks domain rules into "generic" shared code is a liability, not reuse.
- Errors are modeled. A function signature tells the truth about how it fails (Result-shaped), rather than throwing across many layers or silently swallowing.
- Infrastructure is reached only through interfaces (ADR-001: FileStore, EmbeddingClient, VectorStore), never concrete backends (S3/MinIO/pgvector) directly.
- Invariants are written in EARS notation with INV-<MODULE>-<NNN> IDs (ADR-010) and guarded by tests marked @pytest.mark.invariant.

EVERY finding should name a concrete ENFORCEMENT HOOK — the artifact that would keep the improvement true over time: a type that excludes the bad state, a test (ideally @pytest.mark.invariant), an import-linter contract, a pyright-strict boundary, a schema/DB constraint, or a new EARS invariant. A finding that cannot name any hook is probably taste: you may still raise it, but say so and mark its confidence low.

${MIGRATION ? `This scope is IN PLAY for the Python -> Rust migration (PyO3 workers). For every finding, judge migration impact: does the change move logic toward a stable, well-typed boundary Rust can own, reduce Python/Rust semantic drift, and preserve contract compatibility during the migration? A change that makes the Python harder to port cleanly should be flagged or down-weighted.` : `This scope is not currently part of the Python -> Rust migration. Set migration_impact to "n/a" unless a finding has obvious migration relevance.`}

Rules: report findings only, never propose to apply edits. Do not flag pure formatting or style. Every finding must cite specific file:line evidence. If the scope is clean on your lens, return an empty findings list — a clean result is a valid, useful outcome.`

const FILE_LIST = FILES.length
  ? `Files in scope (read what you need; reads are unconstrained):\n${FILES.map(f => `- ${f}`).join('\n')}`
  : 'No explicit file list was provided; the scope notes below describe the region to review.'

const SCOPE_BLOCK = `Scope: ${SCOPE}\n${NOTES ? `\nScope map:\n${NOTES}\n` : ''}\n${FILE_LIST}`

// --- The lens catalog: each entry is one finder's mandate -------------------
const LENS = {
  architecture: `LENS — Architectural pattern fit. What architecture is this code converging toward (event sourcing, CQRS, actor model, hexagonal/ports-and-adapters, pipeline, saga/process-manager, state machine, repository/service)? Is it consistently following one model, or silently mixing incompatible ones? Flag the mixing and the cost of resolving it toward a single coherent model.`,

  mutation_authority: `LENS — Mutation & authority boundaries. Who is allowed to mutate durable state? Are writes funneled through a small set of capability-bearing APIs, or are there hidden write paths? Can a caller bypass an invariant by importing the wrong module and writing directly? Flag every uncontrolled or duplicated write path to durable state.`,

  state_lifecycle: `LENS — State & lifecycle modeling. Are lifecycle states explicit types/enums, and are transitions constrained so impossible transitions are unrepresentable (not merely documented)? Are retry, terminal-failure, lease/visibility expiry, cancellation, and replay states actually modeled? Flag states that live as loose booleans/strings or implicit conventions.`,

  effects_purity: `LENS — Effect boundaries & purity. Where do IO, DB writes, network calls, clock reads, randomness, queue ops, and logging enter? Are pure DECISION functions separated from effectful APPLICATION, so the core logic is testable without infrastructure? Flag decision logic entangled with effects, and effects reachable from places that claim to be pure.`,

  type_modeling: `LENS — Type modeling & domain types. Where would a new domain type make invalid states unrepresentable or encode an invariant currently enforced by runtime checks or comments? Is untrusted input parsed into a validated type at the boundary (parse, don't validate), or passed inward as raw dicts/strings? Do signatures tell the truth about failure (Result-shaped) instead of throwing/swallowing? Are functions total over their input type? Flag primitive-obsession, stringly-typed data, and partial functions.`,

  abstraction_reuse: `LENS — Abstraction & reuse. Where is the same logic duplicated such that a single well-named function/abstraction would simplify the system? CAUTION against false dedup: two blocks that look similar but represent DIFFERENT concepts must not be merged — coupling them is a defect, not reuse. Justify, for each, that the call sites are the same concept and would change together.`,

  interfaces_coupling: `LENS — Interface design & semantic coupling. Are module boundaries and encapsulation clean? Detect SEMANTIC coupling: a module that knows too much about another's internal states, schema quirks, retry behavior, or ordering assumptions; or a "generic" helper that smuggles one caller's domain rules into shared code. Flag the leak and name the boundary that should hide it.`,

  concurrency_resources: `LENS — Concurrency, transactions & resources. Find data races, deadlock/lock-ordering risks, and transaction problems (too-wide or too-narrow boundaries, read-modify-write without locking, isolation assumptions). Find backpressure failures: unbounded queues/buffers, producers outrunning consumers, fan-out with no concurrency limit. Find resource-lifecycle defects: connections/handles/transactions acquired without guaranteed release, leak paths, ownership unclear. For each apparent race, check whether it is already serialized (single-writer, transaction, FOR UPDATE SKIP LOCKED) before raising it.`,

  liveness_termination: `LENS — Liveness & termination. Find non-terminating loops and message loops that never drain: queue redrive with no terminal condition, missing retry cap / dead-letter / max-attempts, missing visibility-timeout or lease bound, conditions that re-enqueue forever. Find idempotency/replay defects: at-least-once delivery means a handler that is not idempotent corrupts state on redelivery. Before raising "infinite loop", verify there is genuinely no retry cap, DLQ, or terminal state you missed.`,

  algorithms: `LENS — Algorithmic efficiency. Find inefficient algorithms and access patterns (accidental O(n^2), N+1 queries, repeated full scans, work inside a loop that could be hoisted). Only raise a finding where the REAL input size makes it bite; state the expected magnitude. Do not flag micro-optimizations on small bounded inputs.`,

  observability: `LENS — Observability & auditability. Can we reconstruct what happened after the fact? Are decisions, inputs, state transitions, and emitted effects traceable (logged/recorded with enough structure)? Does the design support replay, reconciliation, and debugging? Flag silent decision points and effects that leave no trace, especially around state transitions and external calls.`,

  anti_patterns: `LENS — Residual anti-patterns. Catch general anti-patterns the structured lenses miss: import side effects (ADR-008), service access via app.state instead of Depends() (ADR-006), god functions/files over the 1000-LOC ceiling, dead code, leaky defaults that diverge from cloud (ADR-011), config consumed without boot-time validation. Each finding still needs an enforcement hook.`,
}

// Default to all lenses when the skill did not pre-select a subset.
const LENSES = (A.lenses && A.lenses.length ? A.lenses : Object.keys(LENS))

// Verifier refutation hints, matched to each lens's characteristic false-positive mode.
const REFUTE = {
  architecture: 'Is the "mixing" real, or two legitimately different concerns that only look like competing patterns?',
  mutation_authority: 'Is the alternate write path actually reachable and uncontrolled, or already gated by a transaction/guard?',
  state_lifecycle: 'Is the loose state actually reachable in an invalid combination, or constrained elsewhere?',
  effects_purity: 'Is the logic genuinely a pure decision that could be extracted, or irreducibly effectful?',
  type_modeling: 'Would the proposed type EXCLUDE a real invalid state, or just rename an existing one? Is the input genuinely untrusted?',
  abstraction_reuse: 'Are the call sites the SAME concept that will change together, or coincidentally similar? Default to NOT merging.',
  interfaces_coupling: 'Is this real cross-module knowledge, or unavoidable shared vocabulary at a legitimate seam?',
  concurrency_resources: 'Is the race/leak real, or already serialized by a transaction, single-writer, SKIP LOCKED, or context-manager/Drop?',
  liveness_termination: 'Is there genuinely no retry cap / DLQ / terminal state / idempotency key, or did the finder miss one?',
  algorithms: 'Does the REAL input magnitude make this matter, or is the input small and bounded?',
  observability: 'Is the decision/effect genuinely untraceable, or recorded somewhere the finder did not look?',
  anti_patterns: 'Is this a real anti-pattern with a hook, or stylistic taste?',
}

// --- Structured-output schemas ---------------------------------------------
const FINDING_PROPS = {
  finding: { type: 'string', description: 'One-line statement of the issue.' },
  location: { type: 'string', description: 'file:line references, comma-separated.' },
  evidence: { type: 'string', description: 'What is in the code now and why it is a problem.' },
  invariant: { type: 'string', description: 'The invariant violated or absent. Cite an INV-... id if one exists, else state the implicit rule.' },
  risk: { type: 'string', description: 'What breaks, or what stays unprovable, if left as-is.' },
  refactor_move: { type: 'string', description: 'The concrete change to make.' },
  why_better: { type: 'string', description: 'Why this is architecturally better in Fulcrum\'s philosophy: provability/simplicity gained.' },
  migration_impact: { type: 'string', description: 'Toward or away from a Rust-ownable boundary; or "n/a".' },
  enforcement_hook: { type: 'string', description: 'The hook that keeps it true: type | test | lint | import-linter boundary | schema constraint | EARS invariant | none(taste).' },
  effort: { type: 'string', enum: ['S', 'M', 'L'], description: 'Rough implementation effort.' },
  confidence: { type: 'string', enum: ['high', 'med', 'low'], description: 'Finder confidence; low if hook is "none".' },
}

const FINDINGS_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['findings'],
  properties: {
    findings: {
      type: 'array',
      items: {
        type: 'object',
        additionalProperties: false,
        required: ['finding', 'location', 'evidence', 'invariant', 'risk', 'refactor_move', 'why_better', 'migration_impact', 'enforcement_hook', 'effort', 'confidence'],
        properties: FINDING_PROPS,
      },
    },
  },
}

// One verifier per lens reviews all of that lens's findings at once and returns
// one verdict per finding, keyed by the finding's 0-based index in the input list.
const BATCH_VERDICT_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['verdicts'],
  properties: {
    verdicts: {
      type: 'array',
      items: {
        type: 'object',
        additionalProperties: false,
        required: ['index', 'real', 'behavior_preserving', 'has_real_hook', 'taste_leaning', 'adjusted_confidence', 'note'],
        properties: {
          index: { type: 'integer', description: '0-based index of the finding this verdict is for, matching the input list.' },
          real: { type: 'boolean', description: 'Is the finding genuinely true after scrutiny?' },
          behavior_preserving: { type: 'boolean', description: 'Would the refactor preserve observable behavior?' },
          has_real_hook: { type: 'boolean', description: 'Does the enforcement hook actually pin the improvement?' },
          taste_leaning: { type: 'boolean', description: 'Is this closer to taste than to an enforceable correctness/simplicity gain?' },
          adjusted_confidence: { type: 'string', enum: ['high', 'med', 'low'], description: 'Confidence after refutation.' },
          note: { type: 'string', description: 'One or two sentences: why the verdict, and any correction to the finding.' },
        },
      },
    },
  },
}

// --- Prompt builders --------------------------------------------------------
const reviewPrompt = (key) => `${PHILOSOPHY}

${SCOPE_BLOCK}

${LENS[key]}

Return every finding that survives your own scrutiny. Be specific and cite file:line. Empty findings list is fine if the scope is clean on this lens.`

const batchVerifyPrompt = (findings, key) => `${PHILOSOPHY}

You are an adversarial verifier. A reviewer raised the findings below, all from the ${key} lens. REFUTE each one: read the cited code and decide whether it survives. Do not be harsh for its own sake, but do not pass a finding you cannot confirm.

Refutation focus for this lens: ${REFUTE[key] || 'Is the finding real and enforceable?'}

For EACH finding, read the code at its cited locations and decide: is it real, would the refactor preserve behavior, and does the hook actually pin the improvement? A finding with no enforceable hook is taste — keep it but mark taste_leaning true and confidence low. A finding that is simply wrong gets real=false. Return exactly one verdict per finding, with index matching the list.

Findings (JSON, indexed):
${JSON.stringify(findings.map((f, i) => ({ index: i, finding: f.finding, location: f.location, evidence: f.evidence, refactor_move: f.refactor_move, enforcement_hook: f.enforcement_hook })), null, 2)}`

const synthPrompt = (verified) => `${PHILOSOPHY}

The findings below have each passed an adversarial verifier. Produce ONE markdown report.

Tasks:
1. DEDUP across lenses: when several findings point at the same code region or the same root cause, merge them into one finding and list all contributing lenses.
2. RANK by (impact on provability and simplicity) x (confidence) / (effort). A finding that makes a class of bug unrepresentable via a type, or removes a non-termination/race hazard, outranks a local cleanup. Down-rank taste_leaning findings.
3. Make each finding implementation-ready and self-contained, so the reader can hand any single one straight to an implementer.

Verified findings (JSON):
${JSON.stringify(verified, null, 2)}

Output EXACTLY this markdown structure and nothing else:

# Refactor review — ${SCOPE}

_Scope: ${FILES.length} file(s). Lenses: ${LENSES.join(', ')}._

## Summary
<3-5 sentences: the architectural through-line, the highest-leverage moves, and any systemic pattern the findings share.>

## Findings (ranked)

### N. <title>  ·  <lens(es)>  ·  effort <S/M/L>  ·  confidence <high/med/low>
- **Location:** <file:line, ...>
- **Evidence:** ...
- **Invariant violated / absent:** ...
- **Risk:** ...
- **Refactor move:** ...
- **Why architecturally better:** ...
- **Migration impact:** ...
- **Enforcement hook:** ...

<repeat for each finding, numbered by rank>

## Taste-leaning / low-confidence (optional, listed not argued)
- <one line each, with location — items that lacked a clear enforcement hook>

If, after dedup, there are no enforceable findings, say so plainly under Summary and list only the taste-leaning section.`

// --- Orchestration: Review -> Verify (pipelined) -> Synthesize -------------
log(`Reviewing ${FILES.length} file(s) in "${SCOPE}" across ${LENSES.length} lens(es)${MIGRATION ? ', migration-aware' : ''}.`)

const reviewed = await pipeline(
  LENSES,
  (key) => agent(reviewPrompt(key), { label: `review:${key}`, phase: 'Review', schema: FINDINGS_SCHEMA }),
  (review, key) => {
    const findings = (review && review.findings) || []
    if (!findings.length) return []
    return agent(batchVerifyPrompt(findings, key), { label: `verify:${key}`, phase: 'Verify', schema: BATCH_VERDICT_SCHEMA })
      .then((res) => {
        const verdicts = (res && res.verdicts) || []
        return findings.map((f, i) => ({ ...f, lens: key, verdict: verdicts.find((v) => v.index === i) || null }))
      })
      .catch(() => findings.map((f) => ({ ...f, lens: key, verdict: null })))
  },
)

// Drop only findings the verifier judged simply wrong (real === false). Keep taste-leaning
// findings, and keep findings whose batch verdict was lost to an error (verdict === null).
const verified = reviewed
  .flat()
  .filter(Boolean)
  .filter((f) => !(f.verdict && f.verdict.real === false))

log(`${verified.length} finding(s) survived verification. Synthesizing report.`)

if (!verified.length) {
  return `# Refactor review — ${SCOPE}\n\n_Scope: ${FILES.length} file(s). Lenses: ${LENSES.join(', ')}._\n\n## Summary\nNo enforceable findings survived adversarial verification. The reviewed scope is clean on the selected lenses, or the candidate findings were taste rather than enforceable correctness/simplicity gains.\n`
}

const report = await agent(synthPrompt(verified), { label: 'synthesize', phase: 'Synthesize' })
return report

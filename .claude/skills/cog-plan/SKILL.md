---
name: cog-plan
description: Decompose approved multi-step work into a Cog dependency graph with independently executable nodes and an explicit parallel frontier. Use when the user asks to plan, break down, or orchestrate work with Cog, or wants a graph that cog-execute can run.
---

# Plan with Cog

Turn `$ARGUMENTS` (or the work described in the conversation) into a Cog graph.

## 1. Check the boundary

- Read the relevant project instructions, design documents, and code before
  decomposing the work.
- Confirm that `cog graph list` can reach `cogd`. If it cannot, report the
  missing server and expected address; do not pretend the graph exists.
- Resolve factual questions from the repository. Ask the user only about choices
  that materially change scope or architecture.
- Choose one stable actor for the host: `claude-code` in Claude Code and `codex`
  in Codex. Pass it with `--actor` on every mutation.

## 2. Design the graph

- Make each node independently executable and verifiable. Two nodes that can run
  concurrently must not share an edge.
- Add an edge only when the downstream node needs the upstream node's result.
- Give every terminal work node an edge to `omega`; otherwise it is an island and
  never becomes ready.
- Use unique, short node names because graph import resolves edges by name.
- Put enough context in each node for a worker that has not seen this conversation:

```json
{
  "title": "Short imperative",
  "description": "Scope, relevant context, constraints, and expected result",
  "done_when": ["Concrete acceptance criterion", "Verification command"],
  "hints": {"files": ["optional/path"], "spec": "optional/spec.md"}
}
```

Omit `hints` when it adds no value.

## 3. Get approval

- Present node names, one-line purposes, dependency edges, and the resulting
  frontier waves.
- Treat the user's implementation request as approval to materialize the graph.
  Ask again only when the graph expands or leaves the requested scope ambiguous.

## 4. Materialize atomically

Prefer one `cog graph import - --actor <actor>` call with this shape:

```json
{
  "name": "short-graph-name",
  "description": "The user-visible goal",
  "omega_content": {"purpose": "Confirm the whole graph is complete"},
  "nodes": [
    {"name": "task-name", "content": {"title": "...", "description": "...", "done_when": ["..."]}}
  ],
  "edges": [
    {"from": "task-name", "to": "omega"}
  ]
}
```

Use incremental `graph create`, `node add`, and `edge add` commands only when
extending an existing graph.

## 5. Validate and hand off

- Run `cog graph islands <graph-id>`; it must return an empty list.
- Run `cog graph render <graph-id> --frontiers` and compare it with the approved
  waves.
- Report the graph id, its ready nodes, and the exact handoff:
  `/cog-execute <graph-id>`.

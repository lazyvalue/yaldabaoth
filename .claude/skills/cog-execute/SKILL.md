---
name: cog-execute
description: Drive an existing Cog graph to completion by claiming ready nodes, delegating independent work, maintaining leases, recording outputs and deviations, and closing omega. Use when the user supplies a Cog graph id, asks to execute or resume a Cog plan, or wants agents to work the ready frontier.
---

# Execute a Cog graph

Execute graph `$ARGUMENTS`. Require a graph id; if none is present in the
arguments or conversation, ask for it.

## 1. Recover current state

- Choose one stable actor for the host: `claude-code` in Claude Code and `codex`
  in Codex. Pass it with `--actor` on every mutation.
- Read `cog graph get`, `cog graph status`, `cog graph render --frontiers`, and
  `cog graph read-notes` before claiming work. A graph may be resumed after a
  partial run; trust Cog's state rather than the conversation's memory.
- If the graph is complete, report its outputs and stop.

## 2. Claim the frontier

- Repeatedly run `cog node claim-next <graph> --with-inputs --actor <actor>`
  until it exits 3. Each successful response contains one claimed node and its
  predecessor outputs.
- Exit 3 means only that the ready set is empty. Check `graph status`; it does not
  by itself mean the graph is complete.
- If a claimed node is `omega`, close it with a concise aggregate output after
  confirming all preceding work is closed, then verify the graph is complete.

## 3. Run claimed work

- Dispatch file-independent nodes to separate subagents in parallel when the
  active host and repository policy permit delegation. Otherwise execute ready
  nodes serially. Serialize nodes that can edit the same files.
- Give each worker the node id, content, predecessor inputs, project
  instructions, and this lifecycle contract.
- Treat `content.description` as scope and `content.done_when` as the acceptance
  contract. Inspect the repository as needed; do not invent requirements.
- Heartbeat every claimed node with `cog node heartbeat <node> --actor <actor>`
  before its 60-second lease expires. Workers should heartbeat after major tool
  cycles; the coordinator should also heartbeat while waiting when possible.
- Verify the node's acceptance criteria before declaring success.

## 4. Record the outcome

- On success, close the node with `--resolution done --output <json>`. Include a
  concise summary, verification performed, material files or artifacts, and
  information successors need.
- Record plan changes, surprises, or necessary scope adjustments with `cog node
  add-note <node> --topic deviation --data <json> --actor <actor>` before closing
  the node. Use `cog graph add-note` for cross-cutting decisions or deviations.
- On a recoverable interruption, release the node. On a real task failure, add a
  failure note, close it with `--resolution failed --output <json>`, stop the
  dispatch loop, and report the failure. Do not silently unlock and execute
  downstream work after a failed prerequisite.

## 5. Continue to omega

- Re-read graph status and claim the newly ready frontier after every batch.
- Continue until omega is closed and `cog graph status <graph>` reports complete.
- Report the final graph status, node outputs, verification, and any deviation or
  failure notes. Distinguish verified results from assumptions.

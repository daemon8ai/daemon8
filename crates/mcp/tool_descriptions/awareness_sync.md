Capture or update project-state awareness.

Use only when durable semantic state changes: objective, question, hypothesis, fact, decision, constraint, risk, or blocker. Do not call after every tool call and do not store raw logs or transcript text.

Durable `evidence_refs` must point at persistent conclusions or accepted records: session summaries, accepted research notes, plan items, decisions, fixed bugs, lessons, business rules, source verifications, user confirmations, or librarian nodes.

Ephemeral `signal_refs` may point at observations, checkpoints, log rows, browser/device events, or tool output. Signal refs can explain why the agent investigated something, but they do not prove or promote awareness by themselves.

`verify` requires at least one durable `evidence_ref`. Signal refs alone can never promote a node to verified.

Operations:

- `capture`: create a new awareness node.
- `update`: refine an existing node.
- `question`: record an open question.
- `resolve`: answer a question or close a blocker.
- `verify`: attach evidence and raise authority/confidence.
- `retire`: mark a stale, superseded, or disproven node inactive.

If daemon8 returns `conflict_detected`, stop and ask the user which truth should remain active before continuing.

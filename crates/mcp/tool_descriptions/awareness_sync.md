Capture or update project-state awareness.

Use only when durable semantic state changes: objective, question, hypothesis, fact, decision, constraint, risk, or blocker. Do not call after every tool call and do not store raw logs or transcript text.

Operations:

- `capture`: create a new awareness node.
- `update`: refine an existing node.
- `question`: record an open question.
- `resolve`: answer a question or close a blocker.
- `verify`: attach evidence and raise authority/confidence.
- `retire`: mark a stale, superseded, or disproven node inactive.

If daemon8 returns `conflict_detected`, stop and ask the user which truth should remain active before continuing.

Report source posture and compact project-state awareness.

Use at session start, debug-session start, after compaction, or before resuming complex work. With no `focus_path`, returns a compact manifest: source posture, active objectives, open questions, active hypotheses, stale-risk count, conflict count, and suggested focus paths.

Use `focus_path` with a small `depth` only when you need a bounded branch. `include_notes` and `include_evidence` default false to avoid flooding context.

Next: if state changed, call `awareness_sync`; if source posture is not optimal, inspect relevant librarian/source coverage before trusting checkpoint deltas.

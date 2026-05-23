## Purpose

Load focused daemon8 protocol help for the current LLM control flow.

## When

Use before connect to understand the response envelope or connect-first flow, or after connect when a task needs a specific protocol topic.

## Prereq

None. This is a diagnostic exception to connect-first.

## Args
  - topic: optional string. Omit or pass `"index"` for the topic list.

## Returns
Common envelope with `data.topic` and `data.body`.

## Errors

none expected.

## Next

Read the returned topic and call the daemon8 tool it indicates. For normal runtime work, call `daemon8_connect` before guarded tools.

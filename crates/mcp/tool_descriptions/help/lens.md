# Lens

A lens is a daemon-side filter that buffers matching observations between query calls. Useful when the observation stream is high-volume but you only care about a narrow slice — set the lens once, then `read_live_feed` returns a `lens_observations` array of new matches alongside the regular query payload.

## Lifecycle

- `set_lens(filter, capacity?)` — install a filter and ring buffer (default 200, max 1000). Returns the active status.
- `lens_status` — inspect the active lens: filter spec, buffered count, capacity, cursor.
- `clear_lens` — stop buffering.

## Filter shape

Same shape as `read_live_feed`: `kinds`, `severity_min`, `origins`, `service`, `source`, `source_instance`, `text_match`, `correlation_id`, `tags`, `include_system`. NOT supported: `since`, `limit`.

## Why

Lens is push-side filtering against the live broadcast. Querying with the same filter is pull-side. Use lens when you want continuous capture of "everything matching X" without burning round trips on `read_live_feed(text_match="X")` between every action.

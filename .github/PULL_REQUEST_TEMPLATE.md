<!--
Thanks for sending a PR. A few things before you hit submit:

1. Read CONTRIBUTING.md if this is your first PR.
2. If the change is non-trivial, an issue should already exist for scope alignment.
3. CLA Assistant will prompt on first PR. Every contribution must be signed.
-->

## What changed

<!-- One or two sentences on the behavior change. Include user-visible impact. -->

## Why

<!-- The problem this solves. Link the issue if one exists (#123). -->

## Subtree affected

- [ ] `daemon/`
- [ ] `sdks/php/`
- [ ] `sdks/php-laravel/`
- [ ] `sdks/php-symfony/`
- [ ] Docs / meta (root files, .github/, etc.)

## Tests

<!-- What you ran locally. Paste the last few lines of output or a summary. -->

- [ ] `cargo test --workspace -- --test-threads=1` green *(daemon/)*
- [ ] `cargo clippy --workspace -- -D warnings` clean *(daemon/)*
- [ ] `cargo fmt --check` clean *(daemon/)*
- [ ] `composer test` green *(sdks/)*

## Checklist

- [ ] CLA signed (CLA Assistant will prompt automatically)
- [ ] CHANGELOG updated under `[Unreleased]` if this is a user-visible change
- [ ] New behavior is covered by a test; bug fixes include a regression test
- [ ] Commit message is a single line, lowercase subtree prefix (`daemon:`, `sdks:`, `repo:`)

## Notes for reviewers

<!-- Anything worth calling out — tricky edge cases, alternatives you considered, follow-up work you'd queue. -->

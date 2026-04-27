# Security Policy

Daemon8 runs on developer machines and handles runtime output that may include
sensitive application data. Security reports are taken seriously and
prioritized above feature work.

## Reporting a vulnerability

**Do not open a public GitHub issue.** Send the report privately:

- **Email:** `mail@daemon8.ai`
- **Subject prefix:** `[SECURITY]` — helps triage

Include:

1. A description of the issue and its impact.
2. Repro steps or a proof-of-concept.
3. Affected component — the daemon binary (this repo) or a specific
   ingest endpoint.
4. Version or commit SHA of what you tested against.
5. Your disclosure timeline expectations, if any.

You will receive an acknowledgement within **72 hours**. We aim to ship a
fix and coordinated disclosure within **90 days**. If a fix takes longer,
we will tell you why and revise the target.

## Scope

In scope for this policy:

- The daemon binary and its Rust crates (this repo).
- Ingest endpoints (HTTP `/ingest`, UDP when enabled, Unix socket when enabled).
- MCP handlers (14 tools including query, command, lens, memory, and ingest tools).
- Chrome DevTools Protocol bridge — specifically, any path that could let a
  malicious page influence the daemon beyond its intended surface.
- Configuration file parsing and secret handling.

Handled separately (same reporting address):

- Marketing site — [`daemon8ai/daemon8-site`](https://github.com/daemon8ai/daemon8-site).
- daemon8.ai production surfaces —
  report to the same email with subject `[SECURITY] daemon8.ai production`.

Out of scope:

- Social engineering, physical access, stolen-device scenarios.
- Denial of service that requires no vulnerability (resource exhaustion
  against a daemon running with default limits and no additional
  configuration hardening).
- Issues in third-party dependencies that do not affect Daemon8's actual
  usage of them — report those upstream and let us know so we can track.
- Vulnerabilities in software not shipped with Daemon8 that happens to run
  alongside it (your browser, your OS, your editor).

## Safe harbor

Good-faith security research is welcome. We commit to:

- Not pursuing legal action against researchers who report in good faith
  under this policy.
- Not filing a complaint with your employer, school, or hosting provider.
- Treating your report as confidential until a coordinated disclosure
  window closes.

In exchange, we ask:

- Do not exfiltrate data beyond what's necessary to demonstrate the issue.
- Do not publicly disclose until we've agreed on a timeline.
- Do not test against daemon8.ai production beyond passive analysis. If you
  need to probe live infrastructure, email first and we'll set up a staging
  target.

## GPG

A maintainer GPG key will be published here once the OSS release is public.
Until then, `mail@daemon8.ai` is the canonical channel.

## Public disclosures

Post-fix, we publish a CVE (where applicable) and link the advisory in
`/CHANGELOG.md` under the release that carries the fix. Reporters are
credited unless they request anonymity.

Thanks for taking the time. If you find something, we want to know.

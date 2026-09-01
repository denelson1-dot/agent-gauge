# Security Policy

## Reporting a vulnerability

Please report security issues privately through GitHub's
[private vulnerability reporting](https://github.com/denelson1-dot/agent-gauge/security/advisories/new)
rather than opening a public issue.

Include what you found, how to reproduce it, and what an attacker could do with
it. You can expect an acknowledgement within a week.

## What Agent Gauge touches

Agent Gauge is a local application with a deliberately small surface. Knowing
what it does and does not do should help you judge whether something is a
vulnerability:

**It reads:**

- `~/.claude/.credentials.json` (`%USERPROFILE%\.claude\.credentials.json`) —
  read only, never written. It uses the bearer token there for exactly one
  request, to Anthropic's account usage endpoint, carrying nothing else. It
  never attempts to refresh an expired sign-in, because doing so could
  invalidate the token Claude Code is holding.
- The local Codex CLI, via `codex app-server`.
- Its own configuration, cache and state directories.

**It writes:**

- Its own configuration, cache and state directories.
- Claude Code's `settings.json`, and only its status-line command, to install or
  remove the capture. A prior value is preserved and restored on **Disconnect**.
  If that setting changes after connection, Agent Gauge reports a conflict and
  leaves the live setting untouched.

**It never:**

- Scrapes provider websites, or makes model calls.
- Sends telemetry, or contacts any host other than the provider that issued the
  credential being used.
- Transmits your credentials anywhere but back to that provider.

## Custom adapters

Custom adapters are local executables that you supply. They run with your user's
privileges, so they are the most security-relevant part of the application.

Agent Gauge will not execute a new or changed adapter until you explicitly trust
its current manifest **and** executable hashes. Any change to either file
invalidates trust and stops execution until you review it again. Naming an
`interpreter` in the manifest cannot change what runs without invalidating trust
first. Adapter execution is bounded by a timeout and an output limit.

Trust is a decision you make about a program on your own machine. Agent Gauge
enforces that the program cannot change behind your back; it cannot judge
whether the program was trustworthy to begin with.

## Unsigned installers

Release installers are unsigned, because signing requires a code-signing
certificate this project does not have. Windows SmartScreen will warn on first
run. Verify downloads against the checksums published with each release.

## Supported versions

Only the latest release receives fixes.

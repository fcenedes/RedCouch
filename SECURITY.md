# Security Policy

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| 0.1.x   | :white_check_mark: |

Only the latest release receives security fixes.

## Reporting a Vulnerability

**Please do not open a public GitHub issue for security vulnerabilities.**

Instead, report them privately through
[GitHub Security Advisories](https://github.com/fcenedes/RedCouch/security/advisories/new).

When reporting, include:

- A description of the vulnerability and its potential impact.
- Steps to reproduce the issue or a minimal proof of concept.
- The version(s) of RedCouch affected (or "latest" if unsure).

## What to Expect

- **Acknowledgement** within 72 hours of your report.
- An initial assessment and timeline within 1 week.
- A fix or mitigation released as soon as practical, typically within 30 days for confirmed issues.
- Credit in the release notes (unless you prefer to remain anonymous).

## Scope

This policy covers the `red_couch` Rust crate and the Redis module it produces.
Issues in upstream dependencies (Redis, redis-module-rs, etc.) should be reported
to their respective maintainers; however, if you are unsure where the issue lies,
feel free to report it here and we will triage accordingly.

## Disclosure Policy

We follow coordinated disclosure. We ask that you give us reasonable time to
address a reported vulnerability before any public disclosure.

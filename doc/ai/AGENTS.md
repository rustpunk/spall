# AGENTS.md

## Purpose

This directory contains durable AI onboarding documentation. It is not generated output.

## Responsibilities

- Keep onboarding guidance evidence-backed.
- Preserve certainty labels: `Verified`, `Strong inference`, `Hypothesis`, `Open question`.
- Centralize uncertainty in `80_OPEN_QUESTIONS.md`.
- Record meaningful architecture/onboarding changes in `AI_CHANGELOG.md`.

## Rules

- Do not present old plans as current facts without source or test evidence.
- Prefer deleting weak claims over making them sound confident.
- Keep links relative when possible.
- Do not duplicate long prose from root `AGENTS.md`; link to detail docs.
- Update this directory when root or local agent guidance changes.

## Local Commands

- `git diff --check`
- `rg -n "doc/ai" AGENTS.md doc/ai`
- Run a marker search for unfinished placeholder text before finalizing docs.

## Evidence

Initial contents were synthesized from repository inventory, current source/docs/CI, and read-only explorer reports.

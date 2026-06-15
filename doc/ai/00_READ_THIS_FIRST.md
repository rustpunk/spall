# Read This First

## Purpose

This file is the entry point for future agents. It explains how to read the onboarding set, how to label certainty, and what minimum context is required before editing.

## Status

Initial onboarding set created from a read-only repository inventory plus read-only explorer reports for `spall-core`, `spall-config`, `spall-openapi`, `spall-cli`, and repo-level docs/CI/examples.

## Evidence Labels

- `Verified`: Confirmed from current repository files or a command run in this documentation pass.
- `Strong inference`: Supported by multiple current files or source shape, but not directly stated as a rule.
- `Hypothesis`: Plausible, but based on limited evidence or older/stale docs.
- `Open question`: A known uncertainty that should be resolved before relying on it.

## Reading Order By Task

- Any code edit: root `AGENTS.md`, this file, [10_ARCHITECTURE.md](10_ARCHITECTURE.md), [30_DESIGN_RULES.md](30_DESIGN_RULES.md), relevant local `AGENTS.md`.
- CLI behavior: `spall-cli/AGENTS.md`, [40_COMMON_PATTERNS.md](40_COMMON_PATTERNS.md), [50_TESTING_AND_COMMANDS.md](50_TESTING_AND_COMMANDS.md).
- Core resolver/cache/YAML work: `spall-core/AGENTS.md`, [60_PERFORMANCE_NOTES.md](60_PERFORMANCE_NOTES.md).
- Transport-neutral request/streaming work: `spall-openapi/AGENTS.md`, [60_PERFORMANCE_NOTES.md](60_PERFORMANCE_NOTES.md).
- Config/auth config work: `spall-config/AGENTS.md`.
- Documentation-only work: [README.md](README.md), [AI_CHANGELOG.md](AI_CHANGELOG.md), `doc/ai/AGENTS.md`.

## Minimum Checklist Before Editing Code

- Confirm the task is not docs-only.
- Read the root `AGENTS.md`.
- Read the local `AGENTS.md` for each edited crate.
- Check [80_OPEN_QUESTIONS.md](80_OPEN_QUESTIONS.md) for relevant uncertainty.
- Identify the focused test command before editing.
- Do not add dependencies, change lockfiles, push, or commit without explicit approval.

## Repository Memory Model

Use this precedence when repo facts disagree:

1. Current source, tests, manifests, CI, and generated command output.
2. Current user docs in `docs/src/`.
3. Root `CLAUDE.md` and root/local `AGENTS.md`.
4. `doc/ai/` onboarding docs.
5. Older plans, scaffold prompts, and research notes under `docs/internal/`, `.hermes/`, `.pi/`, `notes/`, and root planning files.

Older planning docs are useful evidence of intent, but they can be stale. Prefer current implementation when there is a conflict.

## Rules For Future AI Agents

- Preserve crate boundaries.
- Keep uncertainty visible.
- Prefer evidence over broad summaries.
- Do not convert old plans into current facts without source verification.
- Update docs when changing architecture, commands, invariants, or public behavior.
- Keep `doc/ai` docs durable and concise.

## Definition Of Done

- The change is scoped to the request.
- Relevant source or docs were inspected.
- Focused verification was run, or skipped with a concrete reason.
- Commands and unverified assumptions are reported.
- `doc/ai/AI_CHANGELOG.md` and local `AGENTS.md` were updated if architecture or rules changed.

## Documentation Map

- [10_ARCHITECTURE.md](10_ARCHITECTURE.md): high-level architecture.
- [20_PROJECT_MAP.md](20_PROJECT_MAP.md): factual repo map.
- [30_DESIGN_RULES.md](30_DESIGN_RULES.md): practical rules with certainty labels.
- [40_COMMON_PATTERNS.md](40_COMMON_PATTERNS.md): repeated implementation patterns.
- [50_TESTING_AND_COMMANDS.md](50_TESTING_AND_COMMANDS.md): command guide.
- [60_PERFORMANCE_NOTES.md](60_PERFORMANCE_NOTES.md): performance-sensitive areas.
- [70_GLOSSARY.md](70_GLOSSARY.md): terms and project-specific names.
- [80_OPEN_QUESTIONS.md](80_OPEN_QUESTIONS.md): unresolved questions.
- [90_LOCAL_AGENT_PLAN.md](90_LOCAL_AGENT_PLAN.md): local guidance plan.
- [AI_CHANGELOG.md](AI_CHANGELOG.md): architecture/change memory.

## When To Update Which Doc

- Update architecture docs when crate responsibilities, data flow, or boundaries change.
- Update project map when files, crates, tests, examples, or CI change.
- Update design rules when an invariant changes or becomes verified.
- Update common patterns only when a pattern appears in more than one place.
- Update command docs when CI or recommended local commands change.
- Update open questions when uncertainty is resolved or newly found.
- Update changelog for meaningful architecture or onboarding changes.

## Known Limitations

- This initial set did not run full workspace tests.
- Some explorer findings are based on scoped reads and may miss cross-crate behavior.
- Older design docs disagree with current source in at least a few areas; those are tracked as open questions.

## First Prompt For A New Codex Session

```
Read AGENTS.md, doc/ai/00_READ_THIS_FIRST.md, and the local AGENTS.md for the files you will edit. Treat current source/tests/CI as authoritative over older plans. Keep uncertainty labeled and update doc/ai when architecture, rules, commands, or open questions change.
```

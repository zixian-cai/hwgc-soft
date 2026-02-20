---
name: journey-docs
description: "Write a journey document after a successful multi-session coding effort. Activate when the user asks to document a journey, write a summary, or record lessons learned from a coding session."
---

# Journey Documentation

A journey document captures the **what**, **why**, and **what we learned** from a multi-session coding effort. It serves three audiences:

1. **Future AI sessions** — so you can resume work or extend the feature without re-discovering the architecture, invariants, and pitfalls.
2. **The author** — so they can review AI-produced code with confidence by understanding the design rationale and tradeoffs.
3. **Human collaborators** — so they can onboard to the feature, review changes, or debug regressions without reading every line.

## When to Write

Write a journey document when a coding session produces:
- A new subsystem or integration (e.g., FFI bridge, new backend)
- A non-obvious design that future readers will question
- Debugging that uncovered subtle invariants or silent failure modes
- Configuration that required careful alignment across components

Do **not** write one for routine refactors, simple bug fixes, or mechanical changes.

## Where to Write

Place journey files in `docs/journeys/` with the naming convention:

```
docs/journeys/YYYYMMDD_<short_descriptive_name>.md
```

## Structure

Use this outline. Omit sections that don't apply.

```markdown
# <Feature or Integration Name>

**Date**: YYYY-MM-DD

## Overview

One paragraph. State what was built, why it matters, and what it replaces or
extends. A reader should know the scope and motivation within 30 seconds.

## Architecture

Bullet list of components. For each, name the file, state its role, and note
the key abstraction or mechanism. Keep each bullet to 1–2 sentences.

Think: what would a developer need to know to modify this safely?

## Design Decisions & Lessons Learned

One subsection per non-obvious decision. Use this format:

### N. <Short Title>

**Challenge**: What problem or constraint did we face?

**Mistake** (if applicable): What we tried first and why it failed.
Be specific—name the wrong value, the silent failure, the incorrect assumption.

**Solution**: What we did instead and why it works.

**Lesson** (if applicable): The generalizable takeaway. Write it as an
imperative: "Always X", "Never assume Y", "Verify Z by doing W".

## Verification

How the change was validated. Include:
- The exact command(s) to reproduce verification.
- A results table if quantitative comparison is relevant.
- Brief interpretation of the results (one sentence per row is fine).

## Usage

Exact command(s) to use the feature. Use `<PLACEHOLDER>` for arguments.
Note any defaults that make arguments optional.
```

## Writing Principles

Follow the prose rules in `.agents/rules/prose-style.md`. Additionally:

- **Every claim must be verifiable.** If you name a type, function, or config value, confirm it matches the current codebase before writing it down.
- **Prefer showing to telling.** A config snippet or bit-field diagram is worth more than a paragraph of explanation.
- **Write for the skeptical reviewer.** The reader's question is "why should I trust this?" Answer with specifics: exact values, exact function names, exact file paths.
- **Record mistakes honestly.** The most valuable part of a journey document is what went wrong and why. A future reader facing a similar problem will search for these.
- **Keep it scannable.** Use headers, bold labels, and short paragraphs. A reader should locate any section in under 5 seconds.

## What to Include for Each Audience

### For future AI sessions
- File paths and component roles (so you know where to look)
- Invariants that must hold across components (e.g., "address mapping in Rust must match DRAMsim3's config")
- Configuration values and why they were chosen (not just what they are)
- Known limitations or FIXMEs

### For the author (code review confidence)
- Design rationale for non-obvious choices ("why Mutex instead of RefCell?")
- Verification results with interpretation
- Mistakes made and how they were caught — this builds trust that the code was tested thoroughly

### For human collaborators
- High-level architecture (the bullet list in Architecture)
- Usage commands (copy-paste ready)
- The "Lesson" lines — these are the most transferable knowledge

## Anti-Patterns

- **Don't narrate the coding session chronologically.** A journey doc is organized by topic, not by time. "First I tried X, then Y, then Z" belongs in a commit log, not here.
- **Don't duplicate the README.** If usage instructions already exist elsewhere, link to them instead.
- **Don't include transient debugging details.** Stack traces, intermediate print statements, and dead-end hypotheses are noise unless they reveal a recurring trap.
- **Don't use vague language.** "We adjusted the configuration" — say what you changed and to what value.

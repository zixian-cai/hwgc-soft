---
trigger: always_on
---

1. Run `cargo fmt` whenever any `.rs` file is changed.
2. **DO NOT** commit any changes unless explictly told to.
3. Before yielding to the user, double-check if any of the thought process or monologue has been left in the comments and ensure it explains the design decisions in the surrounding code. If so, rewrite it succinctly into formal code comments; if not, remove it.
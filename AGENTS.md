# Project Working Rules

This file captures local engineering conventions for `agentspec`.
Add new rules as team decisions are made.

## Code Organization

- In `src/main.rs`, keep `main` near the top and place private helper functions below it when practical. This keeps the primary execution flow easy to scan first.
- In library-style modules, prefer placing public API items before private helpers unless local readability is better with a different order.
- Prioritize consistency within a file over strict global ordering rules.

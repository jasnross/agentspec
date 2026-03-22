---
id: api-design
description: API design conventions for REST endpoints
version: 1
paths:
  - "src/api/**"
compat:
  targets:
    - claude
    - cursor
    - codex
    - opencode
---

# API Design Rules

All endpoints must validate input at the boundary.

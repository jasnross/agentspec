---
description: agentspec bracket-option rendering probe
model: claude-opus-5[effort=low,fast=true]
---

Probe subagent. Reply with the single word `ack` and stop.

The answer this probe wants arrives in the `subagentStart` hook payload before this body is ever read, so nothing here affects the measurement.

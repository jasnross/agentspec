"""A Claude Code hook payload, one line of a manual probe's capture.

Field names are the JSON keys. The model declares only what a probe reads; the rest of a
payload stays searchable through `raw`.
"""

from pydantic import MISSING

from probe_harness.models.raw import ProviderModel


class ClaudeHookPayload(ProviderModel):
    hook_event_name: str
    # Only `SessionStart` carries `source`.
    source: str | MISSING = MISSING

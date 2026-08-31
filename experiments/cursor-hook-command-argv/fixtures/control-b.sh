#!/usr/bin/env sh
# The second control's only job is to be a hooks.json entry that is textually
# distinct from the first control while still resolving to a bare executable
# path. Two identical entries would be a dedup hazard; a string with appended
# arguments would not survive a non-shell execution model. This is both.
exec "{{CAPTURE_SCRIPT}}"

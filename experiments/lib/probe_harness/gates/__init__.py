"""Gates: checks that decide whether a run measured what its package claims to.

A gate failure is a statement about the run, never about the provider, so it never
records: the runner prints the gate's lines, keeps the workspace, and exits 1.
"""


class GateFailure(Exception):
    """A refusal whose lines are printed to stderr as-is."""

    def __init__(self, *lines: str) -> None:
        super().__init__("\n".join(lines))
        self.lines = lines

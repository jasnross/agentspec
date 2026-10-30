"""An arm family's parsed capture: each arm's request and response models, in order.

The gates, the family judges, the sample parser and a definition's expressions all read
this one shape, so it sits below every one of them.
"""

from collections.abc import Mapping
from dataclasses import dataclass


@dataclass(frozen=True)
class ArmCapture[Req, Resp]:
    """What one arm's sink held, as models, in capture order."""

    requests: tuple[Req, ...]
    # An expression sees () unless its definition requires responses.
    responses: tuple[Resp, ...]


# Each arm's capture, in the definition's arm order.
type Arms[Req, Resp] = Mapping[str, ArmCapture[Req, Resp]]

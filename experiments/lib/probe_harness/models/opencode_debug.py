"""What `opencode debug` prints: a resolved agent, a resolved config, and the skill list.

Field names are the JSON keys. Each model declares only what a probe reads; the rest of a
document stays searchable through `raw`.
"""

from typing import Annotated

from pydantic import MISSING, Field, RootModel

from probe_harness.models.raw import ProviderModel


class OpenCodeAgentModel(ProviderModel):
    providerID: str
    modelID: str


class OpenCodeAgent(ProviderModel):
    """`opencode debug agent <name>`."""

    model: OpenCodeAgentModel
    # Absent when the agent resolves no variant, as in `opencode-agent-variant`'s
    # discriminator capture, whose variant rides inside the model id instead.
    variant: str | MISSING = MISSING


class OpenCodeCommand(ProviderModel):
    # A command that names no model omits the key, as an operator's global commands do.
    model: str | MISSING = MISSING
    # Absent when the command resolves no variant, as `opencode-command-variant`'s
    # discriminator fixture command does.
    variant: str | MISSING = MISSING


class OpenCodeConfig(ProviderModel):
    """`opencode debug config`."""

    # A config with no commands omits the key.
    command: dict[str, OpenCodeCommand] | MISSING = MISSING


class OpenCodeSkill(ProviderModel):
    name: str
    description: str


# `opencode debug skill` prints a JSON array of every skill it resolves.
OpenCodeSkills = RootModel[Annotated[tuple[OpenCodeSkill, ...], Field(strict=False)]]

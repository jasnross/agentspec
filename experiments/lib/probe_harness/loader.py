"""Load a package's probe definition from its `probe.py`."""

import importlib.util
import sys
from pathlib import Path
from typing import Any


class DefinitionError(Exception):
    """A package's definition cannot be loaded. `str(e)` is the message to print."""


def load_definition(package: Path, accepted: tuple[type, ...]) -> Any:
    """Import `<package>/probe.py` and return its `PROBE`, which must be one of <accepted>.

    Each definition is imported under its own module name, since every file is named
    `probe.py`.
    """
    path = package / "probe.py"
    if not path.is_file():
        raise DefinitionError(f"probe run: {package.name} has no probe.py")

    module_name = f"probe_definition_{package.name.replace('-', '_')}"
    spec = importlib.util.spec_from_file_location(module_name, path)
    if spec is None or spec.loader is None:
        raise DefinitionError(f"probe run: {package.name}/probe.py defines no PROBE")
    module = importlib.util.module_from_spec(spec)
    sys.modules[module_name] = module
    spec.loader.exec_module(module)

    definition = getattr(module, "PROBE", None)
    if not isinstance(definition, accepted):
        raise DefinitionError(f"probe run: {package.name}/probe.py defines no PROBE")
    return definition

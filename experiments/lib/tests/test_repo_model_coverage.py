"""The field-coverage rule: every provider model, and every optional field of one, is shown
present by some sample.

A model no committed sample instantiates is unproven against any capture, and a field that
defaults to `MISSING` only where a provider omits it should be shown sent too: one no sample
carries is either dead, or declared for a shape no capture shows, which `ExpectedAbsent`
says out loud.
"""

import importlib
import inspect
import pkgutil
from collections import defaultdict
from collections.abc import Iterator

import probe_harness.models
from probe_harness.arms import ArmCapture
from probe_harness.contract import parse_sample_file
from probe_harness.families import load_package, parse_sample
from probe_harness.families.claude_init import InitCapture
from probe_harness.models.raw import ExpectedAbsent, ProviderModel
from probe_harness.samples import sample_paths
from pydantic import MISSING, BaseModel, RootModel

from tests.conftest import EXPERIMENTS_DIR


def test_every_provider_model_has_an_instance_in_some_sample() -> None:
    seen = {type(instance) for instance in _instances()}
    missing = [f"{cls.__module__}.{cls.__name__}" for cls in _models() if cls not in seen]
    assert missing == [], "no committed sample instantiates these provider models"


def test_every_optional_field_of_a_model_is_present_in_some_sample() -> None:
    seen: dict[type[ProviderModel], set[str]] = defaultdict(set)
    for instance in _instances():
        seen[type(instance)] |= instance.model_fields_set
    problems = [
        f"{cls.__module__}.{cls.__name__}.{name}"
        for cls in _models()
        for name, info in cls.model_fields.items()
        if info.default is MISSING
        and not any(isinstance(m, ExpectedAbsent) for m in info.metadata)
        and name not in seen[cls]
    ]
    assert problems == [], "no committed sample carries these optional fields"


def _models() -> list[type[ProviderModel]]:
    """Every `ProviderModel` subclass defined under `probe_harness.models`."""
    models: list[type[ProviderModel]] = []
    for info in pkgutil.iter_modules(probe_harness.models.__path__):
        module = importlib.import_module(f"probe_harness.models.{info.name}")
        models.extend(
            cls
            for _, cls in inspect.getmembers(module, inspect.isclass)
            if issubclass(cls, ProviderModel)
            and cls is not ProviderModel
            and cls.__module__ == module.__name__
        )
    # An empty enumeration would pass both rules vacuously.
    assert models, f"no provider model found under {probe_harness.models.__path__}"
    return models


def _instances() -> Iterator[ProviderModel]:
    for package in sorted(EXPERIMENTS_DIR.iterdir()):
        paths = sample_paths(package) if package.is_dir() else []
        if not paths:
            continue
        definition, _ = load_package(package)
        for path in paths:
            yield from _walk(parse_sample(definition, parse_sample_file(path).input, str(path)))


def _walk(value: object) -> Iterator[ProviderModel]:
    match value:
        case ProviderModel():
            yield value
            for name in value.model_fields_set:
                yield from _walk(getattr(value, name))
        case RootModel():
            yield from _walk(value.root)
        case BaseModel():
            for name in value.model_fields_set:
                yield from _walk(getattr(value, name))
        case ArmCapture():
            yield from _walk(value.requests)
            yield from _walk(value.responses)
        case InitCapture():
            yield from _walk(value.init)
            yield from _walk(value.result)
        case tuple() | list():
            for item in value:
                yield from _walk(item)
        case dict():
            for item in value.values():
                yield from _walk(item)
        case _:
            pass

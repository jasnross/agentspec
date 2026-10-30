"""The field-coverage rule: every optional model field is shown present by some sample.

A field defaults to `MISSING` only where a provider omits it, so a committed sample should
show the provider sending it too; one no sample carries is either dead, or declared for a
shape no capture shows, which `ExpectedAbsent` says out loud. Only classes with at least one
instance in the committed samples are checked.
"""

from collections import defaultdict
from collections.abc import Iterator

from probe_harness.arms import ArmCapture
from probe_harness.contract import parse_sample_file
from probe_harness.families import load_package, parse_sample
from probe_harness.models.raw import ExpectedAbsent, ProviderModel
from probe_harness.samples import sample_paths
from pydantic import MISSING, BaseModel, RootModel

from tests.conftest import EXPERIMENTS_DIR


def test_every_optional_field_of_a_sampled_model_is_present_in_some_sample() -> None:
    seen: dict[type[ProviderModel], set[str]] = defaultdict(set)
    for instance in _instances():
        seen[type(instance)] |= instance.model_fields_set
    problems = [
        f"{cls.__module__}.{cls.__name__}.{name}"
        for cls, present in seen.items()
        for name, info in cls.model_fields.items()
        if info.default is MISSING
        and not any(isinstance(m, ExpectedAbsent) for m in info.metadata)
        and name not in present
    ]
    assert problems == [], "no committed sample carries these optional fields"


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
        case tuple() | list():
            for item in value:
                yield from _walk(item)
        case dict():
            for item in value.values():
                yield from _walk(item)
        case _:
            pass

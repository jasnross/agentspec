"""Only `jq_cli.py` starts jq.

Every jq invocation goes through one module, so a manifest expression means one thing
everywhere, and that module runs only the expressions a probe author wrote. This walks the
syntax tree of every harness module and every package definition looking for a subprocess
call that starts jq some other way.
"""

import ast

from tests.conftest import (
    EXPERIMENTS_DIR,
)

HARNESS_DIR = EXPERIMENTS_DIR / "lib" / "probe_harness"


JQ_CLI = HARNESS_DIR / "jq_cli.py"


SUBPROCESS_CALLS = frozenset({"run", "Popen", "check_output", "call"})


JQ = "jq"


def jq_violations(source: str, filename: str, jq_cli_source: str = "") -> list[str]:
    """Every subprocess call in <source> that starts jq, or passes it in an argv sequence.

    <jq_cli_source> is `jq_cli.py`'s text, whose module-level names bound to `"jq"` (such
    as `JQ`) count as the literal wherever they are imported or reached as an attribute.
    """
    tree = ast.parse(source, filename=filename)
    exported = _names_bound_to_jq(ast.parse(jq_cli_source)) if jq_cli_source else set()
    local = _names_bound_to_jq(tree) | _names_imported_from(tree, "jq_cli", exported)
    subprocess_modules, subprocess_functions = _subprocess_aliases(tree)

    def is_jq(node: ast.expr) -> bool:
        match node:
            case ast.Constant(value=str() as value):
                return value == JQ
            case ast.Name(id=name):
                return name in local
            case ast.Attribute(attr=attr):
                return attr in exported
            case _:
                return False

    def is_subprocess_call(call: ast.Call) -> bool:
        match call.func:
            case ast.Attribute(value=ast.Name(id=module), attr=attr):
                return module in subprocess_modules and attr in SUBPROCESS_CALLS
            case ast.Name(id=name):
                return name in subprocess_functions
            case _:
                return False

    problems: list[str] = []
    for node in ast.walk(tree):
        if not isinstance(node, ast.Call) or not is_subprocess_call(node):
            continue
        where = f"{filename}:{node.lineno}"
        argv = node.args[0] if node.args else _keyword(node, "args")
        if isinstance(argv, ast.List | ast.Tuple) and argv.elts and is_jq(argv.elts[0]):
            problems.append(f"{where}: starts jq outside jq_cli.py")
            continue
        if argv is not None and is_jq(argv):
            problems.append(f"{where}: starts jq outside jq_cli.py")
            continue
        arguments = [*node.args, *(k.value for k in node.keywords)]
        for argument in arguments:
            if any(
                isinstance(sequence, ast.List | ast.Tuple)
                and any(is_jq(element) for element in sequence.elts)
                for sequence in ast.walk(argument)
            ):
                problems.append(f"{where}: passes jq in an argv sequence outside jq_cli.py")
                break
    return problems


def test_only_jq_cli_starts_jq() -> None:
    jq_cli_source = JQ_CLI.read_text(encoding="utf-8")
    modules = sorted(HARNESS_DIR.rglob("*.py")) + sorted(EXPERIMENTS_DIR.glob("*/probe.py"))
    assert JQ_CLI in modules

    problems: list[str] = []
    for path in modules:
        if path == JQ_CLI:
            continue
        problems += jq_violations(path.read_text(encoding="utf-8"), str(path), jq_cli_source)
    assert not problems, "\n".join(problems)


def test_the_checker_flags_a_literal_jq_argv() -> None:
    source = 'import subprocess\nsubprocess.run(["jq", "-n", "1"], check=False)\n'
    assert jq_violations(source, "offender.py")


def test_the_checker_flags_a_name_bound_to_jq() -> None:
    source = 'import subprocess\nTOOL = "jq"\nsubprocess.Popen((TOOL, "."))\n'
    assert jq_violations(source, "offender.py")


def test_the_checker_flags_an_aliased_function_import() -> None:
    source = 'from subprocess import check_output as co\nco(["jq", "."])\n'
    assert jq_violations(source, "offender.py")


def test_the_checker_flags_jq_cli_s_own_constant() -> None:
    jq_cli_source = JQ_CLI.read_text(encoding="utf-8")
    imported = "import subprocess\nfrom probe_harness.jq_cli import JQ\nsubprocess.call([JQ])\n"
    attribute = "import subprocess\nfrom probe_harness import jq_cli\nsubprocess.run([jq_cli.JQ])\n"
    assert jq_violations(imported, "offender.py", jq_cli_source)
    assert jq_violations(attribute, "offender.py", jq_cli_source)


def test_the_checker_flags_jq_inside_an_argv_sequence() -> None:
    source = 'import subprocess as sp\nsp.run(["env", "-i", "jq", "."])\n'
    assert jq_violations(source, "offender.py")


def test_the_checker_passes_a_call_that_does_not_start_jq() -> None:
    source = (
        "import shutil\nimport subprocess\n"
        'subprocess.run(["opencode", "--version"])\n'
        'shutil.which("jq")\n'
        'TOOLS = ("jq",)\n'
    )
    assert jq_violations(source, "clean.py") == []


def _names_bound_to_jq(tree: ast.Module) -> set[str]:
    names: set[str] = set()
    for node in ast.walk(tree):
        match node:
            case ast.Assign(targets=targets, value=ast.Constant(value=str() as value)):
                if value == JQ:
                    names |= {t.id for t in targets if isinstance(t, ast.Name)}
            case ast.AnnAssign(target=ast.Name(id=name), value=ast.Constant(value=str() as value)):
                if value == JQ:
                    names.add(name)
            case _:
                pass
    return names


def _names_imported_from(tree: ast.Module, module: str, exported: set[str]) -> set[str]:
    """Local names for <exported> names imported from a module whose last part is <module>."""
    names: set[str] = set()
    for node in ast.walk(tree):
        if isinstance(node, ast.ImportFrom) and (node.module or "").split(".")[-1] == module:
            names |= {alias.asname or alias.name for alias in node.names if alias.name in exported}
    return names


def _subprocess_aliases(tree: ast.Module) -> tuple[set[str], set[str]]:
    """Local names bound to the `subprocess` module, and to its starting functions."""
    modules: set[str] = set()
    functions: set[str] = set()
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            modules |= {a.asname or a.name for a in node.names if a.name == "subprocess"}
        elif isinstance(node, ast.ImportFrom) and node.module == "subprocess":
            functions |= {a.asname or a.name for a in node.names if a.name in SUBPROCESS_CALLS}
    return modules, functions


def _keyword(call: ast.Call, name: str) -> ast.expr | None:
    for keyword in call.keywords:
        if keyword.arg == name:
            return keyword.value
    return None


def test_jq_cli_exposes_only_the_expression_runners() -> None:
    tree = ast.parse(JQ_CLI.read_text(encoding="utf-8"))
    public = {
        node.name
        for node in tree.body
        if isinstance(node, ast.FunctionDef) and not node.name.startswith("_")
    }
    assert public == {"evaluate", "test", "compile_error"}


def test_no_definition_imports_jq_cli() -> None:
    # A definition reads JSON through `json_values` and request bodies through the gates'
    # typed model; jq is for the expressions a manifest or a custom check carries.
    importing = sorted(
        definition.parent.name
        for definition in EXPERIMENTS_DIR.glob("*/probe.py")
        if _imports_jq_cli(ast.parse(definition.read_text(encoding="utf-8")))
    )
    assert importing == []


def _imports_jq_cli(tree: ast.Module) -> bool:
    """Whether <tree> imports `probe_harness.jq_cli` in any spelling."""
    for node in ast.walk(tree):
        match node:
            case ast.ImportFrom(module="probe_harness", names=names) if any(
                alias.name == "jq_cli" for alias in names
            ):
                return True
            case ast.ImportFrom(module="probe_harness.jq_cli"):
                return True
            case ast.Import(names=names) if any(
                alias.name == "probe_harness.jq_cli" for alias in names
            ):
                return True
            case _:
                pass
    return False

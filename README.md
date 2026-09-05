# agentspec

`agentspec` is a utility for managing skills, rules, and custom subagents for AI coding applications.

Every AI coding tool has its own format for defining agents, skills, and rules, which means that if you use more than one tool you'll end up duplicating your prompts for each tool. Inevitably this results in drift, leaving prompts for outdated or poorly configured.

`agentspec` fills this gap by providing a single provider-neutral format which can be compiled to provider-specific formats. It currently supports Claude Code, Cursor, and OpenCode. Each tool gets output tailored to its own conventions while you maintain a single source of truth.

## Installation

### Homebrew

```sh
brew tap jasnross/tap
brew install agentspec
```

### Cargo

```sh
cargo install --git https://github.com/jasnross/agentspec
```

## Quick start

1. Create a spec file at `spec/agents/my-agent.md`:

   ```markdown
   ---
   id: my-agent
   description: A helpful assistant
   ---

   You are a helpful assistant.
   ```

2. Run `agentspec compile`
3. Check `generated/` for provider-specific output (one subdirectory per provider)

## Spec format

Specs are Markdown files with YAML frontmatter. The Markdown body contains the instructions that get delivered to the AI tool. There are three spec types, each stored in a subdirectory under `spec/`:

### Agents

Agents are subagents that can be dispatched for focused tasks. Each agent is a single `.md` file in `spec/agents/`.

`spec/agents/code-reviewer.md`:

```yaml
---
id: code-reviewer
description: Reviews code changes and provides actionable feedback.
execution:
  preset: architect
capabilities:
  tools:
    - shell
    - glob
    - grep
    - read
---
Review the proposed changes for correctness, security, and maintainability.
```

### Skills

Skills are reusable prompts that can be invoked by users or agents. Each skill is a directory in `spec/skills/` containing a `SKILL.md` file (or a single `.md` file, if unambiguous) and any supporting files (scripts, templates, other colocated `.md` files, etc.).

`spec/skills/commit/SKILL.md`:

```yaml
---
id: commit
description: Create git commits with user approval.
user_invocable: true
agent_invocable: true
execution:
  preset: balanced
capabilities:
  tools:
    - shell
---
Create a git commit for the current changes.
```

#### Supporting files

Any file in a skill directory other than the primary spec file is bundled as a supporting file and synced alongside the skill. This is useful for scripts that the skill references in its instructions. For example:

```
spec/skills/deploy/
├── SKILL.md            # skill spec (the .md file)
└── scripts/
    └── deploy.sh       # supporting file
```

Supporting files preserve their relative paths and executable permissions in the compiled output. The skill's instructions can then reference the script by its relative path (e.g., `scripts/deploy.sh`).

#### Ignoring files

The `[spec].ignore` option in `agentspec.toml` is a list of glob patterns for files (and subtrees) to skip when reading the spec directory. Use it to keep test files, fixtures, or editor artifacts colocated with the code they relate to without shipping them to downstream tool config dirs.

Patterns are matched against paths **relative to `sources_dir`**. Slashless patterns match only top-level entries — use `**/` explicitly to match at any depth:

- `*.bats` matches `test.bats` only if it's directly under `sources_dir/`.
- `**/*.bats` matches `.bats` files anywhere in the tree.
- `foo/**` prunes the `foo` subtree entirely (neither descended into nor stat'd).

Example — bats tests colocated with a skill's scripts:

```
spec/skills/deploy/
├── SKILL.md
├── scripts/
│   └── deploy.sh
└── tests/
    └── deploy.bats    # colocated test; ignored
```

```toml
[spec]
ignore = ["**/*.bats"]
```

`agentspec validate` reports which files were ignored and warns about patterns that matched zero files. Use `agentspec compile --verbose` or `agentspec sync --verbose` (or `--dry-run`) to see the same listing during a build.

Gitignore-style negation (`!pattern`) and trailing-slash directory sugar (`foo/`) are **not** supported — write `foo/**` explicitly.

### Rules

Rules are always-on instructions injected into every conversation. Each rule is a single `.md` file in `spec/rules/`.

`spec/rules/cli-tools.md`:

```yaml
---
id: cli-tools
description: Prefer dedicated CLI tools over inline scripts
---
Use `jq` for JSON, `yq` for YAML, and purpose-built CLI tools over inline Python or Ruby scripts.
```

### Hooks

Hooks are scripts that fire on session events (e.g., before a tool runs, when a session starts). agentspec compiles a single `spec/hooks/hooks.toml` plus a `spec/hooks/scripts/` directory into provider-native `hooks/hooks.json` (Claude Code, Cursor) — OpenCode is out of scope for hook output in v1.

`spec/hooks/hooks.toml`:

```toml
[hooks.init-thoughts]
events = ["user_prompt_submit"]
script = "scripts/init-thoughts.sh"
description = "Seed THOUGHTS_DIR context at the start of each turn"
timeout = 30

[hooks.audit-bash]
events = ["pre_tool_use"]
matcher = "shell"
script = "scripts/audit-bash.sh"

[hooks.audit-bash-strict]
events = ["pre_tool_use"]
matcher = "shell"
script = "scripts/audit-bash.sh"
args = ["--strict"]
```

`spec/hooks/scripts/` contains both entry scripts (referenced by `[hooks.<id>].script`) and any helper scripts they `source` — agentspec walks the directory and copies all files. Helper conventions like `_common.sh` are supported. The `_agentspec_*` filename prefix is reserved for future use and rejected at load time.

`args` is an optional list of literal strings passed to the script as positional arguments (`$1`, `$2`, …), alongside the canonical payload on stdin — the same script can back several entries with different parameters, as `audit-bash` and `audit-bash-strict` do above. agentspec quotes every value unconditionally; `hooks.toml` resolves no templating inside `args`, so values are literal text, never shell syntax. Argument values are copied verbatim into the user's `settings.json` or `hooks.json` by `sync`, so they are not a place for secrets. See [`docs/hooks-canonical.md`](docs/hooks-canonical.md#script-invocation-and-argv) for the full argv contract.

`tags` is an optional list of strings, accepted on `[hooks.<id>]` entries the same way it is on agent, skill, and rule frontmatter. It is used for categorization and exposed in the `specs` template variable.

#### Canonical payload format

Hook scripts receive **canonical JSON** on stdin and emit canonical JSON on stdout — a provider-neutral wire format that produces semantically identical behavior under Claude Code and Cursor. agentspec generates a per-event POSIX shell shim (one per `(provider, HookEvent)` pair) that translates between each provider's native shape and the canonical shape; scripts never see raw provider stdin.

`jq` is a runtime prerequisite. Install via `brew install jq` / `apt install jq` / your package manager of choice. If `jq` is missing at hook-fire time, the shim prints a clear `agentspec: jq is required …` error and exits 1.

See [`docs/hooks-canonical.md`](docs/hooks-canonical.md) for the full canonical schema reference (per-event input/output field tables, `provider_raw` escape hatch, documented limitations, schema-versioning policy, and migration examples).

#### Hook events

| Canonical event         | Claude               | Cursor               |
| ----------------------- | -------------------- | -------------------- |
| `pre_tool_use`          | `PreToolUse`         | `preToolUse`         |
| `post_tool_use`         | `PostToolUse`        | `postToolUse`        |
| `post_tool_use_failure` | `PostToolUseFailure` | `postToolUseFailure` |
| `session_start`         | `SessionStart`       | `sessionStart`       |
| `session_end`           | `SessionEnd`         | `sessionEnd`         |
| `stop`                  | `Stop`               | `stop`               |
| `pre_compact`           | `PreCompact`         | `preCompact`         |
| `subagent_start`        | `SubagentStart`      | `subagentStart`      |
| `subagent_stop`         | `SubagentStop`       | `subagentStop`       |
| `user_prompt_submit`    | `UserPromptSubmit`   | `beforeSubmitPrompt` |

#### Matchers

The optional `matcher` field filters which tool calls or subagent invocations a hook fires for. agentspec accepts **canonical lowercase tokens** and translates them to each provider's native name at compile time — so spec authors write `matcher = "shell"` once and each provider receives its own identifier in the compiled output.

`matcher` is valid on five events: `pre_tool_use`, `post_tool_use`, `post_tool_use_failure`, `subagent_start`, `subagent_stop`.

**Tool matchers** (canonical → provider):

| Canonical   | Claude            | Cursor      |
| ----------- | ----------------- | ----------- |
| `read`      | `Read`            | `Read`      |
| `write`     | `Write`           | `Write`     |
| `edit`      | `Edit`            | `Edit`      |
| `grep`      | `Grep`            | `Grep`      |
| `glob`      | `Glob`            | `glob`      |
| `shell`     | `Bash`            | `Shell`     |
| `webfetch`  | `WebFetch`        | `webfetch`  |
| `websearch` | `WebSearch`       | `WebSearch` |
| `question`  | `AskUserQuestion` | `question`  |
| `tasks`     | `TaskCreate`      | `tasks`     |
| `subagent`  | `Agent`           | `Task`      |
| `skill`     | `Skill`           | `skill`     |

**Subagent-type matchers** (canonical → provider):

| Canonical | Claude            | Cursor           |
| --------- | ----------------- | ---------------- |
| `general` | `general-purpose` | `generalPurpose` |
| `explore` | `Explore`         | `explore`        |
| `plan`    | `Plan`            | `plan`           |

Multi-token matchers join with `|` (e.g., `matcher = "read|write|edit"`); each token is translated independently. When a canonical token has no provider equivalent, the lowercase canonical name is emitted verbatim — it won't match any provider tool, so the hook becomes a no-op for that branch. Non-canonical tokens (MCP tool names like `mcp__memory__create`, provider-specific names, custom subagent types) also pass through unchanged, so you can still write provider-specific matchers when needed.

#### Sync modes

- **Plugin mode** (`mode = "plugin"`): agentspec writes a self-contained plugin tree under the configured `dir`: `agents/`, `skills/`, `rules/`, `hooks/scripts/<file>`, `hooks/hooks.json`, plus `.claude-plugin/plugin.json` for Claude (always emitted in plugin mode; populated from the `plugin-name` / `plugin-version` / `plugin-description` / `plugin-author` fields under `[sync.<provider>]`) and `.cursor-plugin/plugin.json` for Cursor (same shape, but Cursor accepts plugins without a manifest entirely). Hook command anchors use the provider's plugin-root env var: `${CLAUDE_PLUGIN_ROOT}/hooks/scripts/<f>` for Claude, `${CURSOR_PLUGIN_ROOT}/hooks/scripts/<f>` for Cursor. Requires `plugin-name` under `[sync.<provider>]`; absence is a parse-time error.
- **User mode** (`mode = "user"`): scripts land at `~/.<provider>/hooks/scripts/`; entries are merged into `~/.<provider>/settings.json` (Claude) or `~/.cursor/hooks.json` (Cursor) via a CST-aware patcher. Comments, trailing commas, and user-authored entries round-trip unchanged. Each agentspec entry carries an `_agentspec_id` sentinel so re-syncing replaces only entries it owns.
- **Project mode** (`mode = "project"`): same as User mode but rooted at the project directory.

> **Note**: Cursor support for the merged-mode path (User/Project) is currently shipped as best-effort. Several Cursor runtime behaviors — event-name aliases, `${CLAUDE_PLUGIN_ROOT}` resolution outside plugin scope, and tolerance of unknown sub-fields like `_agentspec_id` — are inferred from documentation rather than empirically verified against a real Cursor build. Empirical verification is a pre-1.0 blocker; if you hit a Cursor-specific issue, please open an issue with the failing config.

#### OpenCode behavior

If your spec set contains hooks and `[sync.opencode]` is configured, agents/skills/rules sync normally and each hook is reported as a lost spec body: `opencode: 3 specs lost \`content\` — opencode emits no hook`. Use `--verbose` to list each hook id.

**A rule's `paths` does not reach OpenCode, and the rule widens as a result.** OpenCode has no native path scoping, so a rule that Claude and Cursor activate only when matching files are in context is emitted for OpenCode as an always-on instruction registered in `instructions[]` — injected into every conversation. This is the one dropped value whose consequence is more than a missing default: the rule still applies, to more than it was scoped to. `compile` and `sync` report it as `opencode: N specs lost \`paths\``.

### Frontmatter reference

| Field | Agent | Skill | Rule | Description |
| --- | --- | --- | --- | --- |
| `id` | required | required | required | Unique identifier. Must be unique across all spec types. |
| `description` | required | optional | optional | Short description of what this spec does. |
| `user_invocable` | — | required | — | Whether users can invoke this skill directly (e.g., via `/commit`). |
| `agent_invocable` | — | required | — | Whether agents can invoke this skill. At least one of `user_invocable` or `agent_invocable` must be `true`. |
| `execution.preset` | optional | optional | — | Name of a model preset defined in `agentspec.toml`. See [Model presets](#model-presets). |
| `tags` | optional | optional | optional | List of string tags for categorization. Exposed in the `specs` template variable. |
| `capabilities.tools` | optional | optional | — | List of tools the agent/skill can use. See [Tools reference](#tools-reference) below. |

### Tools reference

| Tool        | Description                                   |
| ----------- | --------------------------------------------- |
| `read`      | Read file contents                            |
| `write`     | Create or overwrite files                     |
| `edit`      | Make targeted edits to existing files         |
| `grep`      | Search file contents with regex patterns      |
| `glob`      | Find files by name patterns                   |
| `shell`     | Run shell commands                            |
| `webfetch`  | Fetch content from a URL                      |
| `websearch` | Search the web                                |
| `question`  | Ask the user a question                       |
| `tasks`     | Create and manage tasks for tracking progress |
| `subagent`  | Dispatch work to a subagent                   |
| `skill`     | Invoke a named skill                          |

### Templating

Spec bodies support [MiniJinja](https://docs.rs/minijinja/latest/minijinja/syntax/index.html) template syntax. This is primarily useful for including shared fragments across specs, but the full syntax is available (`{% if %}`, `{% for %}`, filters, etc.).

#### Templates

Templates are structural skeletons that define a layout with named slots. Specs derive from a template with `{% extends %}` and override specific slots, keeping the surrounding structure consistent across a family of specs.

Templates are `.md` files stored in `spec/templates/`:

```
spec/templates/
├── critique.md
└── script-contract.md
```

A template defines slots with `{% block %}`:

```markdown
# Code Review

{% block purpose required %}{% endblock %}

## Guidelines

{% block guidelines %} Follow standard review practices. {% endblock %}

## Output Format

{% block output required %}{% endblock %}
```

- `{% block name required %}{% endblock %}` — mandatory slot; derived specs must override it or compilation fails
- `{% block name %}...{% endblock %}` — optional slot with default content; derived specs can override or keep the default

A spec derives from a template by starting with `{% extends %}` and overriding blocks:

```markdown
---
id: security-review
description: Security-focused code review
---

{% extends "templates/critique.md" %}

{% block purpose %}Review code for security vulnerabilities.{% endblock %}

{% block output %}Return findings as a numbered list with severity ratings.{% endblock %}
```

The optional `guidelines` block keeps its default content since the derived spec doesn't override it.

**`{{ super() }}`** augments rather than replaces a parent block's content:

```
{% block guidelines %}{{ super() }}
Additionally, check for OWASP Top 10 vulnerabilities.
{% endblock %}
```

**Multi-level inheritance** works natively — a template can extend another template. Block overrides accumulate through the chain.

**Fragments inside blocks** compose with templates using the same `{% include %}` and `{% with %}` syntax:

```
{% block details %}
{% with scope = "security" %}{% include "review/checklist.md" %}{% endwith %}
{% endblock %}
```

**Error behavior**:

- Missing template (`{% extends "templates/nonexistent.md" %}`) — compile error naming the spec and template
- Missing required override — compile error from MiniJinja identifying the required block
- Unrecognized block name (typo) — compile error listing the block name(s) and spec path

#### Fragments

Fragments are reusable Markdown snippets stored in `spec/fragments/`. They help avoid duplicating instructions across multiple skills or agents.

All include paths are relative to `sources_dir` (default: `spec/`). Fragments use the `fragments/` prefix:

```
spec/fragments/
├── review-guidelines.md          → {% include "fragments/review-guidelines.md" %}
└── review/
    └── prompt-contract.md        → {% include "fragments/review/prompt-contract.md" %}
```

Include a fragment in any spec body with:

```
{% include "fragments/review-guidelines.md" %}
```

#### Colocated content

`.md` files placed alongside a spec can be included using `./` self-relative syntax (preferred — survives directory renames) or the full spec-relative path:

```
spec/skills/code-review/
├── SKILL.md                      ← the spec
└── review-contract.md            ← colocated content
```

```
{% include "./review-contract.md" %}
{% include "skills/code-review/review-contract.md" %}
```

Both syntaxes resolve to the same file. `./` includes can nest — a colocated file that itself uses `{% include "./subsection.md" %}` resolves relative to its own directory.

`../` is not supported — use full paths for cross-directory references.

#### Variables

Use `{% with %}` to pass variables into a fragment. The fragment references them with `{{ variable_name }}`:

Fragment (`spec/fragments/review/prompt-contract.md`):

```
Review scope: {{ scope }}
Base reference: {{ base_reference }}
{% if focus %}Focus on: {{ focus }}{% endif %}
```

Spec body:

```
{% with scope = "changes against main", base_reference = "main" %}
{% include "fragments/review/prompt-contract.md" %}
{% endwith %}
```

#### Indented include

When including a fragment inside an indented context (e.g., a numbered list), use `{% filter indent() %}` to preserve the surrounding indentation:

```
1. First step

2. Review the changes:

   {% filter indent(3, first=false) %}
   {%- include "fragments/review-guidelines.md" %}
   {%- endfilter %}

3. Next step
```

The `first=false` parameter skips indenting the first line (since it's already at the correct indentation from the call site). The `{%-` trim markers prevent extra whitespace around the included content.

Fragments can include other fragments (nesting is supported).

#### Include directories outside the spec tree

`[spec].extra_include_dirs` registers directories outside `sources_dir` whose files can be included. Each entry has a `name`, which becomes the path prefix in includes, and a `path`, resolved relative to the config file's directory (a leading `~/` expands to `$HOME`):

```toml
[spec]
extra_include_dirs = [{ name = "shared", path = "../shared-fragments" }]
```

A file in that directory is then included under its registered prefix:

```
{% include "shared/note.md" %}
```

#### Built-in variables

In addition to user-defined `{% with %}` variables, agentspec provides built-in variables that expose metadata about all specs in the library.

##### `specs`

The `specs` variable contains all specs in the library, accessible as sorted lists (for iteration) and as keyed maps (for direct lookup by ID).

**Lists** (for iteration):

| Field          | Type            | Description                  |
| -------------- | --------------- | ---------------------------- |
| `specs.agents` | list of entries | All agent specs              |
| `specs.skills` | list of entries | All skill specs              |
| `specs.rules`  | list of entries | All rule specs               |
| `specs.all`    | list of entries | All specs regardless of type |

**Keyed maps** (for direct lookup):

| Field | Type | Description |
| --- | --- | --- |
| `specs.agent` | map of key → entry | Agents keyed by underscore-normalized ID |
| `specs.skill` | map of key → entry | Skills keyed by underscore-normalized ID |
| `specs.rule` | map of key → entry | Rules keyed by underscore-normalized ID |

**Key normalization**: hyphens in spec IDs are replaced with underscores for keyed access. A spec with `id: gh-safe` is accessed as `specs.skill.gh_safe`.

Each entry has:

| Field | Description |
| --- | --- |
| `name` | The spec's name as the model sees it (uses `content-prefix` if set, else `prefix`) |
| `description` | The spec's description (empty string if not set) |
| `type` | One of `agent`, `skill`, or `rule` |
| `tags` | List of tags from frontmatter (empty list if not set) |

When compiled with a sync prefix, the `name` field resolves to the prefix-aware model-facing name. By default this is `{prefix}-{id}` (e.g., `tw-gh-safe`), but when `content-prefix` is set explicitly (e.g., `"tw:"`), the `name` uses that format instead (e.g., `tw:gh-safe`). Without any prefix, `name` is the canonical ID.

> **Best practice**: Use keyed references (`{{ specs.skill.gh_safe.name }}`) instead of hardcoding spec names in body text. This ensures references stay correct when the sync prefix changes, and produces a compile error if the referenced spec is renamed or removed.

Example — referencing a specific skill by name:

```
Load the '{{ specs.skill.gh_safe.name }}' skill before proceeding.
```

This also applies to `subagent_type` values in tool-call examples:

```
subagent_type: "{{ specs.agent.code_reviewer.name }}"
```

Example — listing all available agents in a rule:

```
{% for agent in specs.agents %}
- **{{ agent.name }}**: {{ agent.description }}
{% endfor %}
```

Example — listing all specs with their type:

```
{% for spec in specs.all %}
- [{{ spec.type }}] {{ spec.name }}
{% endfor %}
```

Built-in variables are available in both spec bodies and included fragments. Additional built-in variables may be added in future versions.

## Usage

```sh
agentspec compile                # compile only
agentspec validate               # validate specs without generating output
agentspec sync                   # compile and sync to all configured targets
agentspec sync --dry-run         # preview without making changes
agentspec sync --force           # allow overwriting user-owned destination files
agentspec sync --provider claude --mode user # CLI-only sync for one provider
agentspec remove                 # reverse a prior sync
agentspec prune                  # strip orphaned entries from host config files
agentspec hook test <hook-id>    # run a hook through the shim, showing each stage
agentspec completions <shell>    # print a shell completion script
```

## Configuration

Place an `agentspec.toml` in your project root.

```toml
# All sections are optional. Defaults shown where applicable.

[spec]
sources_dir = "spec" # Directory where your spec sources are located. Can use relative or absolute paths.
ignore = [] # See "Ignoring files" above for details.

[compile]
output_dir = "generated" # Output for the compile command. Can be a relative or absolute path.

[presets.architect] # See "Model presets" documentation below
claude = { model = "opus" }
opencode = { model = "openai/gpt-5.3-codex", variant = "xhigh" }
cursor = { model = "claude-opus-4-6" }

[sync.<claude|cursor|opencode>] # See "Sync" documentation below
mode = "user"
# prefix = "tw"             # namespace prefix for synced file names
# content-prefix = "tw:"    # content-reference prefix (defaults to "{prefix}-")
# overwrite = false         # allow overwriting user-owned files
# dir = "/path/to/output"   # base directory (only used with mode = "plugin")
# plugin-name = "my-plugin"  # required when mode = "plugin"; controls skill namespace + marketplace slug
# plugin-version = "0.1.0"   # optional; any string (neither provider enforces SemVer)
# plugin-description = "..."  # optional human-readable description
# plugin-author = { name = "Name", email = "name@example.com" }  # optional author (email optional)
# plugin-repository = "https://github.com/you/repo"  # optional; passed through to the plugin manifest
# plugin-license = "MIT"     # optional; passed through to the plugin manifest
```

## Model presets

Model presets allow you to specify models in your specs in a provider-neutral way. Rather than hard-coding a specific model name you define "presets" for each of your use-cases in `agentspec.toml`:

```toml
[presets.architect]
claude = { model = "opus", effort = "high" }
opencode = { model = "openai/gpt-5.3-codex", variant = "xhigh" }
cursor = { model = "claude-opus-4-6" }

[presets.balanced]
claude = { model = "sonnet" }
opencode = { model = "openai/gpt-5.3-codex", variant = "medium" }
cursor = { model = "claude-sonnet-4-5" }

[presets.fast]
claude = { model = "haiku" }
opencode = { model = "openai/gpt-5.3-codex", variant = "low" }
cursor = { model = "fast" }
```

Besides naming a model, a preset can say how hard that model should think. Each provider block uses **that provider's own spelling** for the setting, because there is no provider-neutral effort vocabulary to translate between: the legal values are a function of the model in all three providers. A key is only accepted in the block whose provider defines it; an unrecognized key is a parse error rather than a silent no-op.

| Provider | Preset key | Renders as |
| --- | --- | --- |
| Claude | `effort` | an `effort:` frontmatter key |
| Cursor | `effort`, `fast`, `context`, `params` | a `[effort=…,fast=…,context=…,…]` suffix on the model id |
| OpenCode | `variant` | a `variant:` frontmatter key, sibling to `model:` |

Claude's `effort` is a closed enum: `low`, `medium`, `high`, `xhigh`, or `max`. Anything else fails when `agentspec.toml` is parsed, with an error naming the offending field, so a typo never reaches a generated file. It renders as an `effort:` frontmatter key alongside `model:`, and it is independent of `model` — a Claude block may set `effort` with no `model` beside it.

**Claude applies `effort` on some invocation paths and not others**, and it says nothing on the paths where it does not. Measured against Claude Code's outbound requests: an agent's `effort` governs the request when that agent is invoked as a **delegated subagent**, but not when it is the session's own agent. A skill's `effort` governs when the skill is the session's **entry prompt** or is **forked**, but not when it is model-invoked mid-session. So a skill that is only agent-invocable carries `effort:` into its generated file and never has it applied. agentspec emits the key wherever the preset sets it and does not warn about this.

agentspec does **not** verify that an effort value is supported by the model named beside it. That is deliberate: all three providers degrade silently when it is not — Claude clamps an unsupported level down to the highest supported one at or below it, without warning — so agentspec warrants the format it composes, not that the value is meaningful for that model.

### Cursor's model options

Cursor encodes its options as a bracket suffix on the model id rather than as separate frontmatter keys, so agentspec composes one from the named fields you set:

```toml
[presets.deep_review]
cursor = { model = "claude-opus-5", effort = "high", fast = false, context = "300k" }
# renders as → model: claude-opus-5[effort=high,fast=false,context=300k]
```

| Field | Type | Notes |
| --- | --- | --- |
| `effort` | string | Not an enum, unlike Claude's. Cursor documents its legal values as varying by model and discoverable only at runtime, so there is no static set to encode — an enum would reject values Cursor already accepts. |
| `fast` | bool | Written `false`, not `"false"`. agentspec renders a `false` rather than skipping it; whether Cursor acts on an option whose value matches the model's default is not observable — `experiments/cursor-subagent-effort/` records `fast=false` as uninformative on that oracle. |
| `context` | string | A magnitude such as `"300k"`. See the note below. |
| `params` | table | Escape hatch for any other bracket option — `params = { optimize_for = "cost" }` composes `[optimize_for=cost]`. |

**Cursor's option set is open-ended, so `params` is not optional trivia.** Cursor documents bracket options as using "the same `id=value` pairs as the SDK's model parameters", states that parameter ids and values vary by model, and makes the catalog account- and team-specific — discoverable only through `Cursor.models.list()`. `optimize_for` (`cost` / `balanced` / `intelligence`, on Router models) is a documented example with no named field here. The three named fields are the ones agentspec can type and document; `params` carries the rest under the same rules.

**Cursor's `model` must be a bare identifier.** agentspec is the sole writer of the bracket string, so a hand-written `model = "claude-opus-5[effort=high]"` is rejected — the error names the field to move the option to. Two spellings of one option cannot coexist, which is also why a `params` key matching a named field is rejected rather than merged. The ban relocates a setting rather than removing one, because `params` accepts whatever agentspec has no field for.

One thing stays inexpressible: Cursor's `composer-2.5[]`, where _empty_ brackets select the standard variant rather than the fast one. agentspec emits a bare model when no options are set, so there is no way to ask for an empty bracket. Use `fast = false`, which Cursor documents as selecting the same standard variant explicitly.

For the same reason as the bracket ban, none of `[`, `]`, `,`, or `=` may appear in `model`, `effort`, `context`, or any `params` key or value. Cursor documents no escaping syntax for its bracket grammar, so a delimiter inside a value would forge an option the preset never declared — `effort = "high,context=1m"` would otherwise compose `model: claude-opus-5[effort=high,context=1m]`. These are rejected at validation time rather than escaped, since there is no escaping convention to compose against.

**About `context`:** agentspec composes it from Cursor's published syntax, but Cursor exposes no way to observe the option taking effect — its resolved view flattens the model string and hides `context` whether it was honored or discarded. What _is_ measured, by `experiments/cursor-subagent-bracket-tolerance/`, is that a bracket carrying `context` still applies the options beside it. So agentspec passes `context` through without warranting its effect.

### Execution presets reach skill files on Claude only

This covers `model` as well as `effort`. Cursor's skill schema has no model field at all, so nothing from a preset's Cursor block reaches a generated Cursor skill file. OpenCode reads `variant` on agents and commands but does not surface it on skills, so a skill that is only agent-invocable carries neither `model` nor `variant` in its generated OpenCode file, whatever its preset sets. Only Claude's `SKILL.md` carries them. A preset set on a skill spec is silently inert on the other two providers.

The same is true of `capabilities.tools`: OpenCode reads a tool map on agents but not on skills, so declared tools do not reach a generated OpenCode skill file either.

**Cursor reads no tool restriction on any file kind.** Its documented subagent fields are `name`, `description`, `model`, `readonly`, and `is_background`, and a subagent inherits every tool from the parent conversation; Custom Modes, which could restrict tools per mode, were removed in Cursor 2.1, and every remaining control gates approval rather than availability. So `capabilities.tools` reaches no generated Cursor file at all — not an agent file, not a skill file. `compile` and `sync` report this as a loss against each spec that declares tools; on a library of any size it is the largest single group in the report.

An OpenCode `variant` set with no `model` beside it is accepted and inert — unlike Cursor's options, which are rejected without a `model`, because Cursor cannot express them apart from one.

**OpenCode applies a `variant` only when the agent's declared model is the one the session actually resolves.** Reading OpenCode's source, an agent's `variant` survives only if the agent declares a `model`, the resolved model equals that declared model, and that model's catalog entry lists the variant name. So an agent generated from a preset that sets both `model` and `variant` still loses its variant whenever the session is running a different model — switching models mid-session is enough. agentspec cannot detect this: which model a session resolves is decided long after `agentspec sync` has run. The generated frontmatter is correct in every such case; the value simply does not reach the request.

### Referring to a preset

Name the preset in a spec's `execution` block. For example, in `spec/agents/example-agent.md`:

```
id: example-agent
description: Example agent
execution:
  preset: architect # Refers to a preset in your `agentspec.toml`
```

## Sync

`agentspec sync` compiles specs and distributes the generated output to the appropriate location for each provider. Sync targets are configured in `agentspec.toml` under `[sync.<provider>]`.

```toml
# All fields are optional. Only mode is shown; the rest default to off/absent.

[sync.claude]
mode = "user"
# prefix = "tw"
# content-prefix = "tw:"
# overwrite = false
# dir = "/path/to/output"
```

| Field | Default | Description |
| --- | --- | --- |
| `mode` | `"user"` | `"user"` syncs to the tool's user-level config dir (e.g. `~/.claude/`).<br>`"project"` syncs to the project-local config dir (e.g. `.claude/`).<br>`"plugin"` syncs a self-contained plugin tree (including provider plugin manifest) to an explicit directory set by `dir`; requires `plugin-name`. |
| `prefix` | `null` | Namespace prefix applied to synced file names. Can be useful for avoiding collisions with user-owned files or specs from plugins. See [Prefix behavior](#prefix-behavior) below. |
| `content-prefix` | `null` | Literal prefix for content references (model-facing names). Includes its separator (e.g., `"tw:"` → `tw:skill-name`). When unset, defaults to `"{prefix}-"`. See [Content-reference prefix](#content-reference-prefix). |
| `overwrite` | `false` | When `true`, allows overwriting user-owned files at sync destinations (with backup). Can also be set per-invocation with `--force`. |
| `dir` | `null` | Base directory for synced output when `mode = "plugin"`. Subdirectories (`agents/`, `skills/`, `rules/`, `commands/`, `hooks/`, `.claude-plugin/`, `.cursor-plugin/`) are created automatically. |
| `plugin-repository` | `null` | Optional string written to the plugin manifest's `repository` field when `mode = "plugin"`. |
| `plugin-license` | `null` | Optional string written to the plugin manifest's `license` field when `mode = "plugin"`. |

### Prefix behavior

When `prefix` is set, the naming convention varies by provider:

| Provider | Prefix behavior | Invocation example |
| --- | --- | --- |
| Claude | Dash prefix on paths (`tw-commit.md` for agents, `tw-commit/` for skills) | `tw-commit` |
| OpenCode | Commands sync under a prefix subdirectory (`commands/tw/commit.md`); agents/skills use dash-prefixed names | `/tw/commit` |
| Cursor | Path uses dash prefix (`tw-commit/...`) | `tw-commit` |

#### Content-reference prefix

By default, content references (via `{{ specs.skill.foo.name }}`) use the same prefix format as file paths: `{prefix}-{id}`. To use a different format (for example, Claude Code plugins require the colon-namespaced form `tw:skill-name`) set `content-prefix` explicitly:

| Config                                    | File path    | Content reference |
| ----------------------------------------- | ------------ | ----------------- |
| `prefix = "tw"`                           | `tw-commit/` | `tw-commit`       |
| `content-prefix = "tw:"`                  | `commit/`    | `tw:commit`       |
| `prefix = "tw"`, `content-prefix = "tw:"` | `tw-commit/` | `tw:commit`       |

`content-prefix` is a literal string prepended directly to the spec ID. The separator (`:`, `-`, etc.) is part of the value itself.

### Collision detection

By default, `sync` will error when a user-owned file already exists at a destination path. To resolve a collision you can choose one of:

- Use `--force` to overwrite the file
- Set a `prefix` so the synced files will have a different name
- Enable `overwrite` in your `agentspec.toml` (with backup)
- Remove the conflicting file manually

#### Manifest tracking

`sync` writes a `.agentspec-manifest.json` file in each destination directory it manages (e.g. `~/.claude/agents/.agentspec-manifest.json`). This manifest tracks which files agentspec owns so it can distinguish them from your hand-written files.

On each sync, the manifest enables:

- **Stale cleanup** — files that were previously synced but are no longer produced by the current compile are automatically deleted.
- **Collision detection** — if a file exists at a destination path but isn't in the manifest, sync treats it as user-owned and errors rather than overwriting it (unless `--force` is used).
- **Skip unchanged** — files whose content hasn't changed since the last sync are left untouched.

When `--force` overwrites a user-owned file, the original is backed up as `<filename>.bak.<timestamp>` in the same directory before being replaced.

## Remove

`agentspec remove` reverses a prior `sync` by removing all agentspec-tracked files and configuration.

```sh
agentspec remove                       # every provider configured under [sync.<provider>]
agentspec remove --provider claude     # narrow to one provider (repeatable / comma-separated)
agentspec remove --dry-run             # preview every file/manifest deletion without writing
```

### What is not removed

- **`generated/<provider>/`** — `compile` output is independent of `sync`; `remove` reverses `sync`, not `compile`.
- **`.bak.<timestamp>` files** — backups created when `sync --force` overwrites a user-owned file are not in the manifest. Clean them up by hand if you no longer need them: `find ~/.claude -name '*.bak.*' -delete`.
- **Files agentspec did not write** — anything not recorded in `.agentspec-manifest.json` is treated as user-owned and left alone, even if it looks like agentspec output. The manifest is the source of truth.

## Prune

`agentspec prune` strips orphaned agentspec entries from host config files. It consults no `[sync.<provider>]` configuration, so it reaches entries left behind by a provider you have already deleted from `agentspec.toml` — which `remove` can no longer act on.

```sh
agentspec prune                    # every provider
agentspec prune --provider claude  # narrow to one provider (repeatable / comma-separated)
agentspec prune --dry-run          # preview without writing
agentspec prune --verbose          # list every checked path, including those with no entries
```

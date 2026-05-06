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
    - bash
    - glob
    - grep
    - read
---
Review the proposed changes for correctness, security, and maintainability.
```

### Skills

Skills are reusable prompts that can be invoked by users or agents. Each skill is a directory in `spec/skills/` containing exactly one `.md` file and any supporting files (scripts, templates, etc.).

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
    - bash
---
Create a git commit for the current changes.
```

#### Supporting files

Any non-`.md` files in a skill directory are bundled as supporting files and synced alongside the skill. This is useful for scripts that the skill references in its instructions. For example:

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
event = "user_prompt_submit"
script = "scripts/init-thoughts.sh"
description = "Seed THOUGHTS_DIR context at the start of each turn"
timeout = 30

[hooks.audit-bash]
event = "pre_tool_use"
matcher = "Bash"
script = "scripts/audit-bash.sh"
```

`spec/hooks/scripts/` contains both entry scripts (referenced by `[hooks.<id>].script`) and any helper scripts they `source` — agentspec walks the directory and copies all files. Helper conventions like `_common.sh` are supported. The `_agentspec_*` filename prefix is reserved for future use and rejected at load time.

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

`matcher` is only valid on the three tool-execute events (`pre_tool_use`, `post_tool_use`, `post_tool_use_failure`).

#### Sync modes

- **Path mode** (`mode = "path"`): agentspec writes a complete `hooks/hooks.json` plus `hooks/scripts/` under the configured destination. The plugin owns the file.
- **User mode** (`mode = "user"`): scripts land at `~/.<provider>/hooks/scripts/`; entries are merged into `~/.<provider>/settings.json` (Claude) or `~/.cursor/hooks.json` (Cursor) via a CST-aware patcher. Comments, trailing commas, and user-authored entries round-trip unchanged. Each agentspec entry carries an `_agentspec_id` sentinel so re-syncing replaces only entries it owns.
- **Project mode** (`mode = "project"`): same as User mode but rooted at the project directory.

#### OpenCode behavior

If your spec set contains hooks and `[sync.opencode]` is configured, agents/skills/rules sync normally and a per-provider warning is printed: `opencode: skipped N hooks`. Use `--verbose` to list each skipped hook id.

### Frontmatter reference

| Field                | Agent    | Skill    | Rule     | Description                                                                                                 |
| -------------------- | -------- | -------- | -------- | ----------------------------------------------------------------------------------------------------------- |
| `id`                 | required | required | required | Unique identifier. Must be unique across all spec types.                                                    |
| `description`        | required | optional | optional | Short description of what this spec does.                                                                   |
| `user_invocable`     | —        | required | —        | Whether users can invoke this skill directly (e.g., via `/commit`).                                         |
| `agent_invocable`    | —        | required | —        | Whether agents can invoke this skill. At least one of `user_invocable` or `agent_invocable` must be `true`. |
| `execution.preset`   | optional | optional | —        | Name of a model preset defined in `agentspec.toml`. See [Model presets](#model-presets).                    |
| `tags`               | optional | optional | optional | List of string tags for categorization. Exposed in the `specs` template variable.                           |
| `capabilities.tools` | optional | optional | —        | List of tools the agent/skill can use. See [Tools reference](#tools-reference) below.                       |

### Tools reference

| Tool        | Description                                   |
| ----------- | --------------------------------------------- |
| `read`      | Read file contents                            |
| `write`     | Create or overwrite files                     |
| `edit`      | Make targeted edits to existing files         |
| `grep`      | Search file contents with regex patterns      |
| `glob`      | Find files by name patterns                   |
| `bash`      | Run shell commands                            |
| `webfetch`  | Fetch content from a URL                      |
| `websearch` | Search the web                                |
| `question`  | Ask the user a question                       |
| `tasks`     | Create and manage tasks for tracking progress |
| `subagent`  | Dispatch work to a subagent                   |
| `skill`     | Invoke a named skill                          |

### Templating

Spec bodies support [MiniJinja](https://docs.rs/minijinja/latest/minijinja/syntax/index.html) template syntax. This is primarily useful for including shared fragments across specs, but the full syntax is available (`{% if %}`, `{% for %}`, filters, etc.).

#### Fragments

Fragments are reusable Markdown snippets stored in `spec/fragments/`. They help avoid duplicating instructions across multiple skills or agents.

A fragment is a `.md` file referenced by its path relative to `spec/fragments/`:

```
spec/fragments/
├── review-guidelines.md          → {% include "review-guidelines.md" %}
└── review/
    └── prompt-contract.md        → {% include "review/prompt-contract.md" %}
```

Include a fragment in any spec body with:

```
{% include "review-guidelines.md" %}
```

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
{% include "review/prompt-contract.md" %}
{% endwith %}
```

#### Indented include

When including a fragment inside an indented context (e.g., a numbered list), use `{% filter indent() %}` to preserve the surrounding indentation:

```
1. First step

2. Review the changes:

   {% filter indent(3, first=false) %}
   {%- include "review-guidelines.md" %}
   {%- endfilter %}

3. Next step
```

The `first=false` parameter skips indenting the first line (since it's already at the correct indentation from the call site). The `{%-` trim markers prevent extra whitespace around the included content.

Fragments can include other fragments (nesting is supported).

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

| Field         | Type               | Description                              |
| ------------- | ------------------ | ---------------------------------------- |
| `specs.agent` | map of key → entry | Agents keyed by underscore-normalized ID |
| `specs.skill` | map of key → entry | Skills keyed by underscore-normalized ID |
| `specs.rule`  | map of key → entry | Rules keyed by underscore-normalized ID  |

**Key normalization**: hyphens in spec IDs are replaced with underscores for keyed access. A spec with `id: gh-safe` is accessed as `specs.skill.gh_safe`.

Each entry has:

| Field         | Description                                                                        |
| ------------- | ---------------------------------------------------------------------------------- |
| `name`        | The spec's name as the model sees it (uses `content-prefix` if set, else `prefix`) |
| `description` | The spec's description (empty string if not set)                                   |
| `type`        | One of `agent`, `skill`, or `rule`                                                 |
| `tags`        | List of tags from frontmatter (empty list if not set)                              |

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
# dir = "/path/to/output"   # base directory (only used with mode = "path")
```

## Model presets

Model presets allow you to specify models in your specs in a provider-neutral way. Rather than hard-coding a specific model name you define "presets" for each of your use-cases in `agentspec.toml`:

```toml
[presets.architect]
claude = { model = "opus" }
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

Then refer to a preset in your specs. For example, in `spec/agents/example-agent.md`:

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

| Field            | Default  | Description                                                                                                                                                                                                             |
| ---------------- | -------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `mode`           | `"user"` | `"user"` syncs to the tool's user-level config dir (e.g. `~/.claude/`).<br>`"project"` syncs to the project-local config dir (e.g. `.claude/`).<br> `"path"` syncs to an explicit directory set by `dir`.               |
| `prefix`         | `null`   | Namespace prefix applied to synced file names. Can be useful for avoiding collisions with user-owned files or specs from plugins. See [Prefix behavior](#prefix-behavior) below.                                        |
| `content-prefix` | `null`   | Literal prefix for content references (model-facing names). Includes its separator (e.g., `"tw:"` → `tw:skill-name`). When unset, defaults to `"{prefix}-"`. See [Content-reference prefix](#content-reference-prefix). |
| `overwrite`      | `false`  | When `true`, allows overwriting user-owned files at sync destinations (with backup). Can also be set per-invocation with `--force`.                                                                                     |
| `dir`            | `null`   | Base directory for synced output when `mode = "path"`. Subdirectories (`agents/`, `skills/`, `rules/`, `commands/`) are created automatically.                                                                          |

### Prefix behavior

When `prefix` is set, the naming convention varies by provider:

| Provider | Prefix behavior                                                                                            | Invocation example |
| -------- | ---------------------------------------------------------------------------------------------------------- | ------------------ |
| Claude   | Dash prefix on paths (`tw-commit.md` for agents, `tw-commit/` for skills)                                  | `tw-commit`        |
| OpenCode | Commands sync under a prefix subdirectory (`commands/tw/commit.md`); agents/skills use dash-prefixed names | `/tw/commit`       |
| Cursor   | Path uses dash prefix (`tw-commit/...`)                                                                    | `tw-commit`        |

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

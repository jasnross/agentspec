# agentspec

Every AI coding tool has its own format for agents, skills, and rules. If you
use more than one tool, you end up either duplicating your prompts or leaving
some tools poorly configured.

`agentspec` lets you define agents, skills, and rules once in a provider-neutral
format, then compile and sync them to Claude Code, Cursor, Codex, and OpenCode.
Each tool gets output tailored to its own conventions while you maintain a
single source of truth.

## Usage

```sh
agentspec sync                   # compile and sync to all configured targets
agentspec sync --profile work    # apply a machine profile
agentspec sync --dry-run         # preview without making changes
agentspec sync --no-compile      # sync from existing generated output
agentspec sync --target claude   # sync a specific provider only
agentspec compile                # compile only, writes generated/
agentspec validate               # validate specs without generating output
agentspec check                  # verify generated files are up to date
```

## Configuration

Place an `agentspec.toml` in your project root (or any parent directory).
All sections are optional — omit what you don't need. Tool name mappings and
provider capabilities are embedded in the compiler and require no configuration.

```toml
[spec]
agents_dir = "spec/agents"
skills_dir = "spec/skills"
fragments_dir = "spec/fragments"

[output]
dir = "generated"

# Model presets: preset name → per-provider model config.
# Values are either a string shorthand or an object with model/variant/reasoning_effort.
[presets.deep_review]
claude = "opus"
opencode = { model = "anthropic/claude-opus-4-6", variant = "max" }
codex = { model = "gpt-5.3-codex", reasoning_effort = "xhigh" }
cursor = "inherit"

[presets.balanced]
claude = "sonnet"
opencode = { model = "anthropic/claude-sonnet-4-5", variant = "high" }
codex = { model = "gpt-5.3-codex", reasoning_effort = "medium" }
cursor = "fast"

# Machine profiles — merged in when --profile home (or AGENTSPEC_PROFILE=home)
[profiles.home.balanced]
opencode = { model = "openai/gpt-5.3-codex", variant = "medium" }
```

## Sync

`agentspec sync` compiles specs and distributes the generated output to each
tool's config directory in one step. Sync targets are configured in
`agentspec.toml` under `[sync.<provider>]`:

```toml
# Default: user-level symlinks for all providers
[sync.claude]
mode = "user"       # user | project | path
strategy = "symlink" # symlink | copy

# Work profile: copy Claude/Cursor output to a shared workspace
[profiles.work.sync.claude]
mode = "path"
strategy = "copy"
strip_name = true   # remove name: from skill frontmatter (for plugin namespacing)
agents = "~/Workspace/thoughts/plugin/agents"
skills = "~/Workspace/thoughts/plugin/skills"
rules  = "~/Workspace/thoughts/plugin/rules"

[profiles.work.sync.cursor]
mode = "path"
strategy = "copy"
skills = "~/Workspace/.cursor/skills"
```

**Modes:**

- `user` — syncs to the tool's standard user-level config directory (e.g. `~/.claude/`)
- `project` — syncs to a `.claude/` (or equivalent) subdirectory of the current working directory
- `path` — syncs to explicit per-kind paths; use `agents`, `skills`, `rules`, `commands` fields

**Strategies:**

- `symlink` — creates per-entry symlinks from the destination into `generated/`; stale symlinks are removed automatically
- `copy` — copies files and tracks ownership via `.agentspec-manifest.json`; stale copies are removed on the next sync

For OpenCode, `agentspec sync` also patches the `instructions` array in
`opencode.json` with the absolute paths of the synced rule files.

> **Note**: Codex rules are not yet synced — the Codex adapter emits individual
> `.md` rule files but Codex expects a single `~/.codex/AGENTS.md`. A fix is
> tracked in `TODO.md`.

## Machine Profiles

Use `--profile` to apply a machine-specific overlay at compile or sync time:

```sh
agentspec sync --profile home
agentspec compile --profile home
agentspec validate --profile work
```

Provider values in `[profiles.<name>.<preset>]` merge over the corresponding
`[presets.<preset>]` entries at the provider level. Profile overlays also apply
to `[sync.*]` targets via `[profiles.<name>.sync.<provider>]`.

To make a selection permanent, set the `AGENTSPEC_PROFILE` environment variable
in your shell profile instead:

```sh
export AGENTSPEC_PROFILE=home
```

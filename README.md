# agentspec

Compile provider-neutral SKILL.md/AGENT.md spec files into ready-to-use
configurations for Claude Code, Cursor, Codex, and OpenCode.

## Usage

```sh
agentspec compile                         # compile all specs for all providers
agentspec compile --target claude,cursor  # compile for specific providers
agentspec compile --profile home          # apply a machine-specific overlay
agentspec validate                        # validate specs without generating output
agentspec check                           # verify generated files are up to date
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

# Model profiles: profile name → per-provider model config.
# Values are either a string shorthand or an object with model/variant/reasoning_effort.
[profiles.deep_review]
claude = "opus"
opencode = { model = "anthropic/claude-opus-4-6", variant = "max" }
codex = { model = "gpt-5.3-codex", reasoning_effort = "xhigh" }
cursor = "inherit"

[profiles.balanced]
claude = "sonnet"
opencode = { model = "anthropic/claude-sonnet-4-5", variant = "high" }
codex = { model = "gpt-5.3-codex", reasoning_effort = "medium" }
cursor = "fast"

# Machine-specific overrides — merged in when --profile home (or AGENTSPEC_PROFILE=home)
[profile_overrides.home.balanced]
opencode = { model = "openai/gpt-5.3-codex", variant = "medium" }
```

## Profile Overlays

Use `--profile` to apply a machine-specific overlay at compile time:

```sh
agentspec compile --profile home
agentspec validate --profile work
```

Provider values in `[profile_overrides.<name>.<profile>]` merge over the corresponding
`[profiles.<profile>]` entries at the provider level.

To make a selection permanent, set the `AGENTSPEC_PROFILE` environment variable
in your shell profile instead:

```sh
export AGENTSPEC_PROFILE=home
```

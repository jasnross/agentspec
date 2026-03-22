# agentspec

Compile provider-neutral SKILL.md/AGENT.md spec files into ready-to-use
configurations for Claude Code, Cursor, Codex, and OpenCode.

## Usage

```sh
agentspec compile              # compile all specs for all providers
agentspec compile --target claude,cursor  # compile for specific providers
agentspec validate             # validate specs without generating output
agentspec check                # verify generated files are up to date
```

## Configuration

Place an `agentspec.toml` in your project root (or any parent directory):

```toml
[spec]
agents_dir = "spec/agents"
skills_dir = "spec/skills"
fragments_dir = "spec/fragments"

[mappings]
models = "mappings/models.yaml"
tools = "mappings/tools.yaml"
features = "mappings/features.yaml"

[output]
dir = "generated"
```

All values shown are defaults and can be omitted.

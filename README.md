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

## Release Policy

- Version tags use `vX.Y.Z` and must match the `Cargo.toml` package version.
- Standard releases follow the `release-please` PR flow into protected `main`.
- Manual hotfix publication is an exception path and uses `release.yml`
  `workflow_dispatch` with an explicit tag.
- Supported versions: latest release and previous minor release.

## Install and Update

### Homebrew (shared org tap)

```sh
brew tap jasnross/tap
brew install agentspec
brew upgrade agentspec
brew uninstall agentspec
```

The formula lives in the separate tap repository (`jasnross/homebrew-tap`) so
source-repo ACLs, formula CI, and release automation boundaries stay decoupled.

### mise (`github:` backend)

```sh
mise use -g github:jasnross/agentspec@latest
agentspec --version
```

To pin a specific version:

```sh
mise use -g github:jasnross/agentspec@0.1.0
```

To remove it:

```sh
mise uninstall github:jasnross/agentspec
```

## Artifact Compatibility Contract

- Release archives are published as `agentspec-vX.Y.Z-<target>.tar.gz`.
- Supported release targets:
  - `aarch64-apple-darwin`
  - `x86_64-apple-darwin`
  - `x86_64-unknown-linux-gnu`
- Every release publishes `SHA256SUMS`, SPDX JSON SBOM (`*.spdx.json`), and
  GitHub OIDC attestations.
- Homebrew and `mise` consumers rely on this naming contract as stable API.

## Verify Release Integrity

Download release assets and verify checksums:

```sh
shasum -a 256 -c SHA256SUMS
```

Verify provenance attestations with GitHub CLI:

```sh
gh attestation verify agentspec-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz --repo jasnross/agentspec
gh attestation verify agentspec-vX.Y.Z-sbom.spdx.json --repo jasnross/agentspec
```

## Distribution Support

- Primary install channels are Homebrew custom tap and `mise` GitHub releases.
- `asdf` support is intentionally deferred until a named long-term owner commits
  to maintaining the plugin.

## Tap Update Procedure (manual path)

Tap PR automation is currently deferred. After each release:

1. Update `Formula/agentspec.rb` in `jasnross/homebrew-tap` to the new tag.
2. Update SHA256 values from the release `SHA256SUMS` asset.
3. Run tap CI (`brew audit` and `brew test`) and merge with required approvals.

## Troubleshooting

- `brew install agentspec` fails with checksum mismatch: refresh the tap,
  verify formula SHA entries match release `SHA256SUMS`, then retry.
- `mise` selects the wrong asset: configure explicit `asset_pattern` in
  `mise.toml` for your platform.
- Attestation verification fails: confirm you are verifying a file downloaded
  from the matching `jasnross/agentspec` release tag.

## Rollback Guidance

If a bad release is published:

1. Mark the broken release in team comms and ask users to pin to the last known
   good version.
2. Revert or supersede the corresponding Homebrew tap formula update so
   `brew upgrade` does not move users onto the broken release.
3. Cut a replacement patch release (`vX.Y.Z+1`) through the standard
   `release-please` flow when possible.
4. If emergency timing requires `workflow_dispatch`, document the incident and
   follow up with a normal release PR to restore the default process.

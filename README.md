# agentspec

Every AI coding tool has its own format for agents, skills, and rules. If you
use more than one tool, you end up either duplicating your prompts or leaving
some tools poorly configured.

`agentspec` lets you define agents, skills, and rules once in a provider-neutral
format, then compile and sync them to Claude Code, Cursor, and OpenCode.
Each tool gets output tailored to its own conventions while you maintain a
single source of truth.

## Usage

```sh
agentspec sync                   # compile and sync to all configured targets
agentspec sync --dry-run         # preview without making changes
agentspec sync --no-compile      # sync from existing generated output
agentspec sync --force           # allow overwriting user-owned destination files
agentspec sync --provider claude --mode user # CLI-only sync for one provider
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
rules_dir = "spec/rules"
fragments_dir = "spec/fragments"

[output]
dir = "generated"

# Model presets: preset name → per-provider model config.
# Values are either a string shorthand or an object with model/variant/reasoning_effort.
[presets.deep_review]
claude = "opus"
opencode = { model = "anthropic/claude-opus-4-6", variant = "max" }
cursor = "inherit"

[presets.balanced]
claude = "sonnet"
opencode = { model = "anthropic/claude-sonnet-4-5", variant = "high" }
cursor = "fast"
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
```

**Modes:**

- `user` — syncs to the tool's standard user-level config directory (e.g. `~/.claude/`)
- `project` — syncs to a `.claude/` (or equivalent) subdirectory of the current working directory
- `path` — syncs to explicit per-kind paths; use `agents`, `skills`, `rules`, `commands` fields

**Strategies:**

- `symlink` — creates per-entry symlinks from the destination into `generated/`; stale symlinks are removed automatically
- `copy` — copies files and tracks ownership via `.agentspec-manifest.json`; stale copies are removed on the next sync

### Sync target selection

`agentspec sync` selects providers from explicit sync configuration only:

- Default sync scope is providers configured under `[sync.<provider>]`.
- If no providers are configured, sync fails fast with actionable guidance.
- `--provider` requires either configured sync for that provider or explicit CLI-only intent.
- CLI-only sync is supported with an explicit provider plus either:
  - `--mode user` or `--mode project`, or
  - `--dest <path>` (implies `mode=path`)

Examples:

```sh
# configured-only default sync (syncs only configured providers)
agentspec sync

# unconfigured targeted sync fails (no [sync.claude] and no CLI-only mode)
agentspec sync --provider claude

# CLI-only targeted sync succeeds
agentspec sync --provider claude --mode user
agentspec sync --provider claude --mode project
agentspec sync --provider claude --dest /tmp/agentspec-sync
```

**Namespace prefix:**

Set `prefix = "<name>"` in `[sync.<provider>]` to avoid collisions with user-owned
files or other spec libraries. Prefixing applies to agents, skills, and commands,
but not rules.

| Provider | Prefix behavior                                                                                                                 | Invocation example |
| -------- | ------------------------------------------------------------------------------------------------------------------------------- | ------------------ |
| Claude   | Path uses dash prefix (`tw-commit.md` for agents, `tw-commit/` for skills), `name:` frontmatter uses colon prefix (`tw:commit`) | `tw:commit`        |
| OpenCode | Commands sync under a prefix subdirectory (`commands/tw/commit.md`); agents/skills use dash-prefixed names                      | `/tw/commit`       |
| Cursor   | Path uses dash prefix (`tw-commit/...`)                                                                                         | `tw-commit`        |

Notes:

- For Claude, prefixing requires `strategy = "copy"` (frontmatter must be rewritten).
- `prefix` and `strip_name` are mutually exclusive.
- `agentspec` does not create aliases, so prefixed names are the names you invoke.

```toml
# Namespace prefix: avoids collisions with user-owned or cross-library names.
# Claude: agent file becomes tw-commit.md, skill directory becomes tw-commit/,
# and name: frontmatter becomes tw:commit.
# Requires strategy = "copy" for Claude (name: is in frontmatter, not filename).
[sync.claude]
strategy = "copy"
prefix = "tw"

# OpenCode: commands land in a tw/ subdirectory -> invoked as /tw/commit.
# OpenCode agents/skills also use tw-<name> path prefixing.
[sync.opencode]
prefix = "tw"

# Cursor: filename path becomes tw-<name>
[sync.cursor]
prefix = "tw"
```

**Collision detection:**

By default, sync errors when a user-owned file already exists at a destination
path. To resolve a collision, configure a `prefix` or remove the conflicting
file manually.

- Set `allow_overwrite = true` in `[sync.<provider>]` to restore overwrite behavior
  (copy collisions are backed up; symlink strategy backs up regular files/dirs and
  replaces conflicting symlinks).
- Use `agentspec sync --force` for a one-time override.

For OpenCode, `agentspec sync` also patches the `instructions` array in
`opencode.json` with the absolute paths of the synced rule files.

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

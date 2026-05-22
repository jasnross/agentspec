# Changelog

## [0.4.0](https://github.com/jasnross/agentspec/compare/v0.3.0...v0.4.0) (2026-05-22)


### ⚠ BREAKING CHANGES

* **validate:** `SemanticError` renamed to `ValidationError` in the public `validate` module.
* **spec:** spec files using `tools: [bash]` and templates using `{{ tool("bash") }}` must update to `shell`. Compiled output is unchanged — adapters continue emitting provider-specific names (Claude "Bash", Cursor "Run shell commands", OpenCode "bash").
* **plugin:** `plugin-author` now requires an inline table (`plugin-author = { name = "...", email = "..." }`) instead of a bare string. The `email` field is optional.
* **spec:** `hooks.toml` entries must now use `events = ["..."]` (array) instead of `event = "..."`. A hook can now list multiple events and will emit one provider entry per event. Validation rejects an empty list, duplicates within the list, and matchers paired with non-tool events (reporting each offending event by name).
* **sync:** \`mode = "path"\` no longer parses — migrate to \`mode = "plugin"\` and set \`plugin-name = "<id>"\`. Cursor hook commands now use \`\${CURSOR_PLUGIN_ROOT}\` instead of \`\${CLAUDE_PLUGIN_ROOT}\`; hook scripts that hardcode the Claude name need to reference Cursor's env var on Cursor (or use \`"\${CLAUDE_PLUGIN_ROOT:-\$CURSOR_PLUGIN_ROOT}"\` for provider-neutral scripts).
* **adapters:** collapse ProviderAdapter+HookAdapter into single Adapter trait
* **spec:** removes NormalizedSpec, NormalizedSpecs, NormalizedAgentSpec, NormalizedSkillSpec, NormalizedRuleSpec, NormalizedHookSpec, and their *Frontmatter mirrors from the agentspec library crate's public API. ProviderAdapter::adapt, ProviderAdapter::model_facing_name, and HookAdapter::synthesize_hooks now take Spec/&Spec/&[&HookSpec] instead of the Normalized* variants. Specs::normalize is gone; advance directly via Specs::validate.
* **plan:** agentspec::plan::{FileWrite, WriteMode, WritePlan} are removed from the library API. The replacements are CleanSlateWrite, ManifestTrackedWrite, RemoveWrite, CompilePlan, SyncPlan, and RemovePlan. The agentspec binary is the only known external consumer today and updates in this same commit; pre-1.0 status means no stability guarantee to preserve. Plan: thoughts/plans/2026-05-07-filewrite-typestate-refactor.md

### Features

* **claude:** fan out Subagent to Agent + SendMessage ([759e050](https://github.com/jasnross/agentspec/commit/759e05072a03c854d6896f172d963bec773c1f9f))
* **compile:** cross-provider hook portability warnings + shim manifest round-trip ([9d72624](https://github.com/jasnross/agentspec/commit/9d7262436f48e3c1b3e22a04742e41bd9a122cc4))
* **hooks:** add --force override and honor umask for fresh-file creates ([2894280](https://github.com/jasnross/agentspec/commit/2894280d5b73aeb6863fe869efe896c9cc215662))
* **hooks:** add `agentspec hook test` CLI subcommand ([5d3d1e9](https://github.com/jasnross/agentspec/commit/5d3d1e9299f85304777cc93913080fe149beeb45))
* **hooks:** add AGENTSPEC_HOOK_LOG env-var-gated shim debug logging ([1fe96c3](https://github.com/jasnross/agentspec/commit/1fe96c3e2f128dc6f9bb98566473fc9d1f84e9e7))
* **hooks:** add canonical hook payload schema types ([7b281be](https://github.com/jasnross/agentspec/commit/7b281be1c308c2781c31a6c13ad8cf3aaafe38d0))
* **hooks:** add CST-aware merge for Project/User-scope sync (Phase 2) ([fcf0986](https://github.com/jasnross/agentspec/commit/fcf0986846bed923d4a1b050f70386a1422d39b0))
* **hooks:** add deny_unknown_fields-equivalent validation to shim output jq ([b426d74](https://github.com/jasnross/agentspec/commit/b426d74bf90590f50a44be0195101664151561b3))
* **hooks:** add provider-neutral hooks pipeline (Phase 1) ([65bf27e](https://github.com/jasnross/agentspec/commit/65bf27e58111096f537e22db1f8d90f1a6d20fe3))
* **hooks:** add runtime cross-host detection to canonical shim ([8a2a1cb](https://github.com/jasnross/agentspec/commit/8a2a1cb653a9600c25576c5f1d83297e892226b6))
* **hooks:** add subagent-type matcher translation ([5defa1b](https://github.com/jasnross/agentspec/commit/5defa1b06c6adce3f178784e6ba69b45cb53a1ea))
* **hooks:** generate per-(provider, event) POSIX shell shims ([2934389](https://github.com/jasnross/agentspec/commit/2934389358afdd5e9b2a8085ba4323356423793d))
* **hooks:** tag shim error messages with hook ID ([c7f50f9](https://github.com/jasnross/agentspec/commit/c7f50f99c9360496fbb6deb11fbba0f1f18cd9b9))
* **hooks:** translate canonical matcher tokens per provider ([e848ab9](https://github.com/jasnross/agentspec/commit/e848ab91eaecb1a6c9eaa69a7ef0c882ed8549ae))
* **hooks:** wire shim into adapter compile path ([fa69595](https://github.com/jasnross/agentspec/commit/fa69595a573935da241fc3f926eaf7ec02ba6013))
* **load:** follow symlinks in skill and hook script walks ([def53ee](https://github.com/jasnross/agentspec/commit/def53eeb67706b19c2251495f11a9651c2c2999f))
* **plugin:** add repository and license manifest fields ([d36f2a6](https://github.com/jasnross/agentspec/commit/d36f2a646d2586bc7afdce0ba6115c07d7a13ca1))
* **plugin:** change plugin-author to inline table with email support ([ca18c2b](https://github.com/jasnross/agentspec/commit/ca18c2b721a1bddac2805d1cf33735d419608478))
* preserve verbatim file modes for SupportingFile emission ([d3b0aee](https://github.com/jasnross/agentspec/commit/d3b0aeef779de38b78521f5ec8498dddb3cd549d))
* **remove:** activate deletion pipeline behind `WriteMode::Remove` ([5bd98aa](https://github.com/jasnross/agentspec/commit/5bd98aad4ab9f23e91e09f5ffdc881099e4e0702))
* **remove:** delete empty host config files ([4d50a87](https://github.com/jasnross/agentspec/commit/4d50a87a29cef59a8913c0c8806523f423472b18))
* **remove:** predict dest-dir teardown in dry-run mode ([5f32a3b](https://github.com/jasnross/agentspec/commit/5f32a3b222ab0d9ed81417784250cc998e8955c5))
* **remove:** scaffold `agentspec remove` subcommand ([ba83e83](https://github.com/jasnross/agentspec/commit/ba83e83c8051adc7c6cdf0b9e0863069b036f1b4))
* **remove:** tag every dry-run stderr line with [dry-run] prefix ([cb04424](https://github.com/jasnross/agentspec/commit/cb044248d615d806dffc9c4589d4758c8760434a))
* **remove:** tidy Claude/Cursor settings JSON via post-write hooks ([20f7cdd](https://github.com/jasnross/agentspec/commit/20f7cdda8fbf82dbeef895bb4a1772fde9dc8624))
* **remove:** tidy OpenCode `instructions[]` via post-write hook ([2d4aeed](https://github.com/jasnross/agentspec/commit/2d4aeedbec73704f717d569e3ddbda28d33b2546))
* **rules:** add path-scoped rule support via paths frontmatter field ([205877d](https://github.com/jasnross/agentspec/commit/205877d4dd796d325ef64c88a005d307019e58f1))
* **spec:** rename canonical shell tool from `Bash` to `Shell` ([d9e68a0](https://github.com/jasnross/agentspec/commit/d9e68a090a5a10f0a5c82bd5864007b354751b9e))
* **spec:** replace hook `event` with `events` array for multi-event targeting ([72488c5](https://github.com/jasnross/agentspec/commit/72488c5045923b824d17c34715c1d8a04ecf7aef))
* **sync:** replace `mode = "path"` with first-class `mode = "plugin"` ([c93f655](https://github.com/jasnross/agentspec/commit/c93f655509294b1997c28d388d6568ef8f5e8dc0))
* **templating:** add `extra_fragment_dirs` config for cross-directory fragment sharing ([b6b74b7](https://github.com/jasnross/agentspec/commit/b6b74b7b8fe7a72425e251f8d1a651056221f9b5))
* **templating:** add script_path() and body_skill_root() adapter method ([c85b312](https://github.com/jasnross/agentspec/commit/c85b31212509b2ea6a9c43d368c0f14625a365aa))
* **templating:** gate script_path() to skill bodies ([dfec2c3](https://github.com/jasnross/agentspec/commit/dfec2c3529ca439c44e23bc4789e6d9940aec823))
* **validate:** hoist plugin-mode config validations to load time ([1fc0e64](https://github.com/jasnross/agentspec/commit/1fc0e642b48253dd2d65d7f0be3fc5f5a3c01adb))


### Bug Fixes

* **adapters:** always construct OpenCodeInstructionsPatch to preserve orphan cleanup ([2d993c7](https://github.com/jasnross/agentspec/commit/2d993c7f17fcd10195193106ccb662e3a38f3208))
* **cst:** use platform-safe cast for mode_t to u32 conversion ([02bce70](https://github.com/jasnross/agentspec/commit/02bce70ee52879382788e1a6e1fe27789f4eb366))
* **hooks:** address review findings across hooks pipeline ([20112ea](https://github.com/jasnross/agentspec/commit/20112eafd7b1808638350d16b943d14cec6eae72))
* **hooks:** use real cwd in hook test default fixture ([bf430c4](https://github.com/jasnross/agentspec/commit/bf430c4bab4242006d21ef3aa05ea9529f87bdd7))
* **sync:** restore error when path mode has no dir configured ([b478d1e](https://github.com/jasnross/agentspec/commit/b478d1e0c2af6c67ca619a06f1b027c551972e47))


### Refactoring

* **adapters:** add Adapter + ConfigPatch trait surface (bridge phase) ([150d45a](https://github.com/jasnross/agentspec/commit/150d45a450de32253417ebddbe75c0f213f62ce9))
* **adapters:** bridge `impl Adapter` for all three providers ([b296853](https://github.com/jasnross/agentspec/commit/b29685395932a713c0dfe1b7a37d3c6ef0abe2c6))
* **adapters:** collapse ProviderAdapter+HookAdapter into single Adapter trait ([a87fbe0](https://github.com/jasnross/agentspec/commit/a87fbe07469ec989499bca6704a36523c68ec0d6))
* **adapters:** drop dead trait surface and AdapterConfig field ([d639932](https://github.com/jasnross/agentspec/commit/d63993257ff4a7bdc4652568a3fe9307f0b73f10))
* **adapters:** drop hook_adapter dispatch, parameterize dotdir ([b88220b](https://github.com/jasnross/agentspec/commit/b88220b604ac1898478d1a2740a78957e6aa913b))
* **adapters:** extract shared synthesize_hooks into hook_compile ([2b036af](https://github.com/jasnross/agentspec/commit/2b036af1005c7ff91ca32bfe555bec7f8ffb16a7))
* **adapters:** relocate hook helpers from compile.rs into adapters subtree ([73f7aa0](https://github.com/jasnross/agentspec/commit/73f7aa071424aa667626610873eb5cf063d31676))
* **adapters:** switch orchestrator + plans through `Adapter::compile` ([6f9c9da](https://github.com/jasnross/agentspec/commit/6f9c9da96cc9e88d2eab1110b218c0d62964857c))
* **claude:** replace deprecated TodoWrite with TaskCreate as Tasks representative ([fbf325a](https://github.com/jasnross/agentspec/commit/fbf325a539a223efe23cd60ba820de5e4e770448))
* **compile:** move skipped-hooks diagnostics into compile::run ([21184e4](https://github.com/jasnross/agentspec/commit/21184e44e2577473a5b3ca29e8e1a967593f87d1))
* extract ProviderAdapter and HookAdapter traits ([b419b8d](https://github.com/jasnross/agentspec/commit/b419b8dfaa4f4dcb2df1fe5df2d0209c76f81bc5))
* extract shared CST file I/O helpers into cst_io module ([15fa5ae](https://github.com/jasnross/agentspec/commit/15fa5ae5184eb8faa574e7433331f6eb31615c09))
* lift hook-merge JSON shape into HookAdapter trait ([cf2c0ce](https://github.com/jasnross/agentspec/commit/cf2c0ce08222b3770ae922f2e25efa6f2d50a6a5))
* **opencode:** migrate instructions tidy to CST-aware jsonc-parser ([6c52244](https://github.com/jasnross/agentspec/commit/6c522446b9eb07a00dd013196eca1dcec7ce20ad))
* **plan:** split FileWrite into per-mode typed variants ([7d73bf5](https://github.com/jasnross/agentspec/commit/7d73bf5a20fbee6ac89691448d7c70ee01a86fa5))
* **remove:** post-review polish ([be6b885](https://github.com/jasnross/agentspec/commit/be6b8855ebf4ee0c508cb03d696097d01e99673a))
* **spec:** add Clone derives and accessor methods to Spec ([e3bc367](https://github.com/jasnross/agentspec/commit/e3bc3676facfecb832fe7ef1170760f2e3d875d4))
* **spec:** delete Normalized* types and collapse normalize stage ([6768d40](https://github.com/jasnross/agentspec/commit/6768d40b8d678ff211763620eb4836134f24e9d0))
* **spec:** key supporting_files by relative path in IndexMap ([341a0af](https://github.com/jasnross/agentspec/commit/341a0af2b2888f18a877fdb02d3a46485ebcd844))
* **sync:** make Manifest::load strict-by-default ([9b6ba46](https://github.com/jasnross/agentspec/commit/9b6ba46f4c2ea0501565168b92a82d0778caedaa))
* **templating:** address code review minors from script() rename ([6f5190f](https://github.com/jasnross/agentspec/commit/6f5190f428c6145cb62f1121a0196d307e79078d))
* **templating:** move env construction into environment.rs ([e983bdf](https://github.com/jasnross/agentspec/commit/e983bdfa1007bd698fcbd026a5b1a560d5909bbe))
* **templating:** rename script_path() to script(), auto-prefix scripts/ ([36c3463](https://github.com/jasnross/agentspec/commit/36c3463966462ac0fd45d38c3a04dfda0ff90014))
* **test:** migrate inline TOML writes onto write_sync_config ([a5d72ed](https://github.com/jasnross/agentspec/commit/a5d72edcab9e62c8a74539b4fae9bf57a61086c3))
* **test:** rename write_remove_config helper to write_sync_config ([a1cdf7b](https://github.com/jasnross/agentspec/commit/a1cdf7b2d6975082df9ccb0cd3029f9db53ee3ee))


### Documentation

* add TODO for shared spec directory support ([cc47445](https://github.com/jasnross/agentspec/commit/cc47445b1854b77ea5a886bbef896900978cd6a7))
* capture jsonc-parser + manifest-version TODOs ([4e2cc54](https://github.com/jasnross/agentspec/commit/4e2cc547799fc7f1f350a02b81a587a65b023d4f))
* clarify adapter-implementations vs shared-helpers distinction ([051111f](https://github.com/jasnross/agentspec/commit/051111fcf368cf8f33e28d1ec924723141e526b7))
* **claude.md:** add pre-1.0 project status and design philosophy ([ad719d1](https://github.com/jasnross/agentspec/commit/ad719d190db61214d36c2d8575759cf6b61efe53))
* **hooks:** add canonical schema reference for hook authors ([055f3be](https://github.com/jasnross/agentspec/commit/055f3be41fdfeef919ada8c5530070479d0c59af))
* README cleanup ([3bac083](https://github.com/jasnross/agentspec/commit/3bac0838683aecaf169e3877e2cedc5551ee8026))
* **readme:** document canonical matcher tokens ([bf2e4cc](https://github.com/jasnross/agentspec/commit/bf2e4cc1a633499617ec3ebcf2d83c0e791b845f))
* **remove:** add `## Removing` section + cross-cutting round-trip test ([8b570ea](https://github.com/jasnross/agentspec/commit/8b570eae070e6d1bd5d3700ac573d37f3cb45272))
* **rules:** drop stale code citations from provider-logic-in-adapters ([3cb8d62](https://github.com/jasnross/agentspec/commit/3cb8d62b9fdee9579dfced3b17aee6e9c51fafaa))
* scrub Phase N milestone labels from hooks-pipeline doc comments ([8262b2a](https://github.com/jasnross/agentspec/commit/8262b2a494f9b4719cab1d32d73d8cf79ca5a437))
* **todo:** add todo for supporting plugin and repository fields in plugins ([bdeef5b](https://github.com/jasnross/agentspec/commit/bdeef5b34b9d140759b27c101a81b13e4ee40c85))
* **todo:** add todos for TodoWrite deprecation and single-spec stdout compile ([d55cfb4](https://github.com/jasnross/agentspec/commit/d55cfb45f2645866f170d51ee012772b2af83935))
* **todo:** mark FileWrite typestate refactor done; track verbose-on-remove parity ([c489a3d](https://github.com/jasnross/agentspec/commit/c489a3d23901bc329267199e2130a37074f2ab79))
* **todo:** track Phase N label scrub from doc comments ([a3d3be7](https://github.com/jasnross/agentspec/commit/a3d3be73a45b3b0d9eb806e085cb6cab09eb427b))


### Tests

* **adapters:** backfill cursor dest_dir tests, tidy discover_rules helper ([24aa727](https://github.com/jasnross/agentspec/commit/24aa727eefd6c16bd43aa03209c4ff0781059c86))
* **pipeline:** add Project-mode hook sync integration test ([14d2273](https://github.com/jasnross/agentspec/commit/14d2273d5c4d2241d22b3ab8a8ad11292838089c))


### Miscellaneous Chores

* add Claude rules ([4c70661](https://github.com/jasnross/agentspec/commit/4c70661bb47327c5a445a58b976ab485a903e9d8))
* add graphify and watch recipes to justfile ([e361b5c](https://github.com/jasnross/agentspec/commit/e361b5c3dcd9b0e86bc5fda87c74ac6ed872083f))
* add prettier formatting for markdown ([44b2ed4](https://github.com/jasnross/agentspec/commit/44b2ed4a6790a057079ad93089cfd1e1cc2be86b))
* add TODO for jq auto-install via plugin persistent-data dir ([e75703e](https://github.com/jasnross/agentspec/commit/e75703e99a155fdf2cdc46233b3542666c95cb77))
* add TODO to clean up cli_sync_intent_sufficient ([4ac4915](https://github.com/jasnross/agentspec/commit/4ac49151b8d232db83ab15e9f7e028e3a58ed6b9))
* **cargo:** sort dependencies alphabetically ([d67b9db](https://github.com/jasnross/agentspec/commit/d67b9db1cf041cad199548d284f200f2d84eaa85))
* cleanup and backlog updates ([b6cb5ca](https://github.com/jasnross/agentspec/commit/b6cb5ca85ade3cf985f0488c1a92b88ade5893f2))
* **experiments:** add session-id-resume, cursor-output-gates, and cursor-plugin-mode-probes probe suites ([68c41bd](https://github.com/jasnross/agentspec/commit/68c41bd2b7503f63308a927b6c7fb1590068a95a))
* formatting ([b6bb611](https://github.com/jasnross/agentspec/commit/b6bb611edc13832fdc85389bc7e663b7318fbb21))
* gitignore graphify-out/ entirely ([c55587b](https://github.com/jasnross/agentspec/commit/c55587bb8ead50789076c728ed3ddc6516eeb45a))
* **justfile:** add graphify-watch recipe ([9fad849](https://github.com/jasnross/agentspec/commit/9fad849b1832013ae8a70a00761834845df82891))
* **justfile:** update check recipe to run cargo check instead of build ([3e45711](https://github.com/jasnross/agentspec/commit/3e457119f0d86b876a88d0c40d8df574f1e0c0c9))
* prettier format ([df8596b](https://github.com/jasnross/agentspec/commit/df8596bed94c2dee21a34d28ea22d9801e26ee0e))
* reflow README markdown tables under prettier ([b6ef6cb](https://github.com/jasnross/agentspec/commit/b6ef6cbc8fa9c8a55f066cd0bd04385b9f02baa4))
* remove completed TODOs ([e5cb6d5](https://github.com/jasnross/agentspec/commit/e5cb6d5f514a403e0e021f1b1f651ae61b40de87))
* remove completed TODOs ([0ef4c4b](https://github.com/jasnross/agentspec/commit/0ef4c4b7c9981187c66b154547d47952c7eab23b))
* remove outdated TODO item ([e48c1d0](https://github.com/jasnross/agentspec/commit/e48c1d0ddc1ab6db118ad367d6e2922ef8aae100))
* remove stale TODO.md items ([39c015b](https://github.com/jasnross/agentspec/commit/39c015b538e1195d2a26d2c8c563189ac89e33c1))
* run graphify ([0de7e2b](https://github.com/jasnross/agentspec/commit/0de7e2be67cd7ef2b2f529e3c2413f0df9a9861e))
* **todos:** add follow-up items for Tasks tool fan-out ([c21056d](https://github.com/jasnross/agentspec/commit/c21056d27008cd67f7a86246b76ba6334b9944fb))
* update graphify output ([70e73da](https://github.com/jasnross/agentspec/commit/70e73dadac81bb3bbd162ae3e393f7a38fb88fa9))
* update graphify output ([afc15fe](https://github.com/jasnross/agentspec/commit/afc15fecb8b0f0dbf3658ad19d4ce58d4e9ba865))
* update pre-commit instructions ([5b072bd](https://github.com/jasnross/agentspec/commit/5b072bd5f312dc368cf46fa0c4344a6509cd36c8))
* update TODO with hook-matcher gap, provider-specific specs, and plugin-author email items ([315783c](https://github.com/jasnross/agentspec/commit/315783cc61c7ad312f6cb5765b09361f8cb76f91))

## [0.3.0](https://github.com/jasnross/agentspec/compare/v0.2.0...v0.3.0) (2026-04-23)


### Features

* **cli:** surface [spec].ignore diagnostics via --verbose and validate ([78671ec](https://github.com/jasnross/agentspec/commit/78671ec82b072a09e0f1225601634ad70796bdb2))
* **config:** add content-prefix field for independent content-reference namespacing ([d7202ba](https://github.com/jasnross/agentspec/commit/d7202ba998dc1b26d16c60e6e8863fe86a99539c))
* **emit:** introduce BatchStats and kind field for sync report (phase 1) ([2b31348](https://github.com/jasnross/agentspec/commit/2b3134879a4276af087c84a7c3012c862571a2b6))
* **emit:** replace per-batch eprintln with unified sync report table ([96f0514](https://github.com/jasnross/agentspec/commit/96f0514dbe3e995583ae85d57a863b8ce457c7b7))
* **spec:** add IgnoreMatcher type and [spec].ignore config field ([efe2716](https://github.com/jasnross/agentspec/commit/efe2716a43d2994ab68da725889eb800d8075ff9))
* **spec:** add subagent/skill canonicals and rewrite Cursor body_tool_name policy ([9300162](https://github.com/jasnross/agentspec/commit/930016257778fc48f048007fd38dbc23848d17d2))
* **specs:** apply [spec].ignore during load-stage walks ([698745c](https://github.com/jasnross/agentspec/commit/698745c58856cfdfd2b73c5ce83f25d921ed8b41))
* **templating:** add tool() MiniJinja function for provider-aware tool name resolution ([e8c10e8](https://github.com/jasnross/agentspec/commit/e8c10e83e9446fbdb0c7e56329ec0e8e179e0936))


### Bug Fixes

* **clippy:** replace map().unwrap_or() with is_ok_and/map_or on Result values ([e0eadd7](https://github.com/jasnross/agentspec/commit/e0eadd7e6d8d62883e08635ae84eef3c5a39f6c2))


### Documentation

* fix doc comment accuracy and CLI flag consistency ([bf29b5d](https://github.com/jasnross/agentspec/commit/bf29b5d01725d25b137ea0fd0a66d659f32a85b5))
* **opencode:** correct non-canonical tool list in build_tool_map comment ([c137b36](https://github.com/jasnross/agentspec/commit/c137b360b20d8431c5c681b2322541b64bbb4cb2))
* update shell tool TODO with Claude PowerShell coverage note ([c744b92](https://github.com/jasnross/agentspec/commit/c744b92ebe0d2ec963c76172b12bf7b8a002363c))


### Miscellaneous Chores

* add just recipes and document developer workflow ([09ec37e](https://github.com/jasnross/agentspec/commit/09ec37e043ec42fe3e73fb0497c144221484ade1))

## [0.2.0](https://github.com/jasnross/agentspec/compare/v0.1.0...v0.2.0) (2026-04-12)


### Features

* **spec:** add optional tags field to all spec types ([aaa02c2](https://github.com/jasnross/agentspec/commit/aaa02c2980bd4fadf8ef859af7d0b17adac04d1a))
* **templating:** add built-in specs template variable ([997b68f](https://github.com/jasnross/agentspec/commit/997b68f1aaaed4e9da8040dda9efc0e0a281bd93))
* **templating:** add prefix-aware keyed spec access in templates ([a662802](https://github.com/jasnross/agentspec/commit/a662802e1523299708aed01184bebe3734aeb691))
* **validate:** detect underscore-normalization collisions ([8ba26bd](https://github.com/jasnross/agentspec/commit/8ba26bd00018285f4f10ac0da392d87b6e649a68))


### Bug Fixes

* **config:** allow prefix and strip_name to be used together ([16d38f6](https://github.com/jasnross/agentspec/commit/16d38f60d8319c36423647f053a069763dc9f88e))
* **emit:** simplify collision error message ([8b2c6dd](https://github.com/jasnross/agentspec/commit/8b2c6dde3dc3fc577feb3a98ab5c7d73a236c47b))
* **manifest:** sort manifest keys for deterministic output ([41982c8](https://github.com/jasnross/agentspec/commit/41982c853fd00322dde32522faeabc774225dfbf))
* **release:** drop component prefix from release tags ([19f4530](https://github.com/jasnross/agentspec/commit/19f453094b7cb5f835f4b7c75eb8c144abcfe4a9))
* **release:** run checksum verification from dist directory to match generated paths ([e5e7a05](https://github.com/jasnross/agentspec/commit/e5e7a05eb78fb22940c14adc1bbac97b079b8cbc))
* **release:** update tap automation to patch URLs instead of version line ([7dad772](https://github.com/jasnross/agentspec/commit/7dad77232b5d66905f6da54f6c574bb658b6f3ce))
* **release:** use macos-14 for x86_64 cross-compile after macos-13 retirement ([6d98da8](https://github.com/jasnross/agentspec/commit/6d98da82debe44c4133b7aaa715c10e17c92acc8))
* **release:** use temporary tap for homebrew gate after standalone formula rejection ([cc6e7b4](https://github.com/jasnross/agentspec/commit/cc6e7b4f2e103facaf1affd91b210932d07a72c1))


### Refactoring

* **compile:** move template resolution into compile loop ([3c53c9c](https://github.com/jasnross/agentspec/commit/3c53c9ca13c4fd12a7fdc5cc73927ff87436b50a))
* simplify config schema, remove strip_name, add --prefix CLI flag ([ad892d7](https://github.com/jasnross/agentspec/commit/ad892d755105e7448559b6c391b3df1547927b8c))
* **sync:** rename has_provider to has_provider_arg for clarity ([b2f5390](https://github.com/jasnross/agentspec/commit/b2f53904886c7b50f2467838bfbe1b457b625c86))


### Documentation

* document keyed spec access and tags frontmatter field ([ed17026](https://github.com/jasnross/agentspec/commit/ed1702689ac1e0e33e6eaa7abc9e287af3d6926b))
* expand README with quick start, spec format, and configuration reference ([d71551f](https://github.com/jasnross/agentspec/commit/d71551fb34ca9b0d03b706151d0c5e1eeac555f8))


### Tests

* add integration tests and fixture for spec references ([fb555e2](https://github.com/jasnross/agentspec/commit/fb555e2adf4ff8327485002e814052f5c9f69d5d))


### Miscellaneous Chores

* add git-town.toml ([cea5581](https://github.com/jasnross/agentspec/commit/cea5581ad16cd2ef3e4a201244ab6bed8df9a385))
* formatting ([8ae5727](https://github.com/jasnross/agentspec/commit/8ae57277f221d2dee499672adc23f4d3005eb648))

## 0.1.0 (2026-04-03)

### Features

- **adapters:** add frontmatter name prefixing for Cursor agents and skills ([bb97d59](https://github.com/jasnross/agentspec/commit/bb97d598e57d510351f5def83f6fa3be4bfbdac7))
- **adapters:** apply file prefix to rule output paths ([395ee46](https://github.com/jasnross/agentspec/commit/395ee46e6e65a666663bfc4a3965cb5d096270b5))
- add SpecKind::Rule and compile rules to all four providers ([39f10bc](https://github.com/jasnross/agentspec/commit/39f10bc55e38949936a9501b091f3ca253be0ca6))
- add sync command ([e860de7](https://github.com/jasnross/agentspec/commit/e860de781f2b34faaabcd52307f86875b7931140))
- **compile:** embed provider mappings and externalize model profiles ([ec73dc2](https://github.com/jasnross/agentspec/commit/ec73dc291ffd20cfdbe816bb53bd05c59ffaa1ef))
- **config:** reject unknown fields in SpecConfig and OutputConfig ([b90b3eb](https://github.com/jasnross/agentspec/commit/b90b3eb1635ac5b9f588d5ddd9d1035c323c1e84))
- **plan:** introduce WritePlan and move dest resolution to library ([9411b76](https://github.com/jasnross/agentspec/commit/9411b768949e727d3d9f0e31a7bdf24b345eb70b))
- **release:** automate binary releases and distribution docs ([c395045](https://github.com/jasnross/agentspec/commit/c395045de17d70b080aa7f5b0a4c597e24b23433))
- **release:** automate homebrew tap update PRs ([6ec93c0](https://github.com/jasnross/agentspec/commit/6ec93c09065f5f858175634b3f3c225376e9884f))
- **spec:** deny unknown fields in frontmatter structs ([a196d17](https://github.com/jasnross/agentspec/commit/a196d1756d1869d0c4feea61bbb7df0af498a552))
- **sync:** add namespace prefixing and collision-safe overwrite controls ([5af8293](https://github.com/jasnross/agentspec/commit/5af8293191a418ea1ed53f0c05ac31d4beaf45b6))
- **sync:** require explicit sync intent for provider selection ([aa50c29](https://github.com/jasnross/agentspec/commit/aa50c291848e413c8e2189335ff5302b2052900e))

### Bug Fixes

- **adapters:** remove redundant newline before closing --- in frontmatter ([c74205b](https://github.com/jasnross/agentspec/commit/c74205ba48288c771f361610cbf7c60f29da7164))
- **adapters:** sort Claude tools by serialized name, remove unnecessary Result wrappers ([1a92dff](https://github.com/jasnross/agentspec/commit/1a92dff882cb0ed959d5b5eafb36c6409b4c04f5))
- address correctness and type issues ([a17b3b2](https://github.com/jasnross/agentspec/commit/a17b3b278e23dcfd1cd9bef49272d8db82f30820))
- **ci:** pin release workflows to full action SHAs ([9c5f3ef](https://github.com/jasnross/agentspec/commit/9c5f3ef77a217092bbd6443df1c231de141ba216))
- **emit:** make strip_name idempotent, scope to skills, remove ad-hoc logging ([45149fb](https://github.com/jasnross/agentspec/commit/45149fb961a5c8d6aaacbb6db90d4f20fcdc3f58))
- **emit:** remove unnecessary Result wrapper from check_generated_state ([2ef9a89](https://github.com/jasnross/agentspec/commit/2ef9a890898106557982ff12ba343271c60c8d96))
- **main:** use home crate for cross-platform home directory resolution ([8c07dbf](https://github.com/jasnross/agentspec/commit/8c07dbff883e6e7475c5d776ce4b895f3e268310))
- **manifest:** reject unknown fields in ManifestEntry ([9522413](https://github.com/jasnross/agentspec/commit/952241363f127a6d2f69759fead3fc012319bae7))
- **parse:** load skill supporting files from nested paths ([8abfbd7](https://github.com/jasnross/agentspec/commit/8abfbd7844bb96c429a093001969a1c63bbef310))
- **plan:** correct Cursor file_kinds, add Debug derives, remove dead function ([7235a1b](https://github.com/jasnross/agentspec/commit/7235a1bdc91541766ad4fcaa95b682f6e278782d))
- **sync:** preserve backward compat with old manifests ([a8f384b](https://github.com/jasnross/agentspec/commit/a8f384bd2222a1f8ee4e19731e7208395635ee39))
- **sync:** require --provider when --dest is given ([3edfd25](https://github.com/jasnross/agentspec/commit/3edfd25473bed6d9afc5621f7bbdc77ba10e42aa))
- **test:** update fixtures for typed frontmatter structs ([bfe2cf6](https://github.com/jasnross/agentspec/commit/bfe2cf654c42000cab2d71dada335f90bcc364ce))
- **test:** update integration test assertions for new pipeline ([2645855](https://github.com/jasnross/agentspec/commit/264585565eb57972629f4bf738d026c2eccec4a3))

### Refactoring

- **adapters:** rename post_write_hook parameter and update emit doc comment ([e3c2211](https://github.com/jasnross/agentspec/commit/e3c2211fc6406a894c557dbe497287280196b411))
- **compile:** move prefix/strip transforms from emit into adapters ([3f2ef54](https://github.com/jasnross/agentspec/commit/3f2ef54d2495355f372b6b13cb80fa3ec054cab2))
- **config:** replace SyncIntent with validated_sync_target returning Result ([7457052](https://github.com/jasnross/agentspec/commit/7457052c2bc0f98e69fd5edae9c9a97e4d0a4ca1))
- **core:** rename presets and simplify compile pipeline ([895bc4e](https://github.com/jasnross/agentspec/commit/895bc4eeaabecb4ff942e486c27fdbf2901e65b4))
- create lib.rs and split library from binary modules ([09970f4](https://github.com/jasnross/agentspec/commit/09970f49b1669b96078cd19d3acb4a1159c4aeb9))
- decouple compile stage from templating, add compile::run ([4255c3e](https://github.com/jasnross/agentspec/commit/4255c3e1a2154fda79d7cbe36c5ae7aeb01ad779))
- **emit,sync:** consolidate sync helper modules into their consumers ([ec458ce](https://github.com/jasnross/agentspec/commit/ec458ce84c8e274181ed5bca7f456b11d6e21556))
- **emit:** delegate post-write actions to adapter-provided hooks ([5f2fdd2](https://github.com/jasnross/agentspec/commit/5f2fdd2085eb22b4784fe1992a784b1ea78cbfbb))
- **emit:** replace file-based emit with plan-based write ([d891a78](https://github.com/jasnross/agentspec/commit/d891a7890c3f2631ad13bde015ac8fa22ebb6f6b))
- **emit:** store GeneratedFile paths relative to provider root ([66b91c5](https://github.com/jasnross/agentspec/commit/66b91c5e8f1632319619f8d9d07247aa1f65511e))
- extract Provider to its own module ([4a699f4](https://github.com/jasnross/agentspec/commit/4a699f4a3546b95f06dfa52babc616f9fc222ba3))
- extract templating module, introduce ResolvedSpecs ([2764359](https://github.com/jasnross/agentspec/commit/27643593e286b520d2cfd1487d271bc691fe23ad))
- introduce typestate spec pipeline in specs.rs ([9cac907](https://github.com/jasnross/agentspec/commit/9cac9079d00c53f77dd4b2b4e87ed64f690ca544))
- overhaul spec model, remove Codex and profiles ([2c85703](https://github.com/jasnross/agentspec/commit/2c85703515095608865c67dc77f98cc761b2d8a0))
- remove unnecessary Result wrappers from infallible functions ([a494a21](https://github.com/jasnross/agentspec/commit/a494a219d0a5eddb22fd2e5132eee47175bfbb90))
- **spec:** use struct-level rename_all on ToolFrontmatter ([d87ea0e](https://github.com/jasnross/agentspec/commit/d87ea0ef61b51944758c78ada424d448d842262e))
- **sync:** remove SyncStrategy and symlink upgrade path ([c309485](https://github.com/jasnross/agentspec/commit/c309485427724147de0db732eac1740185a363f1))
- **sync:** unify sync into plan/emit pipeline, drop symlink strategy ([17ca1f2](https://github.com/jasnross/agentspec/commit/17ca1f22530ee1b14036fb945127f9a168e2e1d5))
- tighten visibility and remove stale comments ([83f490b](https://github.com/jasnross/agentspec/commit/83f490b044dca85582f40207db68f76f5361ea24))

### Documentation

- add CLAUDE project workflow and architecture guide ([9c949af](https://github.com/jasnross/agentspec/commit/9c949af3cb9031a0262ec6667d909fa88f9a2f30))
- add design principles section to CLAUDE.md ([0208e3f](https://github.com/jasnross/agentspec/commit/0208e3f6a32a82cd2629c8cac5a1f980404fe602))
- **ci:** require clippy all-targets in local and CI workflows ([3ed0280](https://github.com/jasnross/agentspec/commit/3ed028037858de9ad8eb9857b38a237a3a793ea6))
- **cli:** rename --mapping-profile flag to --profile ([571f804](https://github.com/jasnross/agentspec/commit/571f8042a6e2fb1ebc9d530a4e51491be7a13bae))
- prefer adapters.rs over adapters/mod.rs; apply convention now ([15e3a9e](https://github.com/jasnross/agentspec/commit/15e3a9e439e869e80484ae20972ab9286c93ea7a))
- **readme:** document profiles and profile-overrides configuration ([ec3ceea](https://github.com/jasnross/agentspec/commit/ec3ceea87346bebe95bdf4546f634ca8e2080dbb))
- remove schema sync requirement and update pipeline/matrix for rules ([e358fe6](https://github.com/jasnross/agentspec/commit/e358fe6107f6665fea54f703497b31f8ccc728b4))
- update CLAUDE.md and README.md for accuracy ([7bd95d7](https://github.com/jasnross/agentspec/commit/7bd95d79f3ab4d9be063090e0f5a395552922a71))
- update CLAUDE.md and README.md for plan/emit pipeline ([c832864](https://github.com/jasnross/agentspec/commit/c832864f10eaa7e24483a215922ed549bfc29b0d))
- update pipeline docs and TODO for post-write hook refactor ([24385dd](https://github.com/jasnross/agentspec/commit/24385dd3200420d8e636c4b753c12c4ba08abb8f))
- update TODO to reflect rule prefix work and revised post-write approach ([4ecfaf5](https://github.com/jasnross/agentspec/commit/4ecfaf5aecbf45e5b5f2f521d57e253a12b006bc))

### Tests

- **clippy:** remove unwraps and make all-target lint pass ([5325fea](https://github.com/jasnross/agentspec/commit/5325fea3a8cb4881a3bc5a152e1660fc1daa93f4))
- **integration:** use self-contained fixture for dotfiles spec tests ([08fdf12](https://github.com/jasnross/agentspec/commit/08fdf1286dc92a2fae7bfd9f8e57634602c29125))
- **sync:** add unit tests for files_for_kind and sync_plan structure ([46fafad](https://github.com/jasnross/agentspec/commit/46fafad4a799574a969fc3d9c0038d7bfe3712d3))

### Miscellaneous Chores

- add mise.toml ([4e86293](https://github.com/jasnross/agentspec/commit/4e8629383aa6bff92a37e852ade6c9a708e91d7d))
- add tests to just check target ([984d0f8](https://github.com/jasnross/agentspec/commit/984d0f817ce5ac15939555854081e13f413a9e0e))
- **ci:** pin workflow actions to full commit SHAs ([c2a645e](https://github.com/jasnross/agentspec/commit/c2a645efd1338410855115284bd63a929a51f53a))
- **clippy:** adopt new lint denials and fix violations ([d550eda](https://github.com/jasnross/agentspec/commit/d550eda197835951aea5f047259e209f440102d2))
- **clippy:** centralize test expect policy and fix strict lints ([f7384d9](https://github.com/jasnross/agentspec/commit/f7384d97696f2ed37e772bab5c10fea052b5b290))
- **clippy:** enforce expect_used with test-only allowances ([5b3c76a](https://github.com/jasnross/agentspec/commit/5b3c76adacdcfb7a67ffd73197e6b5ba8cb45762))
- format ([18e1113](https://github.com/jasnross/agentspec/commit/18e1113f20bd8a424e831d9646ad316294ca39b1))
- format and remove unnecessary files ([625bffa](https://github.com/jasnross/agentspec/commit/625bffac4f57767452847da0e39a6167975d0633))
- install just ([afff409](https://github.com/jasnross/agentspec/commit/afff409b9aed852f469b867651b1b8a7fa607286))
- misc cleanup in config.rs and spec.rs ([768bf0e](https://github.com/jasnross/agentspec/commit/768bf0eeeed1504b50ea464f88830cf93b5d156f))
- note remaining cleanup items in TODO ([fbd0e8f](https://github.com/jasnross/agentspec/commit/fbd0e8f39e757969e7b97dd406ae15c96a802224))
- **release:** add MIT license and reset manifest for v0.1.0 first release ([55920c0](https://github.com/jasnross/agentspec/commit/55920c0a38f921952d947f431e9750abdb9dca01))
- remove cargo-sort-derives dev dependency ([d2f9271](https://github.com/jasnross/agentspec/commit/d2f92711de5aef9ec31357129f44b0dd22ce812b))
- remove Codex remnants and clean up adapters ([3012acb](https://github.com/jasnross/agentspec/commit/3012acbd90b410706fa4c70ccc5080505c8fa4eb))
- remove subagent support for now ([2ec8690](https://github.com/jasnross/agentspec/commit/2ec86905518ebcdd2a65ca1b62a9a70085ac6a37))
- Update TODO.md ([86674e0](https://github.com/jasnross/agentspec/commit/86674e008d80384a9317e1b2abc7454672426a5d))
- update TODOs ([e0b3a10](https://github.com/jasnross/agentspec/commit/e0b3a100d1a4254b0016dfa6635abc529479a2e2))

### Styles

- **adapters:** use struct-level serde attributes to reduce repetition ([67c9e56](https://github.com/jasnross/agentspec/commit/67c9e56ff18c85a7063a6711315a070d548c4be8))
- apply rustfmt and update formatting guidance ([1d2380a](https://github.com/jasnross/agentspec/commit/1d2380a6897d61b116d2b130285f54a6f967069d))

## Changelog

All notable changes to this project are documented in this file.

The format is based on Keep a Changelog, and this project adheres to
Semantic Versioning.

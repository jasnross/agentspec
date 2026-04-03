# Changelog

## 0.1.0 (2026-04-03)


### Features

* **adapters:** add frontmatter name prefixing for Cursor agents and skills ([bb97d59](https://github.com/jasnross/agentspec/commit/bb97d598e57d510351f5def83f6fa3be4bfbdac7))
* **adapters:** apply file prefix to rule output paths ([395ee46](https://github.com/jasnross/agentspec/commit/395ee46e6e65a666663bfc4a3965cb5d096270b5))
* add SpecKind::Rule and compile rules to all four providers ([39f10bc](https://github.com/jasnross/agentspec/commit/39f10bc55e38949936a9501b091f3ca253be0ca6))
* add sync command ([e860de7](https://github.com/jasnross/agentspec/commit/e860de781f2b34faaabcd52307f86875b7931140))
* **compile:** embed provider mappings and externalize model profiles ([ec73dc2](https://github.com/jasnross/agentspec/commit/ec73dc291ffd20cfdbe816bb53bd05c59ffaa1ef))
* **config:** reject unknown fields in SpecConfig and OutputConfig ([b90b3eb](https://github.com/jasnross/agentspec/commit/b90b3eb1635ac5b9f588d5ddd9d1035c323c1e84))
* **plan:** introduce WritePlan and move dest resolution to library ([9411b76](https://github.com/jasnross/agentspec/commit/9411b768949e727d3d9f0e31a7bdf24b345eb70b))
* **release:** automate binary releases and distribution docs ([c395045](https://github.com/jasnross/agentspec/commit/c395045de17d70b080aa7f5b0a4c597e24b23433))
* **release:** automate homebrew tap update PRs ([6ec93c0](https://github.com/jasnross/agentspec/commit/6ec93c09065f5f858175634b3f3c225376e9884f))
* **spec:** deny unknown fields in frontmatter structs ([a196d17](https://github.com/jasnross/agentspec/commit/a196d1756d1869d0c4feea61bbb7df0af498a552))
* **sync:** add namespace prefixing and collision-safe overwrite controls ([5af8293](https://github.com/jasnross/agentspec/commit/5af8293191a418ea1ed53f0c05ac31d4beaf45b6))
* **sync:** require explicit sync intent for provider selection ([aa50c29](https://github.com/jasnross/agentspec/commit/aa50c291848e413c8e2189335ff5302b2052900e))


### Bug Fixes

* **adapters:** remove redundant newline before closing --- in frontmatter ([c74205b](https://github.com/jasnross/agentspec/commit/c74205ba48288c771f361610cbf7c60f29da7164))
* **adapters:** sort Claude tools by serialized name, remove unnecessary Result wrappers ([1a92dff](https://github.com/jasnross/agentspec/commit/1a92dff882cb0ed959d5b5eafb36c6409b4c04f5))
* address correctness and type issues ([a17b3b2](https://github.com/jasnross/agentspec/commit/a17b3b278e23dcfd1cd9bef49272d8db82f30820))
* **ci:** pin release workflows to full action SHAs ([9c5f3ef](https://github.com/jasnross/agentspec/commit/9c5f3ef77a217092bbd6443df1c231de141ba216))
* **emit:** make strip_name idempotent, scope to skills, remove ad-hoc logging ([45149fb](https://github.com/jasnross/agentspec/commit/45149fb961a5c8d6aaacbb6db90d4f20fcdc3f58))
* **emit:** remove unnecessary Result wrapper from check_generated_state ([2ef9a89](https://github.com/jasnross/agentspec/commit/2ef9a890898106557982ff12ba343271c60c8d96))
* **main:** use home crate for cross-platform home directory resolution ([8c07dbf](https://github.com/jasnross/agentspec/commit/8c07dbff883e6e7475c5d776ce4b895f3e268310))
* **manifest:** reject unknown fields in ManifestEntry ([9522413](https://github.com/jasnross/agentspec/commit/952241363f127a6d2f69759fead3fc012319bae7))
* **parse:** load skill supporting files from nested paths ([8abfbd7](https://github.com/jasnross/agentspec/commit/8abfbd7844bb96c429a093001969a1c63bbef310))
* **plan:** correct Cursor file_kinds, add Debug derives, remove dead function ([7235a1b](https://github.com/jasnross/agentspec/commit/7235a1bdc91541766ad4fcaa95b682f6e278782d))
* **sync:** preserve backward compat with old manifests ([a8f384b](https://github.com/jasnross/agentspec/commit/a8f384bd2222a1f8ee4e19731e7208395635ee39))
* **sync:** require --provider when --dest is given ([3edfd25](https://github.com/jasnross/agentspec/commit/3edfd25473bed6d9afc5621f7bbdc77ba10e42aa))
* **test:** update fixtures for typed frontmatter structs ([bfe2cf6](https://github.com/jasnross/agentspec/commit/bfe2cf654c42000cab2d71dada335f90bcc364ce))
* **test:** update integration test assertions for new pipeline ([2645855](https://github.com/jasnross/agentspec/commit/264585565eb57972629f4bf738d026c2eccec4a3))


### Refactoring

* **adapters:** rename post_write_hook parameter and update emit doc comment ([e3c2211](https://github.com/jasnross/agentspec/commit/e3c2211fc6406a894c557dbe497287280196b411))
* **compile:** move prefix/strip transforms from emit into adapters ([3f2ef54](https://github.com/jasnross/agentspec/commit/3f2ef54d2495355f372b6b13cb80fa3ec054cab2))
* **config:** replace SyncIntent with validated_sync_target returning Result ([7457052](https://github.com/jasnross/agentspec/commit/7457052c2bc0f98e69fd5edae9c9a97e4d0a4ca1))
* **core:** rename presets and simplify compile pipeline ([895bc4e](https://github.com/jasnross/agentspec/commit/895bc4eeaabecb4ff942e486c27fdbf2901e65b4))
* create lib.rs and split library from binary modules ([09970f4](https://github.com/jasnross/agentspec/commit/09970f49b1669b96078cd19d3acb4a1159c4aeb9))
* decouple compile stage from templating, add compile::run ([4255c3e](https://github.com/jasnross/agentspec/commit/4255c3e1a2154fda79d7cbe36c5ae7aeb01ad779))
* **emit,sync:** consolidate sync helper modules into their consumers ([ec458ce](https://github.com/jasnross/agentspec/commit/ec458ce84c8e274181ed5bca7f456b11d6e21556))
* **emit:** delegate post-write actions to adapter-provided hooks ([5f2fdd2](https://github.com/jasnross/agentspec/commit/5f2fdd2085eb22b4784fe1992a784b1ea78cbfbb))
* **emit:** replace file-based emit with plan-based write ([d891a78](https://github.com/jasnross/agentspec/commit/d891a7890c3f2631ad13bde015ac8fa22ebb6f6b))
* **emit:** store GeneratedFile paths relative to provider root ([66b91c5](https://github.com/jasnross/agentspec/commit/66b91c5e8f1632319619f8d9d07247aa1f65511e))
* extract Provider to its own module ([4a699f4](https://github.com/jasnross/agentspec/commit/4a699f4a3546b95f06dfa52babc616f9fc222ba3))
* extract templating module, introduce ResolvedSpecs ([2764359](https://github.com/jasnross/agentspec/commit/27643593e286b520d2cfd1487d271bc691fe23ad))
* introduce typestate spec pipeline in specs.rs ([9cac907](https://github.com/jasnross/agentspec/commit/9cac9079d00c53f77dd4b2b4e87ed64f690ca544))
* overhaul spec model, remove Codex and profiles ([2c85703](https://github.com/jasnross/agentspec/commit/2c85703515095608865c67dc77f98cc761b2d8a0))
* remove unnecessary Result wrappers from infallible functions ([a494a21](https://github.com/jasnross/agentspec/commit/a494a219d0a5eddb22fd2e5132eee47175bfbb90))
* **spec:** use struct-level rename_all on ToolFrontmatter ([d87ea0e](https://github.com/jasnross/agentspec/commit/d87ea0ef61b51944758c78ada424d448d842262e))
* **sync:** remove SyncStrategy and symlink upgrade path ([c309485](https://github.com/jasnross/agentspec/commit/c309485427724147de0db732eac1740185a363f1))
* **sync:** unify sync into plan/emit pipeline, drop symlink strategy ([17ca1f2](https://github.com/jasnross/agentspec/commit/17ca1f22530ee1b14036fb945127f9a168e2e1d5))
* tighten visibility and remove stale comments ([83f490b](https://github.com/jasnross/agentspec/commit/83f490b044dca85582f40207db68f76f5361ea24))


### Documentation

* add CLAUDE project workflow and architecture guide ([9c949af](https://github.com/jasnross/agentspec/commit/9c949af3cb9031a0262ec6667d909fa88f9a2f30))
* add design principles section to CLAUDE.md ([0208e3f](https://github.com/jasnross/agentspec/commit/0208e3f6a32a82cd2629c8cac5a1f980404fe602))
* **ci:** require clippy all-targets in local and CI workflows ([3ed0280](https://github.com/jasnross/agentspec/commit/3ed028037858de9ad8eb9857b38a237a3a793ea6))
* **cli:** rename --mapping-profile flag to --profile ([571f804](https://github.com/jasnross/agentspec/commit/571f8042a6e2fb1ebc9d530a4e51491be7a13bae))
* prefer adapters.rs over adapters/mod.rs; apply convention now ([15e3a9e](https://github.com/jasnross/agentspec/commit/15e3a9e439e869e80484ae20972ab9286c93ea7a))
* **readme:** document profiles and profile-overrides configuration ([ec3ceea](https://github.com/jasnross/agentspec/commit/ec3ceea87346bebe95bdf4546f634ca8e2080dbb))
* remove schema sync requirement and update pipeline/matrix for rules ([e358fe6](https://github.com/jasnross/agentspec/commit/e358fe6107f6665fea54f703497b31f8ccc728b4))
* update CLAUDE.md and README.md for accuracy ([7bd95d7](https://github.com/jasnross/agentspec/commit/7bd95d79f3ab4d9be063090e0f5a395552922a71))
* update CLAUDE.md and README.md for plan/emit pipeline ([c832864](https://github.com/jasnross/agentspec/commit/c832864f10eaa7e24483a215922ed549bfc29b0d))
* update pipeline docs and TODO for post-write hook refactor ([24385dd](https://github.com/jasnross/agentspec/commit/24385dd3200420d8e636c4b753c12c4ba08abb8f))
* update TODO to reflect rule prefix work and revised post-write approach ([4ecfaf5](https://github.com/jasnross/agentspec/commit/4ecfaf5aecbf45e5b5f2f521d57e253a12b006bc))


### Tests

* **clippy:** remove unwraps and make all-target lint pass ([5325fea](https://github.com/jasnross/agentspec/commit/5325fea3a8cb4881a3bc5a152e1660fc1daa93f4))
* **integration:** use self-contained fixture for dotfiles spec tests ([08fdf12](https://github.com/jasnross/agentspec/commit/08fdf1286dc92a2fae7bfd9f8e57634602c29125))
* **sync:** add unit tests for files_for_kind and sync_plan structure ([46fafad](https://github.com/jasnross/agentspec/commit/46fafad4a799574a969fc3d9c0038d7bfe3712d3))


### Miscellaneous Chores

* add mise.toml ([4e86293](https://github.com/jasnross/agentspec/commit/4e8629383aa6bff92a37e852ade6c9a708e91d7d))
* add tests to just check target ([984d0f8](https://github.com/jasnross/agentspec/commit/984d0f817ce5ac15939555854081e13f413a9e0e))
* **ci:** pin workflow actions to full commit SHAs ([c2a645e](https://github.com/jasnross/agentspec/commit/c2a645efd1338410855115284bd63a929a51f53a))
* **clippy:** adopt new lint denials and fix violations ([d550eda](https://github.com/jasnross/agentspec/commit/d550eda197835951aea5f047259e209f440102d2))
* **clippy:** centralize test expect policy and fix strict lints ([f7384d9](https://github.com/jasnross/agentspec/commit/f7384d97696f2ed37e772bab5c10fea052b5b290))
* **clippy:** enforce expect_used with test-only allowances ([5b3c76a](https://github.com/jasnross/agentspec/commit/5b3c76adacdcfb7a67ffd73197e6b5ba8cb45762))
* format ([18e1113](https://github.com/jasnross/agentspec/commit/18e1113f20bd8a424e831d9646ad316294ca39b1))
* format and remove unnecessary files ([625bffa](https://github.com/jasnross/agentspec/commit/625bffac4f57767452847da0e39a6167975d0633))
* install just ([afff409](https://github.com/jasnross/agentspec/commit/afff409b9aed852f469b867651b1b8a7fa607286))
* misc cleanup in config.rs and spec.rs ([768bf0e](https://github.com/jasnross/agentspec/commit/768bf0eeeed1504b50ea464f88830cf93b5d156f))
* note remaining cleanup items in TODO ([fbd0e8f](https://github.com/jasnross/agentspec/commit/fbd0e8f39e757969e7b97dd406ae15c96a802224))
* **release:** add MIT license and reset manifest for v0.1.0 first release ([55920c0](https://github.com/jasnross/agentspec/commit/55920c0a38f921952d947f431e9750abdb9dca01))
* remove cargo-sort-derives dev dependency ([d2f9271](https://github.com/jasnross/agentspec/commit/d2f92711de5aef9ec31357129f44b0dd22ce812b))
* remove Codex remnants and clean up adapters ([3012acb](https://github.com/jasnross/agentspec/commit/3012acbd90b410706fa4c70ccc5080505c8fa4eb))
* remove subagent support for now ([2ec8690](https://github.com/jasnross/agentspec/commit/2ec86905518ebcdd2a65ca1b62a9a70085ac6a37))
* Update TODO.md ([86674e0](https://github.com/jasnross/agentspec/commit/86674e008d80384a9317e1b2abc7454672426a5d))
* update TODOs ([e0b3a10](https://github.com/jasnross/agentspec/commit/e0b3a100d1a4254b0016dfa6635abc529479a2e2))


### Styles

* **adapters:** use struct-level serde attributes to reduce repetition ([67c9e56](https://github.com/jasnross/agentspec/commit/67c9e56ff18c85a7063a6711315a070d548c4be8))
* apply rustfmt and update formatting guidance ([1d2380a](https://github.com/jasnross/agentspec/commit/1d2380a6897d61b116d2b130285f54a6f967069d))
* **config:** reword FIXME comments for clarity ([9f38742](https://github.com/jasnross/agentspec/commit/9f3874291d9ffbe00c09617c0ad804106f055f5b))

## Changelog

All notable changes to this project are documented in this file.

The format is based on Keep a Changelog, and this project adheres to
Semantic Versioning.

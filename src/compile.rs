use sha2::{Digest, Sha256};

use crate::adapters::{adapt_claude, adapt_codex, adapt_cursor, adapt_opencode};
use crate::types::{
    CompileResult, CompileWarning, GeneratedFile, NormalizedSpec, ProfilesMap, Provider, SpecKind,
};

/// Compile normalized specs into provider-specific generated files.
///
/// For each (spec, target) pair:
/// 1. Check if the target provider supports the spec's kind (agent/skill)
/// 2. Dispatch to the appropriate provider adapter
/// 3. Collect files and warnings
/// 4. Sort output files by path for deterministic ordering
/// 5. Compute SHA-256 hash of inputs for the manifest
pub fn compile_specs(
    specs: &[NormalizedSpec],
    profiles: &ProfilesMap,
    targets: &[Provider],
) -> CompileResult {
    let mut files: Vec<GeneratedFile> = Vec::new();
    let mut warnings: Vec<CompileWarning> = Vec::new();

    let mut sorted_targets: Vec<Provider> = targets.to_vec();
    sorted_targets.sort_by_key(|p| p.to_string());

    for spec in specs {
        // Only compile for providers that are both in the spec's targets and the CLI targets
        let spec_targets: Vec<Provider> = spec
            .targets
            .iter()
            .filter(|t| sorted_targets.contains(t))
            .copied()
            .collect();

        for target in spec_targets {
            // Feature-gate: skip if provider doesn't support this spec kind
            if !provider_supports_kind(target, spec.kind) {
                continue;
            }

            let (mut adapter_files, mut adapter_warnings) = match target {
                Provider::Claude => adapt_claude(spec, profiles),
                Provider::Cursor => adapt_cursor(spec, profiles),
                Provider::Codex => adapt_codex(spec, profiles),
                Provider::OpenCode => adapt_opencode(spec, profiles),
            };

            files.append(&mut adapter_files);
            warnings.append(&mut adapter_warnings);
        }
    }

    // Sort output files by path for deterministic ordering
    files.sort_by(|a, b| a.path.cmp(&b.path));

    // Compute SHA-256 hash of inputs
    let source_hash = hash_inputs(specs, profiles, &sorted_targets);

    CompileResult {
        files,
        warnings,
        source_hash,
    }
}

/// Check if a provider supports the given spec kind.
///
/// These are static facts about each provider's capabilities:
/// - Claude and OpenCode support both agents and skills
/// - Codex and Cursor support skills only (no agents)
fn provider_supports_kind(provider: Provider, kind: SpecKind) -> bool {
    match (provider, kind) {
        (Provider::Claude, _) => true,
        (Provider::OpenCode, _) => true,
        (Provider::Codex, SpecKind::Skill) => true,
        (Provider::Codex, SpecKind::Agent) => false,
        (Provider::Cursor, SpecKind::Skill) => true,
        (Provider::Cursor, SpecKind::Agent) => false,
    }
}

/// Compute SHA-256 hash of the compilation inputs for the manifest.
///
/// Matches the TypeScript approach: serialize inputs to JSON then hash.
/// We use `serde_json` with sorted keys for deterministic output.
fn hash_inputs(specs: &[NormalizedSpec], profiles: &ProfilesMap, targets: &[Provider]) -> String {
    // Build a canonical JSON representation of all compilation inputs.
    let payload = serde_json::json!({
        "specs": specs.iter().map(|s| serde_json::json!({
            "id": s.id,
            "kind": match s.kind { SpecKind::Agent => "agent", SpecKind::Skill => "skill" },
            "name": s.name,
            "description": s.description,
            "body": s.body,
            "user_invocable": s.user_invocable,
            "agent_invocable": s.agent_invocable,
            "tools": s.tools,
            "targets": s.targets.iter().map(|t| t.to_string()).collect::<Vec<_>>(),
            "execution": {
                "model_profile": s.execution.model_profile,
                "temperature": s.execution.temperature,
                "mode": s.execution.mode,
                "readonly": s.execution.readonly,
                "background": s.execution.background,
            },
            "supporting_files": s.supporting_files.iter().map(|sf| serde_json::json!({
                "path": sf.relative_path.to_str(),
                "size": sf.content.len(),
                "executable": sf.executable,
            })).collect::<Vec<_>>(),
            "provider_overrides": s.provider_overrides,
            "routing": s.routing.as_ref().map(|r| serde_json::json!({
                "trigger": r.trigger,
                "aliases": r.aliases,
            })),
        })).collect::<Vec<_>>(),
        "profiles": serde_json::to_value(profiles).unwrap_or_default(),
        "targets": targets.iter().map(|t| t.to_string()).collect::<Vec<_>>(),
    });

    let canonical = serde_json::to_string(&payload).expect("JSON serialization");
    let hash = Sha256::digest(canonical.as_bytes());
    format!("{hash:x}")
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::types::Execution;

    fn minimal_spec(id: &str, kind: SpecKind) -> NormalizedSpec {
        NormalizedSpec {
            source_path: format!("/test/{id}.md").into(),
            id: id.to_string(),
            kind,
            name: id.to_string(),
            description: format!("Test {id}"),
            version: 1,
            user_invocable: kind == SpecKind::Skill,
            agent_invocable: true,
            body: format!("# {id}\n\nBody."),
            execution: Execution::default(),
            tools: vec![],
            skill: None,
            supporting_files: vec![],
            targets: Provider::ALL.to_vec(),
            provider_overrides: HashMap::new(),
            routing: None,
        }
    }

    #[test]
    fn test_compile_produces_files_for_all_targets() {
        let specs = vec![minimal_spec("test-skill", SpecKind::Skill)];
        let result = compile_specs(&specs, &ProfilesMap::new(), &Provider::ALL);

        // Should have files for all 4 providers (claude, cursor, codex, opencode)
        let providers: Vec<String> = result
            .files
            .iter()
            .map(|f| f.provider.to_string())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        assert_eq!(providers.len(), 4);
    }

    #[test]
    fn test_compile_respects_target_filter() {
        let specs = vec![minimal_spec("test-skill", SpecKind::Skill)];
        let result = compile_specs(&specs, &ProfilesMap::new(), &[Provider::Claude]);

        for file in &result.files {
            assert_eq!(file.provider, Provider::Claude);
        }
    }

    #[test]
    fn test_compile_feature_gates_agents() {
        let specs = vec![minimal_spec("test-agent", SpecKind::Agent)];
        let result = compile_specs(&specs, &ProfilesMap::new(), &Provider::ALL);

        // Codex and Cursor don't support agents
        assert!(result
            .files
            .iter()
            .all(|f| f.provider != Provider::Cursor && f.provider != Provider::Codex));
        // Claude and OpenCode do support agents
        assert!(result.files.iter().any(|f| f.provider == Provider::Claude));
        assert!(result.files.iter().any(|f| f.provider == Provider::OpenCode));
    }

    #[test]
    fn test_compile_files_sorted_by_path() {
        let specs = vec![
            minimal_spec("zzz-skill", SpecKind::Skill),
            minimal_spec("aaa-skill", SpecKind::Skill),
        ];
        let result = compile_specs(&specs, &ProfilesMap::new(), &Provider::ALL);

        let paths: Vec<String> = result
            .files
            .iter()
            .map(|f| f.path.to_str().unwrap().to_string())
            .collect();
        let mut sorted_paths = paths.clone();
        sorted_paths.sort();
        assert_eq!(paths, sorted_paths);
    }

    #[test]
    fn test_compile_hash_is_deterministic() {
        let specs = vec![minimal_spec("test", SpecKind::Skill)];

        let r1 = compile_specs(&specs, &ProfilesMap::new(), &Provider::ALL);
        let r2 = compile_specs(&specs, &ProfilesMap::new(), &Provider::ALL);
        assert_eq!(r1.source_hash, r2.source_hash);
    }

    #[test]
    fn test_compile_hash_changes_with_different_input() {
        let r1 = compile_specs(
            &[minimal_spec("test-a", SpecKind::Skill)],
            &ProfilesMap::new(),
            &Provider::ALL,
        );
        let r2 = compile_specs(
            &[minimal_spec("test-b", SpecKind::Skill)],
            &ProfilesMap::new(),
            &Provider::ALL,
        );
        assert_ne!(r1.source_hash, r2.source_hash);
    }

    #[test]
    fn test_provider_supports_kind() {
        assert!(provider_supports_kind(Provider::Claude, SpecKind::Agent));
        assert!(provider_supports_kind(Provider::Claude, SpecKind::Skill));
        assert!(provider_supports_kind(Provider::OpenCode, SpecKind::Agent));
        assert!(provider_supports_kind(Provider::OpenCode, SpecKind::Skill));
        assert!(!provider_supports_kind(Provider::Codex, SpecKind::Agent));
        assert!(provider_supports_kind(Provider::Codex, SpecKind::Skill));
        assert!(!provider_supports_kind(Provider::Cursor, SpecKind::Agent));
        assert!(provider_supports_kind(Provider::Cursor, SpecKind::Skill));
    }
}

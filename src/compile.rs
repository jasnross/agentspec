use sha2::{Digest, Sha256};

use crate::adapters::{adapt_claude, adapt_codex, adapt_cursor, adapt_opencode};
use crate::types::{
    CompileResult, CompileWarning, GeneratedFile, MappingBundle, NormalizedSpec, Provider, SpecKind,
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
    mappings: &MappingBundle,
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
            if !provider_supports_kind(target, spec.kind, mappings) {
                continue;
            }

            let (mut adapter_files, mut adapter_warnings) = match target {
                Provider::Claude => adapt_claude(spec, mappings),
                Provider::Cursor => adapt_cursor(spec, mappings),
                Provider::Codex => adapt_codex(spec, mappings),
                Provider::OpenCode => adapt_opencode(spec, mappings),
            };

            files.append(&mut adapter_files);
            warnings.append(&mut adapter_warnings);
        }
    }

    // Sort output files by path for deterministic ordering
    files.sort_by(|a, b| a.path.cmp(&b.path));

    // Compute SHA-256 hash of inputs
    let source_hash = hash_inputs(specs, mappings, &sorted_targets);

    CompileResult {
        files,
        warnings,
        source_hash,
    }
}

/// Check if a provider supports the given spec kind based on features mapping.
///
/// Matches TypeScript behavior: missing feature flag = falsy = unsupported.
/// Only explicitly `true` enables a spec kind for a provider.
fn provider_supports_kind(provider: Provider, kind: SpecKind, mappings: &MappingBundle) -> bool {
    let provider_key = provider.to_string();
    let Some(features) = mappings.features.providers.get(&provider_key) else {
        return false;
    };

    match kind {
        SpecKind::Agent => features.supports_agents.unwrap_or(false),
        SpecKind::Skill => features.supports_skills.unwrap_or(false),
    }
}

/// Compute SHA-256 hash of the compilation inputs for the manifest.
///
/// Matches the TypeScript approach: serialize inputs to JSON then hash.
/// We use `serde_json` with sorted keys for deterministic output.
fn hash_inputs(specs: &[NormalizedSpec], mappings: &MappingBundle, targets: &[Provider]) -> String {
    // Build a canonical JSON representation of all compilation inputs.
    // Matches TypeScript: JSON.stringify({ specs, mappings, targets })
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
        "mappings": {
            "models": serde_json::to_value(&mappings.models).unwrap_or_default(),
            "tools": serde_json::to_value(&mappings.tools).unwrap_or_default(),
            "features": serde_json::to_value(&mappings.features).unwrap_or_default(),
        },
        "targets": targets.iter().map(|t| t.to_string()).collect::<Vec<_>>(),
    });

    let canonical = serde_json::to_string(&payload).expect("JSON serialization");
    let hash = Sha256::digest(canonical.as_bytes());
    format!("{hash:x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Execution, FeaturesMapping, ProviderFeatures, ToolsMapping};
    use std::collections::HashMap;

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

    fn full_mappings() -> MappingBundle {
        let mut providers = HashMap::new();
        for p in Provider::ALL {
            providers.insert(
                p.to_string(),
                ProviderFeatures {
                    supports_agents: Some(true),
                    supports_skills: Some(true),
                    ..Default::default()
                },
            );
        }
        MappingBundle {
            features: FeaturesMapping { providers },
            tools: ToolsMapping {
                tools: HashMap::new(),
            },
            ..Default::default()
        }
    }

    #[test]
    fn test_compile_produces_files_for_all_targets() {
        let specs = vec![minimal_spec("test-skill", SpecKind::Skill)];
        let result = compile_specs(&specs, &full_mappings(), &Provider::ALL);

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
        let result = compile_specs(&specs, &full_mappings(), &[Provider::Claude]);

        for file in &result.files {
            assert_eq!(file.provider, Provider::Claude);
        }
    }

    #[test]
    fn test_compile_feature_gates_agents() {
        let specs = vec![minimal_spec("test-agent", SpecKind::Agent)];
        let mut mappings = full_mappings();
        // Cursor doesn't support agents
        mappings
            .features
            .providers
            .get_mut("cursor")
            .unwrap()
            .supports_agents = Some(false);

        let result = compile_specs(&specs, &mappings, &Provider::ALL);
        assert!(result.files.iter().all(|f| f.provider != Provider::Cursor));
    }

    #[test]
    fn test_compile_files_sorted_by_path() {
        let specs = vec![
            minimal_spec("zzz-skill", SpecKind::Skill),
            minimal_spec("aaa-skill", SpecKind::Skill),
        ];
        let result = compile_specs(&specs, &full_mappings(), &Provider::ALL);

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
        let mappings = full_mappings();

        let r1 = compile_specs(&specs, &mappings, &Provider::ALL);
        let r2 = compile_specs(&specs, &mappings, &Provider::ALL);
        assert_eq!(r1.source_hash, r2.source_hash);
    }

    #[test]
    fn test_compile_hash_changes_with_different_input() {
        let mappings = full_mappings();

        let r1 = compile_specs(
            &[minimal_spec("test-a", SpecKind::Skill)],
            &mappings,
            &Provider::ALL,
        );
        let r2 = compile_specs(
            &[minimal_spec("test-b", SpecKind::Skill)],
            &mappings,
            &Provider::ALL,
        );
        assert_ne!(r1.source_hash, r2.source_hash);
    }
}

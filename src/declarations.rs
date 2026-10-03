use crate::presets::ProviderPresetsMap;

/// The `agentspec.toml` sections specs refer to by name, carried across the
/// binary–library boundary as one struct.
///
/// The binary builds it with `AgentspecConfig::declarations()`;
/// `Specs::validate` checks specs against it, `ValidatedSpecs` carries it, and
/// `compile_specs` reads it, so the declarations a spec was validated against
/// are the ones it compiles with. Each adapter checks its own provider's blocks
/// through `Adapter::validate_declarations`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Declarations {
    /// `[presets.<name>]`: preset name → per-provider model config.
    pub presets: ProviderPresetsMap,
}

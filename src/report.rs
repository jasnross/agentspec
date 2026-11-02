//! Rendering for the compile stage's diagnostics.
//!
//! Pure functions returning `Vec<String>`, following `format_ignored_listing`'s
//! precedent in `main.rs`: collection happens in the library, presentation
//! happens here, and the caller does the printing. That split is what makes
//! the report unit-testable without spawning the binary.

use std::collections::BTreeSet;

use agentspec::adapters::Presentation;
use agentspec::compile::{CompileDiagnostics, ConfigDrop, Loss};

/// Render the compile-stage diagnostics as stderr-ready lines.
///
/// Three sections making claims of different kinds. A **loss** is agentspec's
/// own warranty about its own output — a value the author configured that
/// reached no generated file — and is certain. A **provider limitation** is a
/// measured claim about how a provider's runtime treats bytes agentspec
/// delivered successfully, and cites `docs/hooks-canonical.md`. A **config
/// drop** is a granted tool the author's own `tool-names` entry withheld by
/// setting it to `false`, and names that entry.
///
/// No section renders a success state, because agentspec cannot warrant
/// that a delivered value is honored — `README.md` records the standing
/// example, an `OpenCode` `variant` discarded whenever the session resolves a
/// different model than the agent declares.
///
/// `verbose` lists the specs behind a counted line. It changes nothing else:
/// per-spec loss lines and provider limitations render identically either way.
///
/// Returns an empty `Vec` when there is nothing in any of the three sections,
/// so the caller prints nothing rather than a bare heading.
pub fn format_compile_report(diagnostics: &CompileDiagnostics, verbose: bool) -> Vec<String> {
    let losses = diagnostics.losses();
    let limitations = format_limitations(diagnostics);
    let drops = diagnostics.config_drops();
    if losses.is_empty() && limitations.is_empty() && drops.is_empty() {
        return Vec::new();
    }

    let limitation_count = limitations.len();
    let mut lines = Vec::new();
    if !losses.is_empty() {
        lines.push(String::from("not delivered:"));
        lines.extend(format_losses(losses, verbose));
    }
    if limitation_count > 0 {
        lines.push(String::from("provider limitations:"));
        lines.extend(limitations);
    }
    if !drops.is_empty() {
        lines.push(String::from("dropped by config:"));
        lines.extend(format_config_drops(drops, verbose));
    }
    // A library with no `false` entries keeps the two-count footer.
    let drop_count = if drops.is_empty() {
        String::new()
    } else {
        format!(
            ", {} config {}",
            drops.len(),
            if drops.len() == 1 { "drop" } else { "drops" }
        )
    };
    lines.push(format!(
        "{} {}, {limitation_count} provider {}{drop_count}",
        losses.len(),
        if losses.len() == 1 { "loss" } else { "losses" },
        if limitation_count == 1 {
            "limitation"
        } else {
            "limitations"
        },
    ));
    lines
}

/// Group losses by provider, then by `(setting, kind)`.
///
/// Grouping on `(setting, kind)` rather than on `setting` alone is what lets
/// each line carry a correct explanation: Cursor's `tools` losses split into an
/// agents line and a skills line, and the two sentences differ.
///
/// A `Body` loss has no file kind, and its explanation reads from spec type
/// instead, so `Body` groups split by spec type on the same reasoning — and on
/// the same population its `is_categorical` was counted against.
///
/// The input arrives ordered by `(provider, setting, kind, spec_type,
/// spec_id)` from the `BTreeSet` the subtraction collects into, so consecutive
/// entries sharing a group key are already adjacent.
fn format_losses(losses: &[Loss], verbose: bool) -> Vec<String> {
    let mut lines = Vec::new();
    let groups = losses.chunk_by(|a, b| {
        a.provider() == b.provider()
            && a.setting() == b.setting()
            && a.kind() == b.kind()
            && (a.kind().is_some() || a.spec_type() == b.spec_type())
    });
    for group in groups {
        // `chunk_by` never yields an empty chunk, so this `continue` is
        // unreachable — it is the `unwrap`-free shape for taking the head
        // given `unwrap_used` and `panic` are denied.
        let Some(head) = group.first() else { continue };
        let provider = head.provider();

        // Which shape applies is a property of the loss set rather than
        // anything an adapter declares, which is why the mapping lives here.
        let presentation = if head.is_categorical() {
            Presentation::CountedSubjects {
                singular: "spec",
                plural: "specs",
            }
        } else {
            Presentation::Warning
        };

        match presentation {
            Presentation::CountedSubjects { singular, plural } => {
                let n = group.len();
                let word = if n == 1 { singular } else { plural };
                lines.push(format!(
                    "  {provider}: {n} {word} lost `{}` — {}",
                    head.setting().label(),
                    explain_group(head),
                ));
                if verbose {
                    for loss in group {
                        lines.push(format!("    {}/{}", loss.spec_type(), loss.spec_id()));
                    }
                } else {
                    lines.push(String::from("    (--verbose lists them)"));
                }
            }
            Presentation::Warning => {
                for loss in group {
                    lines.push(format!(
                        "  {provider}: {}/{} lost `{}` — {}",
                        loss.spec_type(),
                        loss.spec_id(),
                        loss.setting().label(),
                        explain(loss),
                    ));
                }
            }
        }
    }
    lines
}

/// Compose the explanation from the provider, the setting, and the file kind
/// the loss names.
///
/// Derived rather than adapter-supplied: an explanation each adapter wrote per
/// setting it cannot carry would be the adapter self-narration this design
/// removes, and every such string goes stale silently the moment a provider
/// gains the capability.
///
/// A `Body` loss has no emitted file to name — that is what it means — so it
/// reads from spec type instead.
///
/// Speaks for one spec only. A per-spec line means some sibling spec holding
/// the same intent *was* delivered, so this must not generalize to the file
/// kind the way [`explain_group`] does.
fn explain(loss: &Loss) -> String {
    match loss.kind() {
        Some(kind) => format!(
            "this {} {kind} file carries no `{}`",
            loss.provider(),
            loss.setting().label()
        ),
        // Speaks only for this spec. A per-spec `Body` group means other
        // specs of the same type *were* emitted, so the sentence cannot
        // generalize the way `explain_group`'s does.
        None => format!(
            "{} emitted no file for {}/{}",
            loss.provider(),
            loss.spec_type(),
            loss.spec_id()
        ),
    }
}

/// [`explain`] for a counted line, which speaks for a whole group and so names
/// no single spec. Every group member shares the file kind or the spec type
/// the sentence is built from, so one sentence is true of all of them.
fn explain_group(head: &Loss) -> String {
    match head.kind() {
        Some(kind) => format!(
            "no {} {kind} file carries `{}`",
            head.provider(),
            head.setting().label()
        ),
        None => format!("{} emits no {}", head.provider(), head.spec_type()),
    }
}

/// Group config drops by `(provider, server, tool)`, one counted line each.
///
/// A `false` entry applies to every spec granting the tool on that provider,
/// so every group is categorical: the line counts the distinct specs, and
/// `verbose` lists them. The input arrives ordered by `(provider, server, tool, kind,
/// spec_type, spec_id)`, so a group's members are adjacent.
fn format_config_drops(drops: &[ConfigDrop], verbose: bool) -> Vec<String> {
    let mut lines = Vec::new();
    let groups = drops.chunk_by(|a, b| {
        a.provider() == b.provider() && a.server() == b.server() && a.tool() == b.tool()
    });
    for group in groups {
        // Unreachable for the reason `format_losses` gives.
        let Some(head) = group.first() else { continue };
        let provider = head.provider();
        // A spec emitting two kinds that both carry grants drops the tool
        // from each, so the count is of distinct specs, not of drops.
        let specs: BTreeSet<(&str, &str)> = group
            .iter()
            .map(|drop| (drop.spec_type(), drop.spec_id()))
            .collect();
        let n = specs.len();
        let (word, verb) = if n == 1 {
            ("spec", "drops")
        } else {
            ("specs", "drop")
        };
        lines.push(format!(
            "  {provider}: {n} {word} {verb} `mcp.{server}.{tool}` — [mcp.{server}.{provider}] \
             `tool-names` sets it to `false`",
            server = head.server(),
            tool = head.tool(),
        ));
        if verbose {
            for (spec_type, spec_id) in specs {
                lines.push(format!("    {spec_type}/{spec_id}"));
            }
        } else {
            lines.push(String::from("    (--verbose lists them)"));
        }
    }
    lines
}

/// Provider limitations, one line each: the adapter-pushed degradations
/// followed by the cross-provider parity warnings.
fn format_limitations(diagnostics: &CompileDiagnostics) -> Vec<String> {
    let mut lines = Vec::new();
    let groups = diagnostics
        .degradations()
        .chunk_by(|a, b| a.provider() == b.provider() && a.kind() == b.kind());
    for group in groups {
        let Some(head) = group.first() else { continue };
        match head.kind().presentation() {
            Presentation::Warning => lines.push(format!("  {}", head.message())),
            // Unreachable: no `DegradationKind` selects `CountedSubjects`.
            // The loss renderer is `CountedSubjects`' only writer. Rendering
            // the head keeps the match total without pretending to a shape
            // this section has no subjects for —
            // `Degradation` carries at most one subject and `message()` does
            // not include it, so a per-member loop would emit duplicates.
            Presentation::CountedSubjects { .. } => {
                debug_assert!(
                    false,
                    "no DegradationKind renders as CountedSubjects; the loss renderer is its only writer"
                );
                lines.push(format!("  {}", head.message()));
            }
        }
    }
    for warning in diagnostics.parity() {
        lines.push(format!("  {}", warning.message()));
    }
    lines
}

#[cfg(test)]
mod tests {
    use agentspec::compile::CompileDiagnostics;
    use agentspec::plan::FileKind;
    use agentspec::provider::Provider;
    use agentspec::setting::SettingKey;

    use super::{format_compile_report, format_config_drops, format_losses};

    /// `Loss` has no public constructor by design, so the renderer's own tests
    /// build the shape they render through the library's test-only builder.
    fn loss(
        spec_id: &str,
        setting: SettingKey,
        kind: Option<FileKind>,
        categorical: bool,
    ) -> agentspec::compile::Loss {
        agentspec::compile::Loss::for_test(
            Provider::OpenCode,
            setting,
            kind,
            "skill",
            spec_id,
            categorical,
        )
    }

    #[test]
    fn test_empty_diagnostics_render_no_lines() {
        let diagnostics = CompileDiagnostics::default();
        assert!(format_compile_report(&diagnostics, false).is_empty());
        assert!(format_compile_report(&diagnostics, true).is_empty());
    }

    #[test]
    fn test_categorical_group_withholds_subjects_without_verbose() {
        let losses = [
            loss("a", SettingKey::Model, Some(FileKind::Skills), true),
            loss("b", SettingKey::Model, Some(FileKind::Skills), true),
        ];

        let quiet = format_losses(&losses, false);
        assert_eq!(quiet.len(), 2, "{quiet:#?}");
        assert!(quiet[0].contains("2 specs lost `model`"), "{quiet:#?}");
        assert!(quiet[0].contains("no opencode skills file carries `model`"));
        assert!(quiet[1].contains("(--verbose lists them)"));

        let loud = format_losses(&losses, true);
        assert_eq!(loud.len(), 3, "{loud:#?}");
        assert!(loud[1].contains("skill/a"), "{loud:#?}");
        assert!(loud[2].contains("skill/b"), "{loud:#?}");
    }

    #[test]
    fn test_per_spec_group_lists_every_spec_at_both_verbosities() {
        let losses = [
            loss("a", SettingKey::Model, Some(FileKind::Skills), false),
            loss("b", SettingKey::Model, Some(FileKind::Skills), false),
        ];
        for verbose in [false, true] {
            let lines = format_losses(&losses, verbose);
            assert_eq!(lines.len(), 2, "verbose={verbose}: {lines:#?}");
            assert!(lines[0].contains("skill/a"), "{lines:#?}");
            assert!(lines[1].contains("skill/b"), "{lines:#?}");
            assert!(
                lines.iter().all(|l| !l.contains("--verbose")),
                "a per-spec group already names every spec: {lines:#?}"
            );
        }
    }

    #[test]
    fn test_counted_body_group_names_no_single_spec() {
        // A counted line speaks for the whole group, so the `Body`
        // explanation drops the spec id it carries in the per-spec form.
        let losses = [
            loss("a", SettingKey::Body, None, true),
            loss("b", SettingKey::Body, None, true),
        ];
        let lines = format_losses(&losses, false);
        assert!(lines[0].contains("2 specs lost `content`"), "{lines:#?}");
        assert!(lines[0].ends_with("opencode emits no skill"), "{lines:#?}");
        assert!(!lines[0].contains("skill/a"), "{lines:#?}");
    }

    #[test]
    fn test_setting_and_kind_split_groups_so_explanations_stay_true() {
        // Grouping on `setting` alone would put these in one group under one
        // sentence, and one of the two would be wrong.
        let losses = [
            loss("a", SettingKey::Tools, Some(FileKind::Agents), true),
            loss("b", SettingKey::Tools, Some(FileKind::Skills), true),
        ];
        let lines = format_losses(&losses, false);
        assert!(
            lines.iter().any(|l| l.contains("no opencode agents file")),
            "{lines:#?}"
        );
        assert!(
            lines.iter().any(|l| l.contains("no opencode skills file")),
            "{lines:#?}"
        );
    }

    #[test]
    fn test_config_drops_render_counted_line_per_tool() {
        let drops = [
            agentspec::compile::ConfigDrop::for_test(
                Provider::OpenCode,
                "s",
                "t",
                FileKind::Agents,
                "agent",
                "a",
            ),
            agentspec::compile::ConfigDrop::for_test(
                Provider::OpenCode,
                "s",
                "t",
                FileKind::Agents,
                "agent",
                "b",
            ),
        ];

        let quiet = format_config_drops(&drops, false);
        assert_eq!(quiet.len(), 2, "{quiet:#?}");
        assert!(quiet[0].contains("2 specs drop `mcp.s.t`"), "{quiet:#?}");
        assert!(
            quiet[0].contains("[mcp.s.opencode] `tool-names` sets it to `false`"),
            "{quiet:#?}"
        );
        assert!(quiet[1].contains("(--verbose lists them)"), "{quiet:#?}");

        let loud = format_config_drops(&drops, true);
        assert_eq!(loud.len(), 3, "{loud:#?}");
        assert!(loud[1].contains("agent/a"), "{loud:#?}");
        assert!(loud[2].contains("agent/b"), "{loud:#?}");
    }

    /// One spec dropping a tool from two file kinds is still one spec.
    #[test]
    fn test_config_drops_count_each_spec_once() {
        let drops = [
            agentspec::compile::ConfigDrop::for_test(
                Provider::OpenCode,
                "s",
                "t",
                FileKind::Agents,
                "skill",
                "a",
            ),
            agentspec::compile::ConfigDrop::for_test(
                Provider::OpenCode,
                "s",
                "t",
                FileKind::Commands,
                "skill",
                "a",
            ),
        ];
        let quiet = format_config_drops(&drops, false);
        assert!(quiet[0].contains("1 spec drops `mcp.s.t`"), "{quiet:#?}");
        let loud = format_config_drops(&drops, true);
        assert_eq!(loud.len(), 2, "{loud:#?}");
    }

    #[test]
    fn test_single_config_drop_uses_singular_verb() {
        let drops = [agentspec::compile::ConfigDrop::for_test(
            Provider::Claude,
            "s",
            "t",
            FileKind::Skills,
            "skill",
            "a",
        )];
        let lines = format_config_drops(&drops, false);
        assert!(lines[0].contains("1 spec drops `mcp.s.t`"), "{lines:#?}");
    }

    #[test]
    fn test_config_drops_split_by_provider_server_and_tool() {
        let two_tools = [
            agentspec::compile::ConfigDrop::for_test(
                Provider::Claude,
                "s",
                "t",
                FileKind::Agents,
                "agent",
                "a",
            ),
            agentspec::compile::ConfigDrop::for_test(
                Provider::Claude,
                "s",
                "u",
                FileKind::Agents,
                "agent",
                "a",
            ),
        ];
        let two_providers = [
            agentspec::compile::ConfigDrop::for_test(
                Provider::Claude,
                "s",
                "t",
                FileKind::Agents,
                "agent",
                "a",
            ),
            agentspec::compile::ConfigDrop::for_test(
                Provider::OpenCode,
                "s",
                "t",
                FileKind::Agents,
                "agent",
                "a",
            ),
        ];
        for drops in [two_tools, two_providers] {
            let lines = format_config_drops(&drops, false);
            let counted: Vec<&String> = lines
                .iter()
                .filter(|l| l.contains("1 spec drops"))
                .collect();
            assert_eq!(counted.len(), 2, "{lines:#?}");
        }
    }
}

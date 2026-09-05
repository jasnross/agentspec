//! Rendering for the compile stage's diagnostics.
//!
//! Pure functions returning `Vec<String>`, following `format_ignored_listing`'s
//! precedent in `main.rs`: collection happens in the library, presentation
//! happens here, and the caller does the printing. That split is what makes
//! the report unit-testable without spawning the binary.

use agentspec::adapters::Presentation;
use agentspec::compile::{CompileDiagnostics, Loss};

/// Render the compile-stage diagnostics as stderr-ready lines.
///
/// Two sections making claims of different strength. A **loss** is agentspec's
/// own warranty about its own output — a value the author configured that
/// reached no generated file — and is certain. A **provider limitation** is a
/// measured claim about how a provider's runtime treats bytes agentspec
/// delivered successfully, and cites `docs/hooks-canonical.md`.
///
/// Neither section renders a success state, because agentspec cannot warrant
/// that a delivered value is honored — `README.md` records the standing
/// example, an `OpenCode` `variant` discarded whenever the session resolves a
/// different model than the agent declares.
///
/// `verbose` lists the specs behind a counted line. It changes nothing else:
/// per-spec loss lines and provider limitations render identically either way.
///
/// Returns an empty `Vec` when there is nothing in either section, so the
/// caller prints nothing rather than a bare heading.
pub fn format_compile_report(diagnostics: &CompileDiagnostics, verbose: bool) -> Vec<String> {
    let losses = diagnostics.losses();
    let limitations = format_limitations(diagnostics);
    if losses.is_empty() && limitations.is_empty() {
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
    lines.push(format!(
        "{} {}, {limitation_count} provider {}",
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
            // Unreachable: `PartialOutputImpl` is the only `DegradationKind`
            // and selects `Warning`. The loss renderer is `CountedSubjects`'
            // only writer. Rendering the head keeps the match total without
            // pretending to a shape this section has no subjects for —
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

    use super::{format_compile_report, format_losses};

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
}

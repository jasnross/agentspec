//! Canonical hook payload schema.
//!
//! Provider-neutral runtime contract that user hook scripts read on stdin
//! and emit on stdout. Adapter-generated shims translate between this
//! canonical shape and each provider's native payload shape; the types in
//! this module are the single source of truth for what fields exist in
//! canonical payloads, and they drive the codegen of those shims at
//! agentspec-compile time.
//!
//! These types are not invoked at hook-fire time on the user's machine —
//! the shim ships POSIX shell + jq programs. The Rust translation methods
//! on [`CanonicalInput::from_provider_stdin`] and
//! [`CanonicalOutput::to_provider_stdout`] are the reference implementation
//! that snapshot/parity tests compare the generated jq programs against;
//! any divergence between the two paths is a codegen bug.
//!
//! ## Schema-versioning posture
//!
//! Every canonical payload carries a wire-form `schema_version` field set
//! to [`SCHEMA_VERSION`]. The schema is forward-compatible by construction:
//! new fields land as `Option<T>` defaulting to `None`. Breaking changes —
//! field removals, type changes, or renames — bump the major version and
//! user hook scripts may need updates.

pub mod shim_template;

use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use serde_with::skip_serializing_none;

use crate::spec::HookEvent;

/// Version of the canonical hook payload schema emitted by this build.
pub const SCHEMA_VERSION: &str = "1.0.0";

fn default_schema_version() -> String {
    SCHEMA_VERSION.to_string()
}

/// Wire-form provider identity in canonical payloads.
///
/// Distinct from [`crate::provider::Provider`] (which carries a heavier
/// adapter-dispatch trait object). [`ProviderName`] is a plain string enum
/// suitable for embedding in the canonical payload's `provider` field. Only
/// hook-emitting providers (Claude and Cursor) are representable —
/// `Provider::OpenCode` has no canonical wire form because its adapter
/// doesn't emit hooks.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderName {
    Claude,
    Cursor,
}

impl ProviderName {
    /// Wire-form name as a `'static` string slice (`"claude"` / `"cursor"`).
    ///
    /// Used by shim codegen and anywhere else the literal is embedded into
    /// generated output. Mirrors the `#[serde(rename_all = "lowercase")]`
    /// wire form without paying for `serde_json::to_string`'s quoting.
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Cursor => "cursor",
        }
    }

    /// Translate a [`crate::provider::Provider`] to its canonical wire form.
    ///
    /// Returns `None` for `Provider::OpenCode`, whose adapter does not
    /// emit hooks and therefore has no canonical wire identity. Callers
    /// that have already gated on `provider.adapter().emits_hooks()` can
    /// rely on this returning `Some`.
    ///
    /// [`TryFrom<Provider>`] is the idiomatic alternative; both spellings
    /// are kept because `from_provider` reads more cleanly at call sites
    /// that already pattern-match on `Option`.
    pub fn from_provider(provider: crate::provider::Provider) -> Option<Self> {
        Self::try_from(provider).ok()
    }
}

impl TryFrom<crate::provider::Provider> for ProviderName {
    /// On failure, return the original `Provider` value — `Provider::OpenCode`
    /// is the only failure case today, and surfacing it lets callers branch
    /// on the unsupported provider for diagnostics.
    type Error = crate::provider::Provider;

    fn try_from(provider: crate::provider::Provider) -> Result<Self, Self::Error> {
        match provider {
            crate::provider::Provider::Claude => Ok(Self::Claude),
            crate::provider::Provider::Cursor => Ok(Self::Cursor),
            crate::provider::Provider::OpenCode => Err(provider),
        }
    }
}

/// Canonical permission outcome.
///
/// The adapter routes this to each provider's permission API: Claude's
/// `hookSpecificOutput.permissionDecision`, Cursor's top-level `permission`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionDecision {
    Allow,
    Deny,
    Ask,
}

/// Canonical input payload that hook scripts read on stdin.
///
/// The wire form is a single flat JSON object made up of the universal
/// envelope fields plus the event-tagged body in [`CanonicalInputBody`]
/// (flattened so all keys sit at the top level), plus a verbatim
/// `provider_raw` escape hatch carrying the original provider-shaped stdin.
///
/// ## Why no separate `event` field on this struct
///
/// The wire-form `event` discriminator key is supplied by
/// [`CanonicalInputBody`]'s serde tag. Carrying a duplicate `event` field
/// on the outer struct would produce two `event` keys in the serialized
/// object. The canonical event is exposed via [`Self::event`].
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CanonicalInput {
    pub schema_version: String,
    pub provider: ProviderName,
    pub session_id: String,
    pub agent_id: Option<String>,
    pub cwd: PathBuf,
    pub transcript_path: Option<PathBuf>,
    #[serde(flatten)]
    pub event_specific: CanonicalInputBody,
    pub provider_raw: Value,
}

/// Event-tagged body of [`CanonicalInput`].
///
/// Variants carry only event-specific fields; the universal envelope is on
/// the outer [`CanonicalInput`] struct. The serde tag (`event`) is the
/// canonical wire-form discriminator.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum CanonicalInputBody {
    PreToolUse {
        tool_name: String,
        tool_use_id: String,
        tool_input: Value,
    },
    PostToolUse {
        tool_name: String,
        tool_use_id: String,
        tool_input: Value,
        /// Provider-shaped in v1: Claude emits an object-or-string, Cursor
        /// emits a JSON-encoded string. v2 may normalize to always-object.
        tool_response: Value,
    },
    PostToolUseFailure {
        tool_name: String,
        tool_use_id: String,
        tool_input: Value,
        tool_response: Value,
    },
    SessionStart {},
    SessionEnd {},
    Stop {},
    PreCompact {},
    SubagentStart {},
    SubagentStop {},
    UserPromptSubmit {
        prompt: String,
    },
}

impl CanonicalInputBody {
    /// Return the canonical [`HookEvent`] this body discriminates on.
    pub fn event(&self) -> HookEvent {
        match self {
            Self::PreToolUse { .. } => HookEvent::PreToolUse,
            Self::PostToolUse { .. } => HookEvent::PostToolUse,
            Self::PostToolUseFailure { .. } => HookEvent::PostToolUseFailure,
            Self::SessionStart {} => HookEvent::SessionStart,
            Self::SessionEnd {} => HookEvent::SessionEnd,
            Self::Stop {} => HookEvent::Stop,
            Self::PreCompact {} => HookEvent::PreCompact,
            Self::SubagentStart {} => HookEvent::SubagentStart,
            Self::SubagentStop {} => HookEvent::SubagentStop,
            Self::UserPromptSubmit { .. } => HookEvent::UserPromptSubmit,
        }
    }
}

/// Canonical output payload that hook scripts emit on stdout.
///
/// All decision fields are optional — a hook that produces no decision
/// emits either empty stdout or only the fields it wants to set, and the
/// adapter omits absent fields from the provider-shaped output.
///
/// `deny_unknown_fields` is set so that typos in a user's hook script
/// (e.g. `permmission_decision`) surface as parse errors rather than
/// silently no-op. Forward-compatible field additions live as new
/// `Option<T>` fields on this struct; users running an older agentspec
/// against a hook that emits a newer field is the documented break.
#[skip_serializing_none]
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalOutput {
    #[serde(default = "default_schema_version")]
    pub schema_version: String,
    pub permission_decision: Option<PermissionDecision>,
    /// Reason routed to the model — Claude's `permissionDecisionReason`
    /// (when `permission_decision` is set) and Cursor's `agent_message`.
    pub decision_reason: Option<String>,
    /// User-facing message — Cursor's `user_message`. Claude lacks a
    /// UI/model split, so on Claude this falls back into
    /// `permissionDecisionReason` when `decision_reason` is absent.
    pub user_facing_message: Option<String>,
    /// Context to inject — Claude's `hookSpecificOutput.additionalContext`
    /// or Cursor's `additional_context` (subject to Cursor 3.2.21's
    /// partial implementation; see `docs/hooks-canonical.md`).
    pub additional_context: Option<String>,
    /// Modified tool input (`PreToolUse` only). The adapter routes this to
    /// each provider's `updatedInput` / `updated_input` field.
    pub updated_input: Option<Value>,
}

impl CanonicalInput {
    /// Return the canonical [`HookEvent`] this input represents.
    pub fn event(&self) -> HookEvent {
        self.event_specific.event()
    }

    /// Reference implementation of provider stdin → canonical translation.
    ///
    /// Used by tests to verify the codegen'd jq programs produce the same
    /// result. Not invoked at hook-fire time — the user's machine runs the
    /// generated shim, not this code.
    pub fn from_provider_stdin(
        provider: ProviderName,
        raw: &str,
        event: HookEvent,
    ) -> Result<Self> {
        let value: Value =
            serde_json::from_str(raw).context("parsing provider hook stdin as JSON")?;
        match provider {
            ProviderName::Claude => from_claude(value, event),
            ProviderName::Cursor => from_cursor(value, event),
        }
    }
}

impl CanonicalOutput {
    /// Reference implementation of canonical → provider stdout translation.
    ///
    /// Same role as [`CanonicalInput::from_provider_stdin`]: drives test
    /// parity against the codegen'd jq programs; not invoked at hook-fire
    /// time.
    pub fn to_provider_stdout(&self, provider: ProviderName, event: HookEvent) -> Result<String> {
        match provider {
            ProviderName::Claude => Ok(to_claude_stdout(self, event)),
            ProviderName::Cursor => Ok(to_cursor_stdout(self)),
        }
    }
}

fn from_claude(value: Value, event: HookEvent) -> Result<CanonicalInput> {
    let session_id = require_str(&value, "session_id", "Claude")?.to_string();
    let agent_id = value
        .get("agent_id")
        .and_then(Value::as_str)
        .map(str::to_string);
    let cwd: PathBuf = require_str(&value, "cwd", "Claude")?.into();
    let transcript_path = value
        .get("transcript_path")
        .and_then(Value::as_str)
        .map(PathBuf::from);
    let event_specific = claude_event_body(&value, event)?;
    Ok(CanonicalInput {
        schema_version: SCHEMA_VERSION.to_string(),
        provider: ProviderName::Claude,
        session_id,
        agent_id,
        cwd,
        transcript_path,
        event_specific,
        provider_raw: value,
    })
}

fn from_cursor(value: Value, event: HookEvent) -> Result<CanonicalInput> {
    // Cursor renews `conversation_id` per subagent and carries the parent
    // link as `parent_conversation_id`. Canonical `session_id` must stay
    // stable across the agent hierarchy, so when a subagent payload arrives
    // we reconstruct it from `parent_conversation_id` and surface the
    // child id as `agent_id`. Top-level (non-subagent) firings have only
    // `conversation_id`, which becomes the `session_id` directly.
    let parent = value.get("parent_conversation_id").and_then(Value::as_str);
    let conv = value.get("conversation_id").and_then(Value::as_str);
    let session_id = parent
        .or(conv)
        .ok_or_else(|| anyhow!("Cursor payload missing conversation_id"))?
        .to_string();
    let agent_id = if parent.is_some() {
        conv.map(str::to_string)
    } else {
        None
    };
    let cwd: PathBuf = value
        .get("workspace_roots")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("Cursor payload missing workspace_roots[0]"))?
        .into();
    let transcript_path = value
        .get("transcript_path")
        .and_then(Value::as_str)
        .map(PathBuf::from);
    let event_specific = cursor_event_body(&value, event)?;
    Ok(CanonicalInput {
        schema_version: SCHEMA_VERSION.to_string(),
        provider: ProviderName::Cursor,
        session_id,
        agent_id,
        cwd,
        transcript_path,
        event_specific,
        provider_raw: value,
    })
}

fn claude_event_body(value: &Value, event: HookEvent) -> Result<CanonicalInputBody> {
    Ok(match event {
        HookEvent::PreToolUse => CanonicalInputBody::PreToolUse {
            tool_name: require_str(value, "tool_name", "Claude PreToolUse")?.to_string(),
            tool_use_id: optional_string(value, "tool_use_id"),
            tool_input: value.get("tool_input").cloned().unwrap_or(Value::Null),
        },
        HookEvent::PostToolUse => CanonicalInputBody::PostToolUse {
            tool_name: require_str(value, "tool_name", "Claude PostToolUse")?.to_string(),
            tool_use_id: optional_string(value, "tool_use_id"),
            tool_input: value.get("tool_input").cloned().unwrap_or(Value::Null),
            tool_response: value.get("tool_response").cloned().unwrap_or(Value::Null),
        },
        HookEvent::PostToolUseFailure => CanonicalInputBody::PostToolUseFailure {
            tool_name: require_str(value, "tool_name", "Claude PostToolUseFailure")?.to_string(),
            tool_use_id: optional_string(value, "tool_use_id"),
            tool_input: value.get("tool_input").cloned().unwrap_or(Value::Null),
            tool_response: value.get("tool_response").cloned().unwrap_or(Value::Null),
        },
        HookEvent::UserPromptSubmit => CanonicalInputBody::UserPromptSubmit {
            prompt: optional_string(value, "prompt"),
        },
        HookEvent::SessionStart => CanonicalInputBody::SessionStart {},
        HookEvent::SessionEnd => CanonicalInputBody::SessionEnd {},
        HookEvent::Stop => CanonicalInputBody::Stop {},
        HookEvent::PreCompact => CanonicalInputBody::PreCompact {},
        HookEvent::SubagentStart => CanonicalInputBody::SubagentStart {},
        HookEvent::SubagentStop => CanonicalInputBody::SubagentStop {},
    })
}

fn cursor_event_body(value: &Value, event: HookEvent) -> Result<CanonicalInputBody> {
    Ok(match event {
        HookEvent::PreToolUse => CanonicalInputBody::PreToolUse {
            tool_name: require_str(value, "tool_name", "Cursor preToolUse")?.to_string(),
            tool_use_id: optional_string(value, "tool_use_id"),
            tool_input: value.get("tool_input").cloned().unwrap_or(Value::Null),
        },
        HookEvent::PostToolUse => CanonicalInputBody::PostToolUse {
            tool_name: require_str(value, "tool_name", "Cursor postToolUse")?.to_string(),
            tool_use_id: optional_string(value, "tool_use_id"),
            tool_input: value.get("tool_input").cloned().unwrap_or(Value::Null),
            // Cursor exposes `tool_output` as an always-JSON-encoded string;
            // canonical `tool_response` is provider-shaped in v1 (the raw
            // value also lives in `provider_raw`). v2 may normalize.
            tool_response: value.get("tool_output").cloned().unwrap_or(Value::Null),
        },
        HookEvent::PostToolUseFailure => CanonicalInputBody::PostToolUseFailure {
            tool_name: require_str(value, "tool_name", "Cursor postToolUseFailure")?.to_string(),
            tool_use_id: optional_string(value, "tool_use_id"),
            tool_input: value.get("tool_input").cloned().unwrap_or(Value::Null),
            tool_response: value.get("tool_output").cloned().unwrap_or(Value::Null),
        },
        HookEvent::UserPromptSubmit => CanonicalInputBody::UserPromptSubmit {
            prompt: optional_string(value, "prompt"),
        },
        HookEvent::SessionStart => CanonicalInputBody::SessionStart {},
        HookEvent::SessionEnd => CanonicalInputBody::SessionEnd {},
        HookEvent::Stop => CanonicalInputBody::Stop {},
        HookEvent::PreCompact => CanonicalInputBody::PreCompact {},
        HookEvent::SubagentStart => CanonicalInputBody::SubagentStart {},
        HookEvent::SubagentStop => CanonicalInputBody::SubagentStop {},
    })
}

fn require_str<'a>(value: &'a Value, key: &str, ctx: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("{ctx} payload missing required string field `{key}`"))
}

fn optional_string(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

// Empty-output contract: this function is only invoked with a non-empty
// `CanonicalOutput` (the shim runtime skips output translation entirely
// when the user's stdout is empty). When invoked with all decision fields
// `None`, the result is `{"hookSpecificOutput":{"hookEventName":"..."}}` —
// matching what the Phase 2 jq program produces after `with_entries`
// filters null fields, since `hookEventName` is a literal and survives.
fn to_claude_stdout(output: &CanonicalOutput, event: HookEvent) -> String {
    let mut inner = serde_json::Map::new();
    inner.insert("hookEventName".into(), json!(event.pascal_case()));
    if let Some(decision) = output.permission_decision {
        inner.insert("permissionDecision".into(), json!(decision));
    }
    // Claude lacks a UI/model split — prefer `decision_reason`, falling
    // back to `user_facing_message` so a single field surfaces either way.
    let reason = output
        .decision_reason
        .as_deref()
        .or(output.user_facing_message.as_deref());
    if let Some(r) = reason {
        inner.insert("permissionDecisionReason".into(), json!(r));
    }
    if let Some(ctx) = &output.additional_context {
        inner.insert("additionalContext".into(), json!(ctx));
    }
    if let Some(input) = &output.updated_input {
        inner.insert("updatedInput".into(), input.clone());
    }
    let v = json!({ "hookSpecificOutput": Value::Object(inner) });
    v.to_string()
}

// Empty-output contract: when invoked with all decision fields `None`,
// the result is `{}` — matching what the Phase 2 jq program produces
// after `with_entries` filters null fields. Cursor's runtime treats `{}`
// as "no opinion" (verified Phase 0.5 Gate #19).
fn to_cursor_stdout(output: &CanonicalOutput) -> String {
    let mut map = serde_json::Map::new();
    if let Some(decision) = output.permission_decision {
        map.insert("permission".into(), json!(decision));
    }
    if let Some(reason) = &output.decision_reason {
        map.insert("agent_message".into(), json!(reason));
    }
    if let Some(msg) = &output.user_facing_message {
        map.insert("user_message".into(), json!(msg));
    }
    if let Some(ctx) = &output.additional_context {
        map.insert("additional_context".into(), json!(ctx));
    }
    if let Some(input) = &output.updated_input {
        map.insert("updated_input".into(), input.clone());
    }
    Value::Object(map).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claude_pre_tool_use_raw() -> &'static str {
        r#"{
            "session_id": "sess-abc",
            "transcript_path": "/tmp/transcript.jsonl",
            "cwd": "/home/u/proj",
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_use_id": "toolu_01",
            "tool_input": { "command": "ls", "description": "list" }
        }"#
    }

    fn cursor_pre_tool_use_raw() -> &'static str {
        r#"{
            "conversation_id": "conv-abc",
            "workspace_roots": ["/home/u/proj"],
            "hook_event_name": "preToolUse",
            "tool_name": "shell",
            "tool_use_id": "tu-1",
            "tool_input": { "command": "ls" }
        }"#
    }

    fn cursor_pre_tool_use_subagent_raw() -> &'static str {
        r#"{
            "conversation_id": "child-id",
            "parent_conversation_id": "root-id",
            "workspace_roots": ["/home/u/proj"],
            "hook_event_name": "preToolUse",
            "tool_name": "shell",
            "tool_use_id": "tu-1",
            "tool_input": { "command": "ls" }
        }"#
    }

    fn claude_pre_tool_use_subagent_raw() -> &'static str {
        r#"{
            "session_id": "sess-abc",
            "agent_id": "sub-1",
            "cwd": "/home/u/proj",
            "tool_name": "Bash",
            "tool_use_id": "toolu_01",
            "tool_input": { "command": "ls" }
        }"#
    }

    #[test]
    fn claude_pre_tool_use_round_trip() {
        let input = CanonicalInput::from_provider_stdin(
            ProviderName::Claude,
            claude_pre_tool_use_raw(),
            HookEvent::PreToolUse,
        )
        .expect("translate Claude PreToolUse");
        assert_eq!(input.schema_version, SCHEMA_VERSION);
        assert_eq!(input.provider, ProviderName::Claude);
        assert_eq!(input.session_id, "sess-abc");
        assert_eq!(input.agent_id, None);
        assert_eq!(input.cwd, PathBuf::from("/home/u/proj"));
        assert_eq!(
            input.transcript_path,
            Some(PathBuf::from("/tmp/transcript.jsonl"))
        );
        let CanonicalInputBody::PreToolUse {
            tool_name,
            tool_use_id,
            tool_input,
        } = &input.event_specific
        else {
            panic!("expected PreToolUse, got {:?}", input.event_specific);
        };
        assert_eq!(tool_name, "Bash");
        assert_eq!(tool_use_id, "toolu_01");
        assert_eq!(tool_input["command"], "ls");
        assert_eq!(tool_input["description"], "list");
        assert_eq!(input.event(), HookEvent::PreToolUse);
    }

    #[test]
    fn cursor_pre_tool_use_top_level() {
        let input = CanonicalInput::from_provider_stdin(
            ProviderName::Cursor,
            cursor_pre_tool_use_raw(),
            HookEvent::PreToolUse,
        )
        .expect("translate Cursor preToolUse");
        assert_eq!(input.provider, ProviderName::Cursor);
        assert_eq!(input.session_id, "conv-abc");
        assert_eq!(input.agent_id, None);
        assert_eq!(input.cwd, PathBuf::from("/home/u/proj"));
        assert_eq!(input.transcript_path, None);
        let CanonicalInputBody::PreToolUse {
            tool_name,
            tool_input,
            ..
        } = &input.event_specific
        else {
            panic!("expected PreToolUse, got {:?}", input.event_specific);
        };
        assert_eq!(tool_name, "shell");
        assert_eq!(tool_input["command"], "ls");
    }

    #[test]
    fn cursor_pre_tool_use_subagent_reconstruction() {
        let input = CanonicalInput::from_provider_stdin(
            ProviderName::Cursor,
            cursor_pre_tool_use_subagent_raw(),
            HookEvent::PreToolUse,
        )
        .expect("translate Cursor subagent preToolUse");
        // Subagent: parent → canonical session_id; conversation_id → agent_id.
        assert_eq!(input.session_id, "root-id");
        assert_eq!(input.agent_id.as_deref(), Some("child-id"));
    }

    #[test]
    fn claude_subagent_preserves_agent_id() {
        let input = CanonicalInput::from_provider_stdin(
            ProviderName::Claude,
            claude_pre_tool_use_subagent_raw(),
            HookEvent::PreToolUse,
        )
        .expect("translate Claude subagent PreToolUse");
        assert_eq!(input.session_id, "sess-abc");
        assert_eq!(input.agent_id.as_deref(), Some("sub-1"));
    }

    #[test]
    fn provider_raw_round_trips_byte_equivalent() {
        let raw = claude_pre_tool_use_raw();
        let input =
            CanonicalInput::from_provider_stdin(ProviderName::Claude, raw, HookEvent::PreToolUse)
                .expect("translate");
        let expected: Value = serde_json::from_str(raw).expect("parse raw");
        assert_eq!(input.provider_raw, expected);
    }

    #[test]
    fn claude_pre_tool_use_deny_output() {
        let out = CanonicalOutput {
            schema_version: SCHEMA_VERSION.to_string(),
            permission_decision: Some(PermissionDecision::Deny),
            decision_reason: Some("blocked".to_string()),
            user_facing_message: None,
            additional_context: None,
            updated_input: None,
        };
        let json_str = out
            .to_provider_stdout(ProviderName::Claude, HookEvent::PreToolUse)
            .expect("emit Claude stdout");
        let parsed: Value = serde_json::from_str(&json_str).expect("parse emitted");
        assert_eq!(parsed["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert_eq!(parsed["hookSpecificOutput"]["permissionDecision"], "deny");
        assert_eq!(
            parsed["hookSpecificOutput"]["permissionDecisionReason"],
            "blocked"
        );
    }

    #[test]
    fn cursor_pre_tool_use_deny_output() {
        let out = CanonicalOutput {
            schema_version: SCHEMA_VERSION.to_string(),
            permission_decision: Some(PermissionDecision::Deny),
            decision_reason: Some("blocked".to_string()),
            user_facing_message: None,
            additional_context: None,
            updated_input: None,
        };
        let json_str = out
            .to_provider_stdout(ProviderName::Cursor, HookEvent::PreToolUse)
            .expect("emit Cursor stdout");
        let parsed: Value = serde_json::from_str(&json_str).expect("parse emitted");
        assert_eq!(parsed["permission"], "deny");
        assert_eq!(parsed["agent_message"], "blocked");
        assert!(parsed.get("user_message").is_none());
    }

    #[test]
    fn cursor_user_facing_message_populates_user_message() {
        let out = CanonicalOutput {
            schema_version: SCHEMA_VERSION.to_string(),
            permission_decision: Some(PermissionDecision::Deny),
            decision_reason: None,
            user_facing_message: Some("hello user".to_string()),
            additional_context: None,
            updated_input: None,
        };
        let json_str = out
            .to_provider_stdout(ProviderName::Cursor, HookEvent::PreToolUse)
            .expect("emit Cursor stdout");
        let parsed: Value = serde_json::from_str(&json_str).expect("parse emitted");
        assert_eq!(parsed["user_message"], "hello user");
        assert!(parsed.get("agent_message").is_none());
    }

    #[test]
    fn claude_both_reasons_prefers_decision_reason() {
        // Documented behavior: Claude has one reason slot
        // (`permissionDecisionReason`) and `decision_reason` wins over
        // `user_facing_message` when both are set, since `decision_reason`
        // is the model-facing primary.
        let out = CanonicalOutput {
            schema_version: SCHEMA_VERSION.to_string(),
            permission_decision: Some(PermissionDecision::Deny),
            decision_reason: Some("for the model".to_string()),
            user_facing_message: Some("for the user".to_string()),
            additional_context: None,
            updated_input: None,
        };
        let json_str = out
            .to_provider_stdout(ProviderName::Claude, HookEvent::PreToolUse)
            .expect("emit Claude stdout");
        let parsed: Value = serde_json::from_str(&json_str).expect("parse emitted");
        assert_eq!(
            parsed["hookSpecificOutput"]["permissionDecisionReason"],
            "for the model"
        );
    }

    #[test]
    fn claude_user_facing_message_fallback_when_no_decision_reason() {
        let out = CanonicalOutput {
            schema_version: SCHEMA_VERSION.to_string(),
            permission_decision: Some(PermissionDecision::Deny),
            decision_reason: None,
            user_facing_message: Some("for the user".to_string()),
            additional_context: None,
            updated_input: None,
        };
        let json_str = out
            .to_provider_stdout(ProviderName::Claude, HookEvent::PreToolUse)
            .expect("emit Claude stdout");
        let parsed: Value = serde_json::from_str(&json_str).expect("parse emitted");
        assert_eq!(
            parsed["hookSpecificOutput"]["permissionDecisionReason"],
            "for the user"
        );
    }

    #[test]
    fn schema_version_always_populated_on_canonical_input() {
        let input = CanonicalInput::from_provider_stdin(
            ProviderName::Claude,
            claude_pre_tool_use_raw(),
            HookEvent::PreToolUse,
        )
        .expect("translate");
        let serialized = serde_json::to_value(&input).expect("serialize");
        assert_eq!(serialized["schema_version"], SCHEMA_VERSION);
    }

    #[test]
    fn canonical_output_skip_serializing_none() {
        // With every Option None, the wire form should be just
        // `{"schema_version": "1.0.0"}` exactly — proving
        // `serde_with::skip_serializing_none` is honored on every
        // Option field.
        let out = CanonicalOutput {
            schema_version: SCHEMA_VERSION.to_string(),
            permission_decision: None,
            decision_reason: None,
            user_facing_message: None,
            additional_context: None,
            updated_input: None,
        };
        let json_str = serde_json::to_string(&out).expect("serialize");
        assert_eq!(json_str, r#"{"schema_version":"1.0.0"}"#);
    }

    #[test]
    fn canonical_output_default_schema_version_on_deserialize() {
        // A user script that omits `schema_version` should still
        // deserialize cleanly, with the field defaulted to the current
        // build's `SCHEMA_VERSION`.
        let parsed: CanonicalOutput =
            serde_json::from_str(r#"{"permission_decision":"deny"}"#).expect("parse");
        assert_eq!(parsed.schema_version, SCHEMA_VERSION);
        assert_eq!(parsed.permission_decision, Some(PermissionDecision::Deny));
    }

    #[test]
    fn input_wire_form_has_single_event_field() {
        // The outer struct omits `event` and lets `CanonicalInputBody`'s
        // serde tag supply it — regression guard against accidentally
        // reintroducing a duplicate `event` field on the outer struct.
        let input = CanonicalInput::from_provider_stdin(
            ProviderName::Claude,
            claude_pre_tool_use_raw(),
            HookEvent::PreToolUse,
        )
        .expect("translate");
        let serialized = serde_json::to_string(&input).expect("serialize");
        let event_occurrences = serialized.matches(r#""event":"#).count();
        assert_eq!(
            event_occurrences, 1,
            "expected exactly one `event` key on the wire, got {event_occurrences}: {serialized}"
        );
    }

    #[test]
    fn every_event_has_envelope_only_round_trip() {
        // Property-style: for every HookEvent variant, both providers can
        // produce a canonical input given an envelope-only stub payload.
        // This guards against missing match arms in claude_event_body /
        // cursor_event_body when new HookEvent variants land.
        let events = [
            HookEvent::SessionStart,
            HookEvent::SessionEnd,
            HookEvent::Stop,
            HookEvent::PreCompact,
            HookEvent::SubagentStart,
            HookEvent::SubagentStop,
        ];
        for event in events {
            // The `hook_event_name` literal on these fixtures is
            // intentionally constant — the translation dispatches on the
            // Rust `event` parameter, not on the payload's event-name
            // field, so a single envelope-only fixture works for every
            // event in this loop.
            let claude_raw = r#"{"session_id":"s","cwd":"/p"}"#;
            let cursor_raw = r#"{"conversation_id":"c","workspace_roots":["/p"]}"#;
            let claude =
                CanonicalInput::from_provider_stdin(ProviderName::Claude, claude_raw, event)
                    .unwrap_or_else(|e| panic!("Claude {event:?} failed: {e}"));
            let cursor =
                CanonicalInput::from_provider_stdin(ProviderName::Cursor, cursor_raw, event)
                    .unwrap_or_else(|e| panic!("Cursor {event:?} failed: {e}"));
            assert_eq!(claude.event(), event);
            assert_eq!(cursor.event(), event);
        }
    }

    #[test]
    fn permission_decision_allow_renders_lowercase() {
        let out = CanonicalOutput {
            schema_version: SCHEMA_VERSION.to_string(),
            permission_decision: Some(PermissionDecision::Allow),
            decision_reason: None,
            user_facing_message: None,
            additional_context: None,
            updated_input: None,
        };
        let claude = out
            .to_provider_stdout(ProviderName::Claude, HookEvent::PreToolUse)
            .expect("Claude allow");
        let cursor = out
            .to_provider_stdout(ProviderName::Cursor, HookEvent::PreToolUse)
            .expect("Cursor allow");
        let claude_v: Value = serde_json::from_str(&claude).expect("parse Claude");
        let cursor_v: Value = serde_json::from_str(&cursor).expect("parse Cursor");
        assert_eq!(
            claude_v["hookSpecificOutput"]["permissionDecision"],
            "allow"
        );
        assert_eq!(cursor_v["permission"], "allow");
    }

    #[test]
    fn permission_decision_ask_renders_lowercase() {
        let out = CanonicalOutput {
            schema_version: SCHEMA_VERSION.to_string(),
            permission_decision: Some(PermissionDecision::Ask),
            decision_reason: None,
            user_facing_message: None,
            additional_context: None,
            updated_input: None,
        };
        let claude = out
            .to_provider_stdout(ProviderName::Claude, HookEvent::PreToolUse)
            .expect("Claude ask");
        let cursor = out
            .to_provider_stdout(ProviderName::Cursor, HookEvent::PreToolUse)
            .expect("Cursor ask");
        let claude_v: Value = serde_json::from_str(&claude).expect("parse Claude");
        let cursor_v: Value = serde_json::from_str(&cursor).expect("parse Cursor");
        assert_eq!(claude_v["hookSpecificOutput"]["permissionDecision"], "ask");
        assert_eq!(cursor_v["permission"], "ask");
    }

    #[test]
    fn additional_context_round_trip_both_providers() {
        let out = CanonicalOutput {
            schema_version: SCHEMA_VERSION.to_string(),
            permission_decision: None,
            decision_reason: None,
            user_facing_message: None,
            additional_context: Some("hidden context".to_string()),
            updated_input: None,
        };
        let claude = out
            .to_provider_stdout(ProviderName::Claude, HookEvent::UserPromptSubmit)
            .expect("Claude additional_context");
        let cursor = out
            .to_provider_stdout(ProviderName::Cursor, HookEvent::UserPromptSubmit)
            .expect("Cursor additional_context");
        let claude_v: Value = serde_json::from_str(&claude).expect("parse Claude");
        let cursor_v: Value = serde_json::from_str(&cursor).expect("parse Cursor");
        assert_eq!(
            claude_v["hookSpecificOutput"]["additionalContext"],
            "hidden context"
        );
        assert_eq!(cursor_v["additional_context"], "hidden context");
    }

    #[test]
    fn updated_input_round_trip_both_providers() {
        let out = CanonicalOutput {
            schema_version: SCHEMA_VERSION.to_string(),
            permission_decision: None,
            decision_reason: None,
            user_facing_message: None,
            additional_context: None,
            updated_input: Some(json!({ "command": "ls -al" })),
        };
        let claude = out
            .to_provider_stdout(ProviderName::Claude, HookEvent::PreToolUse)
            .expect("Claude updated_input");
        let cursor = out
            .to_provider_stdout(ProviderName::Cursor, HookEvent::PreToolUse)
            .expect("Cursor updated_input");
        let claude_v: Value = serde_json::from_str(&claude).expect("parse Claude");
        let cursor_v: Value = serde_json::from_str(&cursor).expect("parse Cursor");
        assert_eq!(
            claude_v["hookSpecificOutput"]["updatedInput"]["command"],
            "ls -al"
        );
        assert_eq!(cursor_v["updated_input"]["command"], "ls -al");
    }

    #[test]
    fn cursor_provider_raw_round_trips_byte_equivalent() {
        let raw = cursor_pre_tool_use_subagent_raw();
        let input =
            CanonicalInput::from_provider_stdin(ProviderName::Cursor, raw, HookEvent::PreToolUse)
                .expect("translate Cursor");
        let expected: Value = serde_json::from_str(raw).expect("parse raw");
        assert_eq!(input.provider_raw, expected);
    }

    #[test]
    fn claude_all_none_output_emits_envelope_with_hook_event_name() {
        // Locks in the documented empty-output contract: when invoked
        // with all decision fields None, the Rust ref impl emits the
        // `hookSpecificOutput.hookEventName` envelope — matching the
        // Phase 2 jq program which keeps `hookEventName` as a literal
        // after `with_entries(select(.value != null))` filters nulls.
        let out = CanonicalOutput {
            schema_version: SCHEMA_VERSION.to_string(),
            permission_decision: None,
            decision_reason: None,
            user_facing_message: None,
            additional_context: None,
            updated_input: None,
        };
        let s = out
            .to_provider_stdout(ProviderName::Claude, HookEvent::PreToolUse)
            .expect("Claude empty");
        assert_eq!(
            s,
            r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse"}}"#
        );
    }

    #[test]
    fn cursor_all_none_output_emits_empty_object() {
        // Locks in the documented empty-output contract on Cursor: when
        // invoked with all decision fields None, the Rust ref impl emits
        // `{}` — matching the Phase 2 jq program.
        let out = CanonicalOutput {
            schema_version: SCHEMA_VERSION.to_string(),
            permission_decision: None,
            decision_reason: None,
            user_facing_message: None,
            additional_context: None,
            updated_input: None,
        };
        let s = out
            .to_provider_stdout(ProviderName::Cursor, HookEvent::PreToolUse)
            .expect("Cursor empty");
        assert_eq!(s, "{}");
    }

    #[test]
    fn canonical_output_rejects_unknown_fields() {
        // `deny_unknown_fields` surfaces user-script typos as parse errors
        // rather than letting them no-op silently.
        let err = serde_json::from_str::<CanonicalOutput>(r#"{"permmission_decision":"deny"}"#)
            .expect_err("typo should reject");
        let msg = err.to_string();
        assert!(
            msg.contains("permmission_decision"),
            "expected error to name the unknown field, got: {msg}"
        );
    }
}

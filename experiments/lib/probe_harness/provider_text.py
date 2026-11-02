"""The catalog: text a provider or the harness writes into a capture, which no package authored.

A committed sample keeps only text the accounting check (`samples.unaccounted`) can vouch
for. Text an author wrote is vouched for by the package's own files; text a provider or the
harness produces is vouched for by an entry here. An entry is a literal string, or a
`Template` whose placeholders each admit one narrow shape, and none admits a free word. So
anything a capture carries that is neither authored nor listed here — an operator's
instructions, a connector's name, a report summarizing either — fails `just probe-test`
rather than reaching a commit.

A `harness` entry's text is the harness constant itself, never a copy, so the catalog cannot
drift from the code that produces the string.
"""

from dataclasses import dataclass
from enum import Enum
from typing import Literal

from probe_harness import assertions, workspace
from probe_harness.families import manual
from probe_harness.gates import claude_otel

type Source = Literal["claude", "opencode", "cursor", "harness"]


class Placeholder(Enum):
    """A part of a `Template` admitting one shape, compiled by `samples._template_regex`.
    `WORKSPACE`, `ENCODED_WORKSPACE` and `TOKEN` depend on the package."""

    # `[0-9]+`
    DIGITS = "digits"
    # `[0-9a-f]{8,}`
    HEX = "hex"
    # `[0-9a-f]{3}`: the fingerprint Claude Code appends to its version in the billing header.
    FINGERPRINT = "fingerprint"
    # `[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}`
    UUID = "uuid"
    # An ISO 8601 date-time.
    TIMESTAMP = "timestamp"
    # A path under the sample workspace root, naming only the package's own variants, arms
    # and fixture-tree paths.
    WORKSPACE = "workspace"
    # The `WORKSPACE` form with every `/` and `.` written as `-`, as Claude Code encodes a
    # working directory in its temp and project paths.
    ENCODED_WORKSPACE = "encoded-workspace"
    # `/tmp/` followed by id-shaped segments.
    TEMP = "temp"
    # Any word-bounded token occurring in the package's authored files.
    TOKEN = "token"


@dataclass(frozen=True)
class Id:
    """A fixed <prefix>, then `[A-Za-z0-9]+`: an id a provider mints, such as `toolu_…`."""

    prefix: str


@dataclass(frozen=True)
class Template:
    parts: tuple[str | Placeholder | Id, ...]


@dataclass(frozen=True)
class Entry:
    provider: Source
    # What the text is, for a reader deciding whether an entry still belongs.
    what: str
    text: str | Template


def _each(provider: Source, what: str, *texts: str) -> tuple[Entry, ...]:
    """One literal entry per text, all of one kind."""
    return tuple(Entry(provider, what, text) for text in texts)


_CLAUDE_BILLING_HEADER = (
    "x-anthropic-billing-header: cc_version=",
    Placeholder.DIGITS,
    ".",
    Placeholder.DIGITS,
    ".",
    Placeholder.DIGITS,
    ".",
    Placeholder.FINGERPRINT,
    "; cc_entrypoint=sdk-cli; cch=00000; cc_is_subagent=true;",
)

_CLAUDE_PERMISSION_DENIED = (
    "Permission for this tool use was denied. It requires approval, and this session has no "
    "approval surface — nobody can answer a permission prompt here — so it was denied "
    "automatically. The action was NOT performed; do not claim it succeeded, and do not retry "
    "it: this action, and anything else that requires approval, will be denied the same way "
    "for the rest of this session. Tell the user what was blocked and why you needed it, then "
    "continue with the parts of the task that do not require approval. What required "
    "approval: Claude requested permissions to use "
)

_OPENCODE_RULE_REFUSAL = (
    "The user has specified a rule which prevents you from using this specific tool call. "
    "Here are some of the relevant rules "
)

CATALOG: tuple[Entry, ...] = (
    *_each(
        "claude",
        "a key of a Messages API request or response body Claude Code sends or receives",
        "betas",
        "cache_control",
        "caller",
        "container",
        "content",
        "context_management",
        "defer_loading",
        "description",
        "diagnostics",
        "eager_input_streaming",
        "effort",
        "id",
        "input",
        "input_schema",
        "input_transformations",
        "is_error",
        "max_tokens",
        "messages",
        "metadata",
        "model",
        "name",
        "output_config",
        "role",
        "signature",
        "stop_details",
        "stop_reason",
        "stop_sequence",
        "stream",
        "system",
        "text",
        "thinking",
        "thread",
        "tool_use_id",
        "tools",
        "type",
        "usage",
    ),
    *_each(
        "claude",
        "a key of a JSON Schema in a tool's input_schema",
        "$schema",
        "additionalProperties",
        "description",
        "properties",
        "required",
        "type",
    ),
    *_each(
        "claude",
        "a property of a Claude Code built-in tool's input schema",
        "prompt",
        "run_in_background",
        "subagent_type",
    ),
    *_each(
        "claude",
        "a key of the payload Claude Code passes a command hook",
        "context_tokens",
        "cwd",
        "estimated_cache_write_usd",
        "hook_event_name",
        "model",
        "permission_mode",
        "prompt",
        "prompt_cache_likely_expired",
        "prompt_id",
        "scratchpad_dir",
        "seconds_since_last_response",
        "session_id",
        "source",
        "tool",
        "tool_name",
        "transcript_path",
    ),
    *_each(
        "claude",
        "a key of the stream-json init or result event",
        "additional_directories",
        "agents",
        "analytics_disabled",
        "apiKeySource",
        "api_error_origin",
        "api_error_status",
        "capabilities",
        "claude_code_version",
        "cwd",
        "duration_api_ms",
        "duration_ms",
        "fast_mode_disabled_reason",
        "fast_mode_state",
        "is_error",
        "mcp_servers",
        "memory_paths",
        "messaging_socket_path",
        "model",
        "modelUsage",
        "name",
        "num_turns",
        "original_cwd",
        "output_style",
        "per_turn_effort_active",
        "permissionMode",
        "permission_denials",
        "plugins",
        "product_feedback_disabled",
        "queued_turn_count",
        "result",
        "result_index",
        "safety_stops",
        "session_id",
        "skills",
        "slash_commands",
        "source",
        "status",
        "stop_reason",
        "subagent_stats",
        "subtype",
        "terminal_reason",
        "terminal_slash_commands",
        "tools",
        "total_cost_usd",
        "type",
        "usage",
        "uuid",
        "view_mode",
    ),
    *_each("claude", "a content block type", "text", "tool_result", "tool_use"),
    Entry("claude", "the source of a SessionStart hook payload on a fresh session", "startup"),
    *_each("claude", "a message role", "user"),
    *_each("claude", "a stream-json event type or subtype", "init", "result", "system"),
    *_each("claude", "an effort level Claude Code sends in output_config", "max", "medium"),
    *_each(
        "claude",
        "the name of a tool Claude Code builds in",
        "Agent",
        "Bash",
        "DeferredToolPlaceholder",
        "Edit",
        "Skill",
        "ToolSearch",
        "Write",
        "advisor",
    ),
    Entry("claude", "a tool-use id", Template((Id("toolu_"),))),
    Entry(
        "claude",
        "the first line of the message announcing deferred tools",
        "The following deferred tools are now available via ToolSearch. Their schemas are NOT "
        "loaded — calling them directly will fail with InputValidationError. Use ToolSearch "
        'with query "select:<name>[,<name>...]" to load tool schemas before calling them:',
    ),
    Entry(
        "claude",
        "the tool result of a call denied for want of an approval surface",
        Template(
            (_CLAUDE_PERMISSION_DENIED, Placeholder.TOKEN, ", but you haven't granted it yet.")
        ),
    ),
    Entry(
        "claude",
        "the billing header a subagent request's first system block carries",
        Template(_CLAUDE_BILLING_HEADER),
    ),
    Entry(
        "claude",
        "the billing header a subagent request's first system block carries, with its prompt id",
        Template((*_CLAUDE_BILLING_HEADER, " cc_prompt_id=", Placeholder.UUID, ";")),
    ),
    Entry(
        "claude",
        "the billing header a subagent follow-up's first system block carries",
        Template(
            (
                *_CLAUDE_BILLING_HEADER,
                " cc_prev_req=",
                Id("req_"),
                "; cc_prompt_id=",
                Placeholder.UUID,
                ";",
            )
        ),
    ),
    *_each(
        "cursor",
        "a key of the payload Cursor passes a hook",
        "attachments",
        "composer_mode",
        "conversation_id",
        "cursor_version",
        "cwd",
        "generation_id",
        "hook_event_name",
        "is_background_agent",
        "is_parallel_worker",
        "model",
        "model_id",
        "model_params",
        "parent_conversation_id",
        "prompt",
        "session_id",
        "subagent_id",
        "subagent_model",
        "subagent_type",
        "task",
        "tool_call_id",
        "tool_input",
        "tool_name",
        "tool_use_id",
        "transcript_path",
        "user_email",
        "workspace_roots",
    ),
    Entry(
        "cursor",
        "a Cursor version",
        Template((Placeholder.DIGITS, ".", Placeholder.DIGITS, ".", Placeholder.DIGITS)),
    ),
    Entry("cursor", "a conversation or session id", Template((Placeholder.UUID,))),
    Entry("cursor", "the hook event Cursor fires as a subagent starts", "subagentStart"),
    Entry(
        "cursor",
        "the model id Cursor reports for claude-opus-5 at high effort",
        "claude-opus-5-thinking-high",
    ),
    *_each(
        "opencode",
        "a key of a chat-completions request body OpenCode sends",
        "content",
        "description",
        "function",
        "max_tokens",
        "messages",
        "model",
        "name",
        "parameters",
        "role",
        "stream",
        "stream_options",
        "tool_call_id",
        "tool_calls",
        "tool_choice",
        "tools",
        "type",
    ),
    *_each(
        "opencode",
        "a key of the agent, command or config `opencode debug` prints",
        "$schema",
        "agent",
        "description",
        "mode",
        "name",
        "native",
        "options",
        "permission",
        "plugin",
        "prompt",
        "template",
        "tools",
        "username",
    ),
    *_each("opencode", "a message role", "system", "tool"),
    Entry(
        "opencode",
        "the modelID OpenCode resolves from anthropic/claude-sonnet-4-5#high",
        "claude-sonnet-4-5#high",
    ),
    Entry(
        "opencode",
        "a file line as OpenCode's read tool returns it, behind its line number",
        Template((Placeholder.DIGITS, ": ", Placeholder.TOKEN)),
    ),
    Entry(
        "opencode",
        "a name line of the resource or template list OpenCode's MCP tool returns",
        Template(('      "name": "', Placeholder.TOKEN, '",')),
    ),
    Entry(
        "opencode",
        "the tool result of a call the operator rejected",
        "The user rejected permission to use this specific tool call.",
    ),
    Entry(
        "opencode",
        "the tool result of a call an agent denying mcp:* reads refused, with the rules it met",
        _OPENCODE_RULE_REFUSAL + '[{"permission":"*","action":"allow","pattern":"*"},'
        '{"permission":"read","pattern":"*","action":"allow"},'
        '{"permission":"read","pattern":"*.env","action":"ask"},'
        '{"permission":"read","pattern":"*.env.*","action":"ask"},'
        '{"permission":"read","pattern":"*.env.example","action":"allow"},'
        '{"permission":"*","action":"deny","pattern":"*"},'
        '{"permission":"read","pattern":"*","action":"allow"},'
        '{"permission":"read","pattern":"mcp:*","action":"deny"}]',
    ),
    Entry(
        "opencode",
        "the tool result of an external read an agent's rules refused, with the rules it met",
        Template(
            (
                _OPENCODE_RULE_REFUSAL + '[{"permission":"*","action":"allow","pattern":"*"},'
                '{"permission":"external_directory","pattern":"*","action":"ask"},'
                '{"permission":"external_directory","pattern":"',
                Placeholder.WORKSPACE,
                '/opencode/tool-output/*","action":"allow"},'
                '{"permission":"external_directory","pattern":"/tmp/opencode/*","action":"allow"},'
                '{"permission":"*","action":"deny","pattern":"*"},'
                '{"permission":"external_directory","pattern":"',
                Placeholder.WORKSPACE,
                '/opencode/tool-output/*","action":"allow"}]',
            )
        ),
    ),
    Entry(
        "harness",
        "the value an arm with no governed request yields",
        assertions.NO_GOVERNED_REQUEST,
    ),
    Entry(
        "harness",
        "relative_effort's value when the control arms do not set one effort",
        claude_otel.NO_CONTROL_SET,
    ),
    Entry(
        "harness",
        "relative_effort's value when an arm's efforts equal the control's",
        claude_otel.SAME_AS_UNGOVERNED,
    ),
    Entry(
        "harness",
        "call_outcome's key for reply text it does not recognize",
        claude_otel.UNRECOGNIZED,
    ),
    Entry("harness", "the manual hook's key for a payload that was not JSON", manual.UNPARSED_KEY),
    Entry("harness", "the home directory as a sample spells it", workspace.SAMPLE_HOME),
    Entry("harness", "a path in a probe workspace", Template((Placeholder.WORKSPACE,))),
    Entry(
        "harness",
        "the capture hook's path under the retired mktemp workspace layout, which no current "
        "code produces, so no constant backs it",
        Template(("/tmp/", Id("probe-cursor-hook-command-argv."), "/capture/dump-hook.sh")),
    ),
)

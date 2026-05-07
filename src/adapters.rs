mod claude;
mod cursor;
mod opencode;

pub use claude::{
    adapt_claude, body_tool_name as claude_body_tool_name, claude_event_name, entry_to_claude_json,
    model_facing_name as claude_model_facing_name, post_write_hook as claude_post_write_hook,
    remove_post_write_hook as claude_remove_post_write_hook,
    synthesize_hooks as claude_synthesize_hooks,
};
pub use cursor::{
    adapt_cursor, body_tool_name as cursor_body_tool_name, cursor_event_name, entry_to_cursor_json,
    model_facing_name as cursor_model_facing_name, post_write_hook as cursor_post_write_hook,
    remove_post_write_hook as cursor_remove_post_write_hook,
    synthesize_hooks as cursor_synthesize_hooks,
};
pub use opencode::{
    adapt_opencode, body_tool_name as opencode_body_tool_name,
    model_facing_name as opencode_model_facing_name, post_write_hook as opencode_post_write_hook,
    remove_post_write_hook as opencode_remove_post_write_hook,
};

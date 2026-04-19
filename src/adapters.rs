mod claude;
mod cursor;
mod opencode;

pub use claude::{
    adapt_claude, body_tool_name as claude_body_tool_name,
    model_facing_name as claude_model_facing_name, post_write_hook as claude_post_write_hook,
};
pub use cursor::{
    adapt_cursor, body_tool_name as cursor_body_tool_name,
    model_facing_name as cursor_model_facing_name, post_write_hook as cursor_post_write_hook,
};
pub use opencode::{
    adapt_opencode, body_tool_name as opencode_body_tool_name,
    model_facing_name as opencode_model_facing_name, post_write_hook as opencode_post_write_hook,
};

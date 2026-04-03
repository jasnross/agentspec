mod claude;
mod cursor;
mod opencode;

pub use claude::{adapt_claude, post_write_hook as claude_post_write_hook};
pub use cursor::{adapt_cursor, post_write_hook as cursor_post_write_hook};
pub use opencode::{adapt_opencode, post_write_hook as opencode_post_write_hook};

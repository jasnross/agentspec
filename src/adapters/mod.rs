mod claude;
mod codex;
mod cursor;
mod opencode;

pub use claude::adapt_claude;
pub use codex::adapt_codex;
pub use cursor::adapt_cursor;
pub use opencode::{adapt_opencode, build_opencode_instructions};

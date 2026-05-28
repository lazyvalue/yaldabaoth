pub mod acp_channel;
pub mod blocks;
pub mod buffer;
pub mod claude_channel;
pub mod command;
pub mod config;
pub mod cursor;
pub mod document;
pub mod editor;
pub mod file_browser;
pub mod highlight;
pub mod keybind;
pub mod keys;
pub mod md_highlight;
pub mod menu;
pub mod parse;
pub mod render;
pub mod session_client;
pub mod session_proto;
pub mod style;
pub mod theme;
pub mod tree;
pub mod view;
pub mod viewport;

/// Human-readable build identifier produced by build.rs:
/// `"<crate-version> (<git-sha>[-dirty] <utc-timestamp>)"`.
pub const BUILD_INFO: &str = env!("SKETCH_BUILD_INFO");
pub const BUILD_SHA: &str = env!("SKETCH_BUILD_SHA");
pub const BUILD_TIME: &str = env!("SKETCH_BUILD_TIME");

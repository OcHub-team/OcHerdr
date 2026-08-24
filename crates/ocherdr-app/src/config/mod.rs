pub mod document;
/// Settings-page import planner. T29-C owns the confirm UI, so nothing in this
/// crate calls it yet; calling it at startup would read Ghostty's config
/// without asking.
#[allow(dead_code)]
pub mod import;
pub mod migrate;
pub mod values;

pub use document::ConfigDocument;
pub use migrate::{AppPaths, LoadedApp, load_app, write_config, write_connections};
pub use values::format_opacity_percent;

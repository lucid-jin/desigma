//! Figma file support.
//! CONTRACT: `import_rest_json`, `import_fig`, `FigError` are used by the app.
//!
//! - [`import_rest_json`]: body of Figma REST `GET /v1/files/:key` (ideally
//!   requested with `geometry=paths`) -> [`Document`].
//! - [`import_fig`]: `.fig` binary (fig-kiwi container, optionally zip-wrapped).
//! - [`export_rest_json`]: [`Document`] -> REST-shaped JSON.
//! - [`image_refs`]: image hashes a document needs fetched separately
//!   (REST `GET /v1/files/:key/images`); `.fig` files embed them already.
//! - `rest` (cargo feature `net`): blocking REST client; the access token is
//!   always supplied by the caller at runtime and never stored or logged.

mod board;
pub mod container;
mod dev;
mod export;
mod fig;
pub mod kiwi;
mod motion;
mod outline;
mod proto;
#[cfg(feature = "net")]
pub mod rest;
mod rest_import;
pub mod svgpath;
#[doc(hidden)]
pub mod testkit;
mod textfmt;
mod variables;
mod vecnet;

pub use desigma_doc::Document;
pub use export::export_rest_json;
#[doc(hidden)]
pub use fig::import_fig_node_changes;
pub use fig::{commands_blob_to_path, decode_fig_to_json, import_fig, import_fig_with, outline_fig};
pub use outline::{FigOutline, ImportOptions, OutlineFrame, OutlinePage};
pub use proto::{gamepad_button_code, gamepad_button_name, key_code, key_name};
#[doc(hidden)]
pub use rest_import::import_rest_json_limited;
pub use rest_import::{image_refs, import_rest_json};

/// Most nodes an import may hold, counting the copies instances expand into. The check runs
/// while instances expand, so a file that would blow past it fails early instead of after the
/// memory is spent. Sized from measurement: a 188,518-node file imports at about 3.3-4.2 GB, so
/// this allows that file with about 30% to spare and keeps the worst case near 6 GB.
pub const MAX_IMPORT_NODES: usize = 250_000;
/// Most nodes a `.fig` may store, before instances expand. A stored node costs far more than a
/// copy (it is held as JSON while converting), so it gets its own, lower, limit.
pub const MAX_STORED_NODES: usize = 150_000;
pub use variables::{export_variables_json, export_variables_meta, import_variables_json, import_variables_value};

#[derive(Debug, thiserror::Error)]
pub enum FigError {
    #[error("invalid json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("not a fig file: {0}")]
    Format(String),
    #[error("unsupported: {0}")]
    Unsupported(String),
}

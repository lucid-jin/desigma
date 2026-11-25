//! Agent bridge: desigma-mcp drives the live document through the tool engine.
pub mod server;
pub mod ui;
pub use server::{Bridge, DISCOVERY_FILE};

use crate::app::App;
use desigma_doc::{Command, Document, NodeId};
use desigma_mcp::engine::{self, Host};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{collections::BTreeSet, path::PathBuf};

/// Settings flag for Preferences → Allow MCP agents to control this editor.
pub const FLAG: &str = "agent_bridge";

#[derive(Default)]
pub struct BridgeState {
    pub bridge: Option<Bridge>,
}

/// The editor as a tool host: commands go through `Tx` so component sync and rerouting apply.
struct AppHost<'a>(&'a mut App);
impl Host for AppHost<'_> {
    fn doc(&self) -> &Document {
        &self.0.doc
    }
    fn exec(&mut self, cmd: Command) -> Result<(), String> {
        let doc = &mut self.0.doc;
        // Lay out what the edit touched, as the editor's own create/delete/property handlers do.
        let around = match &cmd {
            Command::Insert { parent, .. } => Some(*parent),
            Command::Remove { id } => doc.get(*id).and_then(|n| n.parent),
            Command::Update { node } => Some(node.id),
            _ => None,
        };
        // A move lays out the parent it left and the one it joined, and touches the moved layer
        // so the connectors attached to it are rerouted on commit, as a drag on the canvas does.
        let moved = match &cmd {
            Command::Reparent { id, parent, .. } => Some((*id, doc.get(*id).and_then(|n| n.parent), *parent)),
            _ => None,
        };
        let mut tx = crate::edit::Tx::new();
        tx.cmd(doc, cmd).map_err(|e| e.to_string())?;
        if let Some(id) = around.filter(|id| doc.get(*id).is_some()) {
            crate::layout::relayout_around(doc, &mut tx.ed, id);
        }
        if let Some((id, old, new)) = moved {
            tx.ed.touch(doc, id);
            for p in old.into_iter().chain([new]) {
                if doc.get(p).is_some() {
                    crate::layout::relayout_around(doc, &mut tx.ed, p);
                }
            }
        }
        if self.0.run_tx(tx) {
            Ok(())
        } else {
            Err("Edit failed".into())
        }
    }
    fn undo(&mut self) -> Result<bool, String> {
        Ok(self.0.undo())
    }
    fn redo(&mut self) -> Result<bool, String> {
        Ok(self.0.redo())
    }
    fn can_undo(&self) -> bool {
        self.0.hist.can_undo()
    }
    fn can_redo(&self) -> bool {
        self.0.hist.can_redo()
    }
    fn alloc_id(&mut self) -> Result<NodeId, String> {
        self.0.doc.try_alloc_id().ok_or_else(|| "Document has exhausted node IDs".into())
    }
    fn doc_mut(&mut self) -> &mut Document {
        // Content-addressed assets only (SVG bitmaps): the editor's own SVG
        // import writes them the same way, outside the undo stack.
        &mut self.0.doc
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectArgs {
    node_ids: Vec<u64>,
    #[serde(default = "yes")]
    reveal: bool,
}
fn yes() -> bool {
    true
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenArgs {
    path: PathBuf,
    #[serde(default)]
    discard_changes: bool,
    #[serde(default)]
    frames: Option<BTreeSet<String>>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveArgs {
    path: PathBuf,
    #[serde(default)]
    overwrite: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewArgs {
    name: String,
    #[serde(default)]
    discard_changes: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExportArgs {
    node_id: u64,
    #[serde(default = "one")]
    scale: f32,
    path: PathBuf,
}
fn one() -> f32 {
    1.0
}
fn parse<T: serde::de::DeserializeOwned>(args: Value) -> Result<T, String> {
    serde_json::from_value(args).map_err(|e| format!("invalid arguments: {e}"))
}

const UNSAVED: &str = "Unsaved changes in the editor: ask the user to save them first (discard_changes=true throws them away)";

fn validate_bridge_open_path(path: &std::path::Path) -> Result<(), String> {
    if path.is_absolute() {
        Ok(())
    } else {
        Err(engine::generic_import_rejection_guidance("Open path must be absolute".into()))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BridgeOpenRoute {
    LimitedImporter,
    LegacyLoader,
}

fn bridge_open_route(path: &std::path::Path, picked_frames: bool) -> BridgeOpenRoute {
    let is_fig = path.extension().and_then(|ext| ext.to_str()).is_some_and(|ext| ext.eq_ignore_ascii_case("fig"));
    if picked_frames || is_fig {
        BridgeOpenRoute::LimitedImporter
    } else {
        BridgeOpenRoute::LegacyLoader
    }
}

/// Full and frame-selected FIG opens use desigma-library's import limits. Other
/// formats retain the editor's legacy loader to avoid changing normal opens.
/// Exact 256 MiB, stored-node, and expanded-node boundary contracts remain
/// owned by desigma-library; the bridge only guarantees routing through it.
fn load_bridge_document(path: &std::path::Path, frames: Option<&BTreeSet<String>>) -> Result<Document, String> {
    match bridge_open_route(path, frames.is_some()) {
        BridgeOpenRoute::LimitedImporter => {
            let options = desigma_library::ImportOptions { only_frames: frames.cloned() };
            crate::io::load_path_with(path, &options)
        }
        BridgeOpenRoute::LegacyLoader => crate::io::load_path(path),
    }
}

/// Bridge-only calls that change the editor, beside the engine's [`engine::EDIT_TOOLS`].
const MUTATING: &[&str] = &["desigma_select_nodes", "desigma_open_file", "desigma_save_file", "desigma_new_document"];

impl App {
    pub fn start_bridge(&mut self, ctx: &egui::Context) -> Result<(), String> {
        if self.bridge.bridge.is_some() {
            return Ok(());
        }
        let root = self.lib.as_ref().map(|l| l.root().to_path_buf());
        if let Some(root) = &root {
            server::remove_stale(root);
            if root.join(DISCOVERY_FILE).exists() {
                return Err("Agent bridge is already on in another window".into());
            }
        }
        let bridge = Bridge::start(root.as_deref(), &self.doc.name, self.file_path.as_deref(), ctx.clone())?;
        self.bridge.bridge = Some(bridge);
        self.show_toast("Agent bridge on — MCP clients can now edit this document", false);
        Ok(())
    }

    pub fn stop_bridge(&mut self) {
        self.bridge.bridge = None;
    }

    /// Preferences → Allow MCP agents to control this editor (and the pill's Off button).
    pub fn toggle_agent_bridge(&mut self, ctx: &egui::Context) {
        let on = self.bridge.bridge.is_none();
        if on {
            if let Err(e) = self.start_bridge(ctx) {
                self.show_toast(e, true);
                return;
            }
        } else {
            self.stop_bridge();
        }
        if let Some(lib) = &self.lib {
            if let Err(e) = crate::comments::settings::save_flag(lib.root(), FLAG, on) {
                self.show_toast(e, true);
            }
        }
    }

    pub fn poll_bridge(&mut self) {
        // One operation per frame keeps a chatty agent from monopolizing the editor.
        let Some(request) = self.bridge.bridge.as_ref().and_then(Bridge::try_recv) else { return };
        if request.cancelled.load(std::sync::atomic::Ordering::Acquire) {
            return;
        }
        let result = self.bridge_call(&request.name, request.args);
        let _ = request.reply.send(result);
    }

    pub(crate) fn bridge_call(&mut self, name: &str, args: Value) -> Result<Value, String> {
        if self.home.is_some() || self.history.viewing.is_some() {
            return Err("Editor is busy: open an editable document first".into());
        }
        if MUTATING.contains(&name) || engine::EDIT_TOOLS.contains(&name) {
            if self.proto.present.is_some()
                || self.scrub.is_some()
                || self.dialog.is_some()
                || !matches!(self.canvas.inter, crate::canvas::Interaction::None)
                || self.canvas.pen.is_some()
                || self.canvas.builder.is_some()
                || self.canvas.vedit.is_some()
            {
                return Err("Editor is busy: finish the current edit or close the dialog".into());
            }
            crate::canvas::interact::finish_text(self, true);
        }
        match name {
            "desigma_get_document" => self.bridge_document(args),
            "desigma_get_selection" => {
                let _: serde_json::Map<String, Value> = parse(args)?;
                Ok(json!({"nodes": self.selection.iter().filter_map(|id| self.doc.get(*id)).map(engine::summary).collect::<Vec<_>>()}))
            }
            "desigma_select_nodes" => {
                let a: SelectArgs = parse(args)?;
                if a.node_ids.is_empty() || a.node_ids.len() > 500 {
                    return Err("node_ids must list 1..500 layers".into());
                }
                let mut ids: Vec<NodeId> = Vec::new();
                for id in a.node_ids.iter().map(|&i| NodeId(i)) {
                    if self.doc.get(id).is_none_or(|n| n.parent.is_none()) {
                        return Err(format!("node {} not found or is a page", id.0));
                    }
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                }
                let page = self.doc.page_of(ids[0]);
                if ids.iter().any(|id| self.doc.page_of(*id) != page) {
                    return Err("select_nodes: all layers must be on one page".into());
                }
                // the same page switch as Find's "go to result"
                if let Some(pg) = page.filter(|pg| *pg != self.page) {
                    crate::decks::ui::flush_notes(self);
                    self.page = pg;
                    self.canvas.reset_interaction();
                }
                self.selection = ids;
                self.sanitize();
                if a.reveal {
                    self.fit_selection();
                }
                self.changed();
                Ok(json!({"selected": self.selection.len()}))
            }
            "desigma_export_image" => {
                let a: ExportArgs = parse(args)?;
                if a.path.extension().and_then(|e| e.to_str()) != Some("png") || !a.path.is_absolute() {
                    return Err("Export path must be an absolute .png path".into());
                }
                let (png, width, height) = engine::export_image(&AppHost(self), a.node_id, a.scale)?;
                std::fs::write(&a.path, png).map_err(|e| format!("Cannot write PNG: {e}"))?;
                Ok(json!({"path":a.path,"width":width,"height":height}))
            }
            "desigma_export_animation" => {
                let a: desigma_mcp::motion::ExportAnimArgs = parse(args)?;
                let path = std::path::PathBuf::from(&a.path);
                let ext = desigma_mcp::motion::extension(&a.format)?;
                if path.extension().and_then(|e| e.to_str()) != Some(ext) || !path.is_absolute() {
                    return Err(format!("Export path must be an absolute .{ext} path"));
                }
                let (bytes, mut info) = desigma_mcp::motion::export(&AppHost(self), &a)?;
                std::fs::write(&path, &bytes).map_err(|e| format!("Cannot write animation: {e}"))?;
                info["path"] = json!(path);
                info["bytes"] = json!(bytes.len());
                Ok(info)
            }
            "desigma_open_file" => {
                let a: OpenArgs = parse(args)?;
                if self.hist.can_undo() && !a.discard_changes {
                    return Err(UNSAVED.into());
                }
                validate_bridge_open_path(&a.path)?;
                let doc = load_bridge_document(&a.path, a.frames.as_ref()).map_err(|error| engine::import_rejection_guidance(&a.path, error))?;
                match engine::validate_opened_document(&doc) {
                    Ok(()) => {}
                    Err(engine::OpenedDocumentError::LayerLimit(error)) => {
                        drop(doc);
                        return Err(engine::import_rejection_guidance(&a.path, error));
                    }
                    Err(engine::OpenedDocumentError::Invalid(error)) => return Err(error),
                }
                self.replace_document(doc, Some(a.path));
                let mut result = self.bridge_document(json!({}))?;
                if let Some(frames) = a.frames {
                    result["selected_frame_count"] = json!(frames.len());
                    result["selection_note"] = json!("Selected frames are imported with their transitive dependencies, including component frames and used library components, so the number of selected frames can differ from the frames present after import.");
                }
                Ok(result)
            }
            "desigma_save_file" => {
                let a: SaveArgs = parse(args)?;
                if a.path.extension().and_then(|e| e.to_str()) != Some(crate::io::NATIVE_EXT) || !a.path.is_absolute() {
                    return Err("Save path must be an absolute .desigma path".into());
                }
                let bytes = self.doc.to_file_bytes().map_err(|e| e.to_string())?;
                desigma_mcp::engine::write_design(&a.path, &bytes, a.overwrite)?;
                self.show_toast(format!("Saved copy {}", a.path.display()), false);
                Ok(json!({"path":a.path,"saved":true}))
            }
            "desigma_new_document" => {
                let a: NewArgs = parse(args)?;
                if a.name.trim().is_empty() || a.name.len() > 1024 {
                    return Err("name must contain 1..1024 bytes".into());
                }
                if self.hist.can_undo() && !a.discard_changes {
                    return Err(UNSAVED.into());
                }
                let mut d = Document::new(&a.name);
                d.add_page("Page 1");
                self.replace_document(d, None);
                self.bridge_document(json!({}))
            }
            _ => engine::call(&mut AppHost(self), name, args),
        }
    }

    /// `desigma_get_document` plus what only the editor knows. `has_undo` is the editor's undo
    /// stack, not "unsaved": it stays true after the user saves.
    fn bridge_document(&mut self, args: Value) -> Result<Value, String> {
        let mut v = engine::call(&mut AppHost(self), "desigma_get_document", args)?;
        v["session"] = json!("editor");
        v["path"] = json!(self.file_path);
        v["has_undo"] = json!(self.hist.can_undo());
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_absolute_open_path_keeps_error_first_and_adds_generic_guidance() {
        let error = validate_bridge_open_path(std::path::Path::new("relative.fig")).unwrap_err();
        assert!(error.starts_with("Open path must be absolute\n\nImport recovery:"), "{error}");
        assert!(!error.contains("Heaviest public frames:"), "path validation must not try to outline a file: {error}");
    }

    #[test]
    fn full_fig_uses_limited_importer_but_non_fig_keeps_legacy_loader() {
        assert_eq!(bridge_open_route(std::path::Path::new("large.fig"), false), BridgeOpenRoute::LimitedImporter);
        assert_eq!(bridge_open_route(std::path::Path::new("normal.pen"), false), BridgeOpenRoute::LegacyLoader);
        assert_eq!(bridge_open_route(std::path::Path::new("normal.desigma"), false), BridgeOpenRoute::LegacyLoader);
        assert_eq!(bridge_open_route(std::path::Path::new("selected.fig"), true), BridgeOpenRoute::LimitedImporter);
    }

    #[test]
    fn full_fig_bridge_preserves_library_size_rejection_and_adds_guidance() {
        let path = std::env::temp_dir().join(format!("desigma-bridge-size-limit-{}.fig", std::process::id()));
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(desigma_library::MAX_IMPORT_BYTES + 1).unwrap();
        drop(file);

        let error = load_bridge_document(&path, None).map_err(|error| engine::import_rejection_guidance(&path, error)).unwrap_err();
        let original = format!("Cannot import design: Input file exceeds the {} MiB import limit", desigma_library::MAX_IMPORT_BYTES >> 20);
        assert!(error.starts_with(&format!("{original}\n\nImport recovery:")), "{error}");
        std::fs::remove_file(path).ok();
    }
}

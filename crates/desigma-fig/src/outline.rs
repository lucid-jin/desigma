//! What is in a `.fig` file, before it is imported, and how to import only part of it.
//!
//! A design-system file can hold many templates that nobody wants open at once. [`outline_fig`]
//! lists its pages and their top-level frames with a weight for each (nodes and image bytes), and
//! [`ImportOptions::only_frames`] imports just the frames that were picked. The others are skipped
//! whole, and so are the library components and images only they use; what a picked frame needs
//! (the components its instances are of, wherever they are defined) comes along.

use std::collections::BTreeSet;

/// The pages of a `.fig` file and the totals of what it stores.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize)]
pub struct FigOutline {
    pub pages: Vec<OutlinePage>,
    /// Nodes the file stores, before instances expand into copies.
    pub stored_nodes: usize,
    /// Images embedded in the file, and their size in bytes.
    pub image_count: usize,
    pub image_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct OutlinePage {
    /// `session:local` id of the page node.
    pub id: String,
    pub name: String,
    /// The hidden page that holds what the file uses from libraries. Its frames cannot be picked.
    pub internal: bool,
    pub frames: Vec<OutlineFrame>,
}

/// A top-level child of a page.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct OutlineFrame {
    /// `session:local` id; what [`ImportOptions::only_frames`] takes.
    pub id: String,
    pub name: String,
    /// The node type as the file names it (`FRAME`, `SECTION`, `SYMBOL`...).
    pub kind: String,
    /// The frame and everything below it, as stored (an instance counts once, not as its copies).
    pub nodes: usize,
    /// Size of the distinct images the frame's own nodes use.
    pub image_bytes: u64,
}

/// How to import a `.fig` file. The default imports all of it.
#[derive(Debug, Clone, Default)]
pub struct ImportOptions {
    /// Import only these top-level frames (ids from [`OutlinePage::frames`]), plus whatever they
    /// depend on: a frame holding a component an instance of a picked frame is of is imported too,
    /// and so are the library components the picked frames use (not the rest of the library).
    /// Pages with nothing to import are left out, and images no imported node uses are not kept.
    /// `None` imports everything.
    pub only_frames: Option<BTreeSet<String>>,
}

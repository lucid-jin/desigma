//! Convert a binary `.fig` file directly into a native `.desigma` document.
//!
//! The importer builds `Document` nodes incrementally; serialization writes the
//! finished document straight to the destination file without creating a JSON
//! string in memory.

use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let input = args.next().map(PathBuf::from).ok_or("usage: fig2desigma <input.fig> <output.desigma>")?;
    let output = args.next().map(PathBuf::from).ok_or("usage: fig2desigma <input.fig> <output.desigma>")?;
    if args.next().is_some() {
        return Err("usage: fig2desigma <input.fig> <output.desigma>".into());
    }

    let bytes = std::fs::read(&input)?;
    let doc = desigma_fig::import_fig(&bytes)?;
    write_atomic(&output, &doc)?;
    eprintln!("converted {} nodes across {} pages", doc.nodes.len(), doc.pages.len());
    Ok(())
}

fn write_atomic(path: &Path, doc: &desigma_fig::Document) -> Result<(), Box<dyn std::error::Error>> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
    let base = path.file_name().and_then(|name| name.to_str()).unwrap_or("document.desigma");
    let mut temp_file = None;
    for _ in 0..100 {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let count = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(".{base}.{}.{}.{}.tmp", std::process::id(), nonce, count));
        match OpenOptions::new().write(true).create_new(true).open(&candidate) {
            Ok(file) => {
                temp_file = Some((candidate, file));
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    let (temp, file) = temp_file.ok_or("could not create a unique temporary output file")?;

    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let mut writer = BufWriter::new(file);
        serde_json::to_writer_pretty(&mut writer, doc)?;
        writer.write_all(b"\n")?;
        writer.flush()?;
        writer.get_ref().sync_all()?;
        drop(writer);
        std::fs::rename(&temp, path)?;
        OpenOptions::new().read(true).open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

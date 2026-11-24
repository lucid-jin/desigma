//! The file form of a document.
//!
//! A `.desigma` file is JSON. Documents of a normal size stay plain, pretty-printed JSON, which
//! is easy to read and diff. A large document (a design system with a hundred thousand copies
//! of its components) is written as gzip-compressed compact JSON instead: JSON of nodes is
//! highly repetitive and shrinks about 35 times, so a file that would be hundreds of megabytes
//! is a fraction of that, and the file-size limits keep meaning "how big is the content".
//!
//! Reading accepts both forms (a gzip stream starts with `1f 8b`, JSON never does), so every
//! file written by an older version still opens. The unpacked size is capped while reading, so
//! a small file cannot expand into memory without bound.

use crate::{Document, DocumentKind};
use flate2::{read::GzDecoder, write::GzEncoder, Compression};
use serde::de::{self, Deserialize, Deserializer as _, MapAccess, Visitor};
use std::cell::Cell;
use std::io::{self, BufReader, BufWriter, Read, Write};

/// Documents with more nodes than this are written packed.
pub const PACK_ABOVE_NODES: usize = 20_000;

/// Most bytes a packed file may unpack to. Parsing a document costs about twice its JSON text in
/// memory, so this keeps one open under about 2 GiB. A 188,518-node design system unpacks to
/// 630 MB.
pub const MAX_UNPACKED_BYTES: u64 = 1 << 30;

const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];

/// Reads at most `left` bytes and fails, rather than truncating, when there is more.
struct Capped<R> {
    inner: R,
    left: u64,
    limit: u64,
}

impl<R: Read> Read for Capped<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.left == 0 {
            let mut probe = [0u8; 1];
            return match self.inner.read(&mut probe)? {
                0 => Ok(0),
                _ => Err(io::Error::other(format!("the file unpacks to more than {} MiB", self.limit >> 20))),
            };
        }
        let want = buf.len().min(usize::try_from(self.left).unwrap_or(usize::MAX));
        let n = self.inner.read(&mut buf[..want])?;
        self.left -= n as u64;
        Ok(n)
    }
}

fn is_packed(bytes: &[u8]) -> bool {
    bytes.starts_with(&GZIP_MAGIC)
}

fn unpacker(bytes: &[u8], limit: u64) -> BufReader<Capped<GzDecoder<&[u8]>>> {
    BufReader::new(Capped { inner: GzDecoder::new(bytes), left: limit, limit })
}

impl Document {
    /// The bytes to write to a `.desigma` file: pretty JSON, or gzip-compressed compact JSON for
    /// a large document (see the module docs). Streams into the compressor, so no JSON text of
    /// the whole document is ever held.
    pub fn to_file_bytes(&self) -> serde_json::Result<Vec<u8>> {
        let mut out = Vec::new();
        self.write_file_form(&mut out)?;
        Ok(out)
    }

    /// [`Self::to_file_bytes`] straight into a writer (a file), so not even the compressed
    /// bytes are held in memory.
    pub fn write_file_form(&self, w: impl Write) -> serde_json::Result<()> {
        self.write_file_form_above(w, PACK_ABOVE_NODES)
    }

    fn write_file_form_above(&self, mut w: impl Write, pack_above: usize) -> serde_json::Result<()> {
        if self.nodes.len() <= pack_above {
            return serde_json::to_writer_pretty(w, self);
        }
        // serde_json writes many tiny pieces; the buffer hands the compressor large blocks.
        let mut buffered = BufWriter::with_capacity(1 << 16, GzEncoder::new(&mut w, Compression::new(3)));
        serde_json::to_writer(&mut buffered, self)?;
        let gz = buffered.into_inner().map_err(|e| de::Error::custom(e.into_error()))?;
        gz.finish().map(drop).map_err(de::Error::custom)
    }

    #[cfg(test)]
    fn to_file_bytes_above(&self, pack_above: usize) -> serde_json::Result<Vec<u8>> {
        let mut out = Vec::new();
        self.write_file_form_above(&mut out, pack_above)?;
        Ok(out)
    }

    /// Reads the bytes of a `.desigma` file, packed or plain.
    pub fn from_file_bytes(bytes: &[u8]) -> serde_json::Result<Self> {
        Self::from_file_bytes_capped(bytes, MAX_UNPACKED_BYTES)
    }

    fn from_file_bytes_capped(bytes: &[u8], limit: u64) -> serde_json::Result<Self> {
        if !is_packed(bytes) {
            let text = std::str::from_utf8(bytes).map_err(de::Error::custom)?;
            return Self::from_json(text);
        }
        let mut de = serde_json::Deserializer::from_reader(unpacker(bytes, limit));
        let doc = Self::deserialize(&mut de)?;
        de.end()?;
        Ok(doc.finish_load())
    }

    /// The kind (design, board or deck) of a `.desigma` file's bytes, without building the
    /// document. A packed file is unpacked only as far as the `kind` field.
    pub fn file_kind(bytes: &[u8]) -> serde_json::Result<DocumentKind> {
        /// Looks for `kind` among the top-level fields and stops the parse right there.
        struct Find<'a>(&'a Cell<Option<DocumentKind>>);
        impl<'de> Visitor<'de> for Find<'_> {
            type Value = ();
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a document")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
                while let Some(key) = map.next_key::<String>()? {
                    if key == "kind" {
                        self.0.set(Some(map.next_value()?));
                        return Err(de::Error::custom("found `kind`; the rest of the file is not read"));
                    }
                    map.next_value::<de::IgnoredAny>()?;
                }
                Ok(())
            }
        }

        let found = Cell::new(None);
        let result = if is_packed(bytes) {
            serde_json::Deserializer::from_reader(unpacker(bytes, MAX_UNPACKED_BYTES)).deserialize_map(Find(&found))
        } else {
            serde_json::Deserializer::from_slice(bytes).deserialize_map(Find(&found))
        };
        match (found.get(), result) {
            (Some(kind), _) => Ok(kind),
            (None, Ok(())) => Ok(DocumentKind::default()), // no `kind` field: an old file, a design
            (None, Err(e)) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Node;
    use crate::Rect;

    fn doc_with(nodes: usize) -> Document {
        let mut d = Document::new("packed");
        let page = d.add_page("Page 1");
        for i in 0..nodes {
            let id = d.alloc_id();
            let mut n = Node::rectangle(id, Rect::new(i as f32, 0.0, 5.0, 5.0));
            n.name = format!("Rectangle {i}");
            d.insert(page, n).unwrap();
        }
        d
    }

    #[test]
    fn small_documents_stay_plain_pretty_json() {
        let d = doc_with(3);
        let bytes = d.to_file_bytes().unwrap();
        assert!(bytes.starts_with(b"{\n"), "plain JSON, one field per line");
        assert_eq!(bytes, d.to_json().unwrap().into_bytes());
        assert_eq!(Document::from_file_bytes(&bytes).unwrap().nodes.len(), 4);
    }

    #[test]
    fn large_documents_are_packed_and_read_back_the_same() {
        let mut d = doc_with(50);
        d.images.insert("h".into(), vec![9, 8, 7, 255]);
        let plain = d.to_json().unwrap();
        let packed = d.to_file_bytes_above(10).unwrap();
        assert!(packed.starts_with(&GZIP_MAGIC));
        assert!(packed.len() < plain.len() / 2, "{} vs {}", packed.len(), plain.len());
        let back = Document::from_file_bytes(&packed).unwrap();
        assert_eq!(back.to_json().unwrap(), Document::from_json(&plain).unwrap().to_json().unwrap());
        assert_eq!(back.images["h"], vec![9, 8, 7, 255]);
    }

    #[test]
    fn a_file_that_unpacks_past_the_cap_is_an_error_not_a_truncation() {
        let packed = doc_with(50).to_file_bytes_above(10).unwrap();
        let err = Document::from_file_bytes_capped(&packed, 1000).unwrap_err().to_string();
        assert!(err.contains("unpacks to more than"), "{err}");
        // exactly at the size of the JSON is fine
        let size = serde_json::to_vec(&doc_with(50)).unwrap().len() as u64;
        assert!(Document::from_file_bytes_capped(&packed, size).is_ok());
        assert!(Document::from_file_bytes_capped(&packed, size - 1).is_err());
    }

    #[test]
    fn damaged_packed_files_are_errors() {
        let mut packed = doc_with(50).to_file_bytes_above(10).unwrap();
        let half = packed.len() / 2;
        packed.truncate(half);
        assert!(Document::from_file_bytes(&packed).is_err());
        assert!(Document::from_file_bytes(&[0x1f, 0x8b, 0, 0]).is_err());
        assert!(Document::from_file_bytes(b"not json").is_err());
    }

    #[test]
    fn file_kind_reads_the_kind_of_plain_and_packed_files() {
        for kind in [DocumentKind::Design, DocumentKind::Board, DocumentKind::Deck] {
            let mut d = doc_with(30);
            d.kind = kind;
            assert_eq!(Document::file_kind(&d.to_file_bytes().unwrap()).unwrap(), kind);
            assert_eq!(Document::file_kind(&d.to_file_bytes_above(10).unwrap()).unwrap(), kind);
        }
        assert_eq!(Document::file_kind(b"{\"name\": \"old file\"}").unwrap(), DocumentKind::Design);
        assert!(Document::file_kind(b"[1,2]").is_err());
    }
}

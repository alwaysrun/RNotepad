//! Document model: raw bytes plus the interpretation layers shared by all
//! views.
//!
//! A document is either editable (small, decodable) or large/read-only.  Both
//! kinds expose the same raw byte stream, line index and coordinate map; the
//! text and hex views are two ways of looking at the same `Document`.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use memmap2::Mmap;
use thiserror::Error;

use crate::bytes::{ByteError, ByteSource, RawBytes};
use crate::coord::{AnchoredCoordMap, CoordMap, RawByteOffset, Utf8Pos};
use crate::encoding::{
    decode, detect, BomPolicy, Charset, DecodeError, DetectionSource, EncodingInfo,
};
use crate::eol::{detect as detect_eol, EolDetection, EolWritePolicy};
use crate::line_index::{LineIndex, PRIME_BYTES};

/// Unique identifier of an open document.
pub type DocumentId = u64;

/// Maximum size of an editable document, in bytes.
pub const EDITABLE_SIZE_LIMIT: u64 = 16 * 1024 * 1024;
/// Bytes sampled for charset, EOL and text plausibility detection.
pub const DETECTION_SAMPLE_BYTES: u64 = 1 << 20;
/// Upper bound for characters that fail to decode before the document is
/// considered binary.
const MAX_DECODE_ERROR_RATIO: f64 = 0.10;
/// Upper bound for replacement characters in the decoded sample.
const MAX_REPLACEMENT_RATIO: f64 = 0.05;
/// Upper bound for non-whitespace control characters.
const MAX_CONTROL_RATIO: f64 = 0.05;
const REPLACEMENT_CHAR: char = '\u{FFFD}';
const LOG_EXTENSIONS: [&str; 4] = ["log", "out", "trace", "dmp"];

/// How the document may be written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentKind {
    Editable,
    LargeReadOnly,
}

impl DocumentKind {
    /// `true` when the document accepts editing commands.
    #[must_use]
    pub const fn is_editable(self) -> bool {
        matches!(self, Self::Editable)
    }
}

/// Which view currently displays the document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ViewMode {
    Text,
    Hex,
}

/// Encoding / BOM / EOL applied when the document is saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WritePolicy {
    pub charset: Charset,
    pub bom: BomPolicy,
    pub eol: EolWritePolicy,
}

impl WritePolicy {
    #[must_use]
    pub fn from_encoding(info: &EncodingInfo) -> Self {
        Self {
            charset: info.charset,
            bom: info.policy,
            eol: EolWritePolicy::Preserve,
        }
    }
}

/// Byte lengths of the parts of the file that produced decode errors.
#[derive(Debug, Error)]
pub enum DocumentError {
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("byte source error: {0}")]
    Bytes(#[from] ByteError),
    #[error("path has no parent directory: {0}")]
    NoParent(PathBuf),
}

/// Options accepted by [`Document::open`].
#[derive(Debug, Clone, Copy)]
pub struct OpenOptions {
    /// Treat the file as read-only regardless of its size.
    pub force_large: bool,
    /// Overrides [`EDITABLE_SIZE_LIMIT`].
    pub editable_limit: u64,
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self {
            force_large: false,
            editable_limit: EDITABLE_SIZE_LIMIT,
        }
    }
}

/// An open document.
pub struct Document {
    pub id: DocumentId,
    pub path: Option<PathBuf>,
    pub raw: RawBytes,
    pub kind: DocumentKind,
    pub view: ViewMode,
    pub text_available: bool,
    pub encoding: EncodingInfo,
    pub eol: EolDetection,
    pub write_policy: WritePolicy,
    pub line_index: Arc<LineIndex>,
    pub coord: AnchoredCoordMap,
    pub dirty: bool,
    pub revision: u64,
    /// Raw offsets of bytes that could not be decoded.
    pub decode_errors: Vec<DecodeError>,
    /// Confidence reported by charset detection, in `0.0..=1.0`.
    pub confidence: f32,
    /// How [`Document::encoding`] was determined.
    pub detection_source: DetectionSource,
    /// File size recorded when the document was opened or last saved.
    pub opened_size: u64,
    /// Modification time recorded when the document was opened or last saved.
    pub opened_mtime: Option<std::time::SystemTime>,
}

impl Document {
    /// Opens `path` and builds the interpretation layers.
    ///
    /// Charset detection and the text/binary decision use a bounded sample;
    /// the line index is primed with the first megabyte so the first screen
    /// can be drawn immediately.
    pub fn open(path: impl AsRef<Path>, options: OpenOptions) -> Result<Self, DocumentError> {
        let path = path.as_ref().to_path_buf();
        let metadata = std::fs::metadata(&path)?;
        if !metadata.is_file() {
            return Err(DocumentError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "not a regular file",
            )));
        }
        let size = metadata.len();
        let opened_mtime = metadata.modified().ok();
        let is_log = has_log_extension(&path);
        let force_large = options.force_large || is_log || size > options.editable_limit;

        let raw = if force_large {
            let file = File::open(&path)?;
            // SAFETY: the mapping is read-only and stays alive through the
            // Arc<Mmap> stored in RawBytes; the file is not modified while
            // mapped, and atomic save drops the mapping before replacing it.
            let mmap = unsafe { Mmap::map(&file)? };
            RawBytes::from_mmap(mmap)
        } else {
            let mut bytes = Vec::with_capacity(size as usize);
            File::open(&path)?.read_to_end(&mut bytes)?;
            RawBytes::from_vec(bytes)
        };

        let sample_len = usize::try_from(size.min(DETECTION_SAMPLE_BYTES)).unwrap_or(0);
        let sample = raw
            .slice(0..sample_len as u64)
            .unwrap_or_default();
        let detection = detect(sample);
        let bom = detection.bom;
        let bom_len = bom.len();
        let charset = detection.charset;

        let decoded_sample = decode(sample.get(bom_len as usize..).unwrap_or_default(), charset, 0);
        let text_available = text_is_plausible(&decoded_sample.text, decoded_sample.errors.len());

        let kind = if text_available && !force_large && size <= options.editable_limit {
            DocumentKind::Editable
        } else {
            DocumentKind::LargeReadOnly
        };

        let eol = if size <= DETECTION_SAMPLE_BYTES {
            detect_eol(
                raw.slice(0..size).unwrap_or_default(),
                charset,
                bom_len as usize,
            )
        } else {
            detect_eol(sample, charset, bom_len as usize)
        };

        let line_index = Arc::new(LineIndex::new(raw.clone(), charset, bom_len));
        // Prime the index so the first screen has lines available.
        line_index.scan_step(PRIME_BYTES);

        let coord = AnchoredCoordMap::new(raw.clone(), charset, bom_len, line_index.clone());

        let encoding = EncodingInfo::from_detection(&detection);
        let write_policy = WritePolicy::from_encoding(&encoding);

        let mut doc = Self {
            id: 0,
            path: Some(path),
            raw,
            kind,
            view: if text_available {
                ViewMode::Text
            } else {
                ViewMode::Hex
            },
            text_available,
            encoding,
            eol,
            write_policy,
            line_index,
            coord,
            dirty: false,
            revision: 1,
            decode_errors: decoded_sample.errors,
            confidence: detection.confidence,
            detection_source: detection.source,
            opened_size: size,
            opened_mtime,
        };
        doc.validate_coord_capability();
        Ok(doc)
    }

    /// Creates an in-memory document.  Used by tests and by "new file" flows.
    #[must_use]
    pub fn from_bytes(id: DocumentId, bytes: Vec<u8>) -> Self {
        let raw = RawBytes::from_vec(bytes);
        let size = raw.len();
        let sample = raw.slice(0..size.min(DETECTION_SAMPLE_BYTES)).unwrap_or_default();
        let detection = detect(sample);
        let bom = detection.bom;
        let bom_len = bom.len();
        let charset = detection.charset;
        let decoded = decode(sample.get(bom_len as usize..).unwrap_or_default(), charset, 0);
        let text_available = text_is_plausible(&decoded.text, decoded.errors.len());
        let eol = detect_eol(sample, charset, bom_len as usize);
        let line_index = Arc::new(LineIndex::new(raw.clone(), charset, bom_len));
        line_index.scan_step(PRIME_BYTES);
        let coord = AnchoredCoordMap::new(raw.clone(), charset, bom_len, line_index.clone());
        let encoding = EncodingInfo::from_detection(&detection);
        Self {
            id,
            path: None,
            raw,
            kind: if text_available {
                DocumentKind::Editable
            } else {
                DocumentKind::LargeReadOnly
            },
            view: if text_available {
                ViewMode::Text
            } else {
                ViewMode::Hex
            },
            text_available,
            encoding,
            eol,
            write_policy: WritePolicy::from_encoding(&encoding),
            line_index,
            coord,
            dirty: false,
            revision: 1,
            decode_errors: decoded.errors,
            confidence: detection.confidence,
            detection_source: detection.source,
            opened_size: size,
            opened_mtime: None,
        }
    }

    /// Re-reads the document from disk, keeping its identifier and view mode.
    ///
    /// Used after an atomic save and when an external change is accepted.
    /// The in-memory byte source, line index and coordinate map are rebuilt,
    /// which is cheap for editable documents.
    pub fn reload(&mut self) -> Result<(), DocumentError> {
        let Some(path) = self.path.clone() else {
            return Err(DocumentError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "document has no backing file",
            )));
        };
        let id = self.id;
        let view = self.view;
        let mut fresh = Self::open(&path, OpenOptions::default())?;
        fresh.id = id;
        fresh.view = if fresh.text_available {
            view
        } else {
            ViewMode::Hex
        };
        *self = fresh;
        Ok(())
    }

    /// Re-decodes the existing bytes with a different charset.
    ///
    /// The raw source is unchanged: only the interpretation layers (EOL
    /// statistics, line index, coordinate map and write policy) are rebuilt.
    /// This implements the "reinterpret encoding" command.
    pub fn reinterpret(&mut self, charset: Charset) -> Result<(), DocumentError> {
        let bom_len = self.encoding.bom.len();
        let sample_len = self.raw.len().min(DETECTION_SAMPLE_BYTES);
        let sample = self.raw.slice(0..sample_len).unwrap_or_default();
        let decoded = decode(
            sample.get(bom_len as usize..).unwrap_or_default(),
            charset,
            0,
        );
        self.text_available = text_is_plausible(&decoded.text, decoded.errors.len());
        self.eol = detect_eol(sample, charset, bom_len as usize);
        self.encoding = EncodingInfo {
            charset,
            bom: self.encoding.bom,
            policy: self.write_policy.bom,
        };
        self.write_policy = WritePolicy::from_encoding(&self.encoding);
        self.line_index = Arc::new(LineIndex::new(self.raw.clone(), charset, bom_len));
        self.line_index.scan_step(PRIME_BYTES);
        self.coord = AnchoredCoordMap::new(
            self.raw.clone(),
            charset,
            bom_len,
            self.line_index.clone(),
        );
        self.decode_errors = decoded.errors;
        self.revision = self.revision.saturating_add(1);
        if !self.text_available {
            self.view = ViewMode::Hex;
        }
        Ok(())
    }

    /// Marks the document as modified and returns the new revision.
    pub fn mark_dirty(&mut self) -> u64 {
        if self.kind.is_editable() {
            self.dirty = true;
        }
        self.bump_revision()
    }

    /// Increments the revision counter.
    pub fn bump_revision(&mut self) -> u64 {
        self.revision = self.revision.saturating_add(1);
        self.revision
    }

    /// Total byte size.
    #[must_use]
    pub fn size(&self) -> u64 {
        self.raw.len()
    }

    /// Number of decoded characters in the whole document, when cheap to
    /// compute.  Only UTF-8 documents can answer without decoding.
    #[must_use]
    pub fn approx_chars(&self) -> Option<u64> {
        if self.encoding.charset.is_utf8() {
            Some(self.raw.len().saturating_sub(self.encoding.bom.len()))
        } else {
            None
        }
    }

    /// Decodes the whole document.  Intended for small editable documents
    /// only.
    pub fn decoded_text(&self) -> Option<String> {
        if self.raw.len() > EDITABLE_SIZE_LIMIT {
            return None;
        }
        let bom_len = self.encoding.bom.len() as usize;
        let bytes = self.raw.slice(0..self.raw.len())?;
        Some(decode(
            bytes.get(bom_len..).unwrap_or_default(),
            self.encoding.charset,
            0,
        )
        .text)
    }

    /// Converts a decoded UTF-8 position to a raw offset.
    #[must_use]
    pub fn utf8_to_raw(&self, pos: Utf8Pos) -> Option<RawByteOffset> {
        self.coord.utf8_to_raw(pos)
    }

    fn validate_coord_capability(&mut self) {
        // Coordinate mapping is part of the document contract; a failure here
        // would mean the charset codec and the index disagree.  Logging is
        // handled by the application layer, so only a debug assertion is
        // needed in core.
        debug_assert!(
            self.coord
                .raw_to_utf8(RawByteOffset(self.encoding.bom.len()))
                .is_some(),
            "coordinate map must accept the first text byte"
        );
    }
}

fn has_log_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            let lower = ext.to_ascii_lowercase();
            LOG_EXTENSIONS.contains(&lower.as_str())
        })
}

fn text_is_plausible(text: &str, decode_errors: usize) -> bool {
    if text.is_empty() {
        return true;
    }
    let chars = text.chars().count().max(1);
    let mut controls = 0usize;
    let mut replacements = 0usize;
    for ch in text.chars() {
        if ch == REPLACEMENT_CHAR {
            replacements += 1;
        } else if ch.is_control() && !matches!(ch, '\t' | '\n' | '\r' | '\u{000B}' | '\u{000C}') {
            controls += 1;
        }
    }
    let control_ratio = controls as f64 / chars as f64;
    let replacement_ratio = replacements as f64 / chars as f64;
    let error_ratio = decode_errors as f64 / chars as f64;
    control_ratio <= MAX_CONTROL_RATIO
        && replacement_ratio <= MAX_REPLACEMENT_RATIO
        && error_ratio <= MAX_DECODE_ERROR_RATIO
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoding::BomState;

    #[test]
    fn small_text_is_editable() {
        let doc = Document::from_bytes(1, b"hello\nworld".to_vec());
        assert_eq!(doc.kind, DocumentKind::Editable);
        assert_eq!(doc.view, ViewMode::Text);
        assert!(doc.text_available);
    }

    #[test]
    fn control_heavy_bytes_are_binary() {
        let mut bytes = Vec::new();
        for i in 0..1000 {
            bytes.push((i % 0x1F) as u8);
        }
        let doc = Document::from_bytes(2, bytes);
        assert!(!doc.text_available);
        assert_eq!(doc.kind, DocumentKind::LargeReadOnly);
        assert_eq!(doc.view, ViewMode::Hex);
    }

    #[test]
    fn bom_is_reflected_in_encoding() {
        let doc = Document::from_bytes(3, b"\xEF\xBB\xBFhi".to_vec());
        assert_eq!(doc.encoding.bom, BomState::Utf8);
        assert_eq!(doc.raw.len(), 5);
    }

    #[test]
    fn log_extension_forces_read_only() {
        assert!(has_log_extension(Path::new("app.LOG")));
        assert!(!has_log_extension(Path::new("notes.txt")));
    }
}

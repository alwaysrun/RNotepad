//! Atomic save for editable text documents.
//!
//! Saving is allowed only for `Editable + Text`.  The byte stream is built
//! from the editor's UTF-8 buffer with the document's write policy (charset,
//! BOM, EOL), written to a temporary file in the same directory and then
//! replaced with `MoveFileExW`, matching the specification's atomicity rule.

use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use rnotepad_core::document::{Document, DocumentKind, ViewMode};
use rnotepad_core::encoding::{encode, BomPolicy, BomState, Charset, UnmappableChar};
use rnotepad_core::eol::EolKind;
use thiserror::Error;
use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH};

const TEMP_SUFFIX: &str = "rnotepad.tmp";
/// Distinguishes temporary files written by this process.
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const BACKUP_SUFFIX: &str = ".bak";
const BOM_UTF8: BomState = BomState::Utf8;

/// Save request produced by the application.
#[derive(Debug, Clone, Copy)]
pub struct SaveRequest<'a> {
    /// Current editor content in UTF-8.
    pub text: &'a str,
    /// New path chosen by "Save As"; `None` reuses the document path.
    pub target: Option<&'a Path>,
    /// Write a `.bak` copy before replacing the file.
    pub backup: bool,
    /// Overwrite even when the file changed on disk since it was opened.
    pub allow_external_overwrite: bool,
}

/// Why a save attempt failed.
#[derive(Debug, Error)]
pub enum SaveError {
    #[error("the document is not editable")]
    NotEditable,
    #[error("saving is disabled while the hex view is active")]
    HexView,
    #[error("the document has no target path")]
    NoPath,
    #[error("the file changed on disk since it was opened")]
    ExternalModified,
    #[error("{count} characters cannot be encoded with the target charset")]
    Unmappable { count: usize },
    #[error("i/o error: {0}")]
    Io(#[from] io::Error),
    #[error("document reload failed: {0}")]
    Reload(#[from] rnotepad_core::document::DocumentError),
}

/// Saves the editor buffer and reloads the document from disk.
pub fn save_document(doc: &mut Document, request: &SaveRequest<'_>) -> Result<(), SaveError> {
    if !doc.kind.is_editable() || doc.kind == DocumentKind::LargeReadOnly {
        return Err(SaveError::NotEditable);
    }
    if doc.view == ViewMode::Hex {
        return Err(SaveError::HexView);
    }
    let target = request
        .target
        .map(Path::to_path_buf)
        .or_else(|| doc.path.clone())
        .ok_or(SaveError::NoPath)?;

    // "Save As" targets a file this document never loaded, so its metadata
    // says nothing about a newer revision.  The file dialog already asks
    // before an existing file is replaced.
    let same_path = doc.path.as_deref() == Some(target.as_path());
    if !request.allow_external_overwrite && same_path && externally_modified(doc, &target) {
        return Err(SaveError::ExternalModified);
    }

    let eol = doc.write_policy.eol.resolve(doc.eol.dominant);
    let bytes = build_bytes(request.text, doc, eol, doc.write_policy.bom)?;
    write_atomically(&target, &bytes, request.backup)?;

    doc.path = Some(target);
    doc.reload()?;
    doc.dirty = false;
    Ok(())
}

/// Builds the exact byte stream to write.
pub fn build_bytes(
    text: &str,
    doc: &Document,
    eol: EolKind,
    bom_policy: BomPolicy,
) -> Result<Vec<u8>, SaveError> {
    let charset = doc.write_policy.charset;
    let normalized = normalize_eol(text, eol);
    let body = encode(&normalized, charset).map_err(|chars| SaveError::Unmappable {
        count: chars.len(),
    })?;
    let bom = resolve_bom(bom_policy, doc.encoding.bom, charset);
    let mut bytes = Vec::with_capacity(bom.len() as usize + body.len());
    bytes.extend_from_slice(bom.bytes());
    bytes.extend_from_slice(&body);
    Ok(bytes)
}

/// Rewrites every newline sequence as `kind`.
#[must_use]
pub fn normalize_eol(text: &str, kind: EolKind) -> String {
    let lf = text.replace("\r\n", "\n").replace('\r', "\n");
    match kind {
        EolKind::Lf => lf,
        EolKind::CrLf => lf.replace('\n', "\r\n"),
        EolKind::Cr => lf.replace('\n', "\r"),
    }
}

/// Resolves the BOM state for the target charset.
#[must_use]
pub fn resolve_bom(policy: BomPolicy, current: BomState, charset: Charset) -> BomState {
    match policy {
        BomPolicy::Keep => {
            if current.charset() == Some(charset) {
                current
            } else {
                BomState::None
            }
        }
        BomPolicy::Add => match charset {
            Charset::Utf8 => BOM_UTF8,
            Charset::Utf16Le => BomState::Utf16Le,
            Charset::Utf16Be => BomState::Utf16Be,
            _ => BomState::None,
        },
        BomPolicy::Strip => BomState::None,
    }
}

fn externally_modified(doc: &Document, target: &Path) -> bool {
    if !target.exists() {
        return false;
    }
    let Ok(metadata) = std::fs::metadata(target) else {
        return true;
    };
    metadata.len() != doc.opened_size || metadata.modified().ok() != doc.opened_mtime
}

fn write_atomically(target: &Path, bytes: &[u8], backup: bool) -> Result<(), SaveError> {
    let temp = temp_path(target);
    std::fs::write(&temp, bytes)?;
    if backup && target.exists() {
        let backup_path = backup_path(target);
        std::fs::copy(target, &backup_path)?;
    }
    let wide_target = wide_path(target);
    let wide_temp = wide_path(&temp);
    // SAFETY: both pointers reference buffers that outlive the call.
    let moved = unsafe {
        MoveFileExW(
            PCWSTR(wide_temp.as_ptr()),
            PCWSTR(wide_target.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved.is_err() {
        let _ = std::fs::remove_file(&temp);
        return Err(SaveError::Io(io::Error::last_os_error()));
    }
    Ok(())
}

fn temp_path(target: &Path) -> PathBuf {
    // A process-unique suffix keeps two instances saving the same file from
    // sharing (and deleting) each other's temporary file.
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let mut name = target
        .file_name()
        .map(|value| value.to_os_string())
        .unwrap_or_default();
    name.push(format!(".{}.{sequence}.", std::process::id()));
    name.push(TEMP_SUFFIX);
    target.with_file_name(name)
}

fn backup_path(target: &Path) -> PathBuf {
    let mut name = target
        .file_name()
        .map(|value| value.to_os_string())
        .unwrap_or_default();
    name.push(BACKUP_SUFFIX);
    target.with_file_name(name)
}

fn wide_path(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// Human-readable list of unmappable characters, used by pre-flight checks.
#[must_use]
pub fn describe_unmappable(chars: &[UnmappableChar]) -> String {
    chars
        .iter()
        .take(5)
        .map(|entry| format!("{} at byte {}", entry.ch, entry.utf8_pos))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eol_normalisation_is_idempotent() {
        let text = "a\r\nb\nc\rd";
        assert_eq!(normalize_eol(text, EolKind::Lf), "a\nb\nc\nd");
        assert_eq!(normalize_eol(text, EolKind::CrLf), "a\r\nb\r\nc\r\nd");
        assert_eq!(normalize_eol(text, EolKind::Cr), "a\rb\rc\rd");
    }

    #[test]
    fn bom_keep_drops_mismatched_bom() {
        assert_eq!(
            resolve_bom(BomPolicy::Keep, BomState::Utf8, Charset::Gbk),
            BomState::None
        );
        assert_eq!(
            resolve_bom(BomPolicy::Keep, BomState::Utf8, Charset::Utf8),
            BomState::Utf8
        );
    }

    #[test]
    fn build_bytes_preserves_large_integers_as_text() {
        let doc = Document::from_bytes(1, b"{\"n\": 12345678901234567890}".to_vec());
        let bytes = build_bytes("{\"n\": 1}", &doc, EolKind::Lf, BomPolicy::Keep).unwrap();
        assert_eq!(bytes, b"{\"n\": 1}");
    }
}

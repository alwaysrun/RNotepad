//! Raw byte storage and the [`ByteSource`] abstraction.
//!
//! `RawBytes` is the single source of truth for a document's bytes.  It is
//! either an immutable memory map (large / read-only documents) or an owned
//! byte vector (small editable documents).  Both variants are shared behind
//! `Arc`, so background indexers and viewers can hold cheap clones.

use std::ops::Range;
use std::sync::Arc;

use memmap2::Mmap;
use thiserror::Error;

/// Errors produced when reading raw bytes.
#[derive(Debug, Error)]
pub enum ByteError {
    #[error("read offset {offset} is outside the byte source (len={len})")]
    OutOfRange { offset: u64, len: u64 },
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// A read-only random-access byte source.
///
/// Implementors must be `Send + Sync`: indexing and searching run on worker
/// threads while the UI keeps reading the same source.
pub trait ByteSource: Send + Sync {
    /// Total number of bytes.
    fn len(&self) -> u64;

    /// Reads at most `dst.len()` bytes starting at `off`.
    ///
    /// Returns the number of bytes actually copied.  Reading zero bytes past
    /// the end is valid; reading at an offset beyond the end is an error.
    fn read_at(&self, off: u64, dst: &mut [u8]) -> Result<usize, ByteError>;

    /// Returns the requested raw slice when the backing storage can provide it
    /// without copying.
    fn slice(&self, range: Range<u64>) -> Option<&[u8]>;
}

impl<T: ByteSource + ?Sized> ByteSource for Arc<T> {
    fn len(&self) -> u64 {
        self.as_ref().len()
    }

    fn read_at(&self, off: u64, dst: &mut [u8]) -> Result<usize, ByteError> {
        self.as_ref().read_at(off, dst)
    }

    fn slice(&self, range: Range<u64>) -> Option<&[u8]> {
        self.as_ref().slice(range)
    }
}

/// Document bytes: either a memory map or an owned vector.
#[derive(Clone)]
pub enum RawBytes {
    Mmap(Arc<Mmap>),
    Owned(Arc<Vec<u8>>),
}

impl RawBytes {
    #[must_use]
    pub fn from_mmap(mmap: Mmap) -> Self {
        Self::Mmap(Arc::new(mmap))
    }

    #[must_use]
    pub fn from_vec(bytes: Vec<u8>) -> Self {
        Self::Owned(Arc::new(bytes))
    }

}

impl ByteSource for RawBytes {
    fn len(&self) -> u64 {
        match self {
            Self::Mmap(bytes) => bytes.len() as u64,
            Self::Owned(bytes) => bytes.len() as u64,
        }
    }

    fn read_at(&self, off: u64, dst: &mut [u8]) -> Result<usize, ByteError> {
        let len = self.len();
        if off > len {
            return Err(ByteError::OutOfRange { offset: off, len });
        }
        let start = usize::try_from(off).map_err(|_| ByteError::OutOfRange { offset: off, len })?;
        let bytes = match self {
            Self::Mmap(bytes) => &bytes[start..],
            Self::Owned(bytes) => &bytes[start..],
        };
        let n = dst.len().min(bytes.len());
        dst[..n].copy_from_slice(&bytes[..n]);
        Ok(n)
    }

    fn slice(&self, range: Range<u64>) -> Option<&[u8]> {
        let start = usize::try_from(range.start).ok()?;
        let end = usize::try_from(range.end).ok()?;
        let bytes: &[u8] = match self {
            Self::Mmap(bytes) => bytes,
            Self::Owned(bytes) => bytes,
        };
        bytes.get(start..end)
    }
}

/// Reads a bounded window from a source, copying only what exists.
pub fn read_window(
    source: &dyn ByteSource,
    range: Range<u64>,
    buf: &mut Vec<u8>,
) -> Result<(), ByteError> {
    buf.clear();
    let len = source.len();
    if range.start >= len {
        return Ok(());
    }
    let end = range.end.min(len);
    let size = usize::try_from(end - range.start)
        .map_err(|_| ByteError::OutOfRange { offset: range.start, len })?;
    buf.resize(size, 0);
    let mut read = 0usize;
    while read < size {
        let off = range.start + read as u64;
        let n = source.read_at(off, &mut buf[read..])?;
        if n == 0 {
            buf.truncate(read);
            break;
        }
        read += n;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owned_reads_clamp_at_end() {
        let bytes = RawBytes::from_vec(vec![1, 2, 3, 4]);
        let mut dst = [0u8; 5];
        assert_eq!(bytes.read_at(2, &mut dst).unwrap(), 2);
        assert_eq!(&dst[..2], &[3, 4]);
        assert_eq!(bytes.read_at(4, &mut dst).unwrap(), 0);
        assert!(bytes.read_at(5, &mut dst).is_err());
    }

    #[test]
    fn window_reads_are_bounded() {
        let bytes = RawBytes::from_vec(vec![1, 2, 3, 4]);
        let mut buf = Vec::new();
        read_window(&bytes, 1..9, &mut buf).unwrap();
        assert_eq!(buf, vec![2, 3, 4]);
        read_window(&bytes, 4..9, &mut buf).unwrap();
        assert!(buf.is_empty());
    }
}

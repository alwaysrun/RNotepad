//! End-of-line detection and write policy.

use crate::encoding::Charset;

/// One of the three logical newline conventions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum EolKind {
    #[default]
    CrLf,
    Lf,
    Cr,
}

impl EolKind {
    /// Display name used by the status bar.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::CrLf => "CRLF",
            Self::Lf => "LF",
            Self::Cr => "CR",
        }
    }

    /// Raw bytes of the newline in the given charset (BOM excluded).
    #[must_use]
    pub fn bytes(self, charset: Charset) -> Vec<u8> {
        let (cr, lf) = match charset {
            Charset::Utf16Le => (
                vec![0x0D, 0x00],
                vec![0x0A, 0x00],
            ),
            Charset::Utf16Be => (
                vec![0x00, 0x0D],
                vec![0x00, 0x0A],
            ),
            _ => (vec![0x0D], vec![0x0A]),
        };
        match self {
            Self::CrLf => cr.into_iter().chain(lf).collect(),
            Self::Lf => lf,
            Self::Cr => cr,
        }
    }
}

/// Policy applied to newlines when the file is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum EolWritePolicy {
    /// Keep the dominant detected convention.
    #[default]
    Preserve,
    CrLf,
    Lf,
    Cr,
}

impl EolWritePolicy {
    /// Stable display name used by menus and the status bar.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Preserve => "Preserve",
            Self::CrLf => "CRLF",
            Self::Lf => "LF",
            Self::Cr => "CR",
        }
    }

    /// Converts the policy to a concrete newline kind, using `detected` when
    /// the policy is [`EolWritePolicy::Preserve`].
    #[must_use]
    pub fn resolve(self, detected: EolKind) -> EolKind {
        match self {
            Self::Preserve => detected,
            Self::CrLf => EolKind::CrLf,
            Self::Lf => EolKind::Lf,
            Self::Cr => EolKind::Cr,
        }
    }
}

/// Statistics of the newline conventions found in a document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EolDetection {
    pub crlf: u64,
    pub lf: u64,
    pub cr: u64,
    pub dominant: EolKind,
    pub mixed: bool,
}

impl EolDetection {
    /// Number of detected line breaks.
    #[must_use]
    pub fn total(&self) -> u64 {
        self.crlf + self.lf + self.cr
    }

    /// Builds the detection result from the three counters.
    #[must_use]
    pub fn from_counts(crlf: u64, lf: u64, cr: u64) -> Self {
        let non_zero = usize::from(crlf > 0) + usize::from(lf > 0) + usize::from(cr > 0);
        let dominant = if crlf >= lf && crlf >= cr {
            EolKind::CrLf
        } else if lf >= cr {
            EolKind::Lf
        } else {
            EolKind::Cr
        };
        Self {
            crlf,
            lf,
            cr,
            dominant,
            mixed: non_zero > 1,
        }
    }
}

/// Counts newline sequences in `raw`.
///
/// UTF-16 input is scanned in aligned 2-byte units; `align` is the BOM length
/// (0 or 2), which keeps `0A 00` from being mistaken for an unrelated pair.
#[must_use]
pub fn detect(raw: &[u8], charset: Charset, align: usize) -> EolDetection {
    let mut crlf = 0u64;
    let mut lf = 0u64;
    let mut cr = 0u64;
    let mut i = align;

    if charset.is_utf16() {
        let little = charset == Charset::Utf16Le;
        while i + 1 < raw.len() {
            let first = read_unit(raw, i, little);
            if first == 0x000D {
                if i + 3 < raw.len() && read_unit(raw, i + 2, little) == 0x000A {
                    crlf += 1;
                    i += 4;
                    continue;
                }
                cr += 1;
            } else if first == 0x000A {
                lf += 1;
            }
            i += 2;
        }
    } else {
        while i < raw.len() {
            match raw[i] {
                0x0D => {
                    if i + 1 < raw.len() && raw[i + 1] == 0x0A {
                        crlf += 1;
                        i += 2;
                        continue;
                    }
                    cr += 1;
                }
                0x0A => lf += 1,
                _ => {}
            }
            i += 1;
        }
    }
    EolDetection::from_counts(crlf, lf, cr)
}

fn read_unit(raw: &[u8], pos: usize, little_endian: bool) -> u16 {
    if little_endian {
        u16::from_le_bytes([raw[pos], raw[pos + 1]])
    } else {
        u16::from_be_bytes([raw[pos], raw[pos + 1]])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_mixed_newlines() {
        let detection = detect(b"a\r\nb\nc\rd", Charset::Utf8, 0);
        assert_eq!(detection.crlf, 1);
        assert_eq!(detection.lf, 1);
        assert_eq!(detection.cr, 1);
        assert!(detection.mixed);
        assert_eq!(detection.dominant, EolKind::CrLf);
    }

    #[test]
    fn dominant_is_lf_for_lf_only() {
        let detection = detect(b"a\nb\nc", Charset::Utf8, 0);
        assert_eq!(detection.dominant, EolKind::Lf);
        assert!(!detection.mixed);
    }

    #[test]
    fn utf16_crlf_is_not_double_counted() {
        let raw = [0x41, 0x00, 0x0D, 0x00, 0x0A, 0x00, 0x42, 0x00];
        let detection = detect(&raw, Charset::Utf16Le, 0);
        assert_eq!(detection.crlf, 1);
        assert_eq!(detection.cr, 0);
        assert_eq!(detection.lf, 0);
    }

    #[test]
    fn utf16_bom_is_skipped() {
        let raw = [0xFF, 0xFE, 0x41, 0x00, 0x0A, 0x00];
        let detection = detect(&raw, Charset::Utf16Le, 2);
        assert_eq!(detection.lf, 1);
        assert_eq!(detection.crlf, 0);
    }
}

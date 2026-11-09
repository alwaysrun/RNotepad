//! Character-set detection, decoding and encoding.
//!
//! The module is deliberately independent of the UI.  It works on raw byte
//! slices and produces a decoded UTF-8 string plus the raw offsets of bytes
//! that could not be decoded, so callers can jump from a corrupted character
//! to the exact position in the hex view.

use encoding_rs::Encoding;
use thiserror::Error;

/// All character sets supported by the editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Charset {
    Utf8,
    Utf16Le,
    Utf16Be,
    Gb18030,
    Gbk,
    Big5,
    ShiftJis,
    Windows1252,
    Latin1,
}

impl Charset {
    /// Every variant, in the order shown by the encoding menu.
    pub const ALL: [Self; 9] = [
        Self::Utf8,
        Self::Utf16Le,
        Self::Utf16Be,
        Self::Gb18030,
        Self::Gbk,
        Self::Big5,
        Self::ShiftJis,
        Self::Windows1252,
        Self::Latin1,
    ];

    /// Stable display name used by the status bar and menus.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Utf8 => "UTF-8",
            Self::Utf16Le => "UTF-16 LE",
            Self::Utf16Be => "UTF-16 BE",
            Self::Gb18030 => "GB18030",
            Self::Gbk => "GBK",
            Self::Big5 => "Big5",
            Self::ShiftJis => "Shift-JIS",
            Self::Windows1252 => "Windows-1252",
            Self::Latin1 => "Latin-1",
        }
    }

    /// Parses a name produced by [`Charset::display_name`].
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|charset| charset.display_name().eq_ignore_ascii_case(name))
    }

    /// `true` for the two UTF-16 variants.
    #[must_use]
    pub const fn is_utf16(self) -> bool {
        matches!(self, Self::Utf16Le | Self::Utf16Be)
    }

    /// `true` for UTF-8, which has an identity raw/UTF-8 offset mapping.
    #[must_use]
    pub const fn is_utf8(self) -> bool {
        matches!(self, Self::Utf8)
    }

    /// `true` if the encoding uses a single byte per character and can encode
    /// every Unicode scalar value (Latin-1 only).
    #[must_use]
    pub const fn is_single_byte(self) -> bool {
        matches!(self, Self::Windows1252 | Self::Latin1)
    }

    fn encoding_rs(self) -> Option<&'static Encoding> {
        match self {
            Self::Utf8 => Some(encoding_rs::UTF_8),
            Self::Gb18030 => Some(encoding_rs::GB18030),
            Self::Gbk => Some(encoding_rs::GBK),
            Self::Big5 => Some(encoding_rs::BIG5),
            Self::ShiftJis => Some(encoding_rs::SHIFT_JIS),
            // Windows-1252 is handled by our own table so the undefined
            // 0x81/0x8D/0x8F/0x90/0x9D bytes become explicit decode errors.
            _ => None,
        }
    }

    /// Number of bytes occupied by one code unit when the encoding is used
    /// without surrogate pairs.  Only meaningful for UTF-16.
    #[must_use]
    pub const fn unit_bytes(self) -> usize {
        if self.is_utf16() {
            2
        } else {
            1
        }
    }
}

/// Byte-order-mark state of a document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum BomState {
    #[default]
    None,
    Utf8,
    Utf16Le,
    Utf16Be,
}

impl BomState {
    /// Stable display name used by menus and the status bar.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::None => "no BOM",
            Self::Utf8 => "BOM (UTF-8)",
            Self::Utf16Le => "BOM (UTF-16 LE)",
            Self::Utf16Be => "BOM (UTF-16 BE)",
        }
    }

    /// The exact byte sequence written to the file.
    #[must_use]
    pub const fn bytes(self) -> &'static [u8] {
        match self {
            Self::None => &[],
            Self::Utf8 => &[0xEF, 0xBB, 0xBF],
            Self::Utf16Le => &[0xFF, 0xFE],
            Self::Utf16Be => &[0xFE, 0xFF],
        }
    }

    /// Length of the BOM in bytes.
    #[must_use]
    pub const fn len(self) -> u64 {
        self.bytes().len() as u64
    }

    /// `true` when no BOM is written for this state.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.bytes().is_empty()
    }

    /// The character set implied by the BOM, if any.
    #[must_use]
    pub const fn charset(self) -> Option<Charset> {
        match self {
            Self::None => None,
            Self::Utf8 => Some(Charset::Utf8),
            Self::Utf16Le => Some(Charset::Utf16Le),
            Self::Utf16Be => Some(Charset::Utf16Be),
        }
    }

    /// Detects a BOM at the start of `bytes`.
    #[must_use]
    pub fn detect(bytes: &[u8]) -> Self {
        if bytes.starts_with(b"\xEF\xBB\xBF") {
            Self::Utf8
        } else if bytes.starts_with(b"\xFF\xFE") {
            Self::Utf16Le
        } else if bytes.starts_with(b"\xFE\xFF") {
            Self::Utf16Be
        } else {
            Self::None
        }
    }
}

/// Policy applied to the BOM when the document is written back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum BomPolicy {
    /// Keep the BOM state detected when the file was opened.
    #[default]
    Keep,
    Add,
    Strip,
}

/// Charset, BOM and write policy of an open document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodingInfo {
    pub charset: Charset,
    pub bom: BomState,
    pub policy: BomPolicy,
}

impl EncodingInfo {
    /// Builds the encoding information produced by detection.
    #[must_use]
    pub fn from_detection(detection: &EncodingDetection) -> Self {
        Self {
            charset: detection.charset,
            bom: detection.bom,
            policy: BomPolicy::Keep,
        }
    }
}

/// How the charset was determined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectionSource {
    Bom,
    Utf8Validity,
    Utf16Heuristic,
    Statistic,
    Fallback,
}

/// Result of charset detection.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EncodingDetection {
    pub charset: Charset,
    pub bom: BomState,
    pub confidence: f32,
    pub source: DetectionSource,
}

const UTF16_NULL_SAMPLE_LIMIT: usize = 1 << 20;
const UTF16_NULL_RATIO_THRESHOLD: f32 = 0.25;
const UTF8_CONFIDENCE: f32 = 0.98;
const BOM_CONFIDENCE: f32 = 1.0;
const CHARDET_CONFIDENCE: f32 = 0.62;
const FALLBACK_CONFIDENCE: f32 = 0.30;

/// Detects the charset of a byte stream.
///
/// Detection order: BOM -> valid UTF-8 -> UTF-16 heuristic -> `chardetng` ->
/// GB18030/Windows-1252 fallback.  The returned confidence is advisory; the
/// UI shows it in the status bar and lets the user reinterpret the bytes.
#[must_use]
pub fn detect(bytes: &[u8]) -> EncodingDetection {
    let bom = BomState::detect(bytes);
    if let Some(charset) = bom.charset() {
        return EncodingDetection {
            charset,
            bom,
            confidence: BOM_CONFIDENCE,
            source: DetectionSource::Bom,
        };
    }
    if let Some(charset) = utf16_heuristic(bytes) {
        return EncodingDetection {
            charset,
            bom,
            confidence: CHARDET_CONFIDENCE,
            source: DetectionSource::Utf16Heuristic,
        };
    }
    if bytes.len() < 2 || std::str::from_utf8(bytes).is_ok() {
        return EncodingDetection {
            charset: Charset::Utf8,
            bom,
            confidence: UTF8_CONFIDENCE,
            source: DetectionSource::Utf8Validity,
        };
    }

    let mut detector =
        chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Deny);
    detector.feed(bytes, true);
    let guessed = detector.guess(None, chardetng::Utf8Detection::Allow);
    let charset = map_encoding(guessed);
    if let Some(charset) = charset {
        let source = if matches!(charset, Charset::Gb18030 | Charset::Gbk | Charset::Big5 | Charset::ShiftJis) {
            DetectionSource::Statistic
        } else {
            DetectionSource::Fallback
        };
        return EncodingDetection {
            charset,
            bom,
            confidence: CHARDET_CONFIDENCE,
            source,
        };
    }

    EncodingDetection {
        charset: Charset::Windows1252,
        bom,
        confidence: FALLBACK_CONFIDENCE,
        source: DetectionSource::Fallback,
    }
}

fn map_encoding(encoding: &'static Encoding) -> Option<Charset> {
    let name = encoding.name();
    match name {
        "UTF-8" => Some(Charset::Utf8),
        "GB18030" => Some(Charset::Gb18030),
        "GBK" => Some(Charset::Gbk),
        "Big5" => Some(Charset::Big5),
        "Shift_JIS" => Some(Charset::ShiftJis),
        "windows-1252" => Some(Charset::Windows1252),
        _ => None,
    }
}

fn utf16_heuristic(bytes: &[u8]) -> Option<Charset> {
    if bytes.len() < 4 || bytes.len() % 2 != 0 {
        return None;
    }
    let sample = &bytes[..bytes.len().min(UTF16_NULL_SAMPLE_LIMIT)];
    let mut odd_nulls = 0usize;
    let mut even_nulls = 0usize;
    for (idx, byte) in sample.iter().enumerate() {
        if *byte == 0 {
            if idx % 2 == 0 {
                even_nulls += 1;
            } else {
                odd_nulls += 1;
            }
        }
    }
    let units = sample.len() / 2;
    if units == 0 {
        return None;
    }
    let odd_ratio = odd_nulls as f32 / units as f32;
    let even_ratio = even_nulls as f32 / units as f32;
    if odd_ratio > UTF16_NULL_RATIO_THRESHOLD && even_ratio < odd_ratio / 4.0 {
        Some(Charset::Utf16Le)
    } else if even_ratio > UTF16_NULL_RATIO_THRESHOLD && odd_ratio < even_ratio / 4.0 {
        Some(Charset::Utf16Be)
    } else {
        None
    }
}

/// A byte range that could not be decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodeError {
    /// Offset of the offending byte in the raw file.
    pub raw_offset: u64,
}

/// Decoded text plus the raw offsets of undecodable bytes.
#[derive(Debug, Clone)]
pub struct DecodedText {
    /// UTF-8 text with `U+FFFD` replacing undecodable bytes.
    pub text: String,
    pub errors: Vec<DecodeError>,
}

impl DecodedText {
    /// `true` if at least one byte could not be decoded.
    #[must_use]
    pub fn had_errors(&self) -> bool {
        !self.errors.is_empty()
    }
}

/// Decoding failure that is not recoverable.
#[derive(Debug, Error)]
pub enum DecodeFailure {
    #[error("invalid UTF-16 byte sequence")]
    InvalidUtf16,
}

/// Decodes `raw` (BOM already excluded) with `charset`.
#[must_use]
pub fn decode(raw: &[u8], charset: Charset, bom_len: u64) -> DecodedText {
    match charset {
        Charset::Utf8 => decode_utf8(raw, bom_len),
        Charset::Utf16Le => decode_utf16(raw, true, bom_len),
        Charset::Utf16Be => decode_utf16(raw, false, bom_len),
        Charset::Windows1252 => decode_windows1252(raw, bom_len, false),
        Charset::Latin1 => decode_windows1252(raw, bom_len, true),
        _ => decode_legacy(raw, charset, bom_len),
    }
}

fn decode_utf8(raw: &[u8], bom_len: u64) -> DecodedText {
    let mut text = String::with_capacity(raw.len());
    let mut errors = Vec::new();
    let mut i = 0usize;
    while i < raw.len() {
        let b = raw[i];
        if b < 0x80 {
            text.push(b as char);
            i += 1;
            continue;
        }
        match std::str::from_utf8(&raw[i..]) {
            Ok(rest) => {
                text.push_str(rest);
                break;
            }
            Err(err) => {
                let valid = err.valid_up_to();
                if valid > 0 {
                    // The prefix is guaranteed valid.
                    text.push_str(std::str::from_utf8(&raw[i..i + valid]).unwrap_or(""));
                }
                let bad = err.error_len().unwrap_or(1).max(1);
                errors.push(DecodeError {
                    raw_offset: bom_len + (i + valid) as u64,
                });
                text.push('\u{FFFD}');
                i += valid + bad;
            }
        }
    }
    DecodedText { text, errors }
}

fn decode_utf16(raw: &[u8], little_endian: bool, bom_len: u64) -> DecodedText {
    let mut text = String::with_capacity(raw.len() / 2);
    let mut errors = Vec::new();
    let read_unit = |pos: usize| -> u16 {
        if little_endian {
            u16::from_le_bytes([raw[pos], raw[pos + 1]])
        } else {
            u16::from_be_bytes([raw[pos], raw[pos + 1]])
        }
    };
    let mut i = 0usize;
    while i + 1 < raw.len() {
        let unit = read_unit(i);
        let mut consumed = 2usize;
        let value = if (0xD800..=0xDBFF).contains(&unit) {
            if i + 3 < raw.len() {
                let low = read_unit(i + 2);
                if (0xDC00..=0xDFFF).contains(&low) {
                    consumed = 4;
                    let code = 0x1_0000
                        + ((u32::from(unit) - 0xD800) << 10)
                        + (u32::from(low) - 0xDC00);
                    char::from_u32(code).unwrap_or('\u{FFFD}')
                } else {
                    errors.push(DecodeError {
                        raw_offset: bom_len + i as u64,
                    });
                    '\u{FFFD}'
                }
            } else {
                errors.push(DecodeError {
                    raw_offset: bom_len + i as u64,
                });
                '\u{FFFD}'
            }
        } else if (0xDC00..=0xDFFF).contains(&unit) {
            errors.push(DecodeError {
                raw_offset: bom_len + i as u64,
            });
            '\u{FFFD}'
        } else {
            char::from_u32(u32::from(unit)).unwrap_or('\u{FFFD}')
        };
        text.push(value);
        i += consumed;
    }
    if i < raw.len() {
        errors.push(DecodeError {
            raw_offset: bom_len + i as u64,
        });
        text.push('\u{FFFD}');
    }
    DecodedText { text, errors }
}

/// Windows-1252 code points for the 0x80..=0x9F range.  `None` marks the
/// five undefined bytes, which are reported as decode errors.
const WINDOWS_1252_HIGH: [Option<char>; 32] = [
    Some('\u{20AC}'), None, Some('\u{201A}'), Some('\u{0192}'),
    Some('\u{201E}'), Some('\u{2026}'), Some('\u{2020}'), Some('\u{2021}'),
    Some('\u{02C6}'), Some('\u{2030}'), Some('\u{0160}'), Some('\u{2039}'),
    Some('\u{0152}'), None, Some('\u{017D}'), None,
    None, Some('\u{2018}'), Some('\u{2019}'), Some('\u{201C}'),
    Some('\u{201D}'), Some('\u{2022}'), Some('\u{2013}'), Some('\u{2014}'),
    Some('\u{02DC}'), Some('\u{2122}'), Some('\u{0161}'), Some('\u{203A}'),
    Some('\u{0153}'), None, Some('\u{017E}'), Some('\u{0178}'),
];

fn decode_windows1252(raw: &[u8], bom_len: u64, latin1: bool) -> DecodedText {
    let mut text = String::with_capacity(raw.len());
    let mut errors = Vec::new();
    for (i, b) in raw.iter().enumerate() {
        let byte = *b;
        if latin1 || !(0x80..0xA0).contains(&byte) {
            // Latin-1 maps every byte; Windows-1252 maps 0xA0..=0xFF identically.
            text.push(char::from(byte));
        } else {
            match WINDOWS_1252_HIGH[(byte - 0x80) as usize] {
                Some(ch) => text.push(ch),
                None => {
                    errors.push(DecodeError {
                        raw_offset: bom_len + i as u64,
                    });
                    text.push('\u{FFFD}');
                }
            }
        }
    }
    DecodedText { text, errors }
}

fn decode_legacy(raw: &[u8], charset: Charset, bom_len: u64) -> DecodedText {
    let Some(encoding) = charset.encoding_rs() else {
        return decode_windows1252(raw, bom_len, charset == Charset::Latin1);
    };
    let (decoded, had_errors) = encoding.decode_without_bom_handling(raw);
    let mut errors = Vec::new();
    if had_errors {
        // Walk structural units so each replacement character can be mapped
        // back to the raw offset that produced it.
        let mut i = 0usize;
        while i < raw.len() {
            let unit_len = unit_len(raw, i, charset).max(1);
            let end = (i + unit_len).min(raw.len());
            let (unit_text, unit_errors) =
                encoding.decode_without_bom_handling(&raw[i..end]);
            if unit_errors || unit_text.contains('\u{FFFD}') {
                errors.push(DecodeError {
                    raw_offset: bom_len + i as u64,
                });
            }
            i = end;
        }
    }
    DecodedText {
        text: decoded.into_owned(),
        errors,
    }
}

/// Length in raw bytes of the character starting at `pos`.
///
/// The function is structural: it never decodes and therefore works on
/// truncated windows.  It returns at least 1 and is clamped by `raw.len()`
/// only through the caller.
#[must_use]
pub fn unit_len(raw: &[u8], pos: usize, charset: Charset) -> usize {
    if pos >= raw.len() {
        return 1;
    }
    match charset {
        Charset::Utf8 => utf8_unit_len(raw[pos]),
        Charset::Utf16Le | Charset::Utf16Be => utf16_unit_len(raw, pos, charset),
        Charset::Gb18030 => gb18030_unit_len(raw, pos),
        Charset::Gbk => {
            let b = raw[pos];
            if is_gbk_lead(b) && pos + 1 < raw.len() && is_gbk_trail(raw[pos + 1]) {
                2
            } else {
                1
            }
        }
        Charset::Big5 => {
            let b = raw[pos];
            if (0x81..=0xFE).contains(&b) && pos + 1 < raw.len() && is_big5_trail(raw[pos + 1]) {
                2
            } else {
                1
            }
        }
        Charset::ShiftJis => shift_jis_unit_len(raw, pos),
        Charset::Windows1252 | Charset::Latin1 => 1,
    }
}

fn utf8_unit_len(lead: u8) -> usize {
    match lead {
        0x00..=0x7F => 1,
        0xC2..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF4 => 4,
        _ => 1,
    }
}

fn utf16_unit_len(raw: &[u8], pos: usize, charset: Charset) -> usize {
    if pos + 1 >= raw.len() {
        return 1;
    }
    let unit = if charset == Charset::Utf16Le {
        u16::from_le_bytes([raw[pos], raw[pos + 1]])
    } else {
        u16::from_be_bytes([raw[pos], raw[pos + 1]])
    };
    if (0xD800..=0xDBFF).contains(&unit) && pos + 3 < raw.len() {
        4
    } else {
        2
    }
}

fn is_gbk_lead(b: u8) -> bool {
    (0x81..=0xFE).contains(&b)
}

fn is_gbk_trail(b: u8) -> bool {
    (0x40..=0xFE).contains(&b) && b != 0x7F
}

fn gb18030_unit_len(raw: &[u8], pos: usize) -> usize {
    let b = raw[pos];
    if is_gbk_lead(b) {
        if pos + 1 >= raw.len() {
            return 1;
        }
        if is_gbk_trail(raw[pos + 1]) {
            return 2;
        }
        if (0x30..=0x39).contains(&raw[pos + 1]) {
            return 4;
        }
    }
    1
}

fn is_big5_trail(b: u8) -> bool {
    (0x40..=0x7E).contains(&b) || (0xA1..=0xFE).contains(&b)
}

fn shift_jis_unit_len(raw: &[u8], pos: usize) -> usize {
    let b = raw[pos];
    if ((0x81..=0x9F).contains(&b) || (0xE0..=0xFC).contains(&b))
        && pos + 1 < raw.len()
        && ((0x40..=0x7E).contains(&raw[pos + 1]) || (0x80..=0xFC).contains(&raw[pos + 1]))
    {
        return 2;
    }
    1
}

/// A character that cannot be represented in the target charset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnmappableChar {
    /// Byte offset of the character inside the UTF-8 source string.
    pub utf8_pos: usize,
    pub ch: char,
}

/// Encodes `text` with `charset`; fails if any character is unmappable.
pub fn encode(text: &str, charset: Charset) -> Result<Vec<u8>, Vec<UnmappableChar>> {
    match charset {
        Charset::Utf8 => Ok(text.as_bytes().to_vec()),
        Charset::Utf16Le => Ok(encode_utf16(text, true)),
        Charset::Utf16Be => Ok(encode_utf16(text, false)),
        Charset::Windows1252 => encode_single_byte(text, false),
        Charset::Latin1 => encode_single_byte(text, true),
        _ => encode_legacy(text, charset),
    }
}

fn encode_utf16(text: &str, little_endian: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() * 2);
    for unit in text.encode_utf16() {
        if little_endian {
            out.extend_from_slice(&unit.to_le_bytes());
        } else {
            out.extend_from_slice(&unit.to_be_bytes());
        }
    }
    out
}

fn encode_single_byte(text: &str, latin1: bool) -> Result<Vec<u8>, Vec<UnmappableChar>> {
    let mut out = Vec::with_capacity(text.len());
    let mut bad = Vec::new();
    for (pos, ch) in text.char_indices() {
        let byte = if latin1 {
            u32::from(ch) <= 0xFF
        } else {
            windows1252_byte(ch).is_some()
        };
        if byte {
            out.push(if latin1 {
                u32::from(ch) as u8
            } else {
                windows1252_byte(ch).unwrap_or(0)
            });
        } else {
            bad.push(UnmappableChar { utf8_pos: pos, ch });
        }
    }
    if bad.is_empty() {
        Ok(out)
    } else {
        Err(bad)
    }
}

fn windows1252_byte(ch: char) -> Option<u8> {
    let code = u32::from(ch);
    if code < 0x80 || (0xA0..=0xFF).contains(&code) {
        return Some(code as u8);
    }
    WINDOWS_1252_HIGH
        .iter()
        .position(|entry| *entry == Some(ch))
        .map(|idx| 0x80 + idx as u8)
}

fn encode_legacy(text: &str, charset: Charset) -> Result<Vec<u8>, Vec<UnmappableChar>> {
    let Some(encoding) = charset.encoding_rs() else {
        return encode_single_byte(text, charset == Charset::Latin1);
    };
    let mut encoder = encoding.new_encoder();
    let mut out = Vec::with_capacity(text.len());
    let mut buf = vec![0u8; 4096];
    let mut remaining = text;
    let mut consumed_total = 0usize;
    let mut bad = Vec::new();
    loop {
        let (result, read, written) = encoder.encode_from_utf8_without_replacement(
            remaining,
            &mut buf,
            true,
        );
        out.extend_from_slice(&buf[..written]);
        remaining = &remaining[read..];
        consumed_total += read;
        match result {
            encoding_rs::EncoderResult::InputEmpty => break,
            encoding_rs::EncoderResult::OutputFull => continue,
            encoding_rs::EncoderResult::Unmappable(ch) => {
                // `read` already covers the unmappable character, so the
                // remaining input starts after it.
                bad.push(UnmappableChar {
                    utf8_pos: consumed_total.saturating_sub(ch.len_utf8()),
                    ch,
                });
            }
        }
    }
    if bad.is_empty() {
        Ok(out)
    } else {
        Err(bad)
    }
}

/// Checks that `text` can be encoded with `charset` without changing it.
///
/// Returns the list of offending characters; an empty list means the save
/// operation is safe.
#[must_use]
pub fn precheck_unmappable(text: &str, charset: Charset) -> Vec<UnmappableChar> {
    match encode(text, charset) {
        Ok(_) => Vec::new(),
        Err(bad) => bad,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_bom_variants() {
        assert_eq!(BomState::detect(b"\xEF\xBB\xBFabc"), BomState::Utf8);
        assert_eq!(BomState::detect(b"\xFF\xFEa\x00"), BomState::Utf16Le);
        assert_eq!(BomState::detect(b"\xFE\xFF\x00a"), BomState::Utf16Be);
        assert_eq!(BomState::detect(b"plain"), BomState::None);
    }

    #[test]
    fn detection_prefers_bom_and_utf8() {
        let detection = detect(b"\xEF\xBB\xBFhello");
        assert_eq!(detection.charset, Charset::Utf8);
        assert_eq!(detection.bom, BomState::Utf8);
        assert_eq!(detection.source, DetectionSource::Bom);

        let detection = detect(b"hello world");
        assert_eq!(detection.charset, Charset::Utf8);
        assert_eq!(detection.source, DetectionSource::Utf8Validity);
    }

    #[test]
    fn utf16_without_bom_is_recognised() {
        let bytes = b"h\x00e\x00l\x00l\x00o\x00";
        let detection = detect(bytes);
        assert_eq!(detection.charset, Charset::Utf16Le);
    }

    #[test]
    fn utf16_surrogate_pairs_round_trip() {
        let text = "A😀中";
        let bytes = encode(text, Charset::Utf16Le).unwrap();
        assert_eq!(bytes.len(), 2 + 4 + 2);
        let decoded = decode(&bytes, Charset::Utf16Le, 0);
        assert_eq!(decoded.text, text);
        assert!(!decoded.had_errors());
    }

    #[test]
    fn utf16_odd_trailing_byte_is_reported() {
        let decoded = decode(&[0x41, 0x00, 0x42], Charset::Utf16Le, 0);
        assert!(decoded.had_errors());
        assert_eq!(decoded.errors[0].raw_offset, 2);
    }

    #[test]
    fn windows1252_undefined_byte_is_reported() {
        let decoded = decode(&[0x41, 0x81, 0x42], Charset::Windows1252, 0);
        assert_eq!(decoded.text, "A\u{FFFD}B");
        assert_eq!(decoded.errors.len(), 1);
        assert_eq!(decoded.errors[0].raw_offset, 1);
    }

    #[test]
    fn latin1_maps_every_byte() {
        let decoded = decode(&[0x80, 0xFF], Charset::Latin1, 0);
        assert_eq!(decoded.text, "\u{80}\u{FF}");
        assert!(!decoded.had_errors());
    }

    #[test]
    fn gbk_round_trip() {
        let text = "中文 abc";
        let bytes = encode(text, Charset::Gbk).unwrap();
        let decoded = decode(&bytes, Charset::Gbk, 0);
        assert_eq!(decoded.text, text);
        assert!(!decoded.had_errors());
    }

    #[test]
    fn gb18030_four_byte_sequence_length() {
        let raw = [0x81, 0x30, 0x81, 0x30];
        assert_eq!(unit_len(&raw, 0, Charset::Gb18030), 4);
    }

    #[test]
    fn unmappable_is_reported_with_position() {
        let bad = precheck_unmappable("ok😀", Charset::Gbk);
        assert_eq!(bad.len(), 1);
        assert_eq!(bad[0].ch, '😀');
        assert_eq!(bad[0].utf8_pos, 2);
    }

    #[test]
    fn invalid_utf8_offsets_are_absolute() {
        let decoded = decode(b"ab\xFFcd", Charset::Utf8, 0);
        assert_eq!(decoded.text, "ab\u{FFFD}cd");
        assert_eq!(decoded.errors[0].raw_offset, 2);
    }
}

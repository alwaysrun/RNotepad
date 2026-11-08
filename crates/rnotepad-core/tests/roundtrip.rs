//! Property tests for the encoding and indexing layers.

use proptest::prelude::*;
use rnotepad_core::bytes::RawBytes;
use rnotepad_core::coord::{AnchoredCoordMap, CoordMap, RawByteOffset, Utf8Pos};
use rnotepad_core::encoding::{decode, encode, Charset};
use rnotepad_core::line_index::LineIndex;
use std::sync::Arc;

fn text_strategy() -> impl Strategy<Value = String> {
    proptest::collection::vec(
        prop_oneof![
            Just('a'),
            Just('Z'),
            Just(' '),
            Just('\n'),
            Just('\r'),
            Just('\t'),
            Just('中'),
            Just('文'),
        ],
        0..64,
    )
    .prop_map(|chars| chars.into_iter().collect())
}

/// Characters representable by every legacy charset used in the tests.
fn common_text_strategy() -> impl Strategy<Value = String> {
    proptest::collection::vec(
        prop_oneof![Just('a'), Just('Z'), Just(' '), Just('\n'), Just('中')],
        0..64,
    )
    .prop_map(|chars| chars.into_iter().collect())
}

proptest! {
    #[test]
    fn utf8_round_trip(text in text_strategy()) {
        let bytes = encode(&text, Charset::Utf8).unwrap();
        let decoded = decode(&bytes, Charset::Utf8, 0);
        prop_assert!(!decoded.had_errors());
        prop_assert_eq!(&decoded.text, &text);
    }

    #[test]
    fn utf16_round_trip(text in text_strategy()) {
        let bytes = encode(&text, Charset::Utf16Le).unwrap();
        let decoded = decode(&bytes, Charset::Utf16Le, 0);
        prop_assert!(!decoded.had_errors());
        prop_assert_eq!(&decoded.text, &text);
    }

    #[test]
    fn gbk_round_trip_is_lossless_for_common_text(text in common_text_strategy()) {
        let bytes = encode(&text, Charset::Gbk).unwrap();
        let decoded = decode(&bytes, Charset::Gbk, 0);
        prop_assert_eq!(&decoded.text, &text);
    }

    #[test]
    fn utf8_coordinate_mapping_is_identity(text in text_strategy()) {
        let bytes = encode(&text, Charset::Utf8).unwrap();
        let raw = RawBytes::from_vec(bytes);
        let index = Arc::new(LineIndex::new(raw.clone(), Charset::Utf8, 0));
        let map = AnchoredCoordMap::new(raw, Charset::Utf8, 0, index);
        for pos in 0..=text.len() {
            prop_assert_eq!(map.raw_to_utf8(RawByteOffset(pos as u64)), Some(Utf8Pos(pos)));
            prop_assert_eq!(map.utf8_to_raw(Utf8Pos(pos)), Some(RawByteOffset(pos as u64)));
        }
    }

    #[test]
    fn line_index_agrees_with_naive_count(newlines in proptest::collection::vec(prop_oneof![Just('\n'), Just('\r'), Just('a')], 0..100)) {
        let text: String = newlines.into_iter().collect();
        let bytes = text.as_bytes().to_vec();
        let index = LineIndex::new(RawBytes::from_vec(bytes), Charset::Utf8, 0);
        index.build_all();
        let mut expected = 1u32;
        let mut chars = text.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '\r' {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                expected += 1;
            } else if ch == '\n' {
                expected += 1;
            }
        }
        prop_assert_eq!(index.total_lines(), Some(expected));
    }
}

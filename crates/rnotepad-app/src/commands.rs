//! Menu and control identifiers.
//!
//! Every identifier has a descriptive name; the window procedure matches on
//! these constants instead of magic numbers.

/// Create an empty document.
pub const ID_FILE_NEW: u32 = 1000;
/// Open a file.
pub const ID_FILE_OPEN: u32 = 1001;
/// Save the current document.
pub const ID_FILE_SAVE: u32 = 1002;
/// Save the current document under a new path.
pub const ID_FILE_SAVE_AS: u32 = 1003;
/// Reload the current document from disk.
pub const ID_FILE_RELOAD: u32 = 1005;
/// Close the current document.
pub const ID_FILE_CLOSE: u32 = 1004;
/// Exit the application.
pub const ID_FILE_EXIT: u32 = 1009;

/// Undo.
pub const ID_EDIT_UNDO: u32 = 1101;
/// Redo.
pub const ID_EDIT_REDO: u32 = 1102;
/// Cut.
pub const ID_EDIT_CUT: u32 = 1103;
/// Copy.
pub const ID_EDIT_COPY: u32 = 1104;
/// Paste.
pub const ID_EDIT_PASTE: u32 = 1105;
/// Toggle the editable buffer.
pub const ID_EDIT_TOGGLE: u32 = 1106;
/// Show the find bar.
pub const ID_EDIT_FIND: u32 = 1107;
/// Show the find bar with the focus on the replacement field.
pub const ID_EDIT_REPLACE: u32 = 1110;
/// Find the next match (F3).
pub const ID_EDIT_FIND_NEXT: u32 = 1108;
/// Find every match.
pub const ID_EDIT_FIND_ALL: u32 = 1109;

/// Text view.
pub const ID_VIEW_TEXT: u32 = 1201;
/// Hex view.
pub const ID_VIEW_HEX: u32 = 1202;
/// Text and hex side by side.
pub const ID_VIEW_SPLIT: u32 = 1203;
/// Results panel.
pub const ID_VIEW_RESULTS: u32 = 1204;
/// Keep the large text view pinned to the last line.
pub const ID_VIEW_FOLLOW_TAIL: u32 = 1205;
/// Detach the results panel into a floating frame.
pub const ID_VIEW_RESULTS_FLOAT: u32 = 1206;

/// Reinterpret as UTF-8.
pub const ID_ENC_UTF8: u32 = 1301;
/// Reinterpret as UTF-16 LE.
pub const ID_ENC_UTF16LE: u32 = 1302;
/// Reinterpret as UTF-16 BE.
pub const ID_ENC_UTF16BE: u32 = 1303;
/// Reinterpret as GBK.
pub const ID_ENC_GBK: u32 = 1304;
/// Reinterpret as GB18030.
pub const ID_ENC_GB18030: u32 = 1305;
/// Reinterpret as Big5.
pub const ID_ENC_BIG5: u32 = 1306;
/// Reinterpret as Shift-JIS.
pub const ID_ENC_SHIFT_JIS: u32 = 1307;
/// Reinterpret as Windows-1252.
pub const ID_ENC_WINDOWS_1252: u32 = 1308;
/// Reinterpret as Latin-1.
pub const ID_ENC_LATIN1: u32 = 1309;

/// Line feed only.
pub const ID_EOL_LF: u32 = 1401;
/// Carriage return plus line feed.
pub const ID_EOL_CRLF: u32 = 1402;
/// Carriage return only.
pub const ID_EOL_CR: u32 = 1403;
/// Keep line endings unchanged (write policy only).
pub const ID_EOL_PRESERVE: u32 = 1404;

/// Convert the saved bytes to UTF-8.
pub const ID_CONVERT_UTF8: u32 = 1801;
/// Convert the saved bytes to UTF-16 LE.
pub const ID_CONVERT_UTF16LE: u32 = 1802;
/// Convert the saved bytes to UTF-16 BE.
pub const ID_CONVERT_UTF16BE: u32 = 1803;
/// Convert the saved bytes to GBK.
pub const ID_CONVERT_GBK: u32 = 1804;
/// Convert the saved bytes to GB18030.
pub const ID_CONVERT_GB18030: u32 = 1805;
/// Convert the saved bytes to Big5.
pub const ID_CONVERT_BIG5: u32 = 1806;
/// Convert the saved bytes to Shift-JIS.
pub const ID_CONVERT_SHIFT_JIS: u32 = 1807;
/// Convert the saved bytes to Windows-1252.
pub const ID_CONVERT_WINDOWS_1252: u32 = 1808;
/// Convert the saved bytes to Latin-1.
pub const ID_CONVERT_LATIN1: u32 = 1809;

/// Keep the detected BOM.
pub const ID_BOM_KEEP: u32 = 1501;
/// Write a BOM.
pub const ID_BOM_ADD: u32 = 1502;
/// Do not write a BOM.
pub const ID_BOM_STRIP: u32 = 1503;

/// Format JSON with indentation.
pub const ID_FORMAT_JSON: u32 = 1601;
/// Minify JSON.
pub const ID_FORMAT_MINIFY: u32 = 1602;
/// Validate JSON and jump to the first problem.
pub const ID_FORMAT_DIAGNOSTIC: u32 = 1603;

/// Show the about box.
pub const ID_HELP_ABOUT: u32 = 1701;
/// Enable or disable file logging.
pub const ID_HELP_ENABLE_LOG: u32 = 1702;

/// First command id reserved for recent-file entries.
pub const ID_FILE_RECENT_FIRST: u32 = 1901;
/// Last command id reserved for recent-file entries.
pub const ID_FILE_RECENT_LAST: u32 = 1910;

/// Find text editor.
pub const ID_FIND_EDIT: u32 = 2001;
/// Case-sensitive checkbox.
pub const ID_FIND_CASE: u32 = 2002;
/// Find-next button.
pub const ID_FIND_NEXT_BUTTON: u32 = 2003;
/// Find-all button.
pub const ID_FIND_ALL_BUTTON: u32 = 2004;
/// Close the find bar.
pub const ID_FIND_CLOSE: u32 = 2005;
/// Replace-all button.
pub const ID_FIND_REPLACE_BUTTON: u32 = 2006;
/// Replacement text editor.
pub const ID_FIND_REPLACE_EDIT: u32 = 2007;
/// Cancels the running background search.
pub const ID_FIND_CANCEL: u32 = 2008;
/// Replaces the current match.
pub const ID_FIND_REPLACE_ONE: u32 = 2009;

/// Height of the status strip.
pub const STATUS_HEIGHT: i32 = 24;
/// Height of the two-row find bar.
pub const FIND_BAR_HEIGHT: i32 = 64;
/// Height of one find-bar row.
pub const FIND_ROW_HEIGHT: i32 = 26;
/// Gap between find-bar rows.
pub const FIND_ROW_GAP: i32 = 4;
/// Gap between split panes.
pub const SPLIT_GAP: i32 = 4;
/// Timer used for status refresh and background task polling.
pub const TIMER_REFRESH: usize = 1;
/// Refresh interval in milliseconds.
pub const TIMER_INTERVAL_MS: u32 = 250;
/// Ratio of the text pane in split layouts.
pub const TEXT_SPLIT_RATIO: f32 = 0.62;
/// Number of status fields.
pub const STATUS_FIELD_COUNT: usize = 8;
/// Bytes shown on one hex row.
pub const HEX_BYTES_PER_ROW: u64 = 16;
/// Width of the find-bar cancel button.
pub const FIND_CANCEL_WIDTH: i32 = 64;
/// Width of a regular find-bar action button.
pub const FIND_BUTTON_WIDTH: i32 = 80;
/// Width of the case-sensitive checkbox.
pub const FIND_CASE_WIDTH: i32 = 56;
/// Width of the find-bar close button.
pub const FIND_CLOSE_WIDTH: i32 = 28;
/// Default size of the floating results frame.
pub const FLOAT_FRAME_WIDTH: i32 = 480;
/// Default height of the floating results frame.
pub const FLOAT_FRAME_HEIGHT: i32 = 360;

//! Go scanner.go:2813-2931 comment range iteration.

use std::sync::LazyLock;

use memchr::memmem;

use crate::frontend::prelude::*;
use crate::scanner_util::{is_shebang_trivia, scan_shebang_trivia};

use super::scanner_p1::{rune_to_char, utf8_decode_rune_in_string};

// Go: scanner/scanner.go:2799 GetLeadingCommentRanges
// PORT: `for_each_comment_range` is Go's `iter.Seq`; these functions collect it into a Vec.
pub fn get_leading_comment_ranges(f: &NodeFactory, text: &str, pos: i32) -> Vec<CommentRange> {
    iterate_comment_ranges(f, text, pos, false)
}

// Go: scanner/scanner.go:2803 GetTrailingCommentRanges
pub fn get_trailing_comment_ranges(f: &NodeFactory, text: &str, pos: i32) -> Vec<CommentRange> {
    iterate_comment_ranges(f, text, pos, true)
}

// Finds the "*/" that ends a block comment. PORT: Go uses strings.Index.
// One shared searcher gives the same first byte offset without the searcher
// setup that str::find and memmem::find do on each call.
static BLOCK_COMMENT_END: LazyLock<memmem::Finder<'static>> =
    LazyLock::new(|| memmem::Finder::new("*/"));

// Go: scanner/scanner.go:2814 iterateCommentRanges (the iter.Seq body).
// PORT: Go returns an iter.Seq. This calls `yield_` for each range and stops when it returns false.
// PERF: callers in the printer use this so that no Vec is returned by value (effectemit1).
#[inline]
pub fn for_each_comment_range(
    f: &NodeFactory,
    text: &str,
    pos: i32,
    trailing: bool,
    mut yield_: impl FnMut(CommentRange) -> bool,
) {
    let bytes = text.as_bytes();
    let len = bytes.len() as i32;
    let mut pos = pos;
    let mut pending_pos = 0;
    let mut pending_end = 0;
    let mut pending_kind = SyntaxKind::Unknown;
    let mut pending_has_trailing_new_line = false;
    let mut has_pending_comment_range = false;
    let mut collecting = trailing;
    if pos == 0 {
        collecting = true;
        if is_shebang_trivia(text, pos as usize) {
            pos = scan_shebang_trivia(text, pos as usize) as i32;
        }
    }
    'scan: while pos >= 0 && pos < len {
        let (ch, size) = utf8_decode_rune_in_string(text, pos as usize);
        match ch {
            // PORT: Go `case '\r': ...; fallthrough` into `case '\n'`.
            0x0D | 0x0A => {
                if ch == 0x0D && pos + 1 < len && bytes[(pos + 1) as usize] == b'\n' {
                    pos += 1;
                }
                pos += 1;
                if trailing {
                    break 'scan;
                }
                collecting = true;
                if has_pending_comment_range {
                    pending_has_trailing_new_line = true;
                }
                continue;
            }
            0x09 | 0x0B | 0x0C | 0x20 => {
                pos += 1;
                continue;
            }
            0x2F => {
                let next_char = if pos + 1 < len {
                    bytes[(pos + 1) as usize]
                } else {
                    0
                };
                let mut has_trailing_new_line = false;
                if next_char == b'/' || next_char == b'*' {
                    let kind = if next_char == b'/' {
                        SyntaxKind::SingleLineCommentTrivia
                    } else {
                        SyntaxKind::MultiLineCommentTrivia
                    };
                    let start_pos = pos;
                    pos += 2;
                    if next_char == b'/' {
                        while pos < len {
                            let (c, s) = utf8_decode_rune_in_string(text, pos as usize);
                            if is_line_break(rune_to_char(c)) {
                                has_trailing_new_line = true;
                                break;
                            }
                            pos += s;
                        }
                    } else if let Some(i) = BLOCK_COMMENT_END.find(&bytes[pos as usize..]) {
                        pos += i as i32 + 2;
                    } else {
                        pos = len;
                    }
                    if collecting {
                        if has_pending_comment_range {
                            if !yield_(f.new_comment_range(
                                pending_kind,
                                pending_pos,
                                pending_end,
                                pending_has_trailing_new_line,
                            )) {
                                return;
                            }
                        }
                        pending_pos = start_pos;
                        pending_end = pos;
                        pending_kind = kind;
                        pending_has_trailing_new_line = has_trailing_new_line;
                        has_pending_comment_range = true;
                    }
                    continue;
                }
                break 'scan;
            }
            _ => {
                if ch > 0x7F && is_white_space_like(rune_to_char(ch)) {
                    if has_pending_comment_range && is_line_break(rune_to_char(ch)) {
                        pending_has_trailing_new_line = true;
                    }
                    pos += size;
                    continue;
                }
                break 'scan;
            }
        }
    }
    if has_pending_comment_range {
        yield_(f.new_comment_range(
            pending_kind,
            pending_pos,
            pending_end,
            pending_has_trailing_new_line,
        ));
    }
}

// Go: scanner/scanner.go:2813 iterateCommentRanges
fn iterate_comment_ranges(
    f: &NodeFactory,
    text: &str,
    pos: i32,
    trailing: bool,
) -> Vec<CommentRange> {
    let mut out = Vec::new();
    for_each_comment_range(f, text, pos, trailing, |c| {
        out.push(c);
        true
    });
    out
}

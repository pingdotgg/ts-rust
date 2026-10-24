//! Port of internal/lsp/lsproto/util.go.

use crate::lsp::lsproto::prelude::*;

// Go: util.go:11 ComparePositions
// Implements a cmp.Compare like function for two Position
// ComparePositions(pos, other) == cmp.Compare(pos, other)
pub fn compare_positions(pos: Position, other: Position) -> i32 {
    let line_comp = pos.line.cmp(&other.line) as i32;
    if line_comp != 0 {
        return line_comp;
    }
    pos.character.cmp(&other.character) as i32
}

// Go: util.go:22 CompareRanges
// Implements a cmp.Compare like function for two Range
// CompareRanges(lsRange, other) == cmp.Compare(lsRange, other)
//
//	Range.Start is compared before Range.End
pub fn compare_ranges(ls_range: Range, other: Range) -> i32 {
    let start_comp = compare_positions(ls_range.start, other.start);
    if start_comp != 0 {
        return start_comp;
    }
    compare_positions(ls_range.end, other.end)
}

impl StringOrMarkupContent {
    // Go: util.go:31 AsString
    // AsString returns the plain text of a StringOrMarkupContent, reading the
    // MarkupContent value when the message is not a plain string.
    pub fn as_string(&self) -> String {
        if let Some(s) = &self.string {
            return s.clone();
        }
        if let Some(m) = &self.markup_content {
            return m.value.clone();
        }
        String::new()
    }
}

impl IntegerOrString {
    // Go: util.go:41 AsString
    pub fn as_string(&self) -> String {
        if let Some(s) = &self.string {
            s.clone()
        } else if let Some(i) = self.integer {
            i.to_string()
        } else {
            "-1".to_string()
        }
    }
}

// Go: util.go:53 diagnosticExistsInSlice
fn diagnostic_exists_in_slice(elem: &Diagnostic, diags: &[Diagnostic]) -> bool {
    for diag in diags {
        if diagnostics_equal(elem, diag) {
            return true;
        }
    }
    false
}

// Go: util.go:62 diagnosticsEqual
fn diagnostics_equal(diag1: &Diagnostic, diag2: &Diagnostic) -> bool {
    diagnostic_codes_equal(diag1.code.as_ref(), diag2.code.as_ref())
        && diagnostic_messages_equal(&diag1.message, &diag2.message)
        && compare_ranges(diag1.range, diag2.range) == 0
}

// Go: util.go:69 diagnosticCodesEqual
// PORT: a nil `*IntegerOrString` is a Go nil pointer dereference where Go
// reads it: `code1` first, `code2` only after a `code1` field is set.
fn diagnostic_codes_equal(
    code1: Option<&IntegerOrString>,
    code2: Option<&IntegerOrString>,
) -> bool {
    // Go loads `code1.String` (offset 8): addr=0x8 (util.go:70).
    let code1 = code1.unwrap_or_else(|| crate::core::go_nil_dereference_at(0x8));
    if let Some(s1) = &code1.string {
        // Go loads `code2.String` (offset 8): addr=0x8 (util.go:70).
        let code2 = code2.unwrap_or_else(|| crate::core::go_nil_dereference_at(0x8));
        if let Some(s2) = &code2.string {
            return s1 == s2;
        }
    }
    if let Some(i1) = code1.integer {
        // Go loads `code2.Integer` (offset 0): addr=0x0 (util.go:73).
        let code2 = code2.unwrap_or_else(|| crate::core::go_nil_dereference_at(0x0));
        if let Some(i2) = code2.integer {
            return i1 == i2;
        }
    }
    false
}

// Go: util.go:79 diagnosticMessagesEqual
fn diagnostic_messages_equal(
    message1: &StringOrMarkupContent,
    message2: &StringOrMarkupContent,
) -> bool {
    if let (Some(s1), Some(s2)) = (&message1.string, &message2.string) {
        return s1 == s2;
    }
    if let (Some(m1), Some(m2)) = (&message1.markup_content, &message2.markup_content) {
        return m1.kind == m2.kind && m1.value == m2.value;
    }
    false
}

// Go: util.go:89 CompareDiagnostics
// PORT: Go takes and returns `[]*Diagnostic`; the results borrow from the inputs.
pub fn compare_diagnostics<'a>(
    list1: &'a [Diagnostic],
    list2: &'a [Diagnostic],
) -> (Vec<&'a Diagnostic>, Vec<&'a Diagnostic>) {
    let mut missing_from_list1: Vec<&'a Diagnostic> = Vec::new();
    let mut missing_from_list2: Vec<&'a Diagnostic> = Vec::new();
    for elem in list1 {
        if !diagnostic_exists_in_slice(elem, list2) {
            missing_from_list2.push(elem);
        }
    }
    for elem in list2 {
        if !diagnostic_exists_in_slice(elem, list1) {
            missing_from_list1.push(elem);
        }
    }
    (missing_from_list1, missing_from_list2)
}

impl Diagnostic {
    // Go: util.go:105 AsString
    // PORT: Go calls AsString on a nil `Code` and panics; so does this.
    pub fn as_string(&self) -> String {
        format!(
            "{} ({}:{}-{}:{}): {}",
            self.code
                .as_ref()
                // Go copies `*elem.Code` and loads offset 0 first: addr=0x0 (util.go:106).
                .unwrap_or_else(|| crate::core::go_nil_dereference_at(0x0))
                .as_string(),
            self.range.start.line,
            self.range.start.character,
            self.range.end.line,
            self.range.end.character,
            self.message.as_string()
        )
    }

    // Go: util.go:109 CodeAsString
    // PORT: Go calls AsString on a nil `Code` and panics; so does this.
    pub fn code_as_string(&self) -> String {
        format!(
            "Code({})",
            self.code
                .as_ref()
                // Go copies `*elem.Code` and loads offset 0 first: addr=0x0 (util.go:110).
                .unwrap_or_else(|| crate::core::go_nil_dereference_at(0x0))
                .as_string()
        )
    }
}

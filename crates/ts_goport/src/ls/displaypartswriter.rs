//! Port of Go `ls/displaypartswriter.go`.

use crate::ls::prelude::*;

use crate::frontend::scanner::scanner_p1::{RUNE_ERROR, utf8_decode_last_rune_in_string};

// Go: ls/displaypartswriter.go:14 `var _ printer.EmitTextWriter = &displayPartsWriter{}`
// PORT: the `impl EmitTextWriter for DisplayPartsWriter` below.

// Go: ls/displaypartswriter.go:19 displayPartsWriter
// displayPartsWriter implements EmitTextWriter and captures classified text runs
// for VS colorized labels, while also building a plain string.
// When vsCapability is false, only the plain string is built; runs are skipped.
//
// PORT: Go shares `*displayPartsWriter` between its builder and the printer
// (`p.Write(node, file, w, nil)`), so the handle is
// `Rc<RefCell<DisplayPartsWriter>>` (`new_display_parts_writer`). It coerces
// to the printer's `Rc<RefCell<dyn EmitTextWriter>>`.
//
// PORT: `WriteSymbol` classifies the run from the symbol flags
// (`classificationForSymbol`). The printer calls it while the caller holds
// the checker, and the flags live in the checker's symbol arena. So
// `write_symbol` adds the run with an empty classification and records the
// run index and the symbol in `symbol_runs`; `get_runs(c)` fills in the
// classification with the checker. The plain string does not depend on the
// classification. The runs equal Go's when the symbol flags and the first
// declaration do not change between printing and `get_runs`; the checker
// sets both when it makes or merges a symbol, before the language service
// prints it.
#[derive(Clone, Debug, Default)]
pub struct DisplayPartsWriter {
    pub builder: String,
    pub runs: Vec<lsproto::VSClassifiedTextRun>,
    pub vs_capability: bool,
    pub last_written: String,
    /// PORT: runs made by `write_symbol` whose classification `get_runs`
    /// computes: `(index in runs, symbol)`.
    pub symbol_runs: Vec<(usize, SymbolId)>,
}

// Go: ls/displaypartswriter.go:26 newDisplayPartsWriter
pub fn new_display_parts_writer(vs_capability: bool) -> Rc<RefCell<DisplayPartsWriter>> {
    Rc::new(RefCell::new(DisplayPartsWriter {
        vs_capability,
        ..Default::default()
    }))
}

impl DisplayPartsWriter {
    // Go: ls/displaypartswriter.go:30 addRun
    pub fn add_run(&mut self, classification: lsproto::ClassificationTypeName, text: &str) {
        if text.is_empty() {
            return;
        }
        if self.vs_capability {
            self.runs.push(lsproto::VSClassifiedTextRun {
                classification_type_name: classification.0.to_string(),
                text: text.to_string(),
                ..Default::default()
            });
        }
        self.last_written = text.to_string();
        self.builder.push_str(text);
    }

    // Go: ls/displaypartswriter.go:45 WriteClassified
    // WriteClassified writes text with an explicit classification type.
    pub fn write_classified(
        &mut self,
        text: &str,
        classification: lsproto::ClassificationTypeName,
    ) {
        self.add_run(classification, text);
    }

    // Go: ls/displaypartswriter.go:50 WriteFrom
    // WriteFrom copies the accumulated content from another displayPartsWriter.
    // PORT: Go appends `other.GetRuns()`; the runs of `other` that still wait
    // for their symbol classification move along with their new index.
    pub fn write_from(&mut self, other: &DisplayPartsWriter) {
        self.builder.push_str(&other.string());
        if self.vs_capability {
            let offset = self.runs.len();
            self.runs.extend(other.runs.iter().cloned());
            self.symbol_runs.extend(
                other
                    .symbol_runs
                    .iter()
                    .map(|&(index, symbol)| (offset + index, symbol)),
            );
        }
        if !other.last_written.is_empty() {
            self.last_written = other.last_written.clone();
        }
    }

    // Go: ls/displaypartswriter.go:60 GetRuns
    // PORT: takes the checker to classify the `write_symbol` runs (see the
    // struct comment). Go returns the shared run pointers; this returns
    // copies.
    pub fn get_runs(&self, c: &Checker) -> Vec<lsproto::VSClassifiedTextRun> {
        let mut runs = self.runs.clone();
        for &(index, symbol) in &self.symbol_runs {
            runs[index].classification_type_name =
                classification_for_symbol(&c.symbols, symbol).0.to_string();
        }
        runs
    }
}

impl EmitTextWriter for DisplayPartsWriter {
    // Go: ls/displaypartswriter.go:64 String
    fn string(&self) -> String {
        self.builder.clone()
    }

    // Go: ls/displaypartswriter.go:68 Clear
    fn clear(&mut self) {
        self.last_written = String::new();
        self.builder.clear();
        self.runs = Vec::new();
        self.symbol_runs = Vec::new();
    }

    // Go: ls/displaypartswriter.go:74 DecreaseIndent
    fn decrease_indent(&mut self) {}

    // Go: ls/displaypartswriter.go:76 GetColumn
    fn get_column(&self) -> i32 {
        0
    }

    // Go: ls/displaypartswriter.go:78 GetIndent
    fn get_indent(&self) -> i32 {
        0
    }

    // Go: ls/displaypartswriter.go:80 GetLine
    fn get_line(&self) -> i32 {
        0
    }

    // Go: ls/displaypartswriter.go:82 GetTextPos
    // PORT: Go `strings.Builder.Len` is the byte length.
    fn get_text_pos(&self) -> i32 {
        self.builder.len() as i32
    }

    // Go: ls/displaypartswriter.go:86 HasTrailingComment
    fn has_trailing_comment(&self) -> bool {
        false
    }

    // Go: ls/displaypartswriter.go:88 HasTrailingWhitespace
    fn has_trailing_whitespace(&self) -> bool {
        if self.builder.is_empty() {
            return false;
        }
        let (ch, _) = utf8_decode_last_rune_in_string(&self.last_written, self.last_written.len());
        if ch == RUNE_ERROR {
            return false;
        }
        // PORT: a decoded rune other than RuneError is a valid scalar value.
        char::from_u32(ch as u32).is_some_and(is_white_space_like)
    }

    // Go: ls/displaypartswriter.go:99 IncreaseIndent
    fn increase_indent(&mut self) {}

    // Go: ls/displaypartswriter.go:101 IsAtStartOfLine
    fn is_at_start_of_line(&self) -> bool {
        false
    }

    // Go: ls/displaypartswriter.go:103 RawWrite
    fn raw_write(&mut self, s: &str) {
        self.add_run(lsproto::ClassificationTypeName::TEXT, s);
    }

    // Go: ls/displaypartswriter.go:107 Write
    fn write(&mut self, s: &str) {
        self.add_run(lsproto::ClassificationTypeName::TEXT, s);
    }

    // Go: ls/displaypartswriter.go:111 WriteComment
    fn write_comment(&mut self, text: &str) {
        // Strada's writeComment uses unknownWrite → SymbolDisplayPartKind.text → "text"
        self.add_run(lsproto::ClassificationTypeName::TEXT, text);
    }

    // Go: ls/displaypartswriter.go:116 WriteKeyword
    fn write_keyword(&mut self, text: &str) {
        self.add_run(lsproto::ClassificationTypeName::KEYWORD, text);
    }

    // Go: ls/displaypartswriter.go:120 WriteLine
    fn write_line(&mut self) {
        self.add_run(lsproto::ClassificationTypeName::WHITE_SPACE, " ");
    }

    // Go: ls/displaypartswriter.go:124 WriteLineForce
    fn write_line_force(&mut self, _force: bool) {
        self.add_run(lsproto::ClassificationTypeName::WHITE_SPACE, " ");
    }

    // Go: ls/displaypartswriter.go:128 WriteLiteral
    fn write_literal(&mut self, s: &str) {
        // Strada's writeLiteral → SymbolDisplayPartKind.stringLiteral → "string"
        self.add_run(lsproto::ClassificationTypeName::STRING, s);
    }

    // Go: ls/displaypartswriter.go:133 WriteOperator
    fn write_operator(&mut self, text: &str) {
        self.add_run(lsproto::ClassificationTypeName::OPERATOR, text);
    }

    // Go: ls/displaypartswriter.go:137 WriteParameter
    fn write_parameter(&mut self, text: &str) {
        self.add_run(lsproto::ClassificationTypeName::PARAMETER_NAME, text);
    }

    // Go: ls/displaypartswriter.go:141 WriteProperty
    fn write_property(&mut self, text: &str) {
        self.add_run(lsproto::ClassificationTypeName::PROPERTY_NAME, text);
    }

    // Go: ls/displaypartswriter.go:145 WritePunctuation
    fn write_punctuation(&mut self, text: &str) {
        self.add_run(lsproto::ClassificationTypeName::PUNCTUATION, text);
    }

    // Go: ls/displaypartswriter.go:149 WriteSpace
    fn write_space(&mut self, text: &str) {
        self.add_run(lsproto::ClassificationTypeName::WHITE_SPACE, text);
    }

    // Go: ls/displaypartswriter.go:153 WriteStringLiteral
    fn write_string_literal(&mut self, text: &str) {
        self.add_run(lsproto::ClassificationTypeName::STRING, text);
    }

    // Go: ls/displaypartswriter.go:157 WriteSymbol
    // PORT: Go classifies here (`classificationForSymbol(symbol)`). A nil
    // symbol classifies as text without the checker; any other symbol is
    // classified by `get_runs` (see the struct comment).
    fn write_symbol(&mut self, text: &str, symbol: SymbolId) {
        if symbol.is_nil() {
            self.add_run(lsproto::ClassificationTypeName::TEXT, text);
            return;
        }
        let index = self.runs.len();
        self.add_run(lsproto::ClassificationTypeName::default(), text);
        if self.runs.len() > index {
            self.symbol_runs.push((index, symbol));
        }
    }

    // Go: ls/displaypartswriter.go:162 WriteTrailingSemicolon
    fn write_trailing_semicolon(&mut self, text: &str) {
        self.add_run(lsproto::ClassificationTypeName::PUNCTUATION, text);
    }
}

// Go: ls/displaypartswriter.go:168 classificationForSymbol
// classificationForSymbol determines the Roslyn classification type name based on a symbol's flags.
// Matches the Strada translation chain: displayPartKind() → GetClassificationName().
// PORT: Go reads the symbol through its pointer; `symbols` is the arena that
// holds it (the checker's).
pub fn classification_for_symbol(
    symbols: &SymbolArena,
    symbol: SymbolId,
) -> lsproto::ClassificationTypeName {
    if symbol.is_nil() {
        return lsproto::ClassificationTypeName::TEXT;
    }
    let flags = symbols.sym_flags(symbol);
    if flags.intersects(SymbolFlags::VARIABLE) {
        if is_first_declaration_of_symbol_parameter(symbols, symbol) {
            return lsproto::ClassificationTypeName::PARAMETER_NAME;
        }
        lsproto::ClassificationTypeName::LOCAL_NAME
    } else if flags.intersects(SymbolFlags::PROPERTY) {
        lsproto::ClassificationTypeName::PROPERTY_NAME
    } else if flags.intersects(SymbolFlags::GET_ACCESSOR) {
        lsproto::ClassificationTypeName::PROPERTY_NAME
    } else if flags.intersects(SymbolFlags::SET_ACCESSOR) {
        lsproto::ClassificationTypeName::PROPERTY_NAME
    } else if flags.intersects(SymbolFlags::ENUM_MEMBER) {
        lsproto::ClassificationTypeName::FIELD_NAME
    } else if flags.intersects(SymbolFlags::FUNCTION) {
        lsproto::ClassificationTypeName::METHOD_NAME
    } else if flags.intersects(SymbolFlags::CLASS) {
        lsproto::ClassificationTypeName::CLASS_NAME
    } else if flags.intersects(SymbolFlags::INTERFACE) {
        lsproto::ClassificationTypeName::INTERFACE_NAME
    } else if flags.intersects(SymbolFlags::ENUM) {
        lsproto::ClassificationTypeName::ENUM_NAME
    } else if flags.intersects(SymbolFlags::MODULE) {
        lsproto::ClassificationTypeName::MODULE_NAME
    } else if flags.intersects(SymbolFlags::METHOD) {
        lsproto::ClassificationTypeName::METHOD_NAME
    } else if flags.intersects(SymbolFlags::TYPE_PARAMETER) {
        lsproto::ClassificationTypeName::TYPE_PARAMETER_NAME
    } else if flags.intersects(SymbolFlags::TYPE_ALIAS) {
        lsproto::ClassificationTypeName::IDENTIFIER
    } else if flags.intersects(SymbolFlags::ALIAS) {
        lsproto::ClassificationTypeName::IDENTIFIER
    } else {
        lsproto::ClassificationTypeName::TEXT
    }
}

// Go: ls/displaypartswriter.go:211 isFirstDeclarationOfSymbolParameter
// isFirstDeclarationOfSymbolParameter checks if the symbol's first declaration is a parameter.
pub fn is_first_declaration_of_symbol_parameter(symbols: &SymbolArena, symbol: SymbolId) -> bool {
    let declarations = &symbols.sym(symbol).declarations;
    if declarations.is_empty() {
        return false;
    }
    declarations[0].kind() == SyntaxKind::Parameter
}

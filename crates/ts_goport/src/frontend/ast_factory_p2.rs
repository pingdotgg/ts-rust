//! Port of the Go `ast.NodeFactory` constructors that `ast/factory.rs` does
//! not have yet (`ast/ast.go`, `ast/ast_generated.go`). The parser needs them.
//!
//! The argument rules are the same as in `ast/factory.rs`: Go `*Node` is
//! `Node`, Go `*NodeList` is `NodeList`, Go `*ModifierList` is
//! `ModifierList`, and Go `nil` is the `NIL` value of each. Nodes go to the
//! factory target (synthetic arena or the store of the parsed file).

use crate::astdata::NodeData as D;
use crate::frontend::prelude::*;

/// Go `TokenFlagsNone` in astdata form.
const NO_TOKEN_FLAGS: crate::astdata::TokenFlags = crate::astdata::TokenFlags(0);

impl NodeFactory {
    // Go: ast/ast.go:2526 NewSourceFile
    // PORT: Go stores `fileName`, `parseOptions` and `text` on the
    // SourceFile data. Here the file name and text live in the node store
    // (`new_file_store`) and the parse options in `ParsedSourceFile`.
    // ts#64159 removes the Go check on the file name (normalized and
    // absolute): `opts.FileName` is a `RootedFilePath` now.
    // PORT: named `new_parsed_source_file` because `ast/factory.rs` has the
    // synthetic form of Go NewSourceFile with a different signature.
    pub fn new_parsed_source_file(
        &self,
        opts: &SourceFileParseOptions,
        text: &str,
        statements: NodeList,
        end_of_file_token: Node,
    ) -> Node {
        let _ = (opts, text);
        self.new_node(
            SyntaxKind::SourceFile,
            D::SourceFile(Box::new(crate::astdata::SourceFileData {
                end_of_file_token: self.id(end_of_file_token),
                locals: crate::astdata::SymbolTable,
                next_container: None,
                statements: self.req_list(statements),
                symbol: None,
                facts: 0,
            })),
        )
    }

    // Go: ast/ast.go:3115 NewCommentRange
    #[must_use]
    pub fn new_comment_range(
        &self,
        kind: SyntaxKind,
        pos: i32,
        end: i32,
        has_trailing_new_line: bool,
    ) -> CommentRange {
        CommentRange {
            text_range: TextRange::new(pos, end),
            kind,
            has_trailing_new_line,
        }
    }
}

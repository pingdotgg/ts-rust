//! Port of Go `ls/codelens.go`.

use crate::ls::prelude::*;

use crate::frontend::json::{MarshalerTo, json_marshal, json_unmarshal};
use crate::scanner_util::{GoOffsets, port_byte_offset};
use crate::spanmap::Feature;

impl LanguageService {
    // Go: ls/codelens.go:18 ProvideCodeLenses
    pub fn provide_code_lenses(
        &self,
        ctx: &Context,
        document_uri: &lsproto::DocumentUri,
    ) -> Result<lsproto::CodeLensResponse, GoError> {
        let (_, file) = self.get_program_and_file(document_uri);

        let user_prefs = self.user_preferences().code_lens;
        if !user_prefs.references_code_lens_enabled.is_true()
            && !user_prefs.implementations_code_lens_enabled.is_true()
        {
            return Ok(lsproto::CodeLensResponse::default());
        }

        // PORT: the recursive Go closure `visit` and the variables it
        // captures (`lastSymbol`, `result`, `seen`) are the
        // `CodeLensVisitor` below. `result` and `seen` carry over from one
        // projection to the next.
        let mut result: Vec<lsproto::CodeLens> = Vec::new();
        let mut seen: FxHashSet<CodeLensKey> = FxHashSet::default();
        let mut projections = vec![file];
        projections.extend_from_slice(source_file_supplemental_source_files(file));
        for projection in projections {
            let first = result.len();
            let mut visitor = CodeLensVisitor {
                ls: self,
                ctx,
                document_uri,
                file: projection,
                user_prefs,
                // Keeps track of the last symbol to avoid duplicating code lenses across overloads.
                last_symbol: SymbolId::NIL,
                result: std::mem::take(&mut result),
                seen: std::mem::take(&mut seen),
            };

            visitor.visit(projection);
            result = visitor.result;
            seen = visitor.seen;
            // PORT: Go writes `data.position` as a Go byte offset
            // (`newCodeLensForNode`), and the client sends it back on
            // resolve. Port offsets differ from Go offsets after a marker
            // unit (see `scanner_util::GO_STRING_MARKER`), so the lenses of
            // this projection leave with Go offsets (`go_byte_offset`) and
            // `resolve_code_lens` maps them back. The text is scanned once
            // for all lenses (`GoOffsets`).
            let offsets = GoOffsets::new(&source_file_text(projection));
            if offsets.has_units() {
                for code_lens in &mut result[first..] {
                    if let Some(data) = &mut code_lens.data {
                        data.position = offsets.go_offset(data.position);
                    }
                }
            }
        }

        Ok(lsproto::CodeLensResponse {
            code_lenses: Some(result),
        })
    }
}

// Go: ls/codelens.go:30 (the state of the `visit` closure in ProvideCodeLenses)
struct CodeLensVisitor<'a> {
    ls: &'a LanguageService,
    ctx: &'a Context,
    document_uri: &'a lsproto::DocumentUri,
    file: Node,
    user_prefs: lsutil::CodeLensUserPreferences,
    last_symbol: SymbolId,
    result: Vec<lsproto::CodeLens>,
    seen: FxHashSet<CodeLensKey>,
}

impl CodeLensVisitor<'_> {
    // Go: ls/codelens.go:33 visit
    fn visit(&mut self, node: Node) -> bool {
        if self.ctx.err().is_some() {
            return true;
        }

        let current_symbol = node.symbol();
        if self.last_symbol != current_symbol {
            self.last_symbol = current_symbol;

            if self.user_prefs.references_code_lens_enabled.is_true()
                && is_valid_reference_lens_node(node, self.user_prefs)
            {
                if let Some(code_lens) = self.ls.new_code_lens_for_node(
                    self.document_uri,
                    self.file,
                    node,
                    lsproto::CodeLensKind::REFERENCES,
                ) && self.seen.insert(key_for_code_lens(&code_lens))
                {
                    self.result.push(code_lens);
                }
            }

            if self.user_prefs.implementations_code_lens_enabled.is_true()
                && is_valid_implementations_code_lens_node(node, self.user_prefs)
            {
                if let Some(code_lens) = self.ls.new_code_lens_for_node(
                    self.document_uri,
                    self.file,
                    node,
                    lsproto::CodeLensKind::IMPLEMENTATIONS,
                ) && self.seen.insert(key_for_code_lens(&code_lens))
                {
                    self.result.push(code_lens);
                }
            }
        }

        let saved_last_symbol = self.last_symbol;
        node.for_each_child(|child| self.visit(child));
        self.last_symbol = saved_last_symbol;
        false
    }
}

// Go: ls/codelens.go:68 codeLensKey
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct CodeLensKey {
    kind: lsproto::CodeLensKind,
    start_line: u32,
    start_character: u32,
    end_line: u32,
    end_character: u32,
}

// Go: ls/codelens.go:73 keyForCodeLens
fn key_for_code_lens(code_lens: &lsproto::CodeLens) -> CodeLensKey {
    // PORT: Go dereferences `codeLens.Data`; every lens made here sets it.
    let data = code_lens
        .data
        .as_ref()
        .unwrap_or_else(|| crate::core::go_nil_dereference());
    CodeLensKey {
        kind: data.kind.clone(),
        start_line: code_lens.range.start.line,
        start_character: code_lens.range.start.character,
        end_line: code_lens.range.end.line,
        end_character: code_lens.range.end.character,
    }
}

impl LanguageService {
    // Go: ls/codelens.go:83 ResolveCodeLens
    // PORT: Go mutates `*codeLens` and returns the same pointer; here the
    // lens is taken by value and returned. Go `*string` is `Option<String>`.
    pub fn resolve_code_lens(
        &self,
        ctx: &Context,
        mut code_lens: lsproto::CodeLens,
        show_locations_command_name: Option<String>,
        orchestrator: Option<&dyn CrossProjectOrchestrator>,
    ) -> Result<lsproto::CodeLens, GoError> {
        // PORT: Go dereferences `codeLens.Data`, which panics when it is nil.
        let data = code_lens
            .data
            .clone()
            .unwrap_or_else(|| crate::core::go_nil_dereference());
        let uri = data.uri.clone();
        let text_doc = lsproto::TextDocumentIdentifier { uri: uri.clone() };
        let (program, file) = self.get_program_and_file(&uri);
        let file = source_file_for_supplemental_file_index(file, data.supplemental_file_index);
        if file.is_nil() {
            // PORT: Go reads `*codeLens.Data.SupplementalFileIndex` here. A
            // nil index returns the file, so the index is set when the file
            // is nil.
            let index = data
                .supplemental_file_index
                .unwrap_or_else(|| crate::core::go_nil_dereference());
            return Err(gostd::errors::errorf(
                format!("supplemental source file index not found: {index}"),
                Vec::new(),
            ));
        }
        // PORT: `data.position` is a Go byte offset (see
        // `provide_code_lenses`).
        let position = port_byte_offset(&source_file_text(file), data.position);
        let locale = locale::from_context(ctx);
        let mut locs: Vec<lsproto::Location> = Vec::new();
        let mut lens_title = String::new();
        if data.kind == lsproto::CodeLensKind::REFERENCES {
            let (symbol_data, _) = self.provide_symbols_and_entries_at_position(
                ctx, program, file, position, false, false,
            );
            let references_resp = self.provide_references_from_data(
                ctx,
                &lsproto::ReferenceParams {
                    text_document: text_doc,
                    position: code_lens.range.start,
                    context: Some(lsproto::ReferenceContext {
                        // Don't include the declaration in the references count.
                        include_declaration: false,
                    }),
                    ..Default::default()
                },
                orchestrator,
                symbol_data,
            )?;
            if let Some(locations) = references_resp.locations {
                locs = locations;
            }

            if locs.len() == 1 {
                lens_title = crate::diagnostics_loc::message_localize(
                    diag::X_1_reference,
                    &locale,
                    &args![],
                );
            } else {
                lens_title = crate::diagnostics_loc::message_localize(
                    diag::X_0_references,
                    &locale,
                    &args![locs.len()],
                );
            }
        } else if data.kind == lsproto::CodeLensKind::IMPLEMENTATIONS {
            let (symbol_data, _) = self
                .provide_symbols_and_entries_at_position(ctx, program, file, position, false, true);
            let implementations = self.provide_implementations_from_data(
                ctx,
                &lsproto::ImplementationParams {
                    text_document: text_doc,
                    position: code_lens.range.start,
                    ..Default::default()
                },
                // "Force" link support to be false so that we only get `Locations` back,
                // and don't include the "current" node in the results.
                SymbolEntryTransformOptions {
                    require_locations_result: true,
                    drop_origin_nodes: true,
                },
                orchestrator,
                symbol_data,
            )?;

            if let Some(locations) = implementations.locations {
                locs = locations;
            }

            if locs.len() == 1 {
                lens_title = crate::diagnostics_loc::message_localize(
                    diag::X_1_implementation,
                    &locale,
                    &args![],
                );
            } else {
                lens_title = crate::diagnostics_loc::message_localize(
                    diag::X_0_implementations,
                    &locale,
                    &args![locs.len()],
                );
            }
        }

        let mut cmd = lsproto::Command {
            title: lens_title,
            ..Default::default()
        };
        if !locs.is_empty() {
            if let Some(show_locations_command_name) = show_locations_command_name {
                cmd.command = show_locations_command_name;
                cmd.arguments = Some(vec![
                    to_lsp_any(&uri),
                    to_lsp_any(&code_lens.range.start),
                    to_lsp_any(&locs),
                ]);
            }
        }

        code_lens.command = Some(cmd);
        Ok(code_lens)
    }
}

// PORT: Go puts the typed values in `[]any` and the JSON writer marshals
// each one with its own marshaler. `lsproto::Command.arguments` holds
// `LspAny`, so each value is marshaled and read back as `LspAny`. Objects
// keep key order and integers write the same text, so the output JSON is
// the same.
fn to_lsp_any<T: MarshalerTo + ?Sized>(value: &T) -> LspAny {
    let text = json_marshal(value, &[]).expect("marshal code lens argument");
    let mut out = LspAny::Null;
    json_unmarshal(text.as_bytes(), &mut out, &[]).expect("unmarshal code lens argument");
    out
}

impl LanguageService {
    // Go: ls/codelens.go:165 newCodeLensForNode
    // PORT: Go returns `*lsproto.CodeLens`; nil is `None`. `data.position`
    // is the port offset here; `provide_code_lenses` maps it to the Go
    // offset once for each projection.
    pub fn new_code_lens_for_node(
        &self,
        file_uri: &lsproto::DocumentUri,
        file: Node,
        node: Node,
        kind: lsproto::CodeLensKind,
    ) -> Option<lsproto::CodeLens> {
        let mut node_for_range = node;
        let node_name = node.name();
        if node_name.is_some() {
            node_for_range = node_name;
        }
        let pos = skip_trivia(&source_file_text(file), node_for_range.pos());
        let (lsp_range, fidelity) = self.converters.to_lsp_range_for_feature(
            &file,
            TextRange::new(pos, node.end()),
            Feature::CODE_LENS,
        );
        if fidelity.is_none() {
            return None;
        }

        Some(lsproto::CodeLens {
            range: lsp_range,
            data: Some(lsproto::CodeLensData {
                kind,
                uri: file_uri.clone(),
                position: pos,
                supplemental_file_index: supplemental_file_index(file),
            }),
            ..Default::default()
        })
    }
}

// Go: ls/codelens.go:188 isValidImplementationsCodeLensNode
pub fn is_valid_implementations_code_lens_node(
    node: Node,
    user_prefs: lsutil::CodeLensUserPreferences,
) -> bool {
    match node.kind() {
        // Always show on interfaces
        SyntaxKind::InterfaceDeclaration => {
            // TODO: ast.KindTypeAliasDeclaration?
            return true;
        }

        // If configured, show on interface methods
        SyntaxKind::MethodSignature => {
            return user_prefs
                .implementations_code_lens_show_on_interface_methods
                .is_true()
                && node.parent().kind() == SyntaxKind::InterfaceDeclaration;
        }

        // If configured, show on all class methods - but not private ones.
        // Always show on abstract classes/properties/methods
        SyntaxKind::MethodDeclaration
        | SyntaxKind::ClassDeclaration
        | SyntaxKind::Constructor
        | SyntaxKind::GetAccessor
        | SyntaxKind::SetAccessor
        | SyntaxKind::PropertyDeclaration => {
            // PORT: Go `case KindMethodDeclaration` either returns or falls
            // through to the abstract case below.
            if node.kind() == SyntaxKind::MethodDeclaration
                && user_prefs
                    .implementations_code_lens_show_on_all_class_methods
                    .is_true()
                && node.parent().kind() == SyntaxKind::ClassDeclaration
            {
                return !has_modifier(node, ModifierFlags::PRIVATE)
                    && node.name().kind() != SyntaxKind::PrivateIdentifier;
            }
            return has_modifier(node, ModifierFlags::ABSTRACT);
        }
        _ => {}
    }

    false
}

// Go: ls/codelens.go:215 isValidReferenceLensNode
pub fn is_valid_reference_lens_node(
    node: Node,
    user_prefs: lsutil::CodeLensUserPreferences,
) -> bool {
    match node.kind() {
        SyntaxKind::FunctionDeclaration | SyntaxKind::VariableDeclaration => {
            // PORT: Go `case KindFunctionDeclaration` either returns or falls
            // through to the variable declaration case.
            if node.kind() == SyntaxKind::FunctionDeclaration
                && user_prefs
                    .references_code_lens_show_on_all_functions
                    .is_true()
            {
                return true;
            }
            return get_combined_modifier_flags(node).intersects(ModifierFlags::EXPORT);
        }

        SyntaxKind::ClassDeclaration
        | SyntaxKind::InterfaceDeclaration
        | SyntaxKind::TypeAliasDeclaration
        | SyntaxKind::EnumDeclaration
        | SyntaxKind::EnumMember => {
            return true;
        }

        SyntaxKind::MethodDeclaration
        | SyntaxKind::MethodSignature
        | SyntaxKind::Constructor
        | SyntaxKind::GetAccessor
        | SyntaxKind::SetAccessor
        | SyntaxKind::PropertyDeclaration
        | SyntaxKind::PropertySignature => {
            // Don't show if child and parent have same start
            // For https://github.com/microsoft/vscode/issues/90396
            // !!!

            match node.parent().kind() {
                SyntaxKind::ClassDeclaration
                | SyntaxKind::InterfaceDeclaration
                | SyntaxKind::TypeLiteral => {
                    return true;
                }
                _ => {}
            }
        }
        _ => {}
    }

    false
}

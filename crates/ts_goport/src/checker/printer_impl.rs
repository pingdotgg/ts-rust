//! Port of checker/printer.go: the checker entry points that print types,
//! symbols, signatures and type predicates through the node builder and the
//! printer.

use crate::prelude::*;
use crate::printer::{
    EmitContext, EmitTextWriter, PrintHandlers, Printer, PrinterOptions,
    get_single_line_string_writer, new_printer, new_text_writer,
};
use std::cell::Cell;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};

/// Go `VerbosityContext` (checker/nodebuilder.go). Hover code sets it; the
/// checker passes nil.
// PORT: Go passes `*VerbosityContext`, and the node builder writes
// `CanIncreaseVerbosity` and `Truncated` back through the pointer. Here the
// two output fields are shared cells, so a clone that the node builder keeps
// writes to the same place as the caller's value.
#[derive(Clone, Debug, Default)]
pub struct VerbosityContext {
    pub level: i32,
    pub max_truncation_length: i32,
    pub can_increase_verbosity: Rc<Cell<bool>>,
    pub truncated: Rc<Cell<bool>>,
}

/// Rust-only: Go `oldVerbosity := nodeBuilder.verbosity; nodeBuilder.verbosity = vc;
/// defer func() { nodeBuilder.verbosity = oldVerbosity }()`. It installs `vc` on the
/// node builder and puts the old value back on drop. A Go defer also runs when
/// the function panics, and so does this drop, so a caught `unported!` panic
/// does not leave a hover verbosity on the checker's shared node builder.
struct VerbosityRestore {
    node_builder: Rc<RefCell<NodeBuilder>>,
    old: Option<VerbosityContext>,
}

impl VerbosityRestore {
    fn install(node_builder: &Rc<RefCell<NodeBuilder>>, vc: Option<&VerbosityContext>) -> Self {
        let old = std::mem::replace(&mut node_builder.borrow_mut().verbosity, vc.cloned());
        VerbosityRestore {
            node_builder: node_builder.clone(),
            old,
        }
    }
}

impl Drop for VerbosityRestore {
    fn drop(&mut self) {
        // `try_borrow_mut` so a drop during unwinding cannot panic again.
        if let Ok(mut nb) = self.node_builder.try_borrow_mut() {
            nb.verbosity = self.old.take();
        }
    }
}

// Go: checker/printer.go:13 createPrinterWithDefaults
pub fn create_printer_with_defaults(emit_context: Rc<EmitContext>) -> Printer {
    new_printer(
        PrinterOptions::default(),
        PrintHandlers::default(),
        Some(emit_context),
    )
}

// Go: checker/printer.go:17 createPrinterWithRemoveComments
pub fn create_printer_with_remove_comments(emit_context: Rc<EmitContext>) -> Printer {
    new_printer(
        PrinterOptions {
            remove_comments: true,
            ..Default::default()
        },
        PrintHandlers::default(),
        Some(emit_context),
    )
}

// Go: checker/printer.go:21 createPrinterWithRemoveCommentsOmitTrailingSemicolon
pub fn create_printer_with_remove_comments_omit_trailing_semicolon(
    emit_context: Rc<EmitContext>,
) -> Printer {
    new_printer(
        PrinterOptions {
            remove_comments: true,
            omit_trailing_semicolon: true,
            ..Default::default()
        },
        PrintHandlers::default(),
        Some(emit_context),
    )
}

// Go: checker/printer.go:28 createPrinterWithRemoveCommentsOmitTrailingSemicolonNeverAsciiEscape
pub fn create_printer_with_remove_comments_omit_trailing_semicolon_never_ascii_escape(
    emit_context: Rc<EmitContext>,
) -> Printer {
    new_printer(
        PrinterOptions {
            remove_comments: true,
            omit_trailing_semicolon: true,
            never_ascii_escape: true,
            ..Default::default()
        },
        PrintHandlers::default(),
        Some(emit_context),
    )
}

// Go: checker/printer.go:36 createPrinterWithRemoveCommentsNeverAsciiEscape
pub fn create_printer_with_remove_comments_never_ascii_escape(
    emit_context: Rc<EmitContext>,
) -> Printer {
    new_printer(
        PrinterOptions {
            remove_comments: true,
            never_ascii_escape: true,
            ..Default::default()
        },
        PrintHandlers::default(),
        Some(emit_context),
    )
}

// Go: checker/printer.go:51 toNodeBuilderFlags
pub fn to_node_builder_flags(flags: TypeFormatFlags) -> NodeBuilderFlags {
    NodeBuilderFlags((flags & TypeFormatFlags::NODE_BUILDER_FLAGS_MASK).0)
}

// PORT: Go `ast.GetSourceFileOfNode(enclosingDeclaration)` guarded by a nil
// check appears in every entry point. `get_source_file_of_node` panics on
// nil, so the guard is kept at each call through this helper.
fn source_file_of_enclosing(enclosing_declaration: Node) -> Node {
    if enclosing_declaration.is_some() {
        get_source_file_of_node(enclosing_declaration)
    } else {
        Node::NIL
    }
}

impl Checker {
    // Go: checker/printer.go:43 TypeToString
    pub fn type_to_string_exported(&mut self, t: TypeId) -> String {
        self.type_to_string_enclosing(t, Node::NIL)
    }

    /// Go `typeToString(t, nil)`. Calls that pass an enclosing declaration use
    /// `type_to_string_enclosing`.
    // PORT: Go has one `typeToString(t, enclosingDeclaration)`; the nil form is
    // split out so existing callers keep their signature.
    pub fn type_to_string(&mut self, t: TypeId) -> String {
        self.type_to_string_enclosing(t, Node::NIL)
    }

    // Go: checker/printer.go:47 typeToString
    pub fn type_to_string_enclosing(&mut self, t: TypeId, enclosing_declaration: Node) -> String {
        self.type_to_string_ex(
            t,
            enclosing_declaration,
            TypeFormatFlags::ALLOW_UNIQUE_ES_SYMBOL_TYPE
                | TypeFormatFlags::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE,
            None,
        )
    }

    // Go: checker/printer.go:55 TypeToStringEx and checker/printer.go:59 typeToStringEx
    pub fn type_to_string_ex(
        &mut self,
        t: TypeId,
        enclosing_declaration: Node,
        flags: TypeFormatFlags,
        vc: Option<&VerbosityContext>,
    ) -> String {
        // Serialization of types can lead to (lazy) resolution of members, which can cause diagnostics that again require
        // serialization of types. This can potentially result in infinite recursion and stack overflows. To prevent that,
        // after a certain number of recursive invocations the function simply returns "?".
        if self.serialization_level >= MAX_SERIALIZATION_LEVEL {
            return "?".to_string();
        }
        let mut new_line = "";
        if flags.intersects(TypeFormatFlags::MULTILINE_OBJECT_LITERALS) {
            new_line = "\n";
        }
        let writer = Rc::new(RefCell::new(new_text_writer(new_line, 0)));
        let no_truncation = (vc.is_none_or(|vc| vc.max_truncation_length == 0)
            && self.compiler_options.no_error_truncation == Tristate::True)
            || flags.intersects(TypeFormatFlags::NO_TRUNCATION);
        let mut combined_flags = to_node_builder_flags(flags) | NodeBuilderFlags::IGNORE_ERRORS;
        if no_truncation {
            combined_flags = combined_flags | NodeBuilderFlags::NO_TRUNCATION;
        }
        // PORT: Go defers the release func from getNodeBuilder, which lets
        // the factory drop its arenas. Here `PrintScope` frees the nodes of
        // the call when it ends; it drops after the printer.
        let node_builder = self.get_node_builder();
        // Go `defer release()`: the nodes of this call are freed when it ends.
        let _print = PrintScope::open(&node_builder);
        let _verbosity = VerbosityRestore::install(&node_builder, vc);
        self.serialization_level += 1;
        // PORT: Go does not restore serializationLevel when TypeToTypeNode
        // panics. Go recovers a panic per LSP request (`lsp/server.go:1477`
        // `recover`) and keeps the checker, so after two such panics a Go
        // checker stays at maxSerializationLevel and every later type prints
        // as "?". The port also catches port-only `unported!` panics per
        // request, so it lowers the level before any panic continues. This
        // differs from Go only after a Go panic inside TypeToTypeNode.
        let type_node = match catch_unwind(AssertUnwindSafe(|| {
            self.node_builder_type_to_type_node(
                &node_builder,
                t,
                enclosing_declaration,
                combined_flags,
                InternalNodeBuilderFlags::NONE,
                None,
            )
        })) {
            Ok(type_node) => type_node,
            Err(payload) => {
                self.serialization_level -= 1;
                resume_unwind(payload);
            }
        };
        self.serialization_level -= 1;
        if type_node.is_nil() {
            panic!("should always get typenode");
        }
        // The unresolved type gets a synthesized comment on `any` to hint to users that it's not a plain `any`.
        // Otherwise, we always strip comments out.
        let emit_context = node_builder.borrow().emit_context();
        let mut p = if t == self.unresolved_type {
            create_printer_with_defaults(emit_context)
        } else {
            create_printer_with_remove_comments(emit_context)
        };
        let source_file = source_file_of_enclosing(enclosing_declaration);
        p.write_exported(type_node, source_file, writer.clone(), None);
        let result = writer.borrow().string();

        let mut max_length = DEFAULT_MAXIMUM_TRUNCATION_LENGTH * 2;
        if let Some(vc) = vc {
            if vc.max_truncation_length > 0 {
                max_length = vc.max_truncation_length * 10; // hard cutoff matching Strada's absoluteMaximumLength
            }
        }
        if no_truncation {
            max_length = NO_TRUNCATION_MAXIMUM_TRUNCATION_LENGTH * 2;
        }
        // PORT: Go `len(result)` and the slice count Go bytes. `result` is a
        // port form (see `scanner_util::GO_STRING_MARKER`), so this uses its
        // Go length, and `go_slice` keeps the bytes of a cut char as invalid
        // bytes, as Go does.
        let max_length = usize::try_from(max_length).unwrap_or(0);
        if max_length > 0 && !result.is_empty() && go_len(&result) >= max_length {
            if let Some(vc) = vc {
                vc.truncated.set(true);
            }
            return go_slice(&result, 0, max_length - "...".len()).into_owned() + "...";
        }
        result
    }

    // Go: checker/printer.go:120 SymbolToString
    pub fn symbol_to_string_exported(&mut self, symbol: SymbolId) -> String {
        self.symbol_to_string(symbol)
    }

    // Go: checker/printer.go:124 symbolToString
    pub fn symbol_to_string(&mut self, symbol: SymbolId) -> String {
        self.symbol_to_string_ex(
            symbol,
            Node::NIL,
            SymbolFlags::ALL,
            SymbolFormatFlags::ALLOW_ANY_NODE_KIND,
        )
    }

    // Go: checker/printer.go:128 SymbolToStringEx and checker/printer.go:132 symbolToStringEx
    pub fn symbol_to_string_ex(
        &mut self,
        symbol: SymbolId,
        enclosing_declaration: Node,
        meaning: SymbolFlags,
        flags: SymbolFormatFlags,
    ) -> String {
        let writer = Rc::new(RefCell::new(get_single_line_string_writer()));

        let mut node_flags = NodeBuilderFlags::IGNORE_ERRORS;
        let mut internal_node_flags = InternalNodeBuilderFlags::NONE;
        if flags.intersects(SymbolFormatFlags::USE_ONLY_EXTERNAL_ALIASING) {
            node_flags = node_flags | NodeBuilderFlags::USE_ONLY_EXTERNAL_ALIASING;
        }
        if flags.intersects(SymbolFormatFlags::WRITE_TYPE_PARAMETERS_OR_ARGUMENTS) {
            node_flags = node_flags | NodeBuilderFlags::WRITE_TYPE_PARAMETERS_IN_QUALIFIED_NAME;
        }
        if flags.intersects(SymbolFormatFlags::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE) {
            node_flags = node_flags | NodeBuilderFlags::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE;
        }
        if flags.intersects(SymbolFormatFlags::DO_NOT_INCLUDE_SYMBOL_CHAIN) {
            internal_node_flags =
                internal_node_flags | InternalNodeBuilderFlags::DO_NOT_INCLUDE_SYMBOL_CHAIN;
        }
        if flags.intersects(SymbolFormatFlags::WRITE_COMPUTED_PROPS) {
            internal_node_flags =
                internal_node_flags | InternalNodeBuilderFlags::WRITE_COMPUTED_PROPS;
        }

        // PORT: see type_to_string_ex about the release func.
        let node_builder = self.get_node_builder();
        // Go `defer release()`: the nodes of this call are freed when it ends.
        let _print = PrintScope::open(&node_builder);
        let source_file = source_file_of_enclosing(enclosing_declaration);
        let emit_context = node_builder.borrow().emit_context();
        // add neverAsciiEscape for GH#39027
        let mut printer_ = if enclosing_declaration.is_some()
            && enclosing_declaration.kind() == SyntaxKind::SourceFile
        {
            create_printer_with_remove_comments_omit_trailing_semicolon_never_ascii_escape(
                emit_context,
            )
        } else {
            create_printer_with_remove_comments_omit_trailing_semicolon(emit_context)
        };

        let entity = if flags.intersects(SymbolFormatFlags::ALLOW_ANY_NODE_KIND) {
            self.node_builder_symbol_to_node(
                &node_builder,
                symbol,
                meaning,
                enclosing_declaration,
                node_flags,
                internal_node_flags,
                None,
            )
        } else {
            self.node_builder_symbol_to_entity_name(
                &node_builder,
                symbol,
                meaning,
                enclosing_declaration,
                node_flags,
                internal_node_flags,
                None,
            )
        }; // TODO: GH#18217
        let inner: Rc<RefCell<dyn EmitTextWriter>> = writer.clone();
        printer_.write_exported(entity, source_file, inner, None); // TODO: GH#18217
        let text = writer.borrow().string();
        text
    }

    // Go: checker/printer.go:179 signatureToString
    pub fn signature_to_string(&mut self, signature: SignatureId) -> String {
        self.signature_to_string_ex(signature, Node::NIL, TypeFormatFlags::NONE, None)
    }

    // Go: checker/printer.go:183 SignatureToStringEx and checker/printer.go:187 signatureToStringEx
    pub fn signature_to_string_ex(
        &mut self,
        signature: SignatureId,
        enclosing_declaration: Node,
        flags: TypeFormatFlags,
        vc: Option<&VerbosityContext>,
    ) -> String {
        let is_constructor = self
            .sig(signature)
            .flags
            .intersects(SignatureFlags::CONSTRUCT)
            && !flags.intersects(TypeFormatFlags::WRITE_CALL_STYLE_SIGNATURE);
        let sig_output = if flags.intersects(TypeFormatFlags::WRITE_ARROW_STYLE_SIGNATURE) {
            if is_constructor {
                SyntaxKind::ConstructorType
            } else {
                SyntaxKind::FunctionType
            }
        } else if is_constructor {
            SyntaxKind::ConstructSignature
        } else {
            SyntaxKind::CallSignature
        };

        // PORT: see type_to_string_ex about the release func.
        let node_builder = self.get_node_builder();
        // Go `defer release()`: the nodes of this call are freed when it ends.
        let _print = PrintScope::open(&node_builder);
        let _verbosity = VerbosityRestore::install(&node_builder, vc);
        let combined_flags = to_node_builder_flags(flags)
            | NodeBuilderFlags::IGNORE_ERRORS
            | NodeBuilderFlags::WRITE_TYPE_PARAMETERS_IN_QUALIFIED_NAME;
        let sig = self.node_builder_signature_to_signature_declaration(
            &node_builder,
            signature,
            sig_output,
            enclosing_declaration,
            combined_flags,
            InternalNodeBuilderFlags::NONE,
            None,
        );
        let emit_context = node_builder.borrow().emit_context();
        let mut p = create_printer_with_remove_comments_omit_trailing_semicolon_never_ascii_escape(
            emit_context,
        );
        let source_file = source_file_of_enclosing(enclosing_declaration);
        if flags.intersects(TypeFormatFlags::MULTILINE_OBJECT_LITERALS) {
            let writer = Rc::new(RefCell::new(new_text_writer("\n", 0)));
            let inner: Rc<RefCell<dyn EmitTextWriter>> = writer.clone();
            p.write_exported(sig, source_file, inner, None);
            let text = writer.borrow().string();
            return text;
        }
        let writer = Rc::new(RefCell::new(get_single_line_string_writer()));
        let inner: Rc<RefCell<dyn EmitTextWriter>> = writer.clone();
        p.write_exported(sig, source_file, inner, None);
        let text = writer.borrow().string();
        text
    }

    // Go: checker/printer.go:229 typePredicateToString
    pub fn type_predicate_to_string(&mut self, type_predicate: TypePredicateId) -> String {
        self.type_predicate_to_string_ex(
            type_predicate,
            Node::NIL,
            TypeFormatFlags::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE,
        )
    }

    // Go: checker/printer.go:233 typePredicateToStringEx
    pub fn type_predicate_to_string_ex(
        &mut self,
        type_predicate: TypePredicateId,
        enclosing_declaration: Node,
        flags: TypeFormatFlags,
    ) -> String {
        let writer = Rc::new(RefCell::new(get_single_line_string_writer()));
        // PORT: see type_to_string_ex about the release func.
        let node_builder = self.get_node_builder();
        // Go `defer release()`: the nodes of this call are freed when it ends.
        let _print = PrintScope::open(&node_builder);
        let combined_flags = to_node_builder_flags(flags)
            | NodeBuilderFlags::IGNORE_ERRORS
            | NodeBuilderFlags::WRITE_TYPE_PARAMETERS_IN_QUALIFIED_NAME;
        let predicate = self.node_builder_type_predicate_to_type_predicate_node(
            &node_builder,
            type_predicate,
            enclosing_declaration,
            combined_flags,
            InternalNodeBuilderFlags::NONE,
            None,
        ); // TODO: GH#18217
        let emit_context = node_builder.borrow().emit_context();
        let mut printer_ = create_printer_with_remove_comments(emit_context);
        let source_file = source_file_of_enclosing(enclosing_declaration);
        let inner: Rc<RefCell<dyn EmitTextWriter>> = writer.clone();
        printer_.write_exported(predicate, source_file, inner, None);
        let text = writer.borrow().string();
        text
    }

    // PORT: Go checker/printer.go:379 valueToString is a wrapper over
    // `ValueToString`; callers use the free fn `value_to_string`
    // (checker/utilities_p2.rs).

    // Go: checker/printer.go:253 formatUnionTypes
    pub fn format_union_types(&mut self, types: &[TypeId], expanding_enum: bool) -> Vec<TypeId> {
        let mut result = Vec::new();
        let mut flags = TypeFlags::NONE;
        let mut i = 0;
        while i < types.len() {
            let t = types[i];
            let t_flags = self.ty(t).flags;
            flags = flags | t_flags;
            if !t_flags.intersects(TypeFlags::NULLABLE) {
                if t_flags.intersects(TypeFlags::BOOLEAN_LITERAL)
                    || (!expanding_enum && t_flags.intersects(TypeFlags::ENUM_LIKE))
                {
                    let base_type = if t_flags.intersects(TypeFlags::BOOLEAN_LITERAL) {
                        self.boolean_type
                    } else {
                        self.get_base_type_of_enum_like_type(t)
                    };
                    if self.ty(base_type).flags.intersects(TypeFlags::UNION) {
                        // PORT: Go reads AsUnionType().types. Type.types() returns the same slice for a union.
                        let base_types = self.ty(base_type).types_list();
                        let count = base_types.len();
                        if i + count <= types.len()
                            && self.get_regular_type_of_literal_type(types[i + count - 1])
                                == self.get_regular_type_of_literal_type(base_types[count - 1])
                        {
                            result.push(base_type);
                            i += count;
                            continue;
                        }
                    }
                }
                result.push(t);
            }
            i += 1;
        }
        if flags.intersects(TypeFlags::NULL) {
            result.push(self.null_type);
        }
        if flags.intersects(TypeFlags::UNDEFINED) {
            result.push(self.undefined_type);
        }
        result
    }

    // Go: checker/printer.go:288 TypeToTypeNode
    // PORT: `_exported` suffix, because the node builder method
    // `typeToTypeNode` has the same snake name.
    pub fn type_to_type_node_exported(
        &mut self,
        t: TypeId,
        enclosing_declaration: Node,
        flags: NodeBuilderFlags,
        id_to_symbol: Option<FxHashMap<Node, SymbolId>>,
    ) -> Node {
        let node_builder = self.get_node_builder_ex(id_to_symbol);
        self.node_builder_type_to_type_node(
            &node_builder,
            t,
            enclosing_declaration,
            flags,
            InternalNodeBuilderFlags::NONE,
            None,
        )
    }

    // Go: checker/printer.go:293 SignatureToSignatureDeclaration
    // PORT: `_exported` suffix, as for type_to_type_node_exported.
    pub fn signature_to_signature_declaration_exported(
        &mut self,
        signature: SignatureId,
        kind: SyntaxKind,
        enclosing_declaration: Node,
        flags: NodeBuilderFlags,
    ) -> Node {
        // PORT: Go releases here too (see type_to_string_ex), but the caller
        // keeps the node. Inside a to-string call the to-string factory puts
        // it in the print owner, so the call keeps its nodes.
        let node_builder = self.get_node_builder();
        crate::ast::synthetic::pin_print_scope();
        self.node_builder_signature_to_signature_declaration(
            &node_builder,
            signature,
            kind,
            enclosing_declaration,
            flags,
            InternalNodeBuilderFlags::NONE,
            None,
        )
    }

    /// Produces declaration strings for a symbol with verbosity support for expandable hover.
    // Go: checker/printer.go:300 ExpandSymbolForHover
    // PORT: `_exported` suffix, because the node builder method
    // `expandSymbolForHover` has the same snake name.
    pub fn expand_symbol_for_hover_exported(
        &mut self,
        symbol: SymbolId,
        meaning: SymbolFlags,
        vc: Option<&VerbosityContext>,
    ) -> String {
        // PORT: see type_to_string_ex about the release func.
        let node_builder = self.get_node_builder();
        // Go `defer release()`: the nodes of this call are freed when it ends.
        let _print = PrintScope::open(&node_builder);
        let _verbosity = VerbosityRestore::install(&node_builder, vc);
        let nodes = self.node_builder_expand_symbol_for_hover(&node_builder, symbol, meaning);
        let result = if nodes.is_empty() {
            String::new()
        } else {
            let emit_context = node_builder.borrow().emit_context();
            let mut p = create_printer_with_remove_comments(emit_context);
            let value_declaration = self.sym(symbol).value_declaration;
            let source_file = if value_declaration.is_some() {
                get_source_file_of_node(value_declaration)
            } else {
                Node::NIL
            };
            let mut b = String::new();
            for (i, node) in nodes.into_iter().enumerate() {
                if i > 0 {
                    b.push('\n');
                }
                b.push_str(&p.emit(node, source_file));
            }
            b
        };
        result
    }

    /// Renders a type parameter declaration (e.g. "T extends Foo") with optional verbosity support.
    // Go: checker/printer.go:328 TypeParameterToStringEx
    pub fn type_parameter_to_string_ex(
        &mut self,
        t: TypeId,
        enclosing_declaration: Node,
        vc: Option<&VerbosityContext>,
    ) -> String {
        // PORT: see type_to_string_ex about the release func.
        let node_builder = self.get_node_builder();
        // Go `defer release()`: the nodes of this call are freed when it ends.
        let _print = PrintScope::open(&node_builder);
        let _verbosity = VerbosityRestore::install(&node_builder, vc);
        let type_param_node = self.node_builder_type_parameter_to_declaration(
            &node_builder,
            t,
            enclosing_declaration,
            NodeBuilderFlags::IGNORE_ERRORS,
            InternalNodeBuilderFlags::NONE,
            None,
        );
        let result = if type_param_node.is_nil() {
            // PORT: Go calls TypeToString while the deferred verbosity restore
            // is still pending, so the node builder still holds `vc` here.
            self.type_to_string_exported(t)
        } else {
            let emit_context = node_builder.borrow().emit_context();
            let mut p = create_printer_with_remove_comments(emit_context);
            let source_file = source_file_of_enclosing(enclosing_declaration);
            p.emit(type_param_node, source_file)
        };
        result
    }

    // Go: checker/printer.go:348 TypeToTypeNodeEx
    pub fn type_to_type_node_ex_exported(
        &mut self,
        t: TypeId,
        enclosing_declaration: Node,
        flags: NodeBuilderFlags,
        internal_flags: InternalNodeBuilderFlags,
        id_to_symbol: Option<FxHashMap<Node, SymbolId>>,
    ) -> Node {
        let node_builder = self.get_node_builder_ex(id_to_symbol);
        self.node_builder_type_to_type_node(
            &node_builder,
            t,
            enclosing_declaration,
            flags,
            internal_flags,
            None,
        )
    }

    // Go: checker/printer.go:353 TypePredicateToTypePredicateNode
    // PORT: `_exported` suffix, as for type_to_type_node_exported.
    pub fn type_predicate_to_type_predicate_node_exported(
        &mut self,
        t: TypePredicateId,
        enclosing_declaration: Node,
        flags: NodeBuilderFlags,
        id_to_symbol: Option<FxHashMap<Node, SymbolId>>,
    ) -> Node {
        let node_builder = self.get_node_builder_ex(id_to_symbol);
        self.node_builder_type_predicate_to_type_predicate_node(
            &node_builder,
            t,
            enclosing_declaration,
            flags,
            InternalNodeBuilderFlags::NONE,
            None,
        )
    }
}

/// The print scope of a to-string call. Go `getNodeBuilder` returns
/// `release` (`Factory.ReleaseArenas`), which each caller defers. While the
/// outermost scope is open, the nodes and lists that the to-string factory
/// makes belong to the print owner (`ast::synthetic::enter_print_scope`).
/// The end of that scope frees them, with the side data that the builder
/// keeps for them: emit records, original links, idToSymbol entries, the
/// cache records of fake scopes, and the serialized types under the nil key.
// PORT: Go keeps these side tables, and the nodes that they reach, for the
// life of the checker, but no Go code reads them for a node of an ended
// call. The one later reader is the serialized type cache under an
// enclosing declaration, and a store there pins the call
// (`visit_and_transform_type`). `GOPORT_N1=0` turns print scopes off (an A/B
// switch).
struct PrintScope(Option<Rc<RefCell<NodeBuilder>>>);

impl PrintScope {
    fn open(nb: &Rc<RefCell<NodeBuilder>>) -> Self {
        static ON: std::sync::LazyLock<bool> =
            std::sync::LazyLock::new(|| std::env::var("GOPORT_N1").as_deref() != Ok("0"));
        if !*ON {
            return Self(None);
        }
        crate::ast::synthetic::enter_print_scope();
        Self(Some(nb.clone()))
    }
}

impl Drop for PrintScope {
    fn drop(&mut self) {
        let Some(nb) = self.0.take() else {
            return;
        };
        // `None` for a nested scope, a pinned call and a panic.
        let Some(ranges) = crate::ast::synthetic::exit_print_scope() else {
            return;
        };
        if ranges.is_empty() {
            return;
        }
        // `try_borrow`, so a drop never panics. A borrow that is still held
        // only leaves records of freed handles, which no read can name.
        let Ok(b) = nb.try_borrow() else {
            return;
        };
        let (e, imp) = (b.emit_context(), b.impl_.clone());
        drop(b);
        e.forget_synthetic_slots(&ranges);
        let Ok(mut imp) = imp.try_borrow_mut() else {
            return;
        };
        let imp = &mut *imp;
        forget_print_keys(&mut imp.id_to_symbol, &ranges);
        if !imp.links.map_is_empty() {
            for n in crate::ast::synthetic::print_range_nodes(&ranges) {
                imp.links.remove(n);
            }
        }
        // Read only under an enclosing declaration (`visit_and_transform_type`).
        if imp.links.has(Node::NIL) {
            let types = &mut imp.links.get(Node::NIL).serialized_types;
            if types.capacity() > 1024 {
                *types = FxHashMap::default();
            } else {
                types.clear();
            }
        }
    }
}

/// Removes the entries of the freed nodes in `ranges` (from
/// `ast::synthetic::exit_print_scope`) from `map`: one removal per node of
/// the call, or one walk of the table when that is smaller, so the cost
/// follows the size of the call, not of the map.
fn forget_print_keys<V>(map: &mut FxHashMap<Node, V>, ranges: &[(u32, u32)]) {
    if map.is_empty() {
        return;
    }
    let nodes: usize = ranges.iter().map(|&(lo, hi)| (hi - lo) as usize).sum();
    if map.capacity() <= nodes {
        map.retain(|&n, _| !crate::ast::synthetic::in_print_ranges(ranges, n));
    } else {
        for n in crate::ast::synthetic::print_range_nodes(ranges) {
            map.remove(&n);
        }
    }
    // A big call leaves a big empty table.
    if map.is_empty() && map.capacity() > 1024 {
        *map = FxHashMap::default();
    }
}

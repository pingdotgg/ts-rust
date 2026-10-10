//! Port of Go `ls/callhierarchy.go`.
//!
//! PORT: Go `*compiler.Program` is `&compiler::NewProgram`. Go
//! `program.GetTypeChecker(ctx)` and `GetTypeCheckerForFile(ctx, file)` lease
//! a checker through `ls_program`; the `Release` guard stays alive to the end
//! of the Go function, as Go's `defer done()`.

use crate::ls::prelude::*;

use crate::spanmap::Feature;
use std::cell::OnceCell;

// Go: ls/callhierarchy.go:25 CallHierarchyDeclaration
pub type CallHierarchyDeclaration = Node;

// PORT: Go `findImplementationOrAllInitialDeclarations` and
// `resolveCallHierarchyDeclaration` return `any` holding either a
// `*ast.Node` or a `[]*ast.Node`, and callers switch on the dynamic type.
// This enum holds the two cases. A Go nil `any` is `None` where it can occur.
#[derive(Clone, Debug)]
pub enum CallHierarchyDeclarationOrDeclarations {
    Node(Node),
    Nodes(Vec<Node>),
}

// Go: ls/callhierarchy.go:28 isNamedExpression
// Indictates whether a node is named function or class expression.
pub fn is_named_expression(node: Node) -> bool {
    if node.is_nil() {
        return false;
    }
    if !is_function_expression(node) && !is_class_expression(node) {
        return false;
    }
    let name = node.name();
    name.is_some() && is_identifier(name)
}

// Go: ls/callhierarchy.go:39 isVariableLike
pub fn is_variable_like(node: Node) -> bool {
    if node.is_nil() {
        return false;
    }
    is_property_declaration(node) || is_variable_declaration(node)
}

// Go: ls/callhierarchy.go:47 isAssignedExpression
// Indicates whether a node is a function, arrow, or class expression assigned to a constant variable or class property.
pub fn is_assigned_expression(node: Node) -> bool {
    if node.is_nil() {
        return false;
    }
    if !(is_function_expression(node) || is_arrow_function(node) || is_class_expression(node)) {
        return false;
    }
    if node.name().is_some() {
        return false;
    }
    let parent = node.parent();
    if !is_variable_like(parent) {
        return false;
    }

    if parent.initializer() != node {
        return false;
    }

    let name = parent.name();
    if !is_identifier(name) {
        return false;
    }

    get_combined_node_flags(parent).intersects(NodeFlags::CONST) || is_property_declaration(parent)
}

// Go: ls/callhierarchy.go:77 isPossibleCallHierarchyDeclaration
// Indicates whether a node could possibly be a call hierarchy declaration.
//
// See `resolveCallHierarchyDeclaration` for the specific rules.
pub fn is_possible_call_hierarchy_declaration(node: Node) -> bool {
    if node.is_nil() {
        return false;
    }
    is_source_file(node)
        || is_module_declaration(node)
        || is_function_declaration(node)
        || is_function_expression(node)
        || is_class_declaration(node)
        || is_class_expression(node)
        || is_class_static_block_declaration(node)
        || is_method_declaration(node)
        || is_method_signature_declaration(node)
        || is_get_accessor_declaration(node)
        || is_set_accessor_declaration(node)
}

// Go: ls/callhierarchy.go:97 isValidCallHierarchyDeclaration
// Indicates whether a node is a valid a call hierarchy declaration.
//
// See `resolveCallHierarchyDeclaration` for the specific rules.
pub fn is_valid_call_hierarchy_declaration(node: Node) -> bool {
    if node.is_nil() {
        return false;
    }

    if is_source_file(node) {
        return true;
    }

    if is_module_declaration(node) {
        return is_identifier(node.name());
    }

    is_function_declaration(node)
        || is_class_declaration(node)
        || is_class_static_block_declaration(node)
        || is_method_declaration(node)
        || is_method_signature_declaration(node)
        || is_get_accessor_declaration(node)
        || is_set_accessor_declaration(node)
        || is_named_expression(node)
        || is_assigned_expression(node)
}

// Go: ls/callhierarchy.go:122 getCallHierarchyDeclarationReferenceNode
// Gets the node that can be used as a reference to a call hierarchy declaration.
pub fn get_call_hierarchy_declaration_reference_node(node: Node) -> Node {
    if node.is_nil() {
        return Node::NIL;
    }

    if is_source_file(node) {
        return node;
    }

    let name = node.name();
    if name.is_some() {
        return name;
    }

    if is_assigned_expression(node) {
        return node.parent().name();
    }

    let modifiers = node.modifiers();
    if modifiers.is_some() {
        for m in modifiers.nodes() {
            if m.kind() == SyntaxKind::DefaultKeyword {
                return m;
            }
        }
    }

    Node::NIL
}

// Go: ls/callhierarchy.go:151 getSymbolOfCallHierarchyDeclaration
// Gets the symbol for a call hierarchy declaration.
pub fn get_symbol_of_call_hierarchy_declaration(c: &mut Checker, node: Node) -> SymbolId {
    if is_class_static_block_declaration(node) {
        return SymbolId::NIL;
    }
    let location = get_call_hierarchy_declaration_reference_node(node);
    if location.is_nil() {
        return SymbolId::NIL;
    }
    c.get_symbol_at_location_exported(location)
}

// Go: ls/callhierarchy.go:163 getCallHierarchyItemName
// Gets the text and range for the name of a call hierarchy declaration.
// PORT: Go named results `(text string, pos int, end int)` are a tuple.
pub fn get_call_hierarchy_item_name(
    program: &compiler::NewProgram,
    node: Node,
) -> (String, i32, i32) {
    if is_source_file(node) {
        let source_file = node;
        return (source_file_file_name(source_file).to_string(), 0, 0);
    }

    if (is_function_declaration(node) || is_class_declaration(node)) && node.name().is_nil() {
        let modifiers = node.modifiers();
        if modifiers.is_some() {
            for m in modifiers.nodes() {
                if m.kind() == SyntaxKind::DefaultKeyword {
                    let source_file = get_source_file_of_node(node);
                    let start = skip_trivia(&source_file_text(source_file), m.pos());
                    return ("default".to_string(), start, m.end());
                }
            }
        }
    }

    if is_class_static_block_declaration(node) {
        let source_file = get_source_file_of_node(node);
        let pos = skip_trivia(
            &source_file_text(source_file),
            move_range_past_modifiers(node).pos(),
        );
        let end = pos + 6; // "static".length
        let (checker, _done) = ls_program::get_type_checker_for_file(
            program,
            &gostd::context::background(),
            source_file,
        );
        let prefix = {
            let c = &mut *checker.borrow_mut();
            let symbol = c.get_symbol_at_location_exported(node.parent());
            let mut prefix = String::new();
            if symbol.is_some() {
                prefix = c.symbol_to_string_exported(symbol) + " ";
            }
            prefix
        };
        return (prefix + "static {}", pos, end);
    }

    let decl_name = if is_assigned_expression(node) {
        node.parent().name()
    } else {
        get_name_of_declaration(node)
    };

    if decl_name.is_nil() || !node_is_present(decl_name) {
        let source_file = get_source_file_of_node(node);
        if is_function_declaration(node) || is_function_expression(node) {
            let kw_pos = skip_trivia(
                &source_file_text(source_file),
                move_range_past_modifiers(node).pos(),
            );
            return ("(anonymous)".to_string(), kw_pos, kw_pos + 8); // "function".length
        } else if is_class_declaration(node) || is_class_expression(node) {
            let kw_pos = skip_trivia(
                &source_file_text(source_file),
                move_range_past_modifiers(node).pos(),
            );
            return ("(anonymous)".to_string(), kw_pos, kw_pos + 5); // "class".length
        }
        crate::go_assert!(
            decl_name.is_some(),
            "Expected call hierarchy item to have a name"
        );
    }

    let text = get_text_of_call_hierarchy_name(program, node, decl_name, node);

    let source_file = get_source_file_of_node(node);
    let name_pos = skip_trivia(&source_file_text(source_file), decl_name.pos());

    (text, name_pos, decl_name.end())
}

// Go: ls/callhierarchy.go:223 getTextOfCallHierarchyName
pub fn get_text_of_call_hierarchy_name(
    program: &compiler::NewProgram,
    source_node: Node,
    name: Node,
    print_node: Node,
) -> String {
    if is_identifier(name) || is_string_or_numeric_literal_like(name) {
        return name.text().to_string();
    }
    if is_computed_property_name(name) {
        let expr = name.expression();
        if is_string_or_numeric_literal_like(expr) {
            return expr.text().to_string();
        }
    }

    let (checker, _done) = ls_program::get_type_checker_for_file(
        program,
        &gostd::context::background(),
        get_source_file_of_node(source_node),
    );
    {
        let c = &mut *checker.borrow_mut();
        let symbol = c.get_symbol_at_location_exported(name);
        if symbol.is_some() {
            let text = c.symbol_to_string_exported(symbol);
            if !text.is_empty() {
                return text;
            }
        }
    }

    let source_file = get_source_file_of_node(source_node);
    // PORT: Go `GetSingleLineStringWriter` returns a pooled writer and a put
    // func; the pool is not ported.
    let writer = Rc::new(RefCell::new(get_single_line_string_writer()));
    let mut p = new_printer(
        PrinterOptions {
            remove_comments: true,
            ..Default::default()
        },
        PrintHandlers::default(),
        None,
    );
    p.write_exported(print_node, source_file, writer.clone(), None);
    let text = writer.borrow().string();
    text
}

// Go: ls/callhierarchy.go:252 getCallHierarchyItemContainerName
pub fn get_call_hierarchy_item_container_name(
    program: &compiler::NewProgram,
    node: Node,
) -> String {
    if is_assigned_expression(node) {
        let parent = node.parent();
        if is_property_declaration(parent) && is_class_like(parent.parent()) {
            if is_class_expression(parent.parent()) {
                let assigned_name = get_assigned_name(parent.parent());
                if assigned_name.is_some() {
                    return get_text_of_call_hierarchy_name(
                        program,
                        node,
                        assigned_name,
                        assigned_name,
                    );
                }
            } else {
                let name = parent.parent().name();
                if name.is_some() {
                    return get_text_of_call_hierarchy_name(program, node, name, name);
                }
            }
        }
        if parent.parent().parent().is_some()
            && parent.parent().parent().parent().is_some()
            && is_module_block(parent.parent().parent().parent())
        {
            let mod_parent = parent.parent().parent().parent().parent();
            if is_module_declaration(mod_parent) {
                let name = mod_parent.name();
                if name.is_some() && is_identifier(name) {
                    return name.text().to_string();
                }
            }
        }
        return String::new();
    }

    match node.kind() {
        SyntaxKind::GetAccessor | SyntaxKind::SetAccessor | SyntaxKind::MethodDeclaration => {
            if node.parent().kind() == SyntaxKind::ObjectLiteralExpression {
                let assigned_name = get_assigned_name(node.parent());
                if assigned_name.is_some() {
                    return get_text_of_call_hierarchy_name(
                        program,
                        node,
                        assigned_name,
                        assigned_name,
                    );
                }
            }
            let name = get_name_of_declaration(node.parent());
            if name.is_some() {
                return get_text_of_call_hierarchy_name(program, node, name, name);
            }
        }
        SyntaxKind::FunctionDeclaration
        | SyntaxKind::ClassDeclaration
        | SyntaxKind::ModuleDeclaration => {
            if is_module_block(node.parent()) {
                if is_module_declaration(node.parent().parent()) {
                    let name = node.parent().parent().name();
                    if name.is_some() && is_identifier(name) {
                        return name.text().to_string();
                    }
                }
            }
        }
        _ => {}
    }

    String::new()
}

// Go: ls/callhierarchy.go:300 moveRangePastModifiers
pub fn move_range_past_modifiers(node: Node) -> TextRange {
    let modifiers = node.modifiers();
    if modifiers.is_some() && !modifiers.nodes().is_empty() {
        let nodes = modifiers.nodes();
        let last_mod = nodes.get(nodes.len() - 1);
        return TextRange::new(last_mod.end(), node.end());
    }
    TextRange::new(node.pos(), node.end())
}

// Go: ls/callhierarchy.go:309 findImplementation
// Finds the implementation of a function-like declaration, if one exists.
pub fn find_implementation(c: &mut Checker, node: Node) -> Node {
    if node.is_nil() {
        return Node::NIL;
    }

    if !is_function_like_declaration(node) {
        return node;
    }

    if node.body().is_some() {
        return node;
    }

    if is_constructor_declaration(node) {
        return get_first_constructor_with_body(node.parent());
    }

    if is_function_declaration(node) || is_method_declaration(node) {
        let symbol = get_symbol_of_call_hierarchy_declaration(c, node);
        if symbol.is_some() {
            let value_declaration = c.sym(symbol).value_declaration;
            if value_declaration.is_some()
                && is_function_like_declaration(value_declaration)
                && value_declaration.body().is_some()
            {
                return value_declaration;
            }
        }
        return Node::NIL;
    }

    node
}

// Go: ls/callhierarchy.go:339 findAllInitialDeclarations
// PORT: Go returns nil or a non-empty slice; nil is an empty `Vec`.
pub fn find_all_initial_declarations(c: &mut Checker, node: Node) -> Vec<Node> {
    if is_class_static_block_declaration(node) {
        return Vec::new();
    }

    let symbol = get_symbol_of_call_hierarchy_declaration(c, node);
    // PORT: Go tests `symbol.Declarations == nil`; an empty list is the same here.
    if symbol.is_nil() || c.sym(symbol).declarations.is_empty() {
        return Vec::new();
    }
    let symbol_declarations: Vec<Node> = c.sym(symbol).declarations.to_vec();

    // Go: callhierarchy.go:349 declKey
    struct DeclKey {
        file: &'static str,
        pos: i32,
    }

    let mut indices: Vec<usize> = (0..symbol_declarations.len()).collect();
    let keys: Vec<DeclKey> = symbol_declarations
        .iter()
        .map(|&decl| DeclKey {
            file: source_file_file_name(get_source_file_of_node(decl)),
            pos: decl.pos(),
        })
        .collect();

    gostd::slices::sort_func(&mut indices, |&a, &b| {
        if keys[a].file != keys[b].file {
            return keys[a].file.cmp(keys[b].file) as i32;
        }
        keys[a].pos - keys[b].pos
    });

    let mut declarations: Vec<Node> = Vec::new();
    let mut last_decl = Node::NIL;

    for i in indices {
        let decl = symbol_declarations[i];
        if is_valid_call_hierarchy_declaration(decl) {
            if last_decl.is_nil()
                || last_decl.parent() != decl.parent()
                || last_decl.end() != decl.pos()
            {
                declarations.push(decl);
            }
            last_decl = decl;
        }
    }

    declarations
}

// Go: ls/callhierarchy.go:390 findImplementationOrAllInitialDeclarations
// Find the implementation or the first declaration for a call hierarchy declaration.
// PORT: Go returns `any` (see `CallHierarchyDeclarationOrDeclarations`).
pub fn find_implementation_or_all_initial_declarations(
    c: &mut Checker,
    node: Node,
) -> CallHierarchyDeclarationOrDeclarations {
    if is_class_static_block_declaration(node) {
        return CallHierarchyDeclarationOrDeclarations::Node(node);
    }

    if is_function_like_declaration(node) {
        let impl_ = find_implementation(c, node);
        if impl_.is_some() {
            return CallHierarchyDeclarationOrDeclarations::Node(impl_);
        }
        let decls = find_all_initial_declarations(c, node);
        if !decls.is_empty() {
            return CallHierarchyDeclarationOrDeclarations::Nodes(decls);
        }
        return CallHierarchyDeclarationOrDeclarations::Node(node);
    }

    let decls = find_all_initial_declarations(c, node);
    if !decls.is_empty() {
        return CallHierarchyDeclarationOrDeclarations::Nodes(decls);
    }
    CallHierarchyDeclarationOrDeclarations::Node(node)
}

// Go: ls/callhierarchy.go:412 resolveCallHierarchyDeclaration
// Resolves the call hierarchy declaration for a node.
// PORT: Go returns `any`; a nil result is `None`.
pub fn resolve_call_hierarchy_declaration(
    program: &compiler::NewProgram,
    location: Node,
) -> Option<CallHierarchyDeclarationOrDeclarations> {
    // A call hierarchy item must refer to either a SourceFile, Module Declaration, Class Static Block, or something intrinsically callable that has a name:
    // - Class Declarations
    // - Class Expressions (with a name)
    // - Function Declarations
    // - Function Expressions (with a name or assigned to a const variable)
    // - Arrow Functions (assigned to a const variable)
    // - Constructors
    // - Class `static {}` initializer blocks
    // - Methods
    // - Accessors
    //
    // If a call is contained in a non-named callable Node (function expression, arrow function, etc.), then
    // its containing `CallHierarchyItem` is a containing function or SourceFile that matches the above list.

    let (checker, _done) = ls_program::get_type_checker(program, &gostd::context::background());
    let c = &mut *checker.borrow_mut();

    let mut location = location;
    let mut following_symbol = false;

    while location.is_some() {
        if is_valid_call_hierarchy_declaration(location) {
            return Some(find_implementation_or_all_initial_declarations(c, location));
        }

        if is_possible_call_hierarchy_declaration(location) {
            let ancestor = find_ancestor(location, is_valid_call_hierarchy_declaration);
            if ancestor.is_some() {
                return Some(find_implementation_or_all_initial_declarations(c, ancestor));
            }
        }

        if is_declaration_name(location) {
            if is_valid_call_hierarchy_declaration(location.parent()) {
                return Some(find_implementation_or_all_initial_declarations(
                    c,
                    location.parent(),
                ));
            }
            if is_possible_call_hierarchy_declaration(location.parent()) {
                let ancestor =
                    find_ancestor(location.parent(), is_valid_call_hierarchy_declaration);
                if ancestor.is_some() {
                    return Some(find_implementation_or_all_initial_declarations(c, ancestor));
                }
            }
            if is_variable_like(location.parent()) {
                let initializer = location.parent().initializer();
                if initializer.is_some() && is_assigned_expression(initializer) {
                    return Some(CallHierarchyDeclarationOrDeclarations::Node(initializer));
                }
            }
            return None;
        }

        if is_constructor_declaration(location) {
            if is_valid_call_hierarchy_declaration(location.parent()) {
                return Some(CallHierarchyDeclarationOrDeclarations::Node(
                    location.parent(),
                ));
            }
            return None;
        }

        if location.kind() == SyntaxKind::StaticKeyword
            && is_class_static_block_declaration(location.parent())
        {
            location = location.parent();
            continue;
        }

        // #39453
        if is_variable_declaration(location) {
            let initializer = location.initializer();
            if initializer.is_some() && is_assigned_expression(initializer) {
                return Some(CallHierarchyDeclarationOrDeclarations::Node(initializer));
            }
        }

        if !following_symbol {
            let mut symbol = c.get_symbol_at_location_exported(location);
            if symbol.is_some() {
                if c.sym(symbol).flags.intersects(SymbolFlags::ALIAS) {
                    symbol = c.get_aliased_symbol(symbol);
                }
                let value_declaration = c.sym(symbol).value_declaration;
                if value_declaration.is_some() {
                    following_symbol = true;
                    location = value_declaration;
                    continue;
                }
            }
        }

        return None;
    }

    None
}

impl LanguageService {
    // Go: ls/callhierarchy.go:503 createCallHierarchyItem
    // Creates a `CallHierarchyItem` for a call hierarchy declaration.
    // PORT: Go returns `*lsproto.CallHierarchyItem`; nil is `None`.
    pub fn create_call_hierarchy_item(
        &self,
        program: &compiler::NewProgram,
        node: Node,
    ) -> Option<lsproto::CallHierarchyItem> {
        let source_file = get_source_file_of_node(node);
        let (name_text, name_pos, name_end) = get_call_hierarchy_item_name(program, node);
        let container_name = get_call_hierarchy_item_container_name(program, node);

        let kind = get_symbol_kind_from_node(node);

        let full_start = skip_trivia_ex(
            &source_file_text(source_file),
            node.pos(),
            Some(&SkipTriviaOptions {
                stop_at_comments: true,
                ..Default::default()
            }),
        );
        let (mut span, span_fidelity) = self.converters.to_lsp_range_for_feature(
            &source_file,
            TextRange::new(full_start, node.end()),
            Feature::CALL_HIERARCHY,
        );
        let (selection_span, selection_fidelity) = self.converters.to_lsp_range_for_feature(
            &source_file,
            TextRange::new(name_pos, name_end),
            Feature::CALL_HIERARCHY,
        );
        if !selection_fidelity.is_single_segment() {
            return None;
        }
        if span_fidelity.is_none()
            || !source_file_content_mapper(source_file).is_empty()
                && !lsp_range_contains(span, selection_span)
        {
            span = selection_span;
        }

        let mut item = lsproto::CallHierarchyItem {
            name: name_text,
            kind,
            uri: lsconv::file_name_to_document_uri(source_file_original_file_name(source_file)),
            range: span,
            selection_range: selection_span,
            ..Default::default()
        };

        if !container_name.is_empty() {
            item.detail = Some(container_name);
        }

        Some(item)
    }
}

// Go: ls/callhierarchy.go:535 callSite
#[derive(Clone, Copy, Debug)]
pub struct CallSite {
    pub declaration: Node,
    pub text_range: TextRange,
    pub source_file: Node,
}

// Go: ls/callhierarchy.go:541 convertEntryToCallSite
pub fn convert_entry_to_call_site(entry: &Rc<RefCell<ReferenceEntry>>) -> Option<CallSite> {
    let (kind, node) = {
        let e = entry.borrow();
        (e.kind, e.node)
    };
    if kind != EntryKind::NODE {
        return None;
    }

    // PORT: Go calls `ast.IsRightSideOfPropertyAccess`; the ls package has its
    // own `isRightSideOfPropertyAccess`, so the ast one is named by path.
    if !is_call_or_new_expression_target(
        node, true, /*includeElementAccess*/
        true, /*skipPastOuterExpressions*/
    ) && !is_tagged_template_tag(node, true, true)
        && !is_decorator_target(node, true, true)
        && !is_jsx_opening_like_element_tag_name(node, true, true)
        && !crate::ast::is_right_side_of_property_access(node)
        && !is_argument_expression_of_element_access(node)
    {
        return None;
    }

    let source_file = get_source_file_of_node(node);
    let mut ancestor = find_ancestor(node, is_valid_call_hierarchy_declaration);
    if ancestor.is_nil() {
        ancestor = source_file;
    }

    let start = skip_trivia(&source_file_text(source_file), node.pos());
    Some(CallSite {
        declaration: ancestor,
        text_range: TextRange::new(start, node.end()),
        source_file,
    })
}

// Go: ls/callhierarchy.go:570 getCallSiteGroupKey
// PORT: Go `ast.NodeId` is the `u64` from `get_node_id`.
pub fn get_call_site_group_key(site: &CallSite) -> u64 {
    get_node_id(site.declaration)
}

impl LanguageService {
    // Go: ls/callhierarchy.go:574 convertCallSiteGroupToIncomingCall
    // PORT: Go returns `*lsproto.CallHierarchyIncomingCall`; nil is `None`.
    pub fn convert_call_site_group_to_incoming_call(
        &self,
        program: &compiler::NewProgram,
        entries: &[CallSite],
    ) -> Option<lsproto::CallHierarchyIncomingCall> {
        let mut from_ranges: Vec<lsproto::Range> = Vec::with_capacity(entries.len());
        for entry in entries {
            let source_file = entry.source_file;
            let (lsp_range, fidelity) = self.converters.to_lsp_range_for_feature(
                &source_file,
                entry.text_range,
                Feature::CALL_HIERARCHY,
            );
            if !fidelity.is_none() {
                from_ranges.push(lsp_range);
            }
        }
        let from = self.create_call_hierarchy_item(program, entries[0].declaration);
        if from.is_none() || from_ranges.is_empty() {
            return None;
        }

        gostd::slices::sort_func(&mut from_ranges, |a, b| lsproto::compare_ranges(*a, *b));

        Some(lsproto::CallHierarchyIncomingCall { from, from_ranges })
    }
}

// Go: ls/callhierarchy.go:595 incomingEntry
// PORT: the three `sync.Once` + value pairs are `OnceCell` fields filled on
// first use.
pub struct IncomingEntry<'a> {
    pub ls: &'a LanguageService,
    pub node: Node,

    pub source_file: OnceCell<Node>,

    pub document_uri: OnceCell<lsproto::DocumentUri>,

    pub position: OnceCell<lsproto::Position>,
}

impl IncomingEntry<'_> {
    // Go: ls/callhierarchy.go:611 getSourceFile
    pub fn get_source_file(&self) -> Node {
        *self
            .source_file
            .get_or_init(|| get_source_file_of_node(self.node))
    }
}

// Go: ls/callhierarchy.go:609 var _ lsproto.HasTextDocumentPosition = (*incomingEntry)(nil)
impl lsproto::HasTextDocumentURI for IncomingEntry<'_> {
    // Go: ls/callhierarchy.go:618 TextDocumentURI
    fn text_document_uri(&self) -> lsproto::DocumentUri {
        self.document_uri
            .get_or_init(|| {
                lsconv::file_name_to_document_uri(source_file_original_file_name(
                    self.get_source_file(),
                ))
            })
            .clone()
    }
}

impl lsproto::HasTextDocumentPosition for IncomingEntry<'_> {
    // Go: ls/callhierarchy.go:625 TextDocumentPosition
    fn text_document_position(&self) -> lsproto::Position {
        *self.position.get_or_init(|| {
            let start = get_token_pos_of_node(
                self.node,
                self.get_source_file(),
                false, /*includeJsDoc*/
            );
            self.ls.create_lsp_position(start, self.get_source_file()).0
        })
    }
}

impl LanguageService {
    // Go: ls/callhierarchy.go:634 getIncomingCalls
    // Gets the call sites that call into the provided call hierarchy declaration.
    pub fn get_incoming_calls(
        &self,
        ctx: &Context,
        program: &compiler::NewProgram,
        declaration: Node,
        orchestrator: Option<&dyn CrossProjectOrchestrator>,
    ) -> Result<lsproto::CallHierarchyIncomingCallsResponse, GoError> {
        // Source files and modules have no incoming calls.
        if is_source_file(declaration)
            || is_module_declaration(declaration)
            || is_class_static_block_declaration(declaration)
        {
            return Ok(lsproto::CallHierarchyIncomingCallsOrNull::default());
        }

        let location = get_call_hierarchy_declaration_reference_node(declaration);
        if location.is_nil() {
            return Ok(lsproto::CallHierarchyIncomingCallsOrNull::default());
        }
        let location_file = get_source_file_of_node(location);
        let location_start =
            get_token_pos_of_node(location, location_file, false /*includeJsDoc*/);
        if self
            .converters
            .to_lsp_position_for_feature(&location_file, location_start, Feature::CALL_HIERARCHY)
            .1
            .is_none()
        {
            return Ok(lsproto::CallHierarchyIncomingCallsOrNull::default());
        }

        let incoming_entry = IncomingEntry {
            ls: self,
            node: location,
            source_file: OnceCell::new(),
            document_uri: OnceCell::new(),
            position: OnceCell::new(),
        };

        // PORT: Go returns `(result, err)` and sorts `result` before returning
        // both. On an error Go's `result` is the zero value, so the sort does
        // nothing and `?` returns the same error.
        let mut result = handle_cross_project(
            self,
            ctx,
            &incoming_entry,
            orchestrator,
            LanguageService::symbol_and_entries_to_incoming_calls,
            None, /*search*/
            combine_incoming_calls,
            false,
            false,
            SymbolEntryTransformOptions::default(),
            None, /*defaultProjectData*/
        )?;
        if let Some(calls) = result.call_hierarchy_incoming_calls.as_mut() {
            gostd::slices::sort_func(calls, |a, b| {
                let a_from = a
                    .from
                    .as_ref()
                    .unwrap_or_else(|| crate::core::go_nil_dereference());
                let b_from = b
                    .from
                    .as_ref()
                    .unwrap_or_else(|| crate::core::go_nil_dereference());
                let uri_comp = a_from.uri.0.as_str().cmp(b_from.uri.0.as_str()) as i32;
                if uri_comp != 0 {
                    return uri_comp;
                }
                if a.from_ranges.is_empty() || b.from_ranges.is_empty() {
                    return 0;
                }
                lsproto::compare_ranges(a.from_ranges[0], b.from_ranges[0])
            });
        }
        Ok(result)
    }

    // Go: ls/callhierarchy.go:680 symbolAndEntriesToIncomingCalls
    pub fn symbol_and_entries_to_incoming_calls(
        &self,
        ctx: &Context,
        params: &IncomingEntry<'_>,
        data: SymbolAndEntriesData,
        options: SymbolEntryTransformOptions,
    ) -> Result<lsproto::CallHierarchyIncomingCallsResponse, GoError> {
        let program = self.get_program();
        let mut ref_entries: Vec<Rc<RefCell<ReferenceEntry>>> = Vec::new();
        for symbol_and_entry in &data.symbols_and_entries {
            ref_entries.extend(symbol_and_entry.borrow().references.iter().cloned());
        }

        let mut call_sites: Vec<CallSite> = Vec::new();
        for entry in &ref_entries {
            if let Some(site) = convert_entry_to_call_site(entry) {
                call_sites.push(site);
            }
        }

        if call_sites.is_empty() {
            return Ok(lsproto::CallHierarchyIncomingCallsOrNull::default());
        }

        // PORT: Go map order is random; IndexMap keeps insertion order. The
        // caller sorts the calls, so only ties can differ from Go.
        let mut grouped: IndexMap<u64, Vec<CallSite>> = IndexMap::default();
        for site in call_sites {
            let key = get_call_site_group_key(&site);
            grouped.entry(key).or_default().push(site);
        }

        // PORT: Go returns `&result`; a nil `result` (no call kept) is an
        // empty `Vec` here. The provider drops an empty list.
        let mut result: Vec<lsproto::CallHierarchyIncomingCall> = Vec::new();
        for sites in grouped.values() {
            if let Some(incoming_call) =
                self.convert_call_site_group_to_incoming_call(program, sites)
            {
                result.push(incoming_call);
            }
        }
        Ok(lsproto::CallHierarchyIncomingCallsOrNull {
            call_hierarchy_incoming_calls: Some(result),
        })
    }
}

// Go: ls/callhierarchy.go:713 callSiteCollector
pub struct CallSiteCollector<'a> {
    pub program: &'a compiler::NewProgram,
    pub call_sites: Vec<CallSite>,
}

impl CallSiteCollector<'_> {
    // Go: ls/callhierarchy.go:718 recordCallSite
    pub fn record_call_site(&mut self, node: Node) {
        let mut target = Node::NIL;

        // PORT: Go calls `ast.IsTaggedTemplateExpression`; the ls package has
        // its own `isTaggedTemplateExpression`, so the ast one is named by path.
        if crate::ast::is_tagged_template_expression(node) {
            target = node.tag();
        } else if is_jsx_opening_element(node) {
            target = node.tag_name();
        } else if is_jsx_self_closing_element(node) {
            target = node.tag_name();
        } else if is_property_access_expression(node) || is_element_access_expression(node) {
            target = node;
        } else if is_class_static_block_declaration(node) {
            target = node;
        } else if is_call_expression(node) {
            target = node.expression();
        } else if is_new_expression(node) {
            target = node.expression();
        } else if is_decorator(node) {
            target = node.expression();
        }

        if target.is_nil() {
            return;
        }

        let Some(declaration) = resolve_call_hierarchy_declaration(self.program, target) else {
            return;
        };

        let source_file = get_source_file_of_node(target);
        let start = skip_trivia(&source_file_text(source_file), target.pos());
        let text_range = TextRange::new(start, target.end());

        match declaration {
            CallHierarchyDeclarationOrDeclarations::Node(decl) => {
                self.call_sites.push(CallSite {
                    declaration: decl,
                    text_range,
                    source_file,
                });
            }
            CallHierarchyDeclarationOrDeclarations::Nodes(decl) => {
                for d in decl {
                    self.call_sites.push(CallSite {
                        declaration: d,
                        text_range,
                        source_file,
                    });
                }
            }
        }
    }

    // Go: ls/callhierarchy.go:771 collect
    pub fn collect(&mut self, node: Node) {
        if node.is_nil() {
            return;
        }

        // do not descend into ambient nodes.
        if node.flags().intersects(NodeFlags::AMBIENT) {
            return;
        }

        // do not descend into other call site declarations, other than class member names
        if is_valid_call_hierarchy_declaration(node) {
            if is_class_like(node) {
                for member in node.members() {
                    if member.name().is_some() && is_computed_property_name(member.name()) {
                        self.collect(member.name().expression());
                    }
                }
            }
            return;
        }

        match node.kind() {
            SyntaxKind::Identifier
            | SyntaxKind::ImportEqualsDeclaration
            | SyntaxKind::ImportDeclaration
            | SyntaxKind::ExportDeclaration
            | SyntaxKind::InterfaceDeclaration
            | SyntaxKind::TypeAliasDeclaration => {
                // do not descend into nodes that cannot contain callable nodes
                return;
            }
            SyntaxKind::ClassStaticBlockDeclaration => {
                self.record_call_site(node);
                return;
            }
            SyntaxKind::TypeAssertionExpression | SyntaxKind::AsExpression => {
                // do not descend into the type side of an assertion
                self.collect(node.expression());
                return;
            }
            SyntaxKind::VariableDeclaration | SyntaxKind::Parameter => {
                // do not descend into the type of a variable or parameter declaration
                self.collect(node.name());
                self.collect(node.initializer());
                return;
            }
            SyntaxKind::CallExpression => {
                // do not descend into the type arguments of a call expression
                self.record_call_site(node);
                self.collect(node.expression());
                for arg in node.arguments() {
                    self.collect(arg);
                }
                return;
            }
            SyntaxKind::NewExpression => {
                // do not descend into the type arguments of a new expression
                self.record_call_site(node);
                self.collect(node.expression());
                for arg in node.arguments() {
                    self.collect(arg);
                }
                return;
            }
            SyntaxKind::TaggedTemplateExpression => {
                // do not descend into the type arguments of a tagged template expression
                self.record_call_site(node);
                let tagged_template = node;
                self.collect(tagged_template.tag());
                self.collect(tagged_template.template());
                return;
            }
            SyntaxKind::JsxOpeningElement | SyntaxKind::JsxSelfClosingElement => {
                // do not descend into the type arguments of a JsxOpeningLikeElement
                self.record_call_site(node);
                self.collect(node.tag_name());
                self.collect(node.attributes());
                return;
            }
            SyntaxKind::Decorator => {
                self.record_call_site(node);
                self.collect(node.expression());
                return;
            }
            SyntaxKind::PropertyAccessExpression | SyntaxKind::ElementAccessExpression => {
                self.record_call_site(node);
                node.for_each_child(|child| {
                    self.collect(child);
                    false
                });
                return;
            }
            SyntaxKind::SatisfiesExpression => {
                // do not descend into the type side of an assertion
                self.collect(node.expression());
                return;
            }
            _ => {}
        }

        if is_part_of_type_node(node) {
            // do not descend into types
            return;
        }

        node.for_each_child(|child| {
            self.collect(child);
            false
        });
    }
}

// Go: ls/callhierarchy.go:871 collectCallSites
// PORT: Go passes `c *checker.Checker`, leased by `getOutgoingCalls`. The
// collector calls `resolveCallHierarchyDeclaration`, which leases the program
// checker again (Go shares one checker between both leases). A Rust
// `RefCell` borrow held across that call would be a second borrow, so this
// takes the leased `Rc` and borrows it only for `findImplementation`.
pub fn collect_call_sites(
    program: &compiler::NewProgram,
    c: &Rc<RefCell<Checker>>,
    node: Node,
) -> Vec<CallSite> {
    let mut collector = CallSiteCollector {
        program,
        call_sites: Vec::new(),
    };

    match node.kind() {
        SyntaxKind::SourceFile => {
            for stmt in node.statements() {
                collector.collect(stmt);
            }
        }

        SyntaxKind::ModuleDeclaration => {
            let body = node.body();
            if !has_syntactic_modifier(node, ModifierFlags::AMBIENT)
                && body.is_some()
                && is_module_block(body)
            {
                for stmt in body.statements() {
                    collector.collect(stmt);
                }
            }
        }

        SyntaxKind::FunctionDeclaration
        | SyntaxKind::FunctionExpression
        | SyntaxKind::ArrowFunction
        | SyntaxKind::MethodDeclaration
        | SyntaxKind::GetAccessor
        | SyntaxKind::SetAccessor => {
            let impl_ = find_implementation(&mut c.borrow_mut(), node);
            if impl_.is_some() {
                for param in impl_.parameters() {
                    collector.collect(param);
                }
                collector.collect(impl_.body());
            }
        }

        SyntaxKind::ClassDeclaration | SyntaxKind::ClassExpression => {
            let modifiers = node.modifiers();
            if modifiers.is_some() {
                for m in modifiers.nodes() {
                    collector.collect(m);
                }
            }

            let heritage = get_class_extends_heritage_element(node);
            if heritage.is_some() {
                collector.collect(heritage.expression());
            }

            for member in node.members() {
                if can_have_modifiers(member) && member.modifiers().is_some() {
                    for m in member.modifiers().nodes() {
                        collector.collect(m);
                    }
                }

                if is_property_declaration(member) {
                    collector.collect(member.initializer());
                } else if is_constructor_declaration(member) {
                    let body = member.body();
                    if body.is_some() {
                        for param in member.parameters() {
                            collector.collect(param);
                        }
                        collector.collect(body);
                    }
                } else if is_class_static_block_declaration(member) {
                    collector.collect(member);
                }
            }
        }

        SyntaxKind::ClassStaticBlockDeclaration => {
            let static_block = node;
            collector.collect(static_block.body());
        }

        _ => {
            crate::gostd::debug::assert_never(&crate::gostd::debug::kind_string(node.kind()), None)
        }
    }

    collector.call_sites
}

impl LanguageService {
    // Go: ls/callhierarchy.go:944 convertCallSiteGroupToOutgoingCall
    // PORT: Go returns `*lsproto.CallHierarchyOutgoingCall`; nil is `None`.
    pub fn convert_call_site_group_to_outgoing_call(
        &self,
        program: &compiler::NewProgram,
        entries: &[CallSite],
    ) -> Option<lsproto::CallHierarchyOutgoingCall> {
        let mut from_ranges: Vec<lsproto::Range> = Vec::with_capacity(entries.len());
        for entry in entries {
            let source_file = entry.source_file;
            let (lsp_range, fidelity) = self.converters.to_lsp_range_for_feature(
                &source_file,
                entry.text_range,
                Feature::CALL_HIERARCHY,
            );
            if !fidelity.is_none() {
                from_ranges.push(lsp_range);
            }
        }
        let to = self.create_call_hierarchy_item(program, entries[0].declaration);
        if to.is_none() || from_ranges.is_empty() {
            return None;
        }

        gostd::slices::sort_func(&mut from_ranges, |a, b| lsproto::compare_ranges(*a, *b));

        Some(lsproto::CallHierarchyOutgoingCall { to, from_ranges })
    }

    // Go: ls/callhierarchy.go:966 getOutgoingCalls
    // Gets the call sites that call out of the provided call hierarchy declaration.
    // PORT: Go returns nil or a non-empty slice; nil is an empty `Vec`.
    pub fn get_outgoing_calls(
        &self,
        program: &compiler::NewProgram,
        declaration: Node,
    ) -> Vec<lsproto::CallHierarchyOutgoingCall> {
        if declaration.flags().intersects(NodeFlags::AMBIENT)
            || is_method_signature_declaration(declaration)
        {
            return Vec::new();
        }

        let (checker, _done) = ls_program::get_type_checker(program, &gostd::context::background());

        let call_sites = collect_call_sites(program, &checker, declaration);

        if call_sites.is_empty() {
            return Vec::new();
        }

        // PORT: Go map order is random; IndexMap keeps insertion order. The
        // result is sorted below, so only ties can differ from Go.
        let mut grouped: IndexMap<u64, Vec<CallSite>> = IndexMap::default();
        for site in call_sites {
            let key = get_call_site_group_key(&site);
            grouped.entry(key).or_default().push(site);
        }

        let mut result: Vec<lsproto::CallHierarchyOutgoingCall> = Vec::new();
        for sites in grouped.values() {
            if let Some(outgoing_call) =
                self.convert_call_site_group_to_outgoing_call(program, sites)
            {
                result.push(outgoing_call);
            }
        }

        gostd::slices::sort_func(&mut result, |a, b| {
            let a_to =
                a.to.as_ref()
                    .unwrap_or_else(|| crate::core::go_nil_dereference());
            let b_to =
                b.to.as_ref()
                    .unwrap_or_else(|| crate::core::go_nil_dereference());
            let uri_comp = a_to.uri.0.as_str().cmp(b_to.uri.0.as_str()) as i32;
            if uri_comp != 0 {
                return uri_comp;
            }
            if a.from_ranges.is_empty() || b.from_ranges.is_empty() {
                return 0;
            }
            lsproto::compare_ranges(a.from_ranges[0], b.from_ranges[0])
        });

        result
    }

    // Go: ls/callhierarchy.go:1006 ProvidePrepareCallHierarchy
    pub fn provide_prepare_call_hierarchy(
        &self,
        ctx: &Context,
        document_uri: &lsproto::DocumentUri,
        position: lsproto::Position,
    ) -> Result<lsproto::CallHierarchyPrepareResponse, GoError> {
        let (program, file) = self.get_program_and_file(document_uri);
        let declarations = self.call_hierarchy_declarations(file, position, program, false);
        let mut items: Vec<lsproto::CallHierarchyItem> = Vec::new();
        let mut seen: FxHashSet<lsproto::Location> = FxHashSet::default();
        for declaration in declarations {
            if let Some(item) = self.create_call_hierarchy_item(program, declaration) {
                let location = lsproto::Location {
                    uri: item.uri.clone(),
                    range: item.selection_range,
                };
                if seen.insert(location) {
                    items.push(item);
                }
            }
        }

        // PORT: Go `items == nil`; no item was added.
        if items.is_empty() {
            return Ok(lsproto::CallHierarchyItemsOrNull::default());
        }
        Ok(lsproto::CallHierarchyItemsOrNull {
            call_hierarchy_items: Some(items),
        })
    }

    // Go: ls/callhierarchy.go:1030 ProvideCallHierarchyIncomingCalls
    pub fn provide_call_hierarchy_incoming_calls(
        &self,
        ctx: &Context,
        item: &lsproto::CallHierarchyItem,
        orchestrator: Option<&dyn CrossProjectOrchestrator>,
    ) -> Result<lsproto::CallHierarchyIncomingCallsResponse, GoError> {
        let program = self.get_program();
        let file_name = item.uri.file_name();
        // PORT: `NewProgram::get_source_file` returns the parsed file; the Go
        // `*ast.SourceFile` is its root node.
        let Some(file) = program.get_source_file(&file_name).map(|f| f.root) else {
            return Ok(lsproto::CallHierarchyIncomingCallsOrNull::default());
        };

        let declarations =
            self.call_hierarchy_declarations(file, item.selection_range.start, program, true);
        // PORT: Go keeps pointers to the calls in `seen` and appends to
        // `existing.FromRanges` through them; here `seen` holds the index of
        // the call in `calls`.
        let mut calls: Vec<lsproto::CallHierarchyIncomingCall> = Vec::new();
        let mut seen: FxHashMap<lsproto::Location, usize> = FxHashMap::default();
        for declaration in declarations {
            let response = self.get_incoming_calls(ctx, program, declaration, orchestrator)?;
            if let Some(response_calls) = response.call_hierarchy_incoming_calls {
                for call in response_calls {
                    let from = call
                        .from
                        .as_ref()
                        .unwrap_or_else(|| crate::core::go_nil_dereference());
                    let location = lsproto::Location {
                        uri: from.uri.clone(),
                        range: from.selection_range,
                    };
                    if let Some(&existing) = seen.get(&location) {
                        for from_range in call.from_ranges {
                            if !calls[existing].from_ranges.contains(&from_range) {
                                calls[existing].from_ranges.push(from_range);
                            }
                        }
                    } else {
                        seen.insert(location, calls.len());
                        calls.push(call);
                    }
                }
            }
        }
        if calls.is_empty() {
            return Ok(lsproto::CallHierarchyIncomingCallsOrNull::default());
        }
        Ok(lsproto::CallHierarchyIncomingCallsOrNull {
            call_hierarchy_incoming_calls: Some(calls),
        })
    }

    // Go: ls/callhierarchy.go:1072 ProvideCallHierarchyOutgoingCalls
    pub fn provide_call_hierarchy_outgoing_calls(
        &self,
        ctx: &Context,
        item: &lsproto::CallHierarchyItem,
    ) -> Result<lsproto::CallHierarchyOutgoingCallsResponse, GoError> {
        let program = self.get_program();
        let file_name = item.uri.file_name();
        // PORT: see `provide_call_hierarchy_incoming_calls`.
        let Some(file) = program.get_source_file(&file_name).map(|f| f.root) else {
            return Ok(lsproto::CallHierarchyOutgoingCallsOrNull::default());
        };

        let declarations =
            self.call_hierarchy_declarations(file, item.selection_range.start, program, true);
        // PORT: `seen` holds the index of the call in `calls`, as in
        // `provide_call_hierarchy_incoming_calls`.
        let mut calls: Vec<lsproto::CallHierarchyOutgoingCall> = Vec::new();
        let mut seen: FxHashMap<lsproto::Location, usize> = FxHashMap::default();
        for declaration in declarations {
            for call in self.get_outgoing_calls(program, declaration) {
                let to = call
                    .to
                    .as_ref()
                    .unwrap_or_else(|| crate::core::go_nil_dereference());
                let location = lsproto::Location {
                    uri: to.uri.clone(),
                    range: to.selection_range,
                };
                if let Some(&existing) = seen.get(&location) {
                    for from_range in call.from_ranges {
                        if !calls[existing].from_ranges.contains(&from_range) {
                            calls[existing].from_ranges.push(from_range);
                        }
                    }
                } else {
                    seen.insert(location, calls.len());
                    calls.push(call);
                }
            }
        }
        if calls.is_empty() {
            return Ok(lsproto::CallHierarchyOutgoingCallsOrNull::default());
        }
        Ok(lsproto::CallHierarchyOutgoingCallsOrNull {
            call_hierarchy_outgoing_calls: Some(calls),
        })
    }

    // Go: ls/callhierarchy.go:1107 callHierarchyDeclarations
    pub fn call_hierarchy_declarations(
        &self,
        file: Node,
        position: lsproto::Position,
        program: &compiler::NewProgram,
        allow_source_file: bool,
    ) -> Vec<Node> {
        let positions = lsconv::from_lsp_position_for_source_file(
            &self.converters,
            file,
            position,
            Feature::CALL_HIERARCHY,
        );
        let mut declarations: Vec<Node> = Vec::new();
        let mut seen: FxHashSet<Node> = FxHashSet::default();
        for mapped in positions {
            if !mapped.fidelity.is_single_segment() {
                continue;
            }
            let file = mapped.script;
            let pos = mapped.position;
            let mut node = file;
            if pos != 0 {
                node = astnav::get_touching_property_name(file, pos);
            }
            if node.is_nil() || !allow_source_file && node.kind() == SyntaxKind::SourceFile {
                continue;
            }
            match resolve_call_hierarchy_declaration(program, node) {
                Some(CallHierarchyDeclarationOrDeclarations::Node(declaration)) => {
                    if seen.insert(declaration) {
                        declarations.push(declaration);
                    }
                }
                Some(CallHierarchyDeclarationOrDeclarations::Nodes(nodes)) => {
                    for declaration in nodes {
                        if seen.insert(declaration) {
                            declarations.push(declaration);
                        }
                    }
                }
                None => {}
            }
        }
        declarations
    }
}

//! Port of typescript-go `internal/checker/utilities.go` lines 1-903.
//!
//! PORT: Several Go `checker` package functions in this range have the same
//! snake name as an exported `ast` function that is already ported
//! (`ast.IsBinaryOperator`, `ast.IsEmptyObjectLiteral`,
//! `ast.EntityNameToString`, ...). Both modules are glob-exported through the
//! prelude, so a second `pub fn` with the same name would make every use
//! ambiguous. Those checker functions get a `checker_` prefix here (the same
//! choice `checker_p18.rs` made for `entityNameToString`).

use crate::prelude::*;
use std::cmp::Ordering;

// Go: checker/utilities.go:22 NewDiagnosticForNode
pub fn new_diagnostic_for_node(
    node: Node,
    message: &'static crate::diagnostics::Message,
    args: Vec<String>,
) -> Diagnostic {
    let mut file = Node::NIL;
    let mut loc = TextRange::new(0, 0);
    if node.is_some() {
        file = get_source_file_of_node(node);
        loc = get_error_range_for_node(file, node);
    }
    new_diagnostic(file, loc, message, args)
}

// Go: checker/utilities.go:32 NewDiagnosticChainForNode
pub fn new_diagnostic_chain_for_node(
    chain: Option<Diagnostic>,
    node: Node,
    message: &'static crate::diagnostics::Message,
    args: Vec<String>,
) -> Diagnostic {
    if chain.is_some() {
        return new_diagnostic_chain(chain, message, args);
    }
    new_diagnostic_for_node(node, message, args)
}

// Go: checker/utilities.go:39 findInMap
// PORT: Go returns the zero value of V when nothing matches; here `V::default()`.
pub fn find_in_map<K, V: Default + Clone>(
    m: &FxHashMap<K, V>,
    mut predicate: impl FnMut(&V) -> bool,
) -> V {
    for value in m.values() {
        if predicate(value) {
            return value.clone();
        }
    }
    V::default()
}

// Go: checker/utilities.go:48 tokenIsIdentifierOrKeyword
pub fn token_is_identifier_or_keyword(token: SyntaxKind) -> bool {
    (token as u16) >= (SyntaxKind::Identifier as u16)
}

// Go: checker/utilities.go:52 tokenIsIdentifierOrKeywordOrGreaterThan
pub fn token_is_identifier_or_keyword_or_greater_than(token: SyntaxKind) -> bool {
    token == SyntaxKind::GreaterThanToken || token_is_identifier_or_keyword(token)
}

// Go: checker/utilities.go:56 hasOverrideModifier
pub fn has_override_modifier(node: Node) -> bool {
    has_syntactic_modifier(node, ModifierFlags::OVERRIDE)
}

// Go: checker/utilities.go:60 hasAsyncModifier
pub fn has_async_modifier(node: Node) -> bool {
    has_syntactic_modifier(node, ModifierFlags::ASYNC)
}

// Go: checker/utilities.go:64 getSelectedModifierFlags
pub fn get_selected_modifier_flags(node: Node, flags: ModifierFlags) -> ModifierFlags {
    node.modifier_flags() & flags
}

// Go: checker/utilities.go:68 hasReadonlyModifier
pub fn has_readonly_modifier(node: Node) -> bool {
    has_modifier(node, ModifierFlags::READONLY)
}

impl Checker {
    // Go: checker/utilities.go:72 isStaticPrivateIdentifierProperty
    pub fn is_static_private_identifier_property(&self, s: SymbolId) -> bool {
        let sym = self.sym(s);
        // PERF: the binder names the symbol of a private-identifier class
        // element `INTERNAL_SYMBOL_NAME_PREFIX` + "#..." (or
        // `INTERNAL_SYMBOL_NAME_MISSING` outside a class, or "default" when
        // the element has a `default` modifier, see `declare_symbol_ex`).
        // Checker copies (instantiated, transient, merged, union and spread
        // properties) keep the name of a symbol that has that value
        // declaration. Any other name gives false without a declaration load.
        // PERF: effect P7-2. Both tests read only the name id: the id holds
        // the internal bit, and "default" has a fixed id.
        if !sym.name.is_internal() && !sym.name.is_default_symbol_name() {
            return false;
        }
        let value_declaration = sym.value_declaration;
        value_declaration.is_some()
            && is_private_identifier_class_element_declaration(value_declaration)
            && is_static(value_declaration)
    }
}

// Go: checker/utilities.go:76 isEmptyObjectLiteral
// PORT: renamed; `is_empty_object_literal` is `ast.IsEmptyObjectLiteral` (same body).
pub fn checker_is_empty_object_literal(expression: Node) -> bool {
    is_object_literal_expression(expression) && expression.properties().len() == 0
}

// PORT: Go `type AssignmentKind int32` with its consts is `flags::AssignmentKind`
// (`AssignmentKind::NONE`, `DEFINITE`, `COMPOUND`). Go `AssignmentTarget` is a
// `*ast.Node` alias, so it is `Node`.

// Go: checker/utilities.go:90 getAssignmentTargetKind
// PERF: deferbody1. Inline with the first step of `get_assignment_target`:
// most calls end there with no target, with no call. The kind of a target
// is read out of line.
#[inline(always)]
pub fn get_assignment_target_kind(node: Node) -> AssignmentKind {
    let target = get_assignment_target(node);
    if target.is_nil() {
        return AssignmentKind::NONE;
    }
    assignment_kind_of_target(target)
}

/// `getAssignmentTargetKind` of a non-nil assignment target.
#[inline(never)]
fn assignment_kind_of_target(target: Node) -> AssignmentKind {
    match target.kind() {
        SyntaxKind::BinaryExpression => {
            let binary_operator = target.operator_token().kind();
            if binary_operator == SyntaxKind::EqualsToken
                || is_logical_or_coalescing_assignment_operator(binary_operator)
            {
                return AssignmentKind::DEFINITE;
            }
            return AssignmentKind::COMPOUND;
        }
        SyntaxKind::PrefixUnaryExpression | SyntaxKind::PostfixUnaryExpression => {
            return AssignmentKind::COMPOUND;
        }
        SyntaxKind::ForInStatement | SyntaxKind::ForOfStatement => {
            return AssignmentKind::DEFINITE;
        }
        _ => {}
    }
    panic!("Unhandled case in getAssignmentTargetKind")
}

// Go: checker/utilities.go:110 isDeleteTarget
pub fn is_delete_target(node: Node) -> bool {
    if !is_access_expression(node) {
        return false;
    }
    let node = walk_up_parenthesized_expressions(node.parent());
    node.is_some() && node.kind() == SyntaxKind::DeleteExpression
}

// Go: checker/utilities.go:118 isInCompoundLikeAssignment
pub fn is_in_compound_like_assignment(node: Node) -> bool {
    let target = get_assignment_target(node);
    target.is_some()
        && is_assignment_expression(target, true /*excludeCompoundAssignment*/)
        && is_compound_like_assignment(target)
}

// Go: checker/utilities.go:123 isCompoundLikeAssignment
pub fn is_compound_like_assignment(assignment: Node) -> bool {
    let right = skip_parentheses(assignment.right());
    right.kind() == SyntaxKind::BinaryExpression
        && checker_is_shift_operator_or_higher(right.operator_token().kind())
}

// Go: checker/utilities.go:128 isConstTypeReference
// PORT: renamed; `is_const_type_reference` is `ast.IsConstTypeReference` (same body).
pub fn checker_is_const_type_reference(node: Node) -> bool {
    is_type_reference_node(node)
        && node.type_arguments().len() == 0
        && is_identifier(node.type_name())
        && node.type_name().text() == "const"
}

// Go: checker/utilities.go:134 isConstTypeReferenceName
// isConstTypeReferenceName reports whether node is the `const` type name of a `const`
// assertion (`x as const` / `<const>x`), which must not be resolved as a real name.
pub fn is_const_type_reference_name(node: Node) -> bool {
    node.is_some()
        && is_identifier(node)
        && node.parent().is_some()
        && is_const_type_reference(node.parent())
        && node.parent().parent().is_some()
        && is_assertion_expression(node.parent().parent())
}

// Go: checker/utilities.go:144 isExportAssignmentExpressionName
// isExportAssignmentExpressionName reports whether node is (the root entity name of) the
// expression of an `export =` / `export default` assignment. Referencing a namespace or
// type-only name there is legal, and checkExportAssignment decides whether it is an error,
// so checkIdentifier must not report a value-usage error for it.
pub fn is_export_assignment_expression_name(node: Node) -> bool {
    if node.is_nil() {
        return false;
    }
    let mut current = node;
    while current.parent().is_some() && is_property_access_or_qualified_name(current.parent()) {
        current = current.parent();
    }
    current.parent().is_some()
        && is_export_assignment(current.parent())
        && current.parent().expression() == current
}

// Go: checker/utilities.go:155 GetSingleVariableOfVariableStatement
pub fn get_single_variable_of_variable_statement(node: Node) -> Node {
    if !is_variable_statement(node) {
        return Node::NIL;
    }
    let declarations = node.declaration_list().declarations().nodes();
    if declarations.is_empty() {
        return Node::NIL;
    }
    declarations.get(0)
}

// Go: checker/utilities.go:162 isTypeReferenceIdentifier
pub fn is_type_reference_identifier(mut node: Node) -> bool {
    while node.parent().kind() == SyntaxKind::QualifiedName {
        node = node.parent();
    }
    is_type_reference_node(node.parent())
}

// Go: checker/utilities.go:169 IsInTypeQuery
pub fn is_in_type_query(node: Node) -> bool {
    // TypeScript 1.0 spec (April 2014): 3.6.3
    // A type query consists of the keyword typeof followed by an expression.
    // The expression is restricted to a single identifier or a sequence of identifiers separated by periods
    find_ancestor_or_quit(node, |n: Node| match n.kind() {
        SyntaxKind::TypeQuery => FindAncestorResult::FIND_ANCESTOR_TRUE,
        SyntaxKind::Identifier | SyntaxKind::QualifiedName => {
            FindAncestorResult::FIND_ANCESTOR_FALSE
        }
        _ => FindAncestorResult::FIND_ANCESTOR_QUIT,
    })
    .is_some()
}

// Go: checker/utilities.go:184 canHaveLocals
pub fn can_have_locals(node: Node) -> bool {
    matches!(
        node.kind(),
        SyntaxKind::ArrowFunction
            | SyntaxKind::Block
            | SyntaxKind::CallSignature
            | SyntaxKind::CaseBlock
            | SyntaxKind::CatchClause
            | SyntaxKind::ClassStaticBlockDeclaration
            | SyntaxKind::ConditionalType
            | SyntaxKind::Constructor
            | SyntaxKind::ConstructorType
            | SyntaxKind::ConstructSignature
            | SyntaxKind::ForStatement
            | SyntaxKind::ForInStatement
            | SyntaxKind::ForOfStatement
            | SyntaxKind::FunctionDeclaration
            | SyntaxKind::FunctionExpression
            | SyntaxKind::FunctionType
            | SyntaxKind::GetAccessor
            | SyntaxKind::IndexSignature
            | SyntaxKind::JsDocSignature
            | SyntaxKind::MappedType
            | SyntaxKind::MethodDeclaration
            | SyntaxKind::MethodSignature
            | SyntaxKind::ModuleDeclaration
            | SyntaxKind::SetAccessor
            | SyntaxKind::SourceFile
            | SyntaxKind::TypeAliasDeclaration
            | SyntaxKind::JsTypeAliasDeclaration
    )
}

impl Checker {
    // Go: checker/utilities.go:198 isShorthandAmbientModuleSymbol
    pub fn is_shorthand_ambient_module_symbol(&self, module_symbol: SymbolId) -> bool {
        is_shorthand_ambient_module(self.sym(module_symbol).value_declaration)
    }
}

// Go: checker/utilities.go:202 isShorthandAmbientModule
pub fn is_shorthand_ambient_module(node: Node) -> bool {
    // The only kind of module that can be missing a body is a shorthand ambient module.
    node.is_some() && node.kind() == SyntaxKind::ModuleDeclaration && node.body().is_nil()
}

// Go: checker/utilities.go:207 getAliasDeclarationFromName
pub fn get_alias_declaration_from_name(node: Node) -> Node {
    match node.parent().kind() {
        SyntaxKind::ImportClause
        | SyntaxKind::ImportSpecifier
        | SyntaxKind::NamespaceImport
        | SyntaxKind::ExportSpecifier
        | SyntaxKind::ExportAssignment
        | SyntaxKind::ImportEqualsDeclaration
        | SyntaxKind::NamespaceExport => node.parent(),
        SyntaxKind::QualifiedName => get_alias_declaration_from_name(node.parent()),
        _ => Node::NIL,
    }
}

// Go: checker/utilities.go:218 entityNameToString
// PORT: renamed; `entity_name_to_string` is `ast.EntityNameToString(name, getTextOfNode)`.
pub fn checker_entity_name_to_string(name: Node) -> String {
    entity_name_to_string(name, Some(&get_text_of_node))
}

// Go: checker/utilities.go:222 getContainingQualifiedNameNode
pub fn get_containing_qualified_name_node(mut node: Node) -> Node {
    while is_qualified_name(node.parent()) {
        node = node.parent();
    }
    node
}

// Go: checker/utilities.go:229 isSideEffectImport
pub fn is_side_effect_import(node: Node) -> bool {
    let ancestor = find_ancestor(node, is_import_declaration);
    ancestor.is_some() && ancestor.import_clause().is_nil()
}

// Go: checker/utilities.go:234 getExternalModuleRequireArgument
pub fn get_external_module_require_argument(node: Node) -> Node {
    if is_variable_declaration_initialized_to_require(node) {
        return node.initializer().arguments().get(0);
    }
    Node::NIL
}

// Go: checker/utilities.go:241 isRightSideOfAccessExpression
pub fn is_right_side_of_access_expression(node: Node) -> bool {
    node.parent().is_some()
        && (is_property_access_expression(node.parent()) && node.parent().name() == node
            || is_element_access_expression(node.parent())
                && node.parent().argument_expression() == node)
}

// Go: checker/utilities.go:246 isTopLevelInExternalModuleAugmentation
pub fn is_top_level_in_external_module_augmentation(node: Node) -> bool {
    node.is_some()
        && node.parent().is_some()
        && is_module_block(node.parent())
        && is_external_module_augmentation(node.parent().parent())
}

// Go: checker/utilities.go:250 isSyntacticDefault
pub fn is_syntactic_default(node: Node) -> bool {
    (is_export_assignment(node) && !node.is_export_equals())
        || has_syntactic_modifier(node, ModifierFlags::DEFAULT)
        || is_export_specifier(node)
        || is_namespace_export(node)
}

impl Checker {
    // Go: checker/utilities.go:257 hasExportAssignmentSymbol
    pub fn has_export_assignment_symbol(&self, module_symbol: SymbolId) -> bool {
        self.symbols
            .get(
                self.sym(module_symbol).exports,
                INTERNAL_SYMBOL_NAME_EXPORT_EQUALS,
            )
            .is_some()
    }
}

// Go: checker/utilities.go:261 isTypeAlias
pub fn is_type_alias(node: Node) -> bool {
    is_type_or_js_type_alias_declaration(node)
}

// Go: checker/utilities.go:265 hasOnlyExpressionInitializer
pub fn has_only_expression_initializer(node: Node) -> bool {
    matches!(
        node.kind(),
        SyntaxKind::VariableDeclaration
            | SyntaxKind::Parameter
            | SyntaxKind::BindingElement
            | SyntaxKind::PropertyDeclaration
            | SyntaxKind::PropertyAssignment
            | SyntaxKind::EnumMember
    )
}

// Go: checker/utilities.go:273 hasDotDotDotToken
pub fn has_dot_dot_dot_token(node: Node) -> bool {
    match node.kind() {
        SyntaxKind::Parameter => node.dot_dot_dot_token().is_some(),
        SyntaxKind::BindingElement => node.dot_dot_dot_token().is_some(),
        SyntaxKind::NamedTupleMember => node.dot_dot_dot_token().is_some(),
        SyntaxKind::JsxExpression => node.dot_dot_dot_token().is_some(),
        _ => false,
    }
}

impl Checker {
    // Go: checker/utilities.go:287 IsTypeAny
    pub fn is_type_any(&self, t: TypeId) -> bool {
        t.is_some() && self.ty(t).flags.intersects(TypeFlags::ANY)
    }
}

// Go: checker/utilities.go:291 isJSDocOptionalParameter
pub fn is_js_doc_optional_parameter(node: Node) -> bool {
    false // !!!
}

// Go: checker/utilities.go:295 isExclamationToken
pub fn is_exclamation_token(node: Node) -> bool {
    node.is_some() && node.kind() == SyntaxKind::ExclamationToken
}

// Go: checker/utilities.go:299 isOptionalDeclaration
pub fn is_optional_declaration(declaration: Node) -> bool {
    has_question_token(declaration)
}

impl Checker {
    // Go: checker/utilities.go:303 isOptionalParameter
    pub fn is_optional_parameter(&mut self, node: Node) -> bool {
        // !!! TODO: JSDoc support
        if is_parameter_declaration(node) && node.question_token().is_some() {
            return true;
        }
        if !is_parameter_declaration(node) {
            return false;
        }
        if node.initializer().is_some() {
            let signature = self.get_signature_from_declaration(node.parent());
            let parameter_index = find_parameter_index(node);
            debug_assert!(parameter_index >= 0);
            // Only consider syntactic or instantiated parameters as optional, not `void` parameters as this function is used
            // in grammar checks and checking for `void` too early results in parameter types widening too early
            // and causes some noImplicitAny errors to be lost.
            return parameter_index
                >= self.get_min_argument_count_ex(
                    signature,
                    MinArgumentCountFlags::STRONG_ARITY_FOR_UNTYPED_JS
                        | MinArgumentCountFlags::VOID_IS_NON_OPTIONAL,
                );
        }
        let iife = get_immediately_invoked_function_expression(node.parent());
        if iife.is_some() {
            let parameter_index = find_parameter_index(node);
            return node.type_().is_nil()
                && node.dot_dot_dot_token().is_nil()
                && parameter_index >= self.get_effective_call_arguments(iife).len() as i32;
        }
        false
    }
}

// PORT: Go `core.FindIndex(node.Parent.Parameters(), func(p) bool { return p == node })`
// used twice in isOptionalParameter. Returns -1 when not found, like Go.
fn find_parameter_index(node: Node) -> i32 {
    let parameters = node.parent().parameters();
    for i in 0..parameters.len() {
        if parameters.get(i) == node {
            return i as i32;
        }
    }
    -1
}

// Go: checker/utilities.go:330 isEmptyArrayLiteral
// PORT: renamed; `is_empty_array_literal` is `ast.IsEmptyArrayLiteral` (same body).
pub fn checker_is_empty_array_literal(expression: Node) -> bool {
    is_array_literal_expression(expression) && expression.elements().len() == 0
}

// Go: checker/utilities.go:334 declarationBelongsToPrivateAmbientMember
// PORT: Go also has a `Checker` method with this name (checker.go:18465,
// ported in checker_p20.rs); methods and free functions do not clash in Rust.
pub fn declaration_belongs_to_private_ambient_member(declaration: Node) -> bool {
    let root = get_root_declaration(declaration);
    let mut member_declaration = root;
    if root.kind() == SyntaxKind::Parameter {
        member_declaration = root.parent();
    }
    is_private_within_ambient(member_declaration)
}

// Go: checker/utilities.go:343 isPrivateWithinAmbient
pub fn is_private_within_ambient(node: Node) -> bool {
    (has_modifier(node, ModifierFlags::PRIVATE)
        || is_private_identifier_class_element_declaration(node))
        && !node.parser_flags(NodeFlags::AMBIENT).is_empty()
}

// Go: checker/utilities.go:347 isTypeAssertion
// PORT: renamed; `is_type_assertion` is `ast.IsTypeAssertion`, which only
// checks for `KindTypeAssertionExpression`. This checker function differs:
// it skips parentheses and accepts any assertion expression.
pub fn checker_is_type_assertion(node: Node) -> bool {
    is_assertion_expression(skip_parentheses(node))
}

impl Checker {
    // Go: checker/utilities.go:351 createSymbolTable
    pub fn create_symbol_table(&mut self, symbols: &[SymbolId]) -> SymbolTable {
        if symbols.is_empty() {
            return SymbolTable::NIL;
        }
        let result = self.symbols.new_table_with_capacity(symbols.len());
        for &symbol in symbols {
            let name = self.sym(symbol).name.clone();
            self.symbols.set(result, name, symbol);
        }
        result
    }

    // Go: checker/utilities.go:362 sortSymbols
    // PORT: Go sorts with `c.compareSymbols`, which is always
    // `c.compareSymbolsWorker` ("closure optimization"), so this needs only
    // `&self`. Each symbol's first declaration, file index and position are
    // read once into a `SymbolSortKey`; `compare_symbol_sort_keys` is
    // `compareSymbolsWorker` on those cached values.
    pub fn sort_symbols(&self, symbols: &mut [SymbolId]) {
        if symbols.len() < 2 {
            return;
        }
        // Consecutive symbols mostly come from one file, so the last file
        // index lookup is reused.
        let mut last_file = (Node::NIL, 0);
        let mut keys: Vec<SymbolSortKey> = symbols
            .iter()
            .map(|&s| self.symbol_sort_key(s, &mut last_file))
            .collect();
        self.sort_symbol_sort_keys(&mut keys);
        for (slot, key) in symbols.iter_mut().zip(&keys) {
            *slot = key.symbol;
        }
    }

    /// `sort_symbols` on keys that are already built. `get_named_members`
    /// uses it with its reusable key buffer. The sort and the comparator are
    /// the ones `sort_symbols` uses, so the comparisons (and the lazy
    /// `get_symbol_id` calls in them) happen in the same order.
    // Go: checker/utilities.go:363 slices.SortFunc(symbols, c.compareSymbols)
    // PORT: the comparator is not a total order. A file that is not in
    // `file_index_map` reads as index 0, as in Go, so it ties with the file at
    // index 0 and the order is not transitive (the auto-import registry
    // checks package files that are not in the program). Rust std `sort_by`
    // can panic on that; `gostd::slices::sort_func_hot` (a hot site) is Go's
    // pdqsort, which does not, and gives Go's order (and Go's order of the
    // `get_symbol_id` calls) for any comparator.
    pub(crate) fn sort_symbol_sort_keys(&self, keys: &mut [SymbolSortKey]) {
        crate::gostd::slices::sort_func_hot(keys, |a, b| {
            // PERF: two different packed orders give the sign that the full
            // comparator gets from `compare_nodes`, from the declaration
            // test or from the names (see `SymbolSortKey::order`), and the
            // full comparator returns there before its id step. So every
            // comparison has the same sign, the sort makes the same
            // comparisons, and `get_symbol_id` runs in the same order.
            if a.order != b.order && a.order != NO_SORT_ORDER && b.order != NO_SORT_ORDER {
                let r = a.order.cmp(&b.order);
                debug_assert_eq!(r, self.compare_symbol_sort_keys(a, b).cmp(&0));
                return r as i32;
            }
            self.compare_symbol_sort_keys(a, b)
        });
    }

    /// The `compareSymbolsWorker` inputs of one symbol.
    /// `last_file` caches the last `(file, file_index_map[file])` lookup,
    /// with -1 for a file that is not in the map.
    // PERF: always inlined into `get_named_members` and `sort_symbols`, so
    // the key is written straight into its `Vec` slot. Out of line, the
    // callee stored the 48-byte key through the return pointer and the
    // caller read it back with wider loads that could not forward.
    #[inline(always)]
    pub(crate) fn symbol_sort_key(
        &self,
        symbol: SymbolId,
        last_file: &mut (Node, i32),
    ) -> SymbolSortKey {
        if symbol.is_nil() {
            return SymbolSortKey {
                symbol,
                has_declaration: false,
                declaration: Node::NIL,
                file: Node::NIL,
                file_index: 0,
                pos: 0,
                order: NO_SORT_ORDER,
                name: Name::default(),
            };
        }
        let sym = self.sym(symbol);
        let has_declaration = !sym.declarations.is_empty();
        let declaration = sym.declarations.first().copied().unwrap_or(Node::NIL);
        let (file, file_index, pos, order) = if declaration.is_some() {
            // PERF: noderef1 G2. The file and the position through one view
            // (`NodeRef`).
            let declaration_ref = NodeRef::new(declaration);
            let file = declaration_ref
                .source_file_root()
                .unwrap_or_else(|| get_source_file_of_node(declaration));
            debug_assert_eq!(file, get_source_file_of_node(declaration));
            if file != last_file.0 || file.is_nil() {
                *last_file = (file, self.file_index_map.get(&file).copied().unwrap_or(-1));
            }
            let pos = declaration_ref.loc().pos();
            if last_file.1 >= 0 {
                // The sign bit flip maps `i32` order onto `u32` order.
                let order =
                    (u64::from(last_file.1 as u32) << 32) | u64::from(pos as u32 ^ 0x8000_0000);
                (file, last_file.1, pos, order)
            } else {
                // A map miss reads as index 0 (Go map zero value).
                (file, 0, pos, NO_SORT_ORDER)
            }
        } else if has_declaration {
            (Node::NIL, 0, 0, NO_SORT_ORDER)
        } else {
            // The first 63 bits of the name's Go bytes, after every
            // declared symbol (see `SymbolSortKey::order`).
            let order = NO_DECLARATION_ORDER | (go_bytes_prefix(&sym.name) >> 25) as u64;
            (Node::NIL, 0, 0, order)
        };
        SymbolSortKey {
            symbol,
            has_declaration,
            declaration,
            file,
            file_index,
            pos,
            order,
            name: sym.name.clone(),
        }
    }

    /// `compare_symbols_worker` on cached keys. The symbol id fallback stays
    /// lazy so ids are assigned in the same order as before.
    fn compare_symbol_sort_keys(&self, k1: &SymbolSortKey, k2: &SymbolSortKey) -> i32 {
        if k1.symbol == k2.symbol {
            return 0;
        }
        if k1.symbol.is_nil() {
            return 1;
        }
        if k2.symbol.is_nil() {
            return -1;
        }
        if k1.has_declaration && k2.has_declaration {
            // compare_nodes
            let r = if k1.declaration == k2.declaration {
                0
            } else if k1.declaration.is_nil() {
                1
            } else if k2.declaration.is_nil() {
                -1
            } else if k1.file != k2.file {
                k1.file_index - k2.file_index
            } else {
                k1.pos - k2.pos
            };
            if r != 0 {
                return r;
            }
        } else if k1.has_declaration {
            return -1;
        } else if k2.has_declaration {
            return 1;
        }
        // Equal ids are equal texts, which compare as 0.
        if k1.name != k2.name {
            let r = compare_strings(&k1.name, &k2.name);
            if r != 0 {
                return r;
            }
        }
        let id1 = get_symbol_id(&self.symbols, k1.symbol);
        let id2 = get_symbol_id(&self.symbols, k2.symbol);
        id1.cmp(&id2) as i32
    }

    // Go: checker/utilities.go:366 compareSymbolsWorker
    pub fn compare_symbols_worker(&self, s1: SymbolId, s2: SymbolId) -> i32 {
        if s1 == s2 {
            return 0;
        }
        if s1.is_nil() {
            return 1;
        }
        if s2.is_nil() {
            return -1;
        }
        let sym1 = self.sym(s1);
        let sym2 = self.sym(s2);
        if !sym1.declarations.is_empty() && !sym2.declarations.is_empty() {
            let r = self.compare_nodes(sym1.declarations[0], sym2.declarations[0]);
            if r != 0 {
                return r;
            }
        } else if !sym1.declarations.is_empty() {
            return -1;
        } else if !sym2.declarations.is_empty() {
            return 1;
        }
        let r = compare_strings(&sym1.name, &sym2.name);
        if r != 0 {
            return r;
        }
        // Fall back to symbol IDs. This is a last resort that should happen only when symbols have
        // no declaration and duplicate names.
        let id1 = get_symbol_id(&self.symbols, s1);
        let id2 = get_symbol_id(&self.symbols, s2);
        id1.cmp(&id2) as i32
    }

    // Go: checker/utilities.go:393 compareNodes
    pub fn compare_nodes(&self, n1: Node, n2: Node) -> i32 {
        if n1 == n2 {
            return 0;
        }
        if n1.is_nil() {
            return 1;
        }
        if n2.is_nil() {
            return -1;
        }
        let parent = n1.parent();
        if parent.is_some() && parent == n2.parent() {
            return n1.pos() - n2.pos();
        }
        let s1 = get_source_file_of_node(n1);
        let s2 = get_source_file_of_node(n2);
        if s1 != s2 {
            let f1 = self.file_index_map.get(&s1).copied().unwrap_or(0);
            let f2 = self.file_index_map.get(&s2).copied().unwrap_or(0);
            // Order by index of file in the containing program
            return f1 - f2;
        }
        // In the same file, order by source position
        n1.pos() - n2.pos()
    }
}

/// Cached `compareSymbolsWorker` inputs for `sort_symbols`.
pub(crate) struct SymbolSortKey {
    pub(crate) symbol: SymbolId,
    has_declaration: bool,
    /// First declaration, or nil.
    declaration: Node,
    /// Source file of `declaration`.
    file: Node,
    /// `file_index_map[file]`, zero when absent (Go map miss).
    file_index: i32,
    pos: i32,
    /// A `u64` whose order is the order of the full comparator where two
    /// orders differ:
    /// - a declaration in a file of `file_index_map`: `(file_index, pos)`
    ///   packed so that `u64` order is `(file_index, pos)` order (below
    ///   2^63). Map indexes are unique and not negative, so two different
    ///   orders mean `compare_nodes` returns nonzero with the same sign:
    ///   `file_index` difference for different files, `pos` difference in
    ///   one file;
    /// - no declaration: `NO_DECLARATION_ORDER` and the first 63 bits of
    ///   the name's Go bytes (`go_bytes_prefix`). The full comparator puts a
    ///   symbol with a declaration first, which these orders, at 2^63 and
    ///   above, do too. Two symbols without a declaration compare by name
    ///   (Go `strings.Compare`), and different prefixes have its sign;
    /// - `NO_SORT_ORDER` for a nil symbol, a nil first declaration or a
    ///   file that is not in `file_index_map`.
    ///
    /// Equal orders (same declaration, same position, or names with the
    /// same prefix) fall through to the full comparator.
    // PERF (sortmisc1): on elysia-eden 86% of the `get_named_members`
    // comparisons (1.59 M of 1.85 M; Go makes 1.59 M of 1.99 M) are of two
    // symbols without a declaration (mapped type members), which the full
    // comparator orders by name text. The prefix decides all of them there.
    pub(crate) order: u64,
    /// Its text is read only when the orders tie.
    name: Name,
}

/// `SymbolSortKey::order` when the packed order does not apply. A
/// declaration order is below 2^63, and a name order is below this
/// (`go_bytes_prefix` never gives all one bits).
pub(crate) const NO_SORT_ORDER: u64 = u64::MAX;

/// The bit of `SymbolSortKey::order` that marks a symbol without a
/// declaration (its other bits are the name prefix).
const NO_DECLARATION_ORDER: u64 = 1 << 63;

// PORT: Go `strings.Compare` (byte order, returns -1/0/1). It compares the
// Go bytes, so lone surrogates and the internal symbol name prefix sort as in
// Go (see `compare_go_strings`).
fn compare_strings(a: &str, b: &str) -> i32 {
    match crate::scanner_util::compare_go_strings(a, b) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

// PORT: Go computes these differences as 64-bit `int`. Callers only use the
// sign, so a difference that does not fit in `i32` is clamped (sign kept).
fn clamp_compare(v: i64) -> i32 {
    v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

// PORT: Go `cmp.Compare` on `jsnum.Number` (a float64): NaN is less than any
// other value and equal to NaN; -0 equals +0.
fn compare_numbers(a: crate::jsnum::Number, b: crate::jsnum::Number) -> i32 {
    let (x, y) = (a.0, b.0);
    let x_nan = x.is_nan();
    let y_nan = y.is_nan();
    if x_nan {
        if y_nan {
            return 0;
        }
        return -1;
    }
    if y_nan {
        return 1;
    }
    if x < y {
        return -1;
    }
    if x > y {
        return 1;
    }
    0
}

impl Checker {
    // Go: checker/utilities.go:418 CompareTypes
    // PORT: Go panics when the types come from different checkers; one
    // checker owns all `TypeId`s here, so that check is dropped. Go calls
    // `t1.checker.compareSymbols`, which is always `compareSymbolsWorker`, so
    // this calls the worker directly and needs only `&self`.
    pub fn compare_types(&self, t1: TypeId, t2: TypeId) -> i32 {
        if t1 == t2 {
            return 0;
        }
        if t1.is_nil() {
            return -1;
        }
        if t2.is_nil() {
            return 1;
        }
        let ty1 = self.ty(t1);
        let ty2 = self.ty(t2);
        // First sort in order of increasing type flags values.
        let c = clamp_compare(get_sort_order_flags(ty1) - get_sort_order_flags(ty2));
        if c != 0 {
            return c;
        }
        // Order named types by name and, in the case of aliased types, by alias type arguments.
        // PERF: chkA. The two types are passed as read (`compare_type_names_of`).
        let c = self.compare_type_names_of(ty1, ty2);
        if c != 0 {
            return c;
        }
        // We have unnamed types or types with identical names. Now sort by data specific to the type.
        if ty1.flags.intersects(
            TypeFlags::ANY
                | TypeFlags::UNKNOWN
                | TypeFlags::STRING
                | TypeFlags::NUMBER
                | TypeFlags::BOOLEAN
                | TypeFlags::BIG_INT
                | TypeFlags::ES_SYMBOL
                | TypeFlags::VOID
                | TypeFlags::UNDEFINED
                | TypeFlags::NULL
                | TypeFlags::NEVER
                | TypeFlags::NON_PRIMITIVE,
        ) {
            // Only distinguished by type IDs, handled below.
        } else if ty1.flags.intersects(TypeFlags::OBJECT) {
            // Order instantiation expression types without relying on lazy symbol IDs.
            // Order other unnamed or identically named object types by symbol.
            if ty1
                .object_flags
                .intersects(ObjectFlags::INSTANTIATION_EXPRESSION_TYPE)
                && ty2
                    .object_flags
                    .intersects(ObjectFlags::INSTANTIATION_EXPRESSION_TYPE)
            {
                let mut declaration1 = Node::NIL;
                let mut declaration2 = Node::NIL;
                if ty1.symbol.is_some() && !self.sym(ty1.symbol).declarations.is_empty() {
                    declaration1 = self.sym(ty1.symbol).declarations[0];
                }
                if ty2.symbol.is_some() && !self.sym(ty2.symbol).declarations.is_empty() {
                    declaration2 = self.sym(ty2.symbol).declarations[0];
                }
                // A single instantiation expression can produce multiple types for union constituents,
                // so compare their source declarations before comparing the shared expression node.
                let c = self.compare_nodes(declaration1, declaration2);
                if c != 0 {
                    return c;
                }
                let c = self.compare_nodes(
                    ty1.as_instantiation_expression_type().node,
                    ty2.as_instantiation_expression_type().node,
                );
                if c != 0 {
                    return c;
                }
            } else {
                let c = self.compare_symbols_worker(ty1.symbol, ty2.symbol);
                if c != 0 {
                    return c;
                }
            }
            // When object types have the same or no symbol, order by kind. We order type references before other kinds.
            if ty1.object_flags.intersects(ObjectFlags::REFERENCE)
                && ty2.object_flags.intersects(ObjectFlags::REFERENCE)
            {
                let r1 = ty1.as_type_reference();
                let r2 = ty2.as_type_reference();
                let r1_target = r1.object.target;
                let r2_target = r2.object.target;
                if self
                    .ty(r1_target)
                    .object_flags
                    .intersects(ObjectFlags::TUPLE)
                    && self
                        .ty(r2_target)
                        .object_flags
                        .intersects(ObjectFlags::TUPLE)
                {
                    // Tuple types have no associated symbol, instead we order by tuple element information.
                    let c = compare_tuple_types(
                        self.ty(r1_target).as_tuple_type(),
                        self.ty(r2_target).as_tuple_type(),
                    );
                    if c != 0 {
                        return c;
                    }
                }
                // Here we know we have references to instantiations of the same type because we have matching targets.
                if r1.node.is_nil() && r2.node.is_nil() {
                    // Non-deferred type references with the same target are sorted by their type argument lists.
                    let c = self.compare_type_lists(
                        &r1.resolved_type_arguments,
                        &r2.resolved_type_arguments,
                    );
                    if c != 0 {
                        return c;
                    }
                } else {
                    // Deferred type references with the same target are ordered by the source location of the reference.
                    let c = self.compare_nodes(r1.node, r2.node);
                    if c != 0 {
                        return c;
                    }
                    // Instantiations of the same deferred type reference are ordered by their associated type mappers
                    // (which reflect the mapping of in-scope type parameters to type arguments).
                    let c = self.compare_type_mappers(
                        ty1.as_object_type().mapper,
                        ty2.as_object_type().mapper,
                    );
                    if c != 0 {
                        return c;
                    }
                }
            } else if ty1.object_flags.intersects(ObjectFlags::REFERENCE) {
                return -1;
            } else if ty2.object_flags.intersects(ObjectFlags::REFERENCE) {
                return 1;
            } else {
                // Order unnamed non-reference object types by kind and instantiation data.
                let k1 = i64::from((ty1.object_flags & ObjectFlags::OBJECT_TYPE_KIND_MASK).0);
                let k2 = i64::from((ty2.object_flags & ObjectFlags::OBJECT_TYPE_KIND_MASK).0);
                let c = clamp_compare(k1 - k2);
                if c != 0 {
                    return c;
                }
                if ty1.object_flags.intersects(ObjectFlags::REVERSE_MAPPED) {
                    let r1 = ty1.as_reverse_mapped_type();
                    let r2 = ty2.as_reverse_mapped_type();
                    let c = self.compare_types(r1.source, r2.source);
                    if c != 0 {
                        return c;
                    }
                    let c = self.compare_types(r1.mapped_type, r2.mapped_type);
                    if c != 0 {
                        return c;
                    }
                    let c = self.compare_types(r1.constraint_type, r2.constraint_type);
                    if c != 0 {
                        return c;
                    }
                }
                let mut m1 = ty1.as_object_type().mapper;
                let mut m2 = ty2.as_object_type().mapper;
                if ty1.object_flags.intersects(ObjectFlags::MAPPED) {
                    // instantiateAnonymousType prepends a fresh type parameter mapping.
                    // Compare the effective instantiation, not the identity of that fresh parameter.
                    // PORT: Go `m.data.(*CompositeTypeMapper).m2` panics for any other mapper.
                    let composite_m2 = |m: MapperId| match self.mapper(m) {
                        TypeMapper::Composite(d) => d.m2,
                        _ => panic!("interface conversion: not *checker.CompositeTypeMapper"),
                    };
                    if m1.is_some() {
                        m1 = composite_m2(m1);
                    }
                    if m2.is_some() {
                        m2 = composite_m2(m2);
                    }
                }
                let c = self.compare_type_mappers(m1, m2);
                if c != 0 {
                    return c;
                }
            }
        } else if ty1.flags.intersects(TypeFlags::UNION) {
            // Unions are ordered by origin and then constituent type lists.
            let o1 = ty1.as_union_type().origin;
            let o2 = ty2.as_union_type().origin;
            if o1.is_nil() && o2.is_nil() {
                let c = self.compare_type_lists(ty1.types(), ty2.types());
                if c != 0 {
                    return c;
                }
            } else if o1.is_nil() {
                return 1;
            } else if o2.is_nil() {
                return -1;
            } else {
                let c = self.compare_types(o1, o2);
                if c != 0 {
                    return c;
                }
            }
        } else if ty1.flags.intersects(TypeFlags::INTERSECTION) {
            // Intersections are ordered by their constituent type lists.
            let c = self.compare_type_lists(ty1.types(), ty2.types());
            if c != 0 {
                return c;
            }
        } else if ty1
            .flags
            .intersects(TypeFlags::ENUM | TypeFlags::ENUM_LITERAL | TypeFlags::UNIQUE_ES_SYMBOL)
        {
            // Enum members are ordered by their symbol (and thus their declaration order).
            let c = self.compare_symbols_worker(ty1.symbol, ty2.symbol);
            if c != 0 {
                return c;
            }
        } else if ty1.flags.intersects(TypeFlags::STRING_LITERAL) {
            // String literal types are ordered by their values.
            let c = compare_strings(literal_string_value(ty1), literal_string_value(ty2));
            if c != 0 {
                return c;
            }
        } else if ty1.flags.intersects(TypeFlags::NUMBER_LITERAL) {
            // Numeric literal types are ordered by their values.
            let c = compare_numbers(literal_number_value(ty1), literal_number_value(ty2));
            if c != 0 {
                return c;
            }
        } else if ty1.flags.intersects(TypeFlags::BIG_INT_LITERAL) {
            let c = self
                .get_big_int_literal_value(t1)
                .compare(&self.get_big_int_literal_value(t2));
            if c != 0 {
                return c;
            }
        } else if ty1.flags.intersects(TypeFlags::BOOLEAN_LITERAL) {
            let b1 = literal_bool_value(ty1);
            let b2 = literal_bool_value(ty2);
            if b1 != b2 {
                if b1 {
                    return 1;
                }
                return -1;
            }
        } else if ty1.flags.intersects(TypeFlags::TYPE_PARAMETER) {
            let c = self.compare_symbols_worker(ty1.symbol, ty2.symbol);
            if c != 0 {
                return c;
            }
        } else if ty1.flags.intersects(TypeFlags::INDEX) {
            let c = self.compare_types(ty1.as_index_type().target, ty2.as_index_type().target);
            if c != 0 {
                return c;
            }
            let c = clamp_compare(
                i64::from(ty1.as_index_type().index_flags.0)
                    - i64::from(ty2.as_index_type().index_flags.0),
            );
            if c != 0 {
                return c;
            }
        } else if ty1.flags.intersects(TypeFlags::INDEXED_ACCESS) {
            let c = self.compare_types(
                ty1.as_indexed_access_type().object_type,
                ty2.as_indexed_access_type().object_type,
            );
            if c != 0 {
                return c;
            }
            let c = self.compare_types(
                ty1.as_indexed_access_type().index_type,
                ty2.as_indexed_access_type().index_type,
            );
            if c != 0 {
                return c;
            }
        } else if ty1.flags.intersects(TypeFlags::CONDITIONAL) {
            let n1 = ty1.as_conditional_type().root.borrow().node;
            let n2 = ty2.as_conditional_type().root.borrow().node;
            let c = self.compare_nodes(n1, n2);
            if c != 0 {
                return c;
            }
            let c = self.compare_type_mappers(
                ty1.as_conditional_type().mapper,
                ty2.as_conditional_type().mapper,
            );
            if c != 0 {
                return c;
            }
        } else if ty1.flags.intersects(TypeFlags::SUBSTITUTION) {
            let c = self.compare_types(
                ty1.as_substitution_type().base_type,
                ty2.as_substitution_type().base_type,
            );
            if c != 0 {
                return c;
            }
            let c = self.compare_types(
                ty1.as_substitution_type().constraint,
                ty2.as_substitution_type().constraint,
            );
            if c != 0 {
                return c;
            }
        } else if ty1.flags.intersects(TypeFlags::TEMPLATE_LITERAL) {
            let c = compare_string_slices(
                &ty1.as_template_literal_type().texts,
                &ty2.as_template_literal_type().texts,
            );
            if c != 0 {
                return c;
            }
            let c = self.compare_type_lists(
                &ty1.as_template_literal_type().types,
                &ty2.as_template_literal_type().types,
            );
            if c != 0 {
                return c;
            }
        } else if ty1.flags.intersects(TypeFlags::STRING_MAPPING) {
            let c = self.compare_types(
                ty1.as_string_mapping_type().target,
                ty2.as_string_mapping_type().target,
            );
            if c != 0 {
                return c;
            }
        }
        // Fall back to type IDs. This results in type creation order for built-in types.
        clamp_compare(i64::from(ty1.id.0) - i64::from(ty2.id.0))
    }
}

// PERF: unionsort1. The union sort and the union searches decide most
// comparisons at the first steps of `compare_types`: the sort-order flags,
// the name, or the first value that the type's arm compares.
// `union_sort_key` packs those steps into one `u128` per type, read once per
// sort or per searched list, so most comparisons are one integer compare.
//
// The rule: when `union_sort_key(a) < union_sort_key(b)`, Go's
// `CompareTypes(a, b)` is negative, and Go returns at one of those first
// steps. Those steps have no effect: they come before every lazy symbol id
// (`compareSymbolsWorker` returns at `compareNodes` there) and before every
// case in which Go's order is not transitive (two files with the same index
// in `compareNodes`, an instantiation expression type against another
// object type). Equal keys say nothing, and `compare_types` decides. So a
// keyed comparison has the sign and the effects of Go's comparison, and a
// sort or search that uses it makes Go's comparisons in Go's order, with
// Go's result. The fields that the key reads are set when the type is made
// (Go reads the same fields when it compares).
//
// The key: `sort flags << 96 | unnamed << 95 | value` (value < 2^95).
// - sort flags: `getSortOrderFlags`, a `u32`.
// - unnamed: 1 when `getTypeNameSymbol` is nil (Go puts nil names last).
// - value, for a named type: the first 11 Go bytes of the name
//   (`go_bytes_prefix`). For an unnamed type, by the arm that it takes in
//   `compare_types`:
//   - any, unknown, string and the other types that only the id tells
//     apart: the type id (Go's last step);
//   - object, enum-like and unique symbol types: `symbol_sort_rank` of the
//     type's symbol (`compareSymbolsWorker` and the instantiation expression
//     branch both start with the first declarations);
//   - intersection: the member count (Go `compareTypeLists` compares the
//     lengths first), then the first 87 bits of the first member's key (the
//     first members are compared next; truncation keeps the order);
//   - string literal: the first 11 Go bytes of the value;
//   - number literal: `number_sort_value`;
//   - boolean literal: 0 for false, 1 for true;
//   - template literal: the first 11 Go bytes of the first text (Go
//     compares the text lists first, element by element);
//   - the other arms (union, bigint literal, index and so on): 0, so the
//     keys tie and `compare_types` decides.
// Equal sort flags give the same arm: types with equal flags take the same
// branch, and every enum-like unit type takes the enum branch.

/// The first 11 Go bytes of `s` as an 88-bit big-endian number, in an
/// order that never contradicts Go `strings.Compare`. ASCII bytes are their
/// own Go bytes, and every non-ASCII unit has Go bytes of 0x80 or more (see
/// `compare_go_strings`). So the prefix keeps the leading ASCII bytes,
/// writes 0x80 at the first non-ASCII byte and zeros after it, and pads a
/// short string with zeros. Equal prefixes say nothing.
#[inline]
fn go_bytes_prefix(s: &str) -> u128 {
    let b = s.as_bytes();
    let n = b.len().min(11);
    let mut buf = [0u8; 16];
    buf[..n].copy_from_slice(&b[..n]);
    let x = u128::from_be_bytes(buf);
    let high = x & 0x8080_8080_8080_8080_8080_8080_8080_8080;
    let x = if high == 0 {
        x
    } else {
        // Byte `i` (from the left) is the first byte of 0x80 or more.
        let i = high.leading_zeros() / 8;
        (x & !(u128::MAX >> (8 * i))) | (0x80u128 << (120 - 8 * i))
    };
    x >> 40
}

/// Go `cmp.Compare` order of a number literal value: NaN first, -0 equal
/// to +0.
#[inline]
fn number_sort_value(n: crate::jsnum::Number) -> u64 {
    let x = n.0;
    if x.is_nan() {
        return 0;
    }
    // -0 + 0 is +0.
    let bits = (x + 0.0).to_bits();
    if bits >> 63 == 1 {
        !bits
    } else {
        bits | 1 << 63
    }
}

/// Lists shorter than this are sorted without keys: a key costs about as
/// much as a comparison, and a short list makes few comparisons.
const KEYED_SORT_MIN: usize = 8;

/// Longer lists are sorted by `sort_large_union_types`. A key pair is 8
/// times the size of a `TypeId`, and the sort moves its elements: on lists
/// of 10^4 to 10^6 types (eslint-plugin-svelte) a sort of the pairs ran
/// more instructions than the plain sort.
const KEYED_SORT_MAX: usize = 4096;

/// One distinct type of a large union sort (`sort_large_union_types`).
/// `first` and `second` are set for an unnamed intersection of 2 to 254
/// members: its first member, and the key of its second member.
#[derive(Clone, Copy)]
struct LargeSortEntry {
    key: u128,
    second: u128,
    first: TypeId,
    t: TypeId,
}

impl LargeSortEntry {
    /// The order of Go `CompareTypes(self.t, other.t)` when the entries
    /// tell it, else `Equal`. Different keys tell it (the PERF note above).
    /// Equal keys mean the same flags, both unnamed or both named, and for
    /// unnamed intersections the same member count. Then two intersections
    /// with the same first member get 0 for it in Go `compareTypeLists`,
    /// which goes on to the second members: different keys tell their
    /// order.
    #[inline]
    fn keyed_order(&self, other: &Self) -> Ordering {
        self.key.cmp(&other.key).then_with(|| {
            if self.first.is_some() && self.first == other.first {
                self.second.cmp(&other.second)
            } else {
                Ordering::Equal
            }
        })
    }
}

impl Checker {
    /// The union sort key of `t` (see the PERF note above).
    #[inline]
    pub(crate) fn union_sort_key(&self, t: TypeId) -> u128 {
        let ty = self.ty(t);
        let flags = u128::from(get_sort_order_flags(ty) as u32) << 96;
        let s = type_name_symbol(ty);
        if s.is_some() {
            return flags | go_bytes_prefix(&self.sym(s).name) << 7;
        }
        let f = ty.flags;
        // The branches of `compare_types`, in its order.
        let value = if f.intersects(
            TypeFlags::ANY
                | TypeFlags::UNKNOWN
                | TypeFlags::STRING
                | TypeFlags::NUMBER
                | TypeFlags::BOOLEAN
                | TypeFlags::BIG_INT
                | TypeFlags::ES_SYMBOL
                | TypeFlags::VOID
                | TypeFlags::UNDEFINED
                | TypeFlags::NULL
                | TypeFlags::NEVER
                | TypeFlags::NON_PRIMITIVE,
        ) {
            u128::from(ty.id.0)
        } else if f.intersects(TypeFlags::OBJECT) {
            u128::from(self.symbol_sort_rank(ty.symbol))
        } else if f.intersects(TypeFlags::UNION) {
            0
        } else if f.intersects(TypeFlags::INTERSECTION) {
            let members = ty.types();
            // More than 254 members all get 255 and tie.
            match members.first() {
                Some(&first) if members.len() < 255 => {
                    (members.len() as u128) << 87 | self.union_sort_key(first) >> 41
                }
                _ => 255 << 87,
            }
        } else if f
            .intersects(TypeFlags::ENUM | TypeFlags::ENUM_LITERAL | TypeFlags::UNIQUE_ES_SYMBOL)
        {
            u128::from(self.symbol_sort_rank(ty.symbol))
        } else if f.intersects(TypeFlags::STRING_LITERAL) {
            go_bytes_prefix(literal_string_value(ty)) << 7
        } else if f.intersects(TypeFlags::NUMBER_LITERAL) {
            u128::from(number_sort_value(literal_number_value(ty)))
        } else if f.intersects(TypeFlags::BOOLEAN_LITERAL) {
            u128::from(literal_bool_value(ty))
        } else if f.intersects(TypeFlags::TEMPLATE_LITERAL) {
            match ty.as_template_literal_type().texts.first() {
                Some(text) => go_bytes_prefix(text) << 7,
                None => 0,
            }
        } else {
            0
        };
        flags | 1 << 95 | value
    }

    /// The order of `compareSymbolsWorker` at its declaration step, as a
    /// number. A symbol whose first declaration is in the file at index
    /// k >= 1 gets `k << 32 | pos` (pos with its sign bit flipped, so `u32`
    /// order is `i32` order). A declaration in the file at index 0, in a file
    /// that is not in `file_index_map` (Go reads 0) or in no file gets 0:
    /// Go puts those before index k >= 1 and can tie them with each other,
    /// so their ranks tie. No symbol or no declaration gets `u64::MAX`: both
    /// sort after a symbol with declarations (`compareNodes` also puts a nil
    /// node last).
    #[inline]
    fn symbol_sort_rank(&self, s: SymbolId) -> u64 {
        if s.is_nil() {
            return u64::MAX;
        }
        let Some(&declaration) = self.sym(s).declarations.first() else {
            return u64::MAX;
        };
        let file = get_source_file_of_node(declaration);
        match self.file_index_map.get(&file) {
            Some(&k) if k >= 1 => {
                (u64::from(k as u32) << 32) | u64::from(declaration.pos() as u32 ^ 0x8000_0000)
            }
            _ => 0,
        }
    }

    /// `compare_types(t1, t2)` with the keys of `t1` and `t2` first (see the
    /// PERF note above). Callers use only the sign.
    #[inline]
    fn compare_keyed_types(&self, k1: u128, t1: TypeId, k2: u128, t2: TypeId) -> i32 {
        if k1 != k2 {
            return if k1 < k2 { -1 } else { 1 };
        }
        if t1 == t2 {
            return 0;
        }
        self.compare_types(t1, t2)
    }

    /// Go `slices.SortStableFunc(types, CompareTypes)`. Lists of
    /// `KEYED_SORT_MIN` to `KEYED_SORT_MAX` types are sorted as key pairs
    /// with keyed comparisons: Go's algorithm (`gostd`) makes the same
    /// comparisons in the same order, and the keys only answer some of them
    /// early, so the result and the lazy symbol ids are Go's. Longer lists
    /// go to `sort_large_union_types` (one table entry per distinct type,
    /// with its key and the key of its second member).
    // Go: checker/checker.go:26410 slices.SortStableFunc(types, CompareTypes)
    pub(crate) fn sort_union_types(&self, types: &mut [TypeId]) {
        if types.len() > KEYED_SORT_MAX {
            self.sort_large_union_types(types);
            return;
        }
        // Hot sites: `sort_stable_func_hot`.
        if types.len() < KEYED_SORT_MIN {
            crate::gostd::slices::sort_stable_func_hot(types, |&a, &b| self.compare_types(a, b));
            return;
        }
        let mut keyed = self.union_sort_keys(types);
        crate::gostd::slices::sort_stable_func_hot(&mut keyed, |&(k1, t1), &(k2, t2)| {
            self.compare_keyed_types(k1, t1, k2, t2)
        });
        for (slot, &(_, t)) in types.iter_mut().zip(&keyed) {
            *slot = t;
        }
    }

    /// `sort_union_types` for lists over `KEYED_SORT_MAX` (bigsort1). These
    /// lists repeat their types (eslint-plugin-svelte: 1.6 M entries, 56 k
    /// types) and are mostly intersections whose keys tie. So the sort moves
    /// `u32` slots into a table with one entry per distinct type: the same
    /// slot is the same type (Go's first step gives 0), and `keyed_order`
    /// also reads the second members. Each answer has Go's sign with no
    /// effect, or comes from `compare_types`, so Go's algorithm makes Go's
    /// comparisons and gives Go's order and lazy symbol ids.
    fn sort_large_union_types(&self, types: &mut [TypeId]) {
        let mut slot_of: FxHashMap<TypeId, u32> = FxHashMap::default();
        let mut table: Vec<LargeSortEntry> = Vec::new();
        // The slots live in `types` while it is sorted (each `TypeId` holds
        // a table index), so the sort needs no second list of its length.
        for t in types.iter_mut() {
            let ty = *t;
            let slot = *slot_of.entry(ty).or_insert_with(|| {
                table.push(self.large_sort_entry(ty));
                (table.len() - 1) as u32
            });
            *t = TypeId(slot);
        }
        // A hot site (long lists): `sort_stable_func_hot`.
        crate::gostd::slices::sort_stable_func_hot(types, |&a, &b| {
            if a == b {
                return 0;
            }
            let (x, y) = (&table[a.0 as usize], &table[b.0 as usize]);
            match x.keyed_order(y) {
                Ordering::Less => -1,
                Ordering::Greater => 1,
                Ordering::Equal => self.compare_types(x.t, y.t),
            }
        });
        for t in types.iter_mut() {
            *t = table[t.0 as usize].t;
        }
    }

    /// The entry of `t` in a large union sort (`LargeSortEntry`).
    fn large_sort_entry(&self, t: TypeId) -> LargeSortEntry {
        let mut entry = LargeSortEntry {
            key: self.union_sort_key(t),
            second: 0,
            first: TypeId::NIL,
            t,
        };
        let ty = self.ty(t);
        if ty.flags.intersects(TypeFlags::INTERSECTION) && type_name_symbol(ty).is_nil() {
            let members = ty.types();
            // The key holds the member count only below 255.
            if (2..255).contains(&members.len()) {
                entry.first = members[0];
                entry.second = self.union_sort_key(members[1]);
            }
        }
        entry
    }

    /// Go `containsType(targets, t)` for each `t` of `sources` in order, up
    /// to the first that is not found (the loop of Go
    /// `isTypeSubsetOfUnion`). The searches run on the entries of the
    /// targets (`LargeSortEntry`): an entry order has Go's sign with no
    /// effect, and a tie goes to `compare_types`, so Go's searches make Go's
    /// comparisons in Go's order. On eslint-plugin-svelte (sources of
    /// 43,000 to 56,000 types in unions of about the same size) the entries
    /// decide 93% of the comparisons.
    // PERF (unionsub1): for long searches only; making the entries costs
    // about one comparison per type.
    pub(crate) fn contains_types_by_entries(&self, targets: &[TypeId], sources: &[TypeId]) -> bool {
        let table: Vec<LargeSortEntry> =
            targets.iter().map(|&t| self.large_sort_entry(t)).collect();
        sources.iter().all(|&t| {
            let entry = self.large_sort_entry(t);
            crate::gostd::slices::binary_search_func(&table, entry, |x, y| {
                // Go's first step: the same type gives 0.
                if x.t == y.t {
                    return 0;
                }
                match x.keyed_order(y) {
                    Ordering::Less => -1,
                    Ordering::Greater => 1,
                    Ordering::Equal => self.compare_types(x.t, y.t),
                }
            })
            .1
        })
    }

    /// `(union_sort_key(t), t)` for each of `types`, in order.
    pub(crate) fn union_sort_keys(&self, types: &[TypeId]) -> Vec<(u128, TypeId)> {
        types.iter().map(|&t| (self.union_sort_key(t), t)).collect()
    }

    /// Go `slices.BinarySearchFunc(types, t, CompareTypes)` on the key pairs
    /// of sorted types (`union_sort_keys`); `key` is the key of `t`. The
    /// keyed comparisons have Go's signs and effects, so the search makes
    /// Go's comparisons and gives Go's result: the index where `t` is or
    /// would be, and whether it is there.
    // Go: slices/sort.go:152 BinarySearchFunc
    pub(crate) fn search_keyed_types(
        &self,
        keyed: &[(u128, TypeId)],
        key: u128,
        t: TypeId,
    ) -> (usize, bool) {
        crate::gostd::slices::binary_search_func(keyed, (key, t), |&(k1, t1), &(k2, t2)| {
            self.compare_keyed_types(k1, t1, k2, t2)
        })
    }

    /// Go `slices.BinarySearchFunc(types, t, CompareTypes)`: the index where
    /// `t` is or would be, and whether it is there. This is Go's search, not
    /// Rust's `binary_search_by`, which looks at other elements: it made 4%
    /// more comparisons on realworld repos (unionsort1), and a comparison can
    /// assign lazy symbol ids. No keys: one search reads too few elements.
    // Go: slices/sort.go:152 BinarySearchFunc
    pub(crate) fn search_union_types(&self, types: &[TypeId], t: TypeId) -> (usize, bool) {
        crate::gostd::slices::binary_search_func(types, t, |&probe, &t| {
            self.compare_types(probe, t)
        })
    }
}

// PORT: Go `t.AsLiteralType().value.(string)`; panics like the Go type assertion.
fn literal_string_value(t: &Type) -> &str {
    match t.as_literal_type().value.as_ref() {
        Some(LiteralValue::String(s)) => s,
        _ => panic!("interface conversion: interface {{}} is not string"),
    }
}

// PORT: Go `t.AsLiteralType().value.(jsnum.Number)`.
fn literal_number_value(t: &Type) -> crate::jsnum::Number {
    match t.as_literal_type().value.as_ref() {
        Some(LiteralValue::Number(n)) => *n,
        _ => panic!("interface conversion: interface {{}} is not jsnum.Number"),
    }
}

// PORT: Go `t.AsLiteralType().value.(bool)`.
fn literal_bool_value(t: &Type) -> bool {
    match t.as_literal_type().value.as_ref() {
        Some(LiteralValue::Bool(b)) => *b,
        _ => panic!("interface conversion: interface {{}} is not bool"),
    }
}

// PORT: Go `slices.Compare` on `[]string`: element-wise `strings.Compare`,
// then the shorter slice is less.
fn compare_string_slices(s1: &[String], s2: &[String]) -> i32 {
    for (a, b) in s1.iter().zip(s2.iter()) {
        let c = compare_strings(a, b);
        if c != 0 {
            return c;
        }
    }
    if s1.len() < s2.len() {
        return -1;
    }
    if s1.len() > s2.len() {
        return 1;
    }
    0
}

// Go: checker/utilities.go:628 getSortOrderFlags
// PORT: takes `&Type` (pure read of one type's flags); returns Go `int` as `i64`.
pub fn get_sort_order_flags(t: &Type) -> i64 {
    // Return TypeFlagsEnum for all enum-like unit types (they'll be sorted by their symbols)
    if t.flags
        .intersects(TypeFlags::ENUM_LITERAL | TypeFlags::ENUM)
        && !t.flags.intersects(TypeFlags::UNION)
    {
        return i64::from(TypeFlags::ENUM.0);
    }
    i64::from(t.flags.0)
}

impl Checker {
    // Go: checker/utilities.go:636 compareTypeNames
    // PERF: chkA. It takes the two types as `compare_types` read them from
    // the arena. unionfn1: inlined into `compare_types`, its one caller.
    #[inline(always)]
    pub fn compare_type_names_of(&self, ty1: &Type, ty2: &Type) -> i32 {
        let s1 = type_name_symbol(ty1);
        let s2 = type_name_symbol(ty2);
        if s1 == s2 {
            let (a1, a2) = (ty1.alias.type_arguments(), ty2.alias.type_arguments());
            // PERF: unionfn1. Two empty lists compare as 0 (`compare_type_lists`).
            // Unnamed types (both symbols nil) take this branch, so skip the call.
            if a1.is_empty() && a2.is_empty() {
                return 0;
            }
            return self.compare_type_lists(a1, a2);
        }
        if s1.is_nil() {
            return 1;
        }
        if s2.is_nil() {
            return -1;
        }
        let (name1, name2) = (&self.sym(s1).name, &self.sym(s2).name);
        // Equal name ids are equal texts, which compare as 0.
        if name1 != name2 {
            let c = compare_strings(name1, name2);
            if c != 0 {
                return c;
            }
        }
        // Keep distinct same-named declarations together before comparing alias arguments or structure.
        // PORT: Go `t1.checker.compareSymbols` is always the worker (see `compare_types`).
        self.compare_symbols_worker(s1, s2)
    }

    // Go: checker/utilities.go:655 getTypeNameSymbol
    pub fn get_type_name_symbol(&self, t: TypeId) -> SymbolId {
        type_name_symbol(self.ty(t))
    }

    // Go: checker/utilities.go:665 getObjectTypeName
    pub fn get_object_type_name(&self, t: TypeId) -> SymbolId {
        let ty = self.ty(t);
        if ty
            .object_flags
            .intersects(ObjectFlags::CLASS_OR_INTERFACE | ObjectFlags::REFERENCE)
        {
            return ty.symbol;
        }
        SymbolId::NIL
    }
}

/// Go `getTypeNameSymbol` of a type already read from the arena.
fn type_name_symbol(ty: &Type) -> SymbolId {
    if let Some(alias) = &ty.alias {
        return alias.symbol;
    }
    if ty
        .flags
        .intersects(TypeFlags::TYPE_PARAMETER | TypeFlags::STRING_MAPPING)
        || ty
            .object_flags
            .intersects(ObjectFlags::CLASS_OR_INTERFACE | ObjectFlags::REFERENCE)
    {
        return ty.symbol;
    }
    SymbolId::NIL
}

// Go: checker/utilities.go:672 compareTupleTypes
// PORT: Go takes `*TupleType`; here `&TupleType` borrowed from the type arena.
// Go pointer equality is `std::ptr::eq`.
pub fn compare_tuple_types(t1: &TupleType, t2: &TupleType) -> i32 {
    if std::ptr::eq(t1, t2) {
        return 0;
    }
    if t1.readonly != t2.readonly {
        return if t1.readonly { 1 } else { -1 };
    }
    if t1.element_infos.len() != t2.element_infos.len() {
        return t1.element_infos.len() as i32 - t2.element_infos.len() as i32;
    }
    for i in 0..t1.element_infos.len() {
        let c = clamp_compare(
            i64::from(t1.element_infos[i].flags.0) - i64::from(t2.element_infos[i].flags.0),
        );
        if c != 0 {
            return c;
        }
    }
    for i in 0..t1.element_infos.len() {
        let c = compare_element_labels(
            t1.element_infos[i].labeled_declaration,
            t2.element_infos[i].labeled_declaration,
        );
        if c != 0 {
            return c;
        }
    }
    0
}

// Go: checker/utilities.go:695 compareElementLabels
pub fn compare_element_labels(n1: Node, n2: Node) -> i32 {
    if n1 == n2 {
        return 0;
    }
    if n1.is_nil() {
        return -1;
    }
    if n2.is_nil() {
        return 1;
    }
    compare_strings(n1.name().text(), n2.name().text())
}

impl Checker {
    // Go: checker/utilities.go:708 compareTypeLists
    pub fn compare_type_lists(&self, s1: &[TypeId], s2: &[TypeId]) -> i32 {
        if s1.len() != s2.len() {
            return s1.len() as i32 - s2.len() as i32;
        }
        for (&t1, &t2) in s1.iter().zip(s2) {
            // PERF: unionfn1. `compare_types` of a type and itself is 0.
            if t1 == t2 {
                continue;
            }
            let c = self.compare_types(t1, t2);
            if c != 0 {
                return c;
            }
        }
        0
    }

    // Go: checker/utilities.go:720 compareTypeMappers
    pub fn compare_type_mappers(&self, m1: MapperId, m2: MapperId) -> i32 {
        if m1 == m2 {
            return 0;
        }
        if m1.is_nil() {
            return 1;
        }
        if m2.is_nil() {
            return -1;
        }
        let kind1 = self.mapper(m1).kind();
        let kind2 = self.mapper(m2).kind();
        if kind1 != kind2 {
            return kind1 as i32 - kind2 as i32;
        }
        match (self.mapper(m1), self.mapper(m2)) {
            (TypeMapper::Simple(m1), TypeMapper::Simple(m2)) => {
                let c = self.compare_types(m1.source, m2.source);
                if c != 0 {
                    return c;
                }
                self.compare_types(m1.target, m2.target)
            }
            (TypeMapper::Array(m1, _), TypeMapper::Array(m2, _)) => {
                let c = self.compare_type_lists(&m1.sources, &m2.sources);
                if c != 0 {
                    return c;
                }
                self.compare_type_lists(&m1.targets, &m2.targets)
            }
            (TypeMapper::Merged(m1), TypeMapper::Merged(m2)) => {
                let (a1, a2, b1, b2) = (m1.m1, m1.m2, m2.m1, m2.m2);
                let c = self.compare_type_mappers(a1, b1);
                if c != 0 {
                    return c;
                }
                self.compare_type_mappers(a2, b2)
            }
            // PORT: Go switches on `kind1`; kinds other than Simple, Array and
            // Merged (all `TypeMapperKindUnknown`) fall through to `return 0`.
            _ => 0,
        }
    }

    // Go: checker/utilities.go:771 getDeclarationModifierFlagsFromSymbol
    pub fn get_declaration_modifier_flags_from_symbol(&self, s: SymbolId) -> ModifierFlags {
        self.get_declaration_modifier_flags_from_symbol_ex(s, false /*isWrite*/)
    }

    // Go: checker/utilities.go:775 getDeclarationModifierFlagsFromSymbolEx
    pub fn get_declaration_modifier_flags_from_symbol_ex(
        &self,
        s: SymbolId,
        is_write: bool,
    ) -> ModifierFlags {
        let sym = self.sym(s);
        if sym.check_flags.intersects(CheckFlags::SYNTHETIC) {
            let check_flags = sym.check_flags;
            let mut access_modifier = ModifierFlags::NONE;
            if !is_write && check_flags.intersects(CheckFlags::CONTAINS_PUBLIC)
                || is_write && check_flags.intersects(CheckFlags::CONTAINS_WRITE_PUBLIC)
            {
                access_modifier = ModifierFlags::PUBLIC;
            } else if !is_write && check_flags.intersects(CheckFlags::CONTAINS_PROTECTED)
                || is_write && check_flags.intersects(CheckFlags::CONTAINS_WRITE_PROTECTED)
            {
                access_modifier = ModifierFlags::PROTECTED;
            } else if !is_write && check_flags.intersects(CheckFlags::CONTAINS_PRIVATE)
                || is_write && check_flags.intersects(CheckFlags::CONTAINS_WRITE_PRIVATE)
            {
                access_modifier = ModifierFlags::PRIVATE;
            }
            if check_flags.intersects(CheckFlags::CONTAINS_STATIC) {
                return access_modifier | ModifierFlags::STATIC;
            }
            return access_modifier;
        }
        if sym.value_declaration.is_some() {
            let mut declaration = Node::NIL;
            if is_write {
                declaration = sym
                    .declarations
                    .iter()
                    .copied()
                    .find(|&d| is_set_accessor_declaration(d))
                    .unwrap_or(Node::NIL);
            }
            if declaration.is_nil() && sym.flags.intersects(SymbolFlags::GET_ACCESSOR) {
                declaration = sym
                    .declarations
                    .iter()
                    .copied()
                    .find(|&d| is_get_accessor_declaration(d))
                    .unwrap_or(Node::NIL);
            }
            if declaration.is_nil() {
                declaration = sym.value_declaration;
            }
            let flags = get_combined_modifier_flags(declaration);
            if sym.parent.is_some() && self.sym(sym.parent).flags.intersects(SymbolFlags::CLASS) {
                return flags;
            }
            return flags.without(ModifierFlags::ACCESSIBILITY_MODIFIER);
        }
        if sym.flags.intersects(SymbolFlags::PROTOTYPE) {
            return ModifierFlags::PUBLIC | ModifierFlags::STATIC;
        }
        ModifierFlags::NONE
    }
}

// PORT: the operator predicates below are renamed with a `checker_` prefix.
// `ast/fields.rs` already exports `is_exponentiation_operator`, ...,
// `is_binary_operator` (Go `ast.IsXxxOperator`, generated from the same kind
// sets), so the unprefixed names would be ambiguous through the prelude.

// Go: checker/utilities.go:2002 quotedAndCommaSeparated
pub fn quoted_and_comma_separated(items: &[String]) -> String {
    items
        .iter()
        .map(|item| format!("'{item}'"))
        .collect::<Vec<_>>()
        .join(", ")
}

// Go: checker/utilities.go:814 isExponentiationOperator
pub fn checker_is_exponentiation_operator(kind: SyntaxKind) -> bool {
    kind == SyntaxKind::AsteriskAsteriskToken
}

// Go: checker/utilities.go:818 isMultiplicativeOperator
pub fn checker_is_multiplicative_operator(kind: SyntaxKind) -> bool {
    kind == SyntaxKind::AsteriskToken
        || kind == SyntaxKind::SlashToken
        || kind == SyntaxKind::PercentToken
}

// Go: checker/utilities.go:822 isMultiplicativeOperatorOrHigher
pub fn checker_is_multiplicative_operator_or_higher(kind: SyntaxKind) -> bool {
    checker_is_exponentiation_operator(kind) || checker_is_multiplicative_operator(kind)
}

// Go: checker/utilities.go:826 isAdditiveOperator
pub fn checker_is_additive_operator(kind: SyntaxKind) -> bool {
    kind == SyntaxKind::PlusToken || kind == SyntaxKind::MinusToken
}

// Go: checker/utilities.go:830 isAdditiveOperatorOrHigher
pub fn checker_is_additive_operator_or_higher(kind: SyntaxKind) -> bool {
    checker_is_additive_operator(kind) || checker_is_multiplicative_operator_or_higher(kind)
}

// Go: checker/utilities.go:834 isShiftOperator
pub fn checker_is_shift_operator(kind: SyntaxKind) -> bool {
    kind == SyntaxKind::LessThanLessThanToken
        || kind == SyntaxKind::GreaterThanGreaterThanToken
        || kind == SyntaxKind::GreaterThanGreaterThanGreaterThanToken
}

// Go: checker/utilities.go:839 isShiftOperatorOrHigher
pub fn checker_is_shift_operator_or_higher(kind: SyntaxKind) -> bool {
    checker_is_shift_operator(kind) || checker_is_additive_operator_or_higher(kind)
}

// Go: checker/utilities.go:843 isRelationalOperator
pub fn checker_is_relational_operator(kind: SyntaxKind) -> bool {
    kind == SyntaxKind::LessThanToken
        || kind == SyntaxKind::LessThanEqualsToken
        || kind == SyntaxKind::GreaterThanToken
        || kind == SyntaxKind::GreaterThanEqualsToken
        || kind == SyntaxKind::InstanceOfKeyword
        || kind == SyntaxKind::InKeyword
}

// Go: checker/utilities.go:848 isRelationalOperatorOrHigher
pub fn checker_is_relational_operator_or_higher(kind: SyntaxKind) -> bool {
    checker_is_relational_operator(kind) || checker_is_shift_operator_or_higher(kind)
}

// Go: checker/utilities.go:852 isEqualityOperator
pub fn checker_is_equality_operator(kind: SyntaxKind) -> bool {
    kind == SyntaxKind::EqualsEqualsToken
        || kind == SyntaxKind::EqualsEqualsEqualsToken
        || kind == SyntaxKind::ExclamationEqualsToken
        || kind == SyntaxKind::ExclamationEqualsEqualsToken
}

// Go: checker/utilities.go:857 isEqualityOperatorOrHigher
pub fn checker_is_equality_operator_or_higher(kind: SyntaxKind) -> bool {
    checker_is_equality_operator(kind) || checker_is_relational_operator_or_higher(kind)
}

// Go: checker/utilities.go:861 isBitwiseOperator
pub fn checker_is_bitwise_operator(kind: SyntaxKind) -> bool {
    kind == SyntaxKind::AmpersandToken
        || kind == SyntaxKind::BarToken
        || kind == SyntaxKind::CaretToken
}

// Go: checker/utilities.go:865 isBitwiseOperatorOrHigher
pub fn checker_is_bitwise_operator_or_higher(kind: SyntaxKind) -> bool {
    checker_is_bitwise_operator(kind) || checker_is_equality_operator_or_higher(kind)
}

// Go: checker/utilities.go:869 isLogicalOperatorOrHigher
pub fn checker_is_logical_operator_or_higher(kind: SyntaxKind) -> bool {
    is_logical_binary_operator(kind) || checker_is_bitwise_operator_or_higher(kind)
}

// Go: checker/utilities.go:873 isAssignmentOperatorOrHigher
pub fn checker_is_assignment_operator_or_higher(kind: SyntaxKind) -> bool {
    kind == SyntaxKind::QuestionQuestionToken
        || checker_is_logical_operator_or_higher(kind)
        || is_assignment_operator(kind)
}

// Go: checker/utilities.go:877 isBinaryOperator
pub fn checker_is_binary_operator(kind: SyntaxKind) -> bool {
    checker_is_assignment_operator_or_higher(kind) || kind == SyntaxKind::CommaToken
}

impl Checker {
    // Go: checker/utilities.go:881 isObjectLiteralType
    pub fn is_object_literal_type(&self, t: TypeId) -> bool {
        self.ty(t)
            .object_flags
            .intersects(ObjectFlags::OBJECT_LITERAL)
    }
}

// Go: checker/utilities.go:885 isDeclarationReadonly
pub fn is_declaration_readonly(declaration: Node) -> bool {
    get_combined_modifier_flags(declaration).intersects(ModifierFlags::READONLY)
        && !is_parameter_property_declaration(declaration, declaration.parent())
}

// orderedSetMapThreshold is the size at which an orderedSet materializes its dedup map.
// Below this, contains() scans the values slice.
// Go: checker/utilities.go:891 orderedSetMapThreshold
pub const ORDERED_SET_MAP_THRESHOLD: usize = 16;

// Go: checker/utilities.go:893 orderedSet
// PORT: Go nil map is `None`.
#[derive(Clone, Debug)]
pub struct OrderedSet<T: Eq + std::hash::Hash + Clone> {
    pub values_by_key: Option<FxHashSet<T>>,
    pub values: Vec<T>,
}

impl<T: Eq + std::hash::Hash + Clone> Default for OrderedSet<T> {
    fn default() -> Self {
        Self {
            values_by_key: None,
            values: Vec::new(),
        }
    }
}

impl<T: Eq + std::hash::Hash + Clone> OrderedSet<T> {
    // Go: checker/utilities.go:963 orderedSet.contains
    pub fn contains(&self, value: &T) -> bool {
        match &self.values_by_key {
            None => self.values.contains(value),
            Some(values_by_key) => values_by_key.contains(value),
        }
    }

    // Go: checker/utilities.go:971 orderedSet.add
    pub fn add(&mut self, value: T) {
        self.values.push(value.clone());
        // Small sets are served by a linear scan over values; only materialize the map once the set
        // grows large enough for hashing to win.
        if self.values_by_key.is_none() {
            if self.values.len() <= ORDERED_SET_MAP_THRESHOLD {
                return;
            }
            let mut values_by_key = FxHashSet::default();
            values_by_key.reserve(self.values.len());
            for v in &self.values[..self.values.len() - 1] {
                values_by_key.insert(v.clone());
            }
            self.values_by_key = Some(values_by_key);
        }
        if let Some(values_by_key) = &mut self.values_by_key {
            values_by_key.insert(value);
        }
    }
}

// Go: checker/utilities.go:987 getContainingFunctionOrClassStaticBlock
pub fn get_containing_function_or_class_static_block(node: Node) -> Node {
    find_ancestor(
        node.parent(),
        is_function_like_or_class_static_block_declaration,
    )
}

// Go: checker/utilities.go:991 isNodeDescendantOf
// PORT: renamed; `is_node_descendant_of` is `ast.IsNodeDescendantOf` (same body).
pub fn checker_is_node_descendant_of(mut node: Node, ancestor: Node) -> bool {
    while node.is_some() {
        if node == ancestor {
            return true;
        }
        node = node.parent();
    }
    false
}

impl Checker {
    // Go: checker/utilities.go:1001 isTypeUsableAsPropertyName
    pub fn is_type_usable_as_property_name(&self, t: TypeId) -> bool {
        self.ty(t)
            .flags
            .intersects(TypeFlags::STRING_OR_NUMBER_LITERAL_OR_UNIQUE)
    }

    // Go: checker/utilities.go:1008 getPropertyNameFromType
    /**
     * Gets the symbolic name for a member from its type.
     */
    pub fn get_property_name_from_type(&self, t: TypeId) -> String {
        let ty = self.ty(t);
        if ty.flags.intersects(TypeFlags::STRING_LITERAL) {
            return literal_string_value(ty).to_string();
        }
        if ty.flags.intersects(TypeFlags::NUMBER_LITERAL) {
            return literal_number_value(ty).to_string();
        }
        if ty.flags.intersects(TypeFlags::UNIQUE_ES_SYMBOL) {
            return ty.as_unique_es_symbol_type().name.clone();
        }
        panic!("Unhandled case in getPropertyNameFromType")
    }
}

// Go: checker/utilities.go:1020 isNumericLiteralName
pub fn is_numeric_literal_name(name: &str) -> bool {
    // The intent of numeric names is that
    //     - they are names with text in a numeric form, and that
    //     - setting properties/indexing with them is always equivalent to doing so with the numeric literal 'numLit',
    //         acquired by applying the abstract 'ToNumber' operation on the name's text.
    //
    // The subtlety is in the latter portion, as we cannot reliably say that anything that looks like a numeric literal is a numeric name.
    // In fact, it is the case that the text of the name must be equal to 'ToString(numLit)' for this to hold.
    //
    // Consider the property name '"0xF00D"'. When one indexes with '0xF00D', they are actually indexing with the value of 'ToString(0xF00D)'
    // according to the ECMAScript specification, so it is actually as if the user indexed with the string '"61453"'.
    // Thus, the text of all numeric literals equivalent to '61543' such as '0xF00D', '0xf00D', '0170015', etc. are not valid numeric names
    // because their 'ToString' representation is not equal to their original text.
    // This is motivated by ECMA-262 sections 9.3.1, 9.8.1, 11.1.5, and 11.2.1.
    //
    // Here, we test whether 'ToString(ToNumber(name))' is exactly equal to 'name'.
    // The '+' prefix operator is equivalent here to applying the abstract ToNumber operation.
    // Applying the 'toString()' method on a number gives us the abstract ToString operation on a number.
    //
    // Note that this accepts the values 'Infinity', '-Infinity', and 'NaN', and that this is intentional.
    // This is desired behavior, because when indexing with them as numeric entities, you are indexing
    // with the strings '"Infinity"', '"-Infinity"', and '"NaN"' respectively.
    crate::jsnum::from_string(name).to_string() == name
}

// Go: checker/utilities.go:1045 isThisProperty
pub fn is_this_property(node: Node) -> bool {
    (is_property_access_expression(node) || is_element_access_expression(node))
        && node.expression().kind() == SyntaxKind::ThisKeyword
}

#[cfg(test)]
pub(crate) mod union_sort_tests {
    use super::*;
    use crate::gostd::slices::{binary_search_func, sort_stable_func};
    use crate::scanner_util::go_string_from_bytes;
    use std::cmp::Ordering;

    /// Strings in the port form of Go strings: ASCII, long shared prefixes,
    /// NUL, non-ASCII, lone surrogate and invalid byte units, the internal
    /// symbol name prefix.
    fn strings() -> Vec<String> {
        let mut v: Vec<String> = [
            "",
            "a",
            "ab",
            "abc",
            "abcdefghijk",
            "abcdefghijkl",
            "abcdefghijkm",
            "abcdefghij",
            "abcdefghijk\u{0}",
            "\u{0}",
            "Z",
            "zz",
            "é",
            "aé",
            "a\u{7f}",
            "abcdefghijé",
            "abcdefghijkéz",
            "\u{FDD0}",
            crate::ast::INTERNAL_SYMBOL_NAME_PREFIX,
            "日本",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        for bytes in [
            &[0x61, 0xfe][..],
            &[0x61, 0x80, 0x62],
            &[0xed, 0xa0, 0x80],
            &[0x61, 0xed, 0xbf, 0xbf],
            &[0xff],
        ] {
            v.push(go_string_from_bytes(bytes.to_vec()));
        }
        v
    }

    /// The prefix order never contradicts Go `strings.Compare`.
    #[test]
    fn go_bytes_prefix_keeps_go_string_order() {
        let v = strings();
        let mut decided = 0;
        for a in &v {
            for b in &v {
                if go_bytes_prefix(a) < go_bytes_prefix(b) {
                    decided += 1;
                    assert_eq!(compare_go_strings(a, b), Ordering::Less, "{a:?} {b:?}");
                }
            }
        }
        assert!(decided > 200, "{decided}");
    }

    /// `number_sort_value` order is Go `cmp.Compare` order.
    #[test]
    fn number_sort_value_is_cmp_compare_order() {
        let v = [
            f64::NAN,
            f64::NEG_INFINITY,
            -1e300,
            -1.5,
            -f64::MIN_POSITIVE,
            -0.0,
            0.0,
            5e-324,
            1.0,
            1.5,
            1e300,
            f64::INFINITY,
        ];
        for &a in &v {
            for &b in &v {
                let (a, b) = (crate::jsnum::Number(a), crate::jsnum::Number(b));
                let c = compare_numbers(a, b);
                assert_eq!(
                    number_sort_value(a).cmp(&number_sort_value(b)),
                    c.cmp(&0),
                    "{a:?} {b:?}"
                );
            }
        }
    }

    /// The types of the type aliases in `source` (one file `a.ts`), then
    /// the members of those that are unions or intersections, on the
    /// checker of that file. Each call writes its files in a temp dir of
    /// its own, so no call reads or removes the files of another.
    pub(crate) fn with_alias_types<R: Send + 'static>(
        source: &str,
        f: impl FnOnce(&mut Checker, &[TypeId]) -> R + Send + 'static,
    ) -> R {
        static CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let call = CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("ts_goport_unionsort_{}_{call}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.ts"), source).unwrap();
        std::fs::write(
            dir.join("tsconfig.json"),
            r#"{ "compilerOptions": { "strict": true, "target": "es2020", "types": [] }, "files": ["a.ts"] }"#,
        )
        .unwrap();
        let config = dir.join("tsconfig.json");
        let program = crate::program::try_load_version(&config.to_string_lossy(), |_| {})
            .unwrap_or_else(|e| panic!("cannot load {}: {e}", config.display()));
        let _ = std::fs::remove_dir_all(&dir);
        let scope = crate::core::enter_program(Some(program));
        let file = program
            .source_files()
            .find(|file| file.info.file_name.ends_with("/a.ts"))
            .expect("a.ts is not in the program")
            .root;
        let result = crate::program::with_type_checker_for_file(file, move |checker| {
            let mut types: Vec<TypeId> = file
                .statements()
                .iter()
                .filter(|s| s.kind() == SyntaxKind::TypeAliasDeclaration)
                .map(|alias| checker.get_type_from_type_node(alias.type_()))
                .collect();
            // Then the members of the unions and intersections, which have
            // no alias.
            for i in 0..types.len() {
                if checker
                    .ty(types[i])
                    .flags
                    .intersects(TypeFlags::UNION_OR_INTERSECTION)
                {
                    for &m in checker.ty(types[i]).types().to_vec().iter() {
                        if !types.contains(&m) {
                            types.push(m);
                        }
                    }
                }
            }
            f(checker, &types)
        });
        drop(scope);
        crate::program::release_program(program);
        result
    }

    /// Members with a declaration (interfaces, a class, an object literal
    /// type) and mapped type members without one, whose names share long
    /// prefixes or are not ASCII.
    const SYMBOL_SOURCE: &str = r#"
interface I { b: 1; a: 1; abcdefghij1: 1; abcdefghij2: 1 }
interface J { a: 2; z: 2 }
class C { m() {} n = 1 }
type O = { q: 1; p: 2 }
type K = "b" | "a" | "abcdefghij1" | "abcdefghij2" | "abcdefgh" | "abcdefghX" | "abcdefg" | "abcdefgX" | "é" | "aé" | "Z" | "";
type M = { [P in K]: P };
type M2 = { [P in K | "extra" | "q"]: 1 };
type H = { [P in keyof I]: 2 };
type U = I & M & O;
"#;

    /// The `SymbolSortKey` order never contradicts `compareSymbolsWorker`,
    /// and `sort_symbol_sort_keys` gives Go's sort (`gostd` with
    /// `compare_symbols_worker`) on many orders of the symbols.
    #[test]
    fn symbol_sort_order_agrees_with_compare_symbols() {
        with_alias_types(SYMBOL_SOURCE, |c, types| {
            let mut symbols: Vec<SymbolId> = Vec::new();
            for &t in types {
                for &s in c.get_properties_of_type(t).iter() {
                    if !symbols.contains(&s) {
                        symbols.push(s);
                    }
                }
            }
            let mut last_file = (Node::NIL, 0);
            let keys: Vec<SymbolSortKey> = symbols
                .iter()
                .map(|&s| c.symbol_sort_key(s, &mut last_file))
                .collect();
            let mut decided = 0;
            let mut by_name = 0;
            let mut ties = 0;
            for a in &keys {
                for b in &keys {
                    if a.symbol == b.symbol {
                        continue;
                    }
                    assert!(a.order != NO_SORT_ORDER, "{:?}", a.name);
                    let s = c.compare_symbols_worker(a.symbol, b.symbol).signum();
                    match a.order.cmp(&b.order) {
                        Ordering::Less => assert_eq!(s, -1, "{:?} {:?}", a.name, b.name),
                        Ordering::Greater => assert_eq!(s, 1, "{:?} {:?}", a.name, b.name),
                        Ordering::Equal => ties += 1,
                    }
                    decided += usize::from(a.order != b.order);
                    by_name +=
                        usize::from(a.order != b.order && !a.has_declaration && !b.has_declaration);
                }
            }
            // Names that share their first 8 bytes tie (the order keeps 63
            // bits of them).
            let key = |name: &str| {
                keys.iter()
                    .find(|k| !k.has_declaration && &*k.name == name)
                    .unwrap_or_else(|| panic!("no member {name:?} without a declaration"))
                    .order
            };
            assert_eq!(key("abcdefghij1"), key("abcdefghij2"));
            assert_eq!(key("abcdefgh"), key("abcdefghX"));
            assert_ne!(key("abcdefg"), key("abcdefgX"));
            assert!(
                decided > 400 && by_name > 100 && ties > 0,
                "{decided} {by_name} {ties}"
            );
            let mut state = 0x9e37_79b9_7f4a_7c15u64;
            for round in 0..64 {
                let mut list: Vec<SymbolId> = symbols.clone();
                for i in (1..list.len()).rev() {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    list.swap(i, (state % (i as u64 + 1)) as usize);
                }
                let n = 2 + round % (list.len() - 1);
                list.truncate(n);
                let mut keyed = list.clone();
                c.sort_symbols(&mut keyed);
                let mut go = list;
                crate::gostd::slices::sort_func(&mut go, |&a, &b| c.compare_symbols_worker(a, b));
                assert_eq!(keyed, go, "round {round}");
            }
        });
    }

    /// Each arm of `compare_types`, and the ties: two interfaces with one
    /// name, references to one generic type, intersections with the same
    /// first member, string literals with a long shared prefix, objects
    /// with no symbol (tuples), instantiations of one anonymous type.
    const SOURCE: &str = r#"
interface A { a: 1 }
interface B { b: 1 }
interface Abcdefghijklmn1 { c: 1 }
interface Abcdefghijklmn2 { d: 1 }
namespace N1 { export interface Same { x: 1 } }
namespace N2 { export interface Same { y: 1 } }
enum E { X, Y, Z = "z" }
declare const s1: unique symbol;
declare const s2: unique symbol;
declare function f<T>(x: T): { v: T };
type T0 = string;
type T1 = number;
type T2 = boolean;
type T3 = any;
type T4 = unknown;
type T5 = never;
type T6 = undefined;
type T7 = null;
type T8 = void;
type T9 = object;
type T10 = bigint;
type T11 = symbol;
type T12 = "abcdefghijklmnop1";
type T13 = "abcdefghijklmnop2";
type T14 = "abc";
type T15 = "";
type T16 = "é";
type T17 = "aé";
type T18 = 1;
type T19 = -1;
type T20 = 0;
type T21 = 1.5;
type T22 = 1e300;
type T23 = true;
type T24 = false;
type T25 = 10n;
type T26 = -5n;
type T27 = A;
type T28 = B;
type T29 = N1.Same;
type T30 = N2.Same;
type T31 = A[];
type T32 = B[];
type T33 = Array<string>;
type T34 = Promise<A>;
type T35 = Promise<B>;
type T36 = [string];
type T37 = [number, string];
type T38 = readonly [string];
type T39 = { x: 1 };
type T40 = { y: 2 };
type T41 = () => void;
type T42 = A & B;
type T43 = A & { z: 1 };
type T44 = A & B & { z: 1 };
type T45 = string & { __brand: 1 };
type T46 = number & { __brand: 1 };
type T47 = E;
type T48 = E.X;
type T49 = E.Y;
type T50 = E.Z;
type T51 = typeof s1;
type T52 = typeof s2;
type T53 = `a${string}`;
type T54 = `b${number}`;
type T55 = `abcdefghijklmn${string}x`;
type T56 = `abcdefghijklmn${number}y`;
type T57 = keyof A;
type T58 = A["a"];
type T59 = Uppercase<string>;
type T60 = A | B;
type T61 = "a" | "b";
type T62 = string | number;
type T63 = Abcdefghijklmn1;
type T64 = Abcdefghijklmn2;
type T65 = ReturnType<typeof f<string>>;
type T66 = ReturnType<typeof f<number>>;
type T67 = Abcdefghijklmn1 & B;
type T68 = Abcdefghijklmn2 & B;
type T69 = Promise<A> | Promise<B> | Array<A> | Array<B> | [A] | [B, A];
type T70 = (A & B) | (A & { z: 1 }) | (Abcdefghijklmn1 & B) | (B & Abcdefghijklmn2);
type T71 = "abcdefghijklmnop1" | "abcdefghijklmnop2" | `x${string}` | `x${number}`;
type T72 = { x: 1 } | { y: 2 } | ReturnType<typeof f<string>> | ReturnType<typeof f<number>>;
"#;

    /// Keys never contradict `compare_types`, keys decide many pairs, and
    /// the ties named in `SOURCE` tie.
    #[test]
    fn union_sort_key_agrees_with_compare_types() {
        with_alias_types(SOURCE, |c, types| {
            let mut decided = 0;
            let mut ties = 0;
            for &a in types {
                for &b in types {
                    if a == b {
                        continue;
                    }
                    let (ka, kb) = (c.union_sort_key(a), c.union_sort_key(b));
                    let s = c.compare_types(a, b).signum();
                    match ka.cmp(&kb) {
                        Ordering::Less => assert_eq!(s, -1, "{a:?} {b:?} {ka:#x} {kb:#x}"),
                        Ordering::Greater => assert_eq!(s, 1, "{a:?} {b:?} {ka:#x} {kb:#x}"),
                        Ordering::Equal => ties += 1,
                    }
                    decided += usize::from(ka != kb);
                }
            }
            // The same name in two namespaces: equal keys.
            let key = |i: usize| c.union_sort_key(types[i]);
            assert_eq!(key(29), key(30));
            // Names and values that share their first 11 bytes tie.
            assert_eq!(key(63), key(64));
            assert_eq!(key(12), key(13));
            assert_ne!(key(14), key(15));
            assert!(decided > 3000 && ties > 0, "{decided} {ties}");
        });
    }

    /// The keyed sort and the keyed search give Go's results (`gostd` sort
    /// and search with `compare_types`) on many orders of the types, with
    /// duplicates.
    #[test]
    fn keyed_sort_and_search_match_go() {
        with_alias_types(SOURCE, |c, types| {
            let mut state = 0x9e37_79b9_7f4a_7c15u64;
            for round in 0..64 {
                // A deterministic shuffle with duplicates, of 8 to 80 types.
                let n = 8 + round % 73;
                let list: Vec<TypeId> = (0..n)
                    .map(|_| {
                        state ^= state << 13;
                        state ^= state >> 7;
                        state ^= state << 17;
                        types[(state % types.len() as u64) as usize]
                    })
                    .collect();
                let mut keyed = list.clone();
                c.sort_union_types(&mut keyed);
                let mut go = list.clone();
                sort_stable_func(&mut go, |&a, &b| c.compare_types(a, b));
                assert_eq!(keyed, go, "round {round}");
                go.dedup();
                let pairs = c.union_sort_keys(&go);
                for &t in types {
                    let want = binary_search_func(&go[..], t, |&a, &b| c.compare_types(a, b));
                    let key = c.union_sort_key(t);
                    assert_eq!(c.search_keyed_types(&pairs, key, t), want, "round {round}");
                    assert_eq!(c.search_union_types(&go, t), want, "round {round}");
                }
            }
        });
    }

    /// `SOURCE` with intersections like eslint-plugin-svelte's: unions of
    /// interfaces crossed with unions of interfaces, so many intersections
    /// share their first member, and names that share 11 bytes tie.
    const LARGE_SOURCE: &str = r#"
interface SvelteShorthandAttribute { k: 1 }
interface SvelteShorthandDirective { k: 2 }
interface SvelteProgram { k: 3 }
interface SvelteScriptElement { k: 4 }
interface Box<T> { v: T }
type U0 = (A | B | SvelteProgram | SvelteScriptElement) & (SvelteShorthandAttribute | SvelteShorthandDirective | Box<A> | Box<B> | { w: 1 });
type U1 = (A | SvelteProgram) & (B | Box<string>) & ({ p: 1 } | SvelteShorthandAttribute);
"#;

    /// Lists over `KEYED_SORT_MAX` (`sort_large_union_types`): when
    /// `keyed_order` tells an order it is the order of `compare_types`, it
    /// tells many pairs through the second members, and the sort gives Go's
    /// list for shuffles with many repeated types and tied keys.
    #[test]
    fn large_union_sort_matches_go() {
        with_alias_types(&format!("{SOURCE}{LARGE_SOURCE}"), |c, aliases| {
            // The aliases and the members of the union aliases.
            let mut types = aliases.to_vec();
            for &t in aliases {
                if c.ty(t).flags.intersects(TypeFlags::UNION) {
                    types.extend_from_slice(c.ty(t).types());
                }
            }
            types.sort_unstable();
            types.dedup();
            let (mut second, mut ties) = (0, 0);
            for &a in &types {
                for &b in &types {
                    if a == b {
                        continue;
                    }
                    let (ea, eb) = (c.large_sort_entry(a), c.large_sort_entry(b));
                    let s = c.compare_types(a, b).signum();
                    match ea.keyed_order(&eb) {
                        Ordering::Less => assert_eq!(s, -1, "{a:?} {b:?}"),
                        Ordering::Greater => assert_eq!(s, 1, "{a:?} {b:?}"),
                        Ordering::Equal => ties += 1,
                    }
                    second += usize::from(ea.key == eb.key && ea.keyed_order(&eb).is_ne());
                }
            }
            assert!(second > 40 && ties > 0, "{second} {ties}");
            let mut state = 0x2545_f491_4f6c_dd1du64;
            for n in [KEYED_SORT_MAX + 1, 3 * KEYED_SORT_MAX + 7] {
                let list: Vec<TypeId> = (0..n)
                    .map(|_| {
                        state ^= state << 13;
                        state ^= state >> 7;
                        state ^= state << 17;
                        types[(state % types.len() as u64) as usize]
                    })
                    .collect();
                let mut large = list.clone();
                c.sort_union_types(&mut large);
                let mut go = list;
                sort_stable_func(&mut go, |&a, &b| c.compare_types(a, b));
                assert_eq!(large, go, "n {n}");
            }
        });
    }

    /// `contains_types_by_entries` (unionsub1) gives Go's answer: each
    /// source searched with `compare_types` in Go's order of the targets,
    /// up to the first miss. The types are those of
    /// `large_union_sort_matches_go`; the targets lack some of them.
    #[test]
    fn entry_searches_match_go() {
        with_alias_types(&format!("{SOURCE}{LARGE_SOURCE}"), |c, aliases| {
            let mut types = aliases.to_vec();
            for &t in aliases {
                if c.ty(t).flags.intersects(TypeFlags::UNION) {
                    types.extend_from_slice(c.ty(t).types());
                }
            }
            types.sort_unstable();
            types.dedup();
            sort_stable_func(&mut types, |&a, &b| c.compare_types(a, b));
            let go = |targets: &[TypeId], sources: &[TypeId]| {
                sources
                    .iter()
                    .all(|&t| binary_search_func(targets, t, |&a, &b| c.compare_types(a, b)).1)
            };
            let mut state = 0x9e37_79b9_7f4a_7c15u64;
            let (mut found, mut missed) = (0, 0);
            for round in 0..64 {
                let (mut targets, mut sources) = (Vec::new(), Vec::new());
                for &t in &types {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    if round % 2 == 0 || state % 16 != 0 {
                        targets.push(t);
                    }
                    if state % 3 == 0 {
                        sources.push(t);
                    }
                }
                let want = go(&targets, &sources);
                assert_eq!(
                    c.contains_types_by_entries(&targets, &sources),
                    want,
                    "round {round}"
                );
                if want {
                    found += 1
                } else {
                    missed += 1
                }
                for &t in &types {
                    let want = go(&targets, &[t]);
                    assert_eq!(c.contains_types_by_entries(&targets, &[t]), want, "{t:?}");
                }
            }
            assert!(found > 0 && missed > 0, "{found} {missed}");
        });
    }

    /// Lists over `KEYED_SORT_MAX`: the large sort makes the comparisons of
    /// Go's sort that have an effect, in Go's order. The one effect of
    /// `compare_types` is a lazy symbol id: `compare_symbols_worker` falls
    /// back to ids for two symbols with no declaration and one name (Go
    /// makes such symbols for import attributes, checker.go:5604). So the
    /// `LARGE_SOURCE` types get 24 anonymous types of such symbols, and the
    /// intersections of each with `A`, and the two sorts of one shuffle,
    /// each on its own checker, give the same list and the same ids to the
    /// same symbols, in the same order.
    #[test]
    fn large_union_sort_assigns_go_symbol_ids() {
        let run = |large: bool| {
            with_alias_types(&format!("{SOURCE}{LARGE_SOURCE}"), move |c, aliases| {
                let mut types = aliases.to_vec();
                for &t in aliases {
                    if c.ty(t).flags.intersects(TypeFlags::UNION) {
                        types.extend_from_slice(c.ty(t).types());
                    }
                }
                types.sort_unstable();
                types.dedup();
                let a = aliases[0];
                let bare: Vec<SymbolId> = (0..24)
                    .map(|_| {
                        c.new_symbol(
                            SymbolFlags::TYPE_LITERAL,
                            crate::ast::INTERNAL_SYMBOL_NAME_TYPE,
                        )
                    })
                    .collect();
                for &symbol in &bare {
                    let t = c.new_anonymous_type(symbol, SymbolTable::NIL, &[], &[], &[]);
                    let both = c.get_intersection_type(&[t, a]);
                    types.extend([t, both]);
                }
                let mut state = 0x6a09_e667_f3bc_c908u64;
                let mut list: Vec<TypeId> = (0..=KEYED_SORT_MAX)
                    .map(|_| {
                        state ^= state << 13;
                        state ^= state >> 7;
                        state ^= state << 17;
                        types[(state % types.len() as u64) as usize]
                    })
                    .collect();
                let first = crate::ast::next_ids().1;
                if large {
                    c.sort_union_types(&mut list);
                } else {
                    sort_stable_func(&mut list, |&x, &y| c.compare_types(x, y));
                }
                let last = crate::ast::next_ids().1;
                // The id that the sort gave each bare symbol, from 1 in the
                // order the sort gave them; 0 for none.
                let ids: Vec<u64> = bare
                    .iter()
                    .map(|&symbol| {
                        let id = crate::ast::get_symbol_id(&c.symbols, symbol);
                        if id <= last { id - first } else { 0 }
                    })
                    .collect();
                let order: Vec<usize> = list
                    .iter()
                    .map(|t| types.iter().position(|x| x == t).unwrap())
                    .collect();
                (order, last - first, ids)
            })
        };
        let (large, go) = (run(true), run(false));
        assert!(go.1 >= 12, "Go's sort gave {} symbol ids", go.1);
        assert_eq!(large.1, go.1, "symbol ids given");
        assert_eq!(large.2, go.2, "the ids of the bare symbols");
        assert_eq!(large.0, go.0, "the order");
    }
}

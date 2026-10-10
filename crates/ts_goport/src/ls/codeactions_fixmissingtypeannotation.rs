use crate::ls::prelude::*;

// Port of Go `ls/codeactions_fixmissingtypeannotation.go`.
//
// PORT (whole file):
// - Go `isolatedDeclarationsFixer` holds pointers to the checker and the
//   change tracker, and the caller keeps using the tracker afterwards. The
//   Rust fixer borrows both (`&'a mut`).
// - Go `fixedNodes map[*ast.Node]bool` is `FxHashSet<Node>`;
//   `symbolsToImport` keeps its order (`Vec<SymbolId>`).
// - Go `autoimport.ImportAdder` (nil allowed) is borrowed as
//   `Option<&mut dyn autoimport::ImportAdder>`; the owner holds
//   `Option<Box<dyn autoimport::ImportAdder>>` (w3 shape).
// - Go `f.changeTracker.NodeFactory` is `change_tracker.node_factory()`.
//   Go passes the factory to some helpers as a parameter; in Rust a
//   factory borrowed from the tracker cannot live across a `&mut self`
//   call, so those helpers read it from the tracker again (same factory).
// - Go reads `*ast.Symbol` and `*checker.Type` fields directly. Here they
//   are read from the checker arenas (`checker.sym(s)`, `checker.ty(t)`).

use crate::flags_macros::go_enum;
use std::sync::LazyLock;

// Go: ls/codeactions_fixmissingtypeannotation.go:21 isolatedDeclarationsFixErrorCodes
static ISOLATED_DECLARATIONS_FIX_ERROR_CODES: LazyLock<Vec<i32>> = LazyLock::new(|| {
    vec![
        diag::Function_must_have_an_explicit_return_type_annotation_with_isolatedDeclarations.code() as i32,
        diag::Method_must_have_an_explicit_return_type_annotation_with_isolatedDeclarations.code() as i32,
        diag::At_least_one_accessor_must_have_an_explicit_type_annotation_with_isolatedDeclarations.code() as i32,
        diag::Variable_must_have_an_explicit_type_annotation_with_isolatedDeclarations.code() as i32,
        diag::Parameter_must_have_an_explicit_type_annotation_with_isolatedDeclarations.code() as i32,
        diag::Property_must_have_an_explicit_type_annotation_with_isolatedDeclarations.code() as i32,
        diag::Expression_type_can_t_be_inferred_with_isolatedDeclarations.code() as i32,
        diag::Binding_elements_with_initializers_can_t_be_exported_directly_with_isolatedDeclarations.code() as i32,
        diag::Computed_property_names_on_class_or_object_literals_cannot_be_inferred_with_isolatedDeclarations.code() as i32,
        diag::Computed_properties_must_be_number_or_string_literals_variables_or_dotted_expressions_with_isolatedDeclarations.code() as i32,
        diag::Enum_member_initializers_must_be_computable_without_references_to_external_symbols_with_isolatedDeclarations.code() as i32,
        diag::Extends_clause_can_t_contain_an_expression_with_isolatedDeclarations.code() as i32,
        diag::Objects_that_contain_shorthand_properties_can_t_be_inferred_with_isolatedDeclarations.code() as i32,
        diag::Objects_that_contain_spread_assignments_can_t_be_inferred_with_isolatedDeclarations.code() as i32,
        diag::Arrays_with_spread_elements_can_t_inferred_with_isolatedDeclarations.code() as i32,
        diag::Default_exports_can_t_be_inferred_with_isolatedDeclarations.code() as i32,
        diag::Only_const_arrays_can_be_inferred_with_isolatedDeclarations.code() as i32,
        diag::Assigning_properties_to_functions_without_declaring_them_is_not_supported_with_isolatedDeclarations_Add_an_explicit_declaration_for_the_properties_assigned_to_this_function.code() as i32,
        diag::Declaration_emit_for_this_parameter_requires_implicitly_adding_undefined_to_its_type_This_is_not_supported_with_isolatedDeclarations.code() as i32,
        diag::Type_containing_private_name_0_can_t_be_used_with_isolatedDeclarations.code() as i32,
        diag::Add_satisfies_and_a_type_assertion_to_this_expression_satisfies_T_as_T_to_make_the_type_explicit.code() as i32,
    ]
});

// Go: ls/codeactions_fixmissingtypeannotation.go:45 fixMissingTypeAnnotationOnExportsFixID
const FIX_MISSING_TYPE_ANNOTATION_ON_EXPORTS_FIX_ID: &str = "fixMissingTypeAnnotationOnExports";

// Go: ls/codeactions_fixmissingtypeannotation.go:48 IsolatedDeclarationsFixProvider
/// IsolatedDeclarationsFixProvider is the CodeFixProvider for isolatedDeclarations-related type annotation fixes.
pub static ISOLATED_DECLARATIONS_FIX_PROVIDER: LazyLock<CodeFixProvider> =
    LazyLock::new(|| CodeFixProvider {
        error_codes: ISOLATED_DECLARATIONS_FIX_ERROR_CODES.clone(),
        get_code_actions: get_isolated_declarations_code_actions,
        fix_ids: vec![FIX_MISSING_TYPE_ANNOTATION_ON_EXPORTS_FIX_ID.to_string()],
        get_all_code_actions: Some(get_all_isolated_declarations_code_actions),
    });

// Go: ls/codeactions_fixmissingtypeannotation.go:56 canHaveTypeAnnotationKinds
// canHaveTypeAnnotationKinds are the node kinds that can have type annotations added.
static CAN_HAVE_TYPE_ANNOTATION_KINDS: LazyLock<FxHashMap<SyntaxKind, bool>> =
    LazyLock::new(|| {
        let mut kinds: FxHashMap<SyntaxKind, bool> = FxHashMap::default();
        kinds.insert(SyntaxKind::GetAccessor, true);
        kinds.insert(SyntaxKind::MethodDeclaration, true);
        kinds.insert(SyntaxKind::PropertyDeclaration, true);
        kinds.insert(SyntaxKind::FunctionDeclaration, true);
        kinds.insert(SyntaxKind::FunctionExpression, true);
        kinds.insert(SyntaxKind::ArrowFunction, true);
        kinds.insert(SyntaxKind::VariableDeclaration, true);
        kinds.insert(SyntaxKind::Parameter, true);
        kinds.insert(SyntaxKind::ExportAssignment, true);
        kinds.insert(SyntaxKind::ClassDeclaration, true);
        kinds.insert(SyntaxKind::ObjectBindingPattern, true);
        kinds.insert(SyntaxKind::ArrayBindingPattern, true);
        kinds
    });

// Go: ls/codeactions_fixmissingtypeannotation.go:72 declarationEmitNodeBuilderFlags
// declarationEmitNodeBuilderFlags are the node builder flags used for declaration emit.
const DECLARATION_EMIT_NODE_BUILDER_FLAGS: NodeBuilderFlags =
    NodeBuilderFlags::MULTILINE_OBJECT_LITERALS
        .union(NodeBuilderFlags::WRITE_CLASS_EXPRESSION_AS_TYPE_LITERAL)
        .union(NodeBuilderFlags::USE_TYPE_OF_FUNCTION)
        .union(NodeBuilderFlags::USE_STRUCTURAL_FALLBACK)
        .union(NodeBuilderFlags::ALLOW_EMPTY_TUPLE)
        .union(NodeBuilderFlags::GENERATE_NAMES_FOR_SHADOWED_TYPE_PARAMS)
        .union(NodeBuilderFlags::NO_TRUNCATION);

// Go: ls/codeactions_fixmissingtypeannotation.go:80 typePrintMode
go_enum!(TypePrintMode, i32 {
    FULL = 0; // typePrintModeFull
    RELATIVE = 1; // typePrintModeRelative: typeof X
    WIDENED = 2; // typePrintModeWidened: widened literal type
});

// Go: ls/codeactions_fixmissingtypeannotation.go:88 getIsolatedDeclarationsCodeActions
fn get_isolated_declarations_code_actions(
    ctx: &Context,
    fix_context: &CodeFixContext<'_>,
) -> Result<Vec<CodeAction>, GoError> {
    let (checker, _done) =
        ls_program::get_type_checker_for_file(fix_context.program, ctx, fix_context.source_file);
    // Go: defer done() (`_done` releases at the end of the function)
    let mut checker_ref = checker.borrow_mut();
    let ch: &mut Checker = &mut checker_ref;

    let mut fixes: Vec<CodeAction> = Vec::new();

    let mut add_fix = |action: Option<CodeAction>| {
        let Some(action) = action else {
            return;
        };
        fixes.push(action);
    };

    // Match TS ordering: Full annotation, Relative annotation, Widened annotation,
    // Full inline, Relative inline, Widened inline, Full extract
    let modes = [
        TypePrintMode::FULL,
        TypePrintMode::RELATIVE,
        TypePrintMode::WIDENED,
    ];

    for mode in modes {
        add_fix(try_code_action(ctx, fix_context, ch, &mut |f| {
            f.type_print_mode = mode;
            f.add_type_annotation(fix_context.span)
        }));
    }

    for mode in modes {
        add_fix(try_code_action(ctx, fix_context, ch, &mut |f| {
            f.type_print_mode = mode;
            f.add_inline_assertion(fix_context.span)
        }));
    }

    // extractAsVariable only in Full mode
    add_fix(try_code_action(ctx, fix_context, ch, &mut |f| {
        f.type_print_mode = TypePrintMode::FULL;
        f.extract_as_variable(fix_context.span)
    }));

    Ok(fixes)
}

// Go: ls/codeactions_fixmissingtypeannotation.go:128 getAllIsolatedDeclarationsCodeActions
fn get_all_isolated_declarations_code_actions(
    ctx: &Context,
    fix_context: &CodeFixContext<'_>,
) -> Result<Option<CombinedCodeActions>, GoError> {
    // ts#64543: the diagnostics are read before the checker is acquired,
    // because `getAllDiagnostics` acquires a checker itself and acquisitions
    // are not reentrant.
    let all_diags = get_all_diagnostics(ctx, fix_context.program, fix_context.source_file);

    let (checker, _done) =
        ls_program::get_type_checker_for_file(fix_context.program, ctx, fix_context.source_file);
    // Go: defer done() (`_done` releases at the end of the function)

    let mut change_tracker = change::new_tracker(
        ctx,
        fix_context.program.options(),
        fix_context.ls.format_options(),
        Rc::clone(&fix_context.ls.converters),
    );

    let mut checker_ref = checker.borrow_mut();
    let mut fixer = IsolatedDeclarationsFixer {
        source_file: fix_context.source_file,
        program: fix_context.program,
        checker: &mut *checker_ref,
        change_tracker: &mut change_tracker,
        import_adder: None,
        locale: locale::from_context(ctx),
        fixed_nodes: FxHashSet::default(),
        type_print_mode: TypePrintMode::FULL,
        symbols_to_import: Vec::new(),
        mutated_target: false,
    };

    for diag in all_diags {
        if is_fixable_diagnostic(&diag, &ISOLATED_DECLARATIONS_FIX_ERROR_CODES) {
            let span = TextRange::new(diag.pos, diag.end);
            fixer.add_type_annotation(span);
        }
    }

    let symbols_to_import = fixer.symbols_to_import.clone();
    for sym in symbols_to_import {
        fixer.add_symbol_to_existing_import(sym);
    }

    let (mut changes, _) = change_tracker.get_changes();
    // PORT: Go indexes the map; a missing file gives a nil slice.
    let file_changes = changes
        .shift_remove(source_file_original_file_name(fix_context.source_file))
        .unwrap_or_default();
    if file_changes.is_empty() {
        return Ok(None);
    }

    Ok(Some(CombinedCodeActions {
        description: crate::diagnostics_loc::message_localize(
            diag::Add_all_missing_type_annotations,
            &locale::from_context(ctx),
            &args![],
        ),
        changes: file_changes,
    }))
}

// Go: ls/codeactions_fixmissingtypeannotation.go:169 tryCodeAction
fn try_code_action(
    ctx: &Context,
    fix_context: &CodeFixContext<'_>,
    ch: &mut Checker,
    fn_: &mut dyn FnMut(&mut IsolatedDeclarationsFixer<'_>) -> String,
) -> Option<CodeAction> {
    let mut change_tracker = change::new_tracker(
        ctx,
        fix_context.program.options(),
        fix_context.ls.format_options(),
        Rc::clone(&fix_context.ls.converters),
    );

    let mut import_adder: Option<Box<dyn autoimport::ImportAdder>> = None;
    // importAdder may be nil if the auto-import registry is not available;
    // type node transformation still works without it, just without adding imports.

    let mut fixer = IsolatedDeclarationsFixer {
        source_file: fix_context.source_file,
        program: fix_context.program,
        checker: ch,
        change_tracker: &mut change_tracker,
        import_adder: import_adder.as_deref_mut(),
        locale: locale::from_context(ctx),
        fixed_nodes: FxHashSet::default(),
        type_print_mode: TypePrintMode::FULL,
        symbols_to_import: Vec::new(),
        mutated_target: false,
    };

    let description = fn_(&mut fixer);
    if description.is_empty() {
        return None;
    }

    // Add any symbols that need to be imported to existing import declarations
    let symbols_to_import = fixer.symbols_to_import.clone();
    for sym in symbols_to_import {
        fixer.add_symbol_to_existing_import(sym);
    }

    let (mut changes, _) = change_tracker.get_changes();
    // PORT: Go indexes the map; a missing file gives a nil slice.
    let mut file_changes = changes
        .shift_remove(source_file_original_file_name(fix_context.source_file))
        .unwrap_or_default();

    // Add import edits if import adder has fixes
    if let Some(import_adder) = import_adder.as_deref_mut()
        && import_adder.has_fixes()
    {
        file_changes.extend(import_adder.edits());
    }

    if file_changes.is_empty() {
        return None;
    }

    Some(CodeAction {
        description,
        changes: file_changes,
        fix_id: FIX_MISSING_TYPE_ANNOTATION_ON_EXPORTS_FIX_ID.to_string(),
        fix_all_description: crate::diagnostics_loc::message_localize(
            diag::Add_all_missing_type_annotations,
            &locale::from_context(ctx),
            &args![],
        ),
    })
}

// Go: ls/codeactions_fixmissingtypeannotation.go:217 isolatedDeclarationsFixer
// isolatedDeclarationsFixer encapsulates the state for fixing isolated declarations errors.
struct IsolatedDeclarationsFixer<'a> {
    source_file: Node,
    program: &'a compiler::NewProgram,
    checker: &'a mut Checker,
    change_tracker: &'a mut change::Tracker,
    import_adder: Option<&'a mut (dyn autoimport::ImportAdder + 'static)>,
    locale: locale::Locale,
    fixed_nodes: FxHashSet<Node>,
    type_print_mode: TypePrintMode,
    symbols_to_import: Vec<SymbolId>,
    mutated_target: bool, // set by inferType/relativeType when the target was mutated (e.g., spread decomposition)
}

impl<'a> IsolatedDeclarationsFixer<'a> {
    // Go: ls/codeactions_fixmissingtypeannotation.go:230 addTypeAnnotation
    fn add_type_annotation(&mut self, span: TextRange) -> String {
        let node_with_diag = astnav::get_token_at_position(self.source_file, span.pos());

        let expando_function = find_expando_function(self.checker, node_with_diag);
        if expando_function.is_some() {
            if is_function_declaration(expando_function) {
                return self.create_namespace_for_expando_properties(expando_function);
            }
            return self.fix_isolated_declaration_error(expando_function);
        }

        let node_missing_type = find_ancestor_with_missing_type(node_with_diag);
        if node_missing_type.is_some() {
            return self.fix_isolated_declaration_error(node_missing_type);
        }
        String::new()
    }

    // Go: ls/codeactions_fixmissingtypeannotation.go:248 createNamespaceForExpandoProperties
    fn create_namespace_for_expando_properties(&mut self, expando_func: Node) -> String {
        let func_decl = expando_func;
        if func_decl.name().is_nil() {
            return String::new();
        }

        let t = self.checker.get_type_at_location(expando_func);
        let elements = self.checker.get_properties_of_type_exported(t);
        if elements.is_empty() {
            return String::new();
        }

        let mut new_properties: Vec<Node> = Vec::new();
        for symbol in elements {
            let symbol_name = self.checker.sym(symbol).name.as_str();
            if !is_identifier_text(symbol_name, LanguageVariant::STANDARD) {
                continue;
            }
            // skip symbols that already have a variable declaration
            let value_declaration = self.checker.sym(symbol).value_declaration;
            if value_declaration.is_some() && is_variable_declaration(value_declaration) {
                continue;
            }

            let sym_type = self.checker.get_type_of_symbol_exported(symbol);
            let type_node = self.type_to_minimized_reference_type(
                sym_type,
                expando_func,
                DECLARATION_EMIT_NODE_BUILDER_FLAGS,
            );
            if type_node.is_nil() {
                continue;
            }

            let factory = self.change_tracker.node_factory();
            let var_decl = factory.new_variable_declaration(
                factory.new_identifier(symbol_name),
                Node::NIL,
                type_node,
                Node::NIL,
            );
            let export_token = factory.new_token(SyntaxKind::ExportKeyword);
            let var_decl_list = factory
                .new_variable_declaration_list(factory.new_node_list(&[var_decl]), NodeFlags::NONE);
            let var_stmt = factory
                .new_variable_statement(factory.new_modifier_list(&[export_token]), var_decl_list);
            new_properties.push(var_stmt);
        }

        if new_properties.is_empty() {
            return String::new();
        }

        let factory = self.change_tracker.node_factory();
        let mut modifiers: Vec<Node> = Vec::new();
        if has_syntactic_modifier(expando_func, ModifierFlags::EXPORT) {
            modifiers.push(factory.new_token(SyntaxKind::ExportKeyword));
        }
        modifiers.push(factory.new_token(SyntaxKind::DeclareKeyword));

        let namespace = factory.new_module_declaration(
            factory.new_modifier_list(&modifiers),
            SyntaxKind::NamespaceKeyword,
            factory.new_identifier(func_decl.name().text()),
            Node::NIL, /*attributes*/
            factory.new_module_block(factory.new_node_list(&new_properties)),
        );
        // Set the flags for namespace
        set_node_flags(
            namespace,
            NodeFlags::AMBIENT | NodeFlags::EXPORT_CONTEXT | NodeFlags::CONTEXT_FLAGS,
        );

        self.change_tracker
            .insert_node_after(self.source_file, expando_func, namespace);
        crate::diagnostics_loc::message_localize(
            diag::Annotate_types_of_properties_expando_function_in_a_namespace,
            &self.locale,
            &args![],
        )
    }
}

// Go: ls/codeactions_fixmissingtypeannotation.go:310 needsParenthesizedExpressionForAssertion
// needsParenthesizedExpressionForAssertion checks if an expression needs parentheses for an assertion.
fn needs_parenthesized_expression_for_assertion(node: Node) -> bool {
    !is_entity_name_expression(node)
        && !is_call_expression(node)
        && !is_object_literal_expression(node)
        && !is_array_literal_expression(node)
}

// Go: ls/codeactions_fixmissingtypeannotation.go:315 createAsExpression
// createAsExpression creates an `expr as Type` expression, parenthesizing if needed.
fn create_as_expression(factory: &NodeFactory, node: Node, type_node: Node) -> Node {
    let mut node = node;
    if needs_parenthesized_expression_for_assertion(node) {
        node = factory.new_parenthesized_expression(node);
    }
    factory.new_as_expression(node, type_node)
}

impl<'a> IsolatedDeclarationsFixer<'a> {
    // Go: ls/codeactions_fixmissingtypeannotation.go:322 addInlineAssertion
    fn add_inline_assertion(&mut self, span: TextRange) -> String {
        let node_with_diag = astnav::get_token_at_position(self.source_file, span.pos());

        // No inline assertions for expando members
        let expando_function = find_expando_function(self.checker, node_with_diag);
        if expando_function.is_some() {
            return String::new();
        }

        let target_node = find_best_fitting_node(node_with_diag, span);
        if target_node.is_nil()
            || is_value_signature_declaration(target_node)
            || is_value_signature_declaration(target_node.parent())
        {
            return String::new();
        }

        let is_expression_target = is_expression(target_node);
        let is_shorthand_property_assignment_target = is_shorthand_property_assignment(target_node);

        // Go's IsDeclaration is broader than TS's isDeclaration (e.g. CallExpression has DeclarationData
        // in Go but is not a declaration kind in TS). Use isNamedDeclarationKind to match TS behavior.
        if !is_shorthand_property_assignment_target && is_named_declaration_kind(target_node) {
            return String::new();
        }
        // No inline assertions on binding patterns
        if find_ancestor(target_node, is_binding_pattern).is_some() {
            return String::new();
        }
        // No inline assertions on enum members
        if find_ancestor(target_node, is_enum_member).is_some() {
            return String::new();
        }
        // No support for typeof in extends clauses
        if is_expression_target
            && (find_ancestor_kind(target_node, SyntaxKind::HeritageClause).is_some()
                || find_ancestor(target_node, is_type_node).is_some())
        {
            return String::new();
        }
        // Can't inline type spread elements
        if is_spread_element(target_node) {
            return String::new();
        }

        let variable_declaration = find_ancestor_kind(target_node, SyntaxKind::VariableDeclaration);
        let mut variable_type = TypeId::NIL;
        if variable_declaration.is_some() {
            variable_type = self.checker.get_type_at_location(variable_declaration);
        }
        // Can't use typeof on unique symbols
        if variable_type.is_some()
            && self
                .checker
                .ty(variable_type)
                .flags()
                .intersects(TypeFlags::UNIQUE_ES_SYMBOL)
        {
            return String::new();
        }

        if !is_expression_target && !is_shorthand_property_assignment_target {
            return String::new();
        }

        let type_node = self.infer_type(target_node, variable_type);
        if type_node.is_nil() || self.mutated_target {
            return String::new();
        }

        if is_shorthand_property_assignment_target {
            // Insert `: expr as Type` after the shorthand property name
            let factory = self.change_tracker.node_factory();
            let cloned_name = factory.deep_clone_node(target_node.name());
            let as_expr = create_as_expression(factory, cloned_name, type_node);
            self.change_tracker.insert_node_at(
                self.source_file,
                target_node.end(),
                as_expr,
                change::NodeOptions {
                    prefix: ": ".to_string(),
                    ..change::NodeOptions::default()
                },
            );
        } else if is_expression_target {
            // Replace expression with `(expression) satisfies Type as Type` or `expression satisfies Type as Type`
            let factory = self.change_tracker.node_factory();
            let mut cloned_target = factory.deep_clone_node(target_node);
            if needs_parenthesized_expression_for_assertion(target_node) {
                cloned_target = factory.new_parenthesized_expression(cloned_target);
            }
            let cloned_type = factory.deep_clone_node(type_node);
            let satisfies_as_expr = factory.new_as_expression(
                factory.new_satisfies_expression(cloned_target, cloned_type),
                type_node,
            );
            self.change_tracker.replace_node(
                self.source_file,
                target_node,
                satisfies_as_expr,
                None,
            );
        } else {
            return String::new();
        }

        let type_text = type_to_string_for_diag(type_node, self.source_file, self.change_tracker);
        crate::diagnostics_loc::message_localize(
            diag::Add_satisfies_and_an_inline_type_assertion_with_0,
            &self.locale,
            &args![type_text],
        )
    }

    // Go: ls/codeactions_fixmissingtypeannotation.go:406 extractAsVariable
    fn extract_as_variable(&mut self, span: TextRange) -> String {
        let node_with_diag = astnav::get_token_at_position(self.source_file, span.pos());
        let target_node = find_best_fitting_node(node_with_diag, span);
        if target_node.is_nil()
            || is_value_signature_declaration(target_node)
            || is_value_signature_declaration(target_node.parent())
        {
            return String::new();
        }

        if !is_expression(target_node) {
            return String::new();
        }

        // Array literals should be marked as const
        if is_array_literal_expression(target_node) {
            let factory = self.change_tracker.node_factory();
            let const_ref =
                factory.new_type_reference_node(factory.new_identifier("const"), NodeList::NIL);
            let cloned = factory.deep_clone_node(target_node);
            let as_expr = create_as_expression(factory, cloned, const_ref);
            self.change_tracker
                .replace_node(self.source_file, target_node, as_expr, None);
            return crate::diagnostics_loc::message_localize(
                diag::Mark_array_literal_as_const,
                &self.locale,
                &args![],
            );
        }

        let parent_property_assignment =
            find_ancestor_kind(target_node, SyntaxKind::PropertyAssignment);
        if parent_property_assignment.is_some() {
            // Identifiers or entity names can already be typeof-ed
            if parent_property_assignment == target_node.parent()
                && is_entity_name_expression(target_node)
            {
                return String::new();
            }

            let temp_name = self
                .change_tracker
                .emit_context
                .factory()
                .new_unique_name_ex(
                    &get_identifier_name_for_node(target_node),
                    AutoGenerateOptions {
                        flags: GeneratedIdentifierFlags::OPTIMISTIC,
                        ..AutoGenerateOptions::default()
                    },
                );

            let mut replacement_target = target_node;
            let mut initialization_node = target_node;

            // Handle spread elements: walk up to the spread's parent and handle const assertions
            if is_spread_element(replacement_target) {
                replacement_target = walk_up_parenthesized_expressions(replacement_target.parent());
                if is_const_assertion(replacement_target.parent()) {
                    replacement_target = replacement_target.parent();
                    initialization_node = replacement_target;
                } else {
                    let factory = self.change_tracker.node_factory();
                    let const_ref = factory
                        .new_type_reference_node(factory.new_identifier("const"), NodeList::NIL);
                    initialization_node = create_as_expression(
                        factory,
                        factory.deep_clone_node(replacement_target),
                        const_ref,
                    );
                }
            }

            if is_entity_name_expression(replacement_target) {
                return String::new();
            }

            let factory = self.change_tracker.node_factory();
            let cloned_init = factory.deep_clone_node(initialization_node);
            let var_decl =
                factory.new_variable_declaration(temp_name, Node::NIL, Node::NIL, cloned_init);
            let var_decl_list = factory.new_variable_declaration_list(
                factory.new_node_list(&[var_decl]),
                NodeFlags::CONST,
            );
            let var_stmt = factory.new_variable_statement(ModifierList::NIL, var_decl_list);

            let statement = find_ancestor(target_node, is_statement);
            if statement.is_nil() {
                return String::new();
            }
            self.change_tracker.insert_node_before(
                self.source_file,
                statement,
                var_stmt,
                false,
                change::LeadingTriviaOption::NONE,
            );

            let factory = self.change_tracker.node_factory();
            let type_query = factory.new_type_query_node(temp_name, NodeList::NIL);
            let as_expr = factory.new_as_expression(temp_name, type_query);
            self.change_tracker
                .replace_node(self.source_file, replacement_target, as_expr, None);

            let id_text = type_to_string_for_diag(temp_name, self.source_file, self.change_tracker);
            return crate::diagnostics_loc::message_localize(
                diag::Extract_to_variable_and_replace_with_0_as_typeof_0,
                &self.locale,
                &args![id_text],
            );
        }

        String::new()
    }
}

// Go: ls/codeactions_fixmissingtypeannotation.go:481 isExpandoPropertyDeclarationForFix
// findExpandoFunction finds the function declaration that has expando properties assigned to it.
// isExpandoPropertyDeclarationForFix matches TS's isExpandoPropertyDeclaration which includes
// PropertyAccessExpression, ElementAccessExpression, and BinaryExpression. The shared
// ast.IsExpandoPropertyDeclaration was narrowed to BinaryExpression only for checker purposes.
fn is_expando_property_declaration_for_fix(node: Node) -> bool {
    node.is_some()
        && (is_property_access_expression(node)
            || is_element_access_expression(node)
            || is_binary_expression(node))
}

// Go: ls/codeactions_fixmissingtypeannotation.go:485 findExpandoFunction
fn find_expando_function(ch: &mut Checker, node: Node) -> Node {
    let expando_declaration = find_ancestor_or_quit(node, |n: Node| -> FindAncestorResult {
        if is_statement(n) {
            return FindAncestorResult::FIND_ANCESTOR_QUIT;
        }
        if is_expando_property_declaration_for_fix(n) {
            return FindAncestorResult::FIND_ANCESTOR_TRUE;
        }
        FindAncestorResult::FIND_ANCESTOR_FALSE
    });

    if expando_declaration.is_nil() || !is_expando_property_declaration_for_fix(expando_declaration)
    {
        return Node::NIL;
    }

    let mut assignment_target = expando_declaration;
    // Some late bound expando members use the whole expression as the declaration.
    if is_binary_expression(assignment_target) {
        assignment_target = assignment_target.left();
        if !is_expando_property_declaration_for_fix(assignment_target) {
            return Node::NIL;
        }
    }

    let expression: Node;
    if is_property_access_expression(assignment_target) {
        expression = assignment_target.expression();
    } else if is_element_access_expression(assignment_target) {
        expression = assignment_target.expression();
    } else {
        return Node::NIL;
    }

    let target_type = ch.get_type_at_location(expression);
    if target_type.is_nil() {
        return Node::NIL;
    }

    let properties = ch.get_properties_of_type_exported(target_type);
    let mut found = false;
    for p in properties {
        let value_declaration = ch.sym(p).value_declaration;
        if value_declaration == expando_declaration
            || value_declaration == expando_declaration.parent()
        {
            found = true;
            break;
        }
    }
    if !found {
        return Node::NIL;
    }

    let symbol = ch.ty(target_type).symbol();
    if symbol.is_nil() || ch.sym(symbol).value_declaration.is_nil() {
        return Node::NIL;
    }

    let fn_ = ch.sym(symbol).value_declaration;
    if (is_function_expression(fn_) || is_arrow_function(fn_))
        && is_variable_declaration(fn_.parent())
    {
        return fn_.parent();
    }
    if is_function_declaration(fn_) {
        return fn_;
    }

    Node::NIL
}

impl<'a> IsolatedDeclarationsFixer<'a> {
    // Go: ls/codeactions_fixmissingtypeannotation.go:551 fixIsolatedDeclarationError
    fn fix_isolated_declaration_error(&mut self, node: Node) -> String {
        // Avoid creating duplicate fixes for the same node
        if self.fixed_nodes.contains(&node) {
            return String::new();
        }
        self.fixed_nodes.insert(node);

        match node.kind() {
            SyntaxKind::Parameter
            | SyntaxKind::PropertyDeclaration
            | SyntaxKind::VariableDeclaration => self.add_type_to_variable_like(node),
            SyntaxKind::ArrowFunction
            | SyntaxKind::FunctionExpression
            | SyntaxKind::FunctionDeclaration
            | SyntaxKind::MethodDeclaration
            | SyntaxKind::GetAccessor => self.add_type_to_signature_declaration(node),
            SyntaxKind::ExportAssignment => self.transform_export_assignment(node),
            SyntaxKind::ClassDeclaration => self.transform_extends_clause_with_expression(node),
            SyntaxKind::ObjectBindingPattern | SyntaxKind::ArrayBindingPattern => {
                self.transform_destructuring_patterns(node)
            }
            _ => String::new(),
        }
    }

    // Go: ls/codeactions_fixmissingtypeannotation.go:575 addTypeToSignatureDeclaration
    fn add_type_to_signature_declaration(&mut self, func_node: Node) -> String {
        if func_node.type_().is_some() {
            return String::new();
        }
        let type_node = self.infer_type(func_node, TypeId::NIL);
        if type_node.is_nil() {
            return String::new();
        }
        self.change_tracker
            .try_insert_type_annotation(self.source_file, func_node, type_node);
        let type_text = type_to_string_for_diag(type_node, self.source_file, self.change_tracker);
        crate::diagnostics_loc::message_localize(
            diag::Add_return_type_0,
            &self.locale,
            &args![type_text],
        )
    }

    // Go: ls/codeactions_fixmissingtypeannotation.go:587 transformExportAssignment
    fn transform_export_assignment(&mut self, default_export: Node) -> String {
        let export_assignment = default_export;
        if export_assignment.is_export_equals() {
            return String::new();
        }

        let expression = export_assignment.expression();
        let type_node = self.infer_type(expression, TypeId::NIL);
        if type_node.is_nil() {
            return String::new();
        }

        let factory = self.change_tracker.node_factory();

        let default_identifier = self
            .change_tracker
            .emit_context
            .factory()
            .new_unique_name("_default");

        // Deep clone the expression so synthesized nodes don't reference original source positions
        let cloned_expression = factory.deep_clone_node(expression);

        let var_decl = factory.new_variable_declaration(
            default_identifier,
            Node::NIL,
            type_node,
            cloned_expression,
        );
        let var_decl_list = factory
            .new_variable_declaration_list(factory.new_node_list(&[var_decl]), NodeFlags::CONST);
        let var_stmt = factory.new_variable_statement(ModifierList::NIL, var_decl_list);

        let new_export = factory.update_export_assignment(
            default_export,
            default_export.modifiers(),
            false,
            Node::NIL,
            default_identifier,
        );

        self.change_tracker.replace_node_with_nodes(
            self.source_file,
            default_export,
            &[var_stmt, new_export],
            None,
        );
        crate::diagnostics_loc::message_localize(
            diag::Extract_default_export_to_variable,
            &self.locale,
            &args![],
        )
    }

    // Go: ls/codeactions_fixmissingtypeannotation.go:616 transformExtendsClauseWithExpression
    fn transform_extends_clause_with_expression(&mut self, class_decl: Node) -> String {
        let cd = class_decl;
        let mut extends_clause = Node::NIL;
        if cd.heritage_clauses().is_some() {
            for clause in cd.heritage_clauses().nodes().iter() {
                if clause.token() == SyntaxKind::ExtendsKeyword {
                    extends_clause = clause;
                    break;
                }
            }
        }
        if extends_clause.is_nil() {
            return String::new();
        }

        let heritage_types = extends_clause.types();
        if heritage_types.is_nil() || heritage_types.nodes().len() == 0 {
            return String::new();
        }
        let heritage_expression = heritage_types.nodes().get(0);
        let expression = heritage_expression.expression();

        let heritage_type_node = self.infer_type(expression, TypeId::NIL);
        if heritage_type_node.is_nil() {
            return String::new();
        }

        let mut base_name = "Anonymous".to_string();
        if cd.name().is_some() {
            base_name = format!("{}Base", cd.name().text());
        }
        let base_class_name = self
            .change_tracker
            .emit_context
            .factory()
            .new_unique_name_ex(
                &base_name,
                AutoGenerateOptions {
                    flags: GeneratedIdentifierFlags::OPTIMISTIC,
                    ..AutoGenerateOptions::default()
                },
            );

        // Create: const <BaseName>: <type> = <expression>;
        let factory = self.change_tracker.node_factory();
        let cloned_expression = factory.deep_clone_node(expression);
        let var_decl = factory.new_variable_declaration(
            base_class_name,
            Node::NIL,
            heritage_type_node,
            cloned_expression,
        );
        let var_decl_list = factory
            .new_variable_declaration_list(factory.new_node_list(&[var_decl]), NodeFlags::CONST);
        let var_stmt = factory.new_variable_statement(ModifierList::NIL, var_decl_list);

        self.change_tracker.insert_node_before(
            self.source_file,
            class_decl,
            var_stmt,
            false,
            change::LeadingTriviaOption::NONE,
        );

        // Replace the heritage expression with the base class name
        let new_heritage = self
            .change_tracker
            .node_factory()
            .new_expression_with_type_arguments(base_class_name, NodeList::NIL);
        self.change_tracker
            .replace_node(self.source_file, heritage_expression, new_heritage, None);

        crate::diagnostics_loc::message_localize(
            diag::Extract_base_class_to_variable,
            &self.locale,
            &args![],
        )
    }

    // Go: ls/codeactions_fixmissingtypeannotation.go:665 transformDestructuringPatterns
    fn transform_destructuring_patterns(&mut self, binding_pattern: Node) -> String {
        let enclosing_variable_declaration = binding_pattern.parent();
        if !is_variable_declaration(enclosing_variable_declaration) {
            return String::new();
        }
        let enclosing_var_stmt = enclosing_variable_declaration.parent().parent();
        if !is_variable_statement(enclosing_var_stmt) {
            return String::new();
        }

        let initializer = enclosing_variable_declaration.initializer();
        if initializer.is_nil() {
            return String::new();
        }

        let mut new_nodes: Vec<Node> = Vec::new();

        let base_expr_node: Node;
        if !is_identifier(initializer) {
            // Create a temporary variable for complex expressions
            let temp_name = self
                .change_tracker
                .emit_context
                .factory()
                .new_unique_name_ex(
                    "dest",
                    AutoGenerateOptions {
                        flags: GeneratedIdentifierFlags::OPTIMISTIC,
                        ..AutoGenerateOptions::default()
                    },
                );
            let factory = self.change_tracker.node_factory();
            let cloned_initializer = factory.deep_clone_node(initializer);
            let var_decl = factory.new_variable_declaration(
                temp_name,
                Node::NIL,
                Node::NIL,
                cloned_initializer,
            );
            let var_decl_list = factory.new_variable_declaration_list(
                factory.new_node_list(&[var_decl]),
                NodeFlags::CONST,
            );
            let var_stmt = factory.new_variable_statement(ModifierList::NIL, var_decl_list);
            new_nodes.push(var_stmt);
            base_expr_node = temp_name;
        } else {
            // Use a new identifier to avoid referencing original source positions
            base_expr_node = self
                .change_tracker
                .node_factory()
                .new_identifier(initializer.text());
        }

        // Extract each binding element as a separate variable with type annotation
        self.extract_binding_elements(
            binding_pattern,
            base_expr_node,
            &mut new_nodes,
            enclosing_var_stmt,
        );

        if new_nodes.is_empty() {
            return String::new();
        }

        // If the enclosing variable statement has multiple declarations, preserve the non-destructuring ones
        let decl_list = enclosing_var_stmt.declaration_list();
        if decl_list.declarations().nodes().len() > 1 {
            let mut remaining_decls: Vec<Node> = Vec::new();
            for d in decl_list.declarations().nodes().iter() {
                if d != enclosing_variable_declaration {
                    remaining_decls.push(d);
                }
            }
            if !remaining_decls.is_empty() {
                let factory = self.change_tracker.node_factory();
                new_nodes.push(factory.update_variable_statement(
                    enclosing_var_stmt,
                    enclosing_var_stmt.modifiers(),
                    factory.update_variable_declaration_list(
                        decl_list,
                        factory.new_node_list(&remaining_decls),
                        decl_list.flags(),
                    ),
                ));
            }
        }

        self.change_tracker.replace_node_with_nodes(
            self.source_file,
            enclosing_var_stmt,
            &new_nodes,
            None,
        );
        crate::diagnostics_loc::message_localize(
            diag::Extract_binding_expressions_to_variable,
            &self.locale,
            &args![],
        )
    }

    // Go: ls/codeactions_fixmissingtypeannotation.go:731 extractBindingElements
    fn extract_binding_elements(
        &mut self,
        binding_pattern: Node,
        base_expr: Node,
        new_nodes: &mut Vec<Node>,
        enclosing_var_stmt: Node,
    ) {
        if is_object_binding_pattern(binding_pattern) {
            for element in binding_pattern.element_list().nodes().iter() {
                if is_omitted_expression(element) {
                    continue;
                }
                let be = element;
                let name = be.name();
                if name.is_nil() {
                    continue;
                }

                // Build property access expression
                let access_expr: Node;
                let property_name = be.property_name();
                if property_name.is_some() && is_computed_property_name(property_name) {
                    // Handle computed property names: create a temp variable for the computed expression
                    let computed_expression = property_name.expression();
                    let identifier_for_computed_property = self
                        .change_tracker
                        .emit_context
                        .factory()
                        .new_generated_name_for_node(computed_expression);
                    let factory = self.change_tracker.node_factory();
                    let comp_var_decl = factory.new_variable_declaration(
                        identifier_for_computed_property,
                        Node::NIL,
                        Node::NIL,
                        computed_expression,
                    );
                    let comp_var_decl_list = factory.new_variable_declaration_list(
                        factory.new_node_list(&[comp_var_decl]),
                        NodeFlags::CONST,
                    );
                    let comp_var_stmt =
                        factory.new_variable_statement(ModifierList::NIL, comp_var_decl_list);
                    new_nodes.push(comp_var_stmt);
                    access_expr = factory.new_element_access_expression(
                        base_expr,
                        Node::NIL,
                        identifier_for_computed_property,
                        NodeFlags::NONE,
                    );
                } else if property_name.is_some() {
                    // Use property name text (handles identifiers, string literals, numeric literals)
                    let prop_text = property_name.text();
                    let factory = self.change_tracker.node_factory();
                    access_expr = factory.new_property_access_expression(
                        base_expr,
                        Node::NIL,
                        factory.new_identifier(prop_text),
                        NodeFlags::NONE,
                    );
                } else if is_identifier(name) {
                    let factory = self.change_tracker.node_factory();
                    access_expr = factory.new_property_access_expression(
                        base_expr,
                        Node::NIL,
                        factory.new_identifier(name.text()),
                        NodeFlags::NONE,
                    );
                } else {
                    continue;
                }

                if is_binding_pattern(name) {
                    self.extract_binding_elements(name, access_expr, new_nodes, enclosing_var_stmt);
                } else {
                    self.emit_binding_element_variable(
                        name,
                        be,
                        access_expr,
                        new_nodes,
                        enclosing_var_stmt,
                    );
                }
            }
        } else if is_array_binding_pattern(binding_pattern) {
            for (i, element) in binding_pattern.element_list().nodes().iter().enumerate() {
                if is_omitted_expression(element) {
                    continue;
                }
                let be = element;
                let name = be.name();
                if name.is_nil() {
                    continue;
                }

                let factory = self.change_tracker.node_factory();
                let access_expr = factory.new_element_access_expression(
                    base_expr,
                    Node::NIL,
                    factory.new_numeric_literal(i.to_string(), TokenFlags::NONE),
                    NodeFlags::NONE,
                );

                if is_binding_pattern(name) {
                    self.extract_binding_elements(name, access_expr, new_nodes, enclosing_var_stmt);
                } else {
                    self.emit_binding_element_variable(
                        name,
                        be,
                        access_expr,
                        new_nodes,
                        enclosing_var_stmt,
                    );
                }
            }
        }
    }

    // Go: ls/codeactions_fixmissingtypeannotation.go:801 emitBindingElementVariable
    // emitBindingElementVariable creates a variable declaration for a single binding element,
    // handling default initializers by creating a ternary `temp === undefined ? default : temp`.
    // PORT: Go also takes `factory`; this reads the tracker's factory (see
    // the file header).
    fn emit_binding_element_variable(
        &mut self,
        name: Node,
        be: Node,
        access_expr: Node,
        new_nodes: &mut Vec<Node>,
        enclosing_var_stmt: Node,
    ) {
        let type_node = self.infer_type(name, TypeId::NIL);
        let mut variable_initializer = access_expr;

        if be.initializer().is_some() {
            // Create a temp variable to hold the accessed value, then use a conditional expression
            // to apply the default: temp === undefined ? defaultValue : temp
            let prop_name = be.property_name();
            let mut temp_base_name = "temp".to_string();
            if prop_name.is_some() && is_identifier(prop_name) {
                temp_base_name = prop_name.text().to_string();
            }
            let temp_name = self
                .change_tracker
                .emit_context
                .factory()
                .new_unique_name_ex(
                    &temp_base_name,
                    AutoGenerateOptions {
                        flags: GeneratedIdentifierFlags::OPTIMISTIC,
                        ..AutoGenerateOptions::default()
                    },
                );
            let factory = self.change_tracker.node_factory();
            let temp_var_decl = factory.new_variable_declaration(
                temp_name,
                Node::NIL,
                Node::NIL,
                variable_initializer,
            );
            let temp_var_decl_list = factory.new_variable_declaration_list(
                factory.new_node_list(&[temp_var_decl]),
                NodeFlags::CONST,
            );
            let temp_var_stmt =
                factory.new_variable_statement(ModifierList::NIL, temp_var_decl_list);
            new_nodes.push(temp_var_stmt);

            variable_initializer = factory.new_conditional_expression(
                factory.new_binary_expression(
                    ModifierList::NIL,
                    temp_name,
                    Node::NIL,
                    factory.new_token(SyntaxKind::EqualsEqualsEqualsToken),
                    factory.new_identifier("undefined"),
                ),
                factory.new_token(SyntaxKind::QuestionToken),
                be.initializer(),
                factory.new_token(SyntaxKind::ColonToken),
                variable_initializer,
            );
        }

        let export_modifier = self.get_export_modifier(enclosing_var_stmt);
        let factory = self.change_tracker.node_factory();
        let var_decl = factory.new_variable_declaration(
            factory.new_identifier(name.text()),
            Node::NIL,
            type_node,
            variable_initializer,
        );
        let var_decl_list = factory
            .new_variable_declaration_list(factory.new_node_list(&[var_decl]), NodeFlags::CONST);
        let var_stmt = factory.new_variable_statement(export_modifier, var_decl_list);
        new_nodes.push(var_stmt);
    }

    // Go: ls/codeactions_fixmissingtypeannotation.go:848 getExportModifier
    fn get_export_modifier(&self, enclosing_var_stmt: Node) -> ModifierList {
        if has_syntactic_modifier(enclosing_var_stmt, ModifierFlags::EXPORT) {
            let factory = self.change_tracker.node_factory();
            let export_token = factory.new_token(SyntaxKind::ExportKeyword);
            return factory.new_modifier_list(&[export_token]);
        }
        ModifierList::NIL
    }

    // Go: ls/codeactions_fixmissingtypeannotation.go:856 inferType
    fn infer_type(&mut self, node: Node, variable_type: TypeId) -> Node {
        self.mutated_target = false;

        // Handle Relative mode first: return typeof X for identifiers
        if self.type_print_mode == TypePrintMode::RELATIVE {
            return self.relative_type(node);
        }

        let mut t = TypeId::NIL;

        if is_value_signature_declaration(node) {
            let signature = self.checker.get_signature_from_declaration_exported(node);
            if signature.is_some() {
                let type_predicate = self
                    .checker
                    .get_type_predicate_of_signature_exported(signature);
                if type_predicate.is_some() {
                    let predicate_type = self.checker.pred(type_predicate).type_();
                    if predicate_type.is_nil() {
                        return Node::NIL;
                    }
                    let mut enclosing_decl = find_ancestor(node, is_declaration);
                    if enclosing_decl.is_nil() {
                        enclosing_decl = self.source_file;
                    }
                    let mut flags = DECLARATION_EMIT_NODE_BUILDER_FLAGS;
                    if self
                        .checker
                        .ty(predicate_type)
                        .flags()
                        .intersects(TypeFlags::UNIQUE_ES_SYMBOL)
                    {
                        flags |= NodeBuilderFlags::ALLOW_UNIQUE_ES_SYMBOL_TYPE;
                    }
                    let result = self.checker.type_predicate_to_type_predicate_node_exported(
                        type_predicate,
                        enclosing_decl,
                        flags,
                        None,
                    );
                    if result.is_some() {
                        return result;
                    }
                    return Node::NIL;
                }
                t = self
                    .checker
                    .get_return_type_of_signature_exported(signature);
            }
        } else {
            t = self.checker.get_type_at_location(node);
        }

        if t.is_nil() {
            return Node::NIL;
        }

        // Handle Widened mode: return widened literal type if different
        if self.type_print_mode == TypePrintMode::WIDENED {
            if variable_type.is_some() {
                t = variable_type;
            }
            let widened_type = self.checker.get_widened_literal_type_exported(t);
            if self.checker.is_type_assignable_to_exported(widened_type, t) {
                return Node::NIL; // widened type is same, no fix needed
            }
            t = widened_type;
        }

        let mut enclosing_decl = find_ancestor(node, is_declaration);
        if enclosing_decl.is_nil() {
            enclosing_decl = self.source_file;
        }

        let flags = DECLARATION_EMIT_NODE_BUILDER_FLAGS | self.get_extra_flags(node, t);

        // For parameters that require adding implicit undefined, add it to the type
        if is_parameter_declaration(node)
            && self
                .checker
                .requires_adding_implicit_undefined_exported(node)
        {
            let undefined_type = self.checker.get_undefined_type();
            t = self
                .checker
                .get_union_type_ex_exported(&[undefined_type, t], UnionReduction::NONE);
        }

        self.type_to_minimized_reference_type(t, enclosing_decl, flags)
    }

    // Go: ls/codeactions_fixmissingtypeannotation.go:926 getExtraFlags
    fn get_extra_flags(&self, node: Node, t: TypeId) -> NodeBuilderFlags {
        if (is_variable_declaration(node)
            || (is_property_declaration(node)
                && has_syntactic_modifier(node, ModifierFlags::STATIC | ModifierFlags::READONLY)))
            && self
                .checker
                .ty(t)
                .flags()
                .intersects(TypeFlags::UNIQUE_ES_SYMBOL)
        {
            return NodeBuilderFlags::ALLOW_UNIQUE_ES_SYMBOL_TYPE;
        }
        NodeBuilderFlags::NONE
    }

    // Go: ls/codeactions_fixmissingtypeannotation.go:936 createTypeOfFromEntityNameExpression
    // createTypeOfFromEntityNameExpression creates a `typeof X` type query node.
    fn create_type_of_from_entity_name_expression(&self, node: Node) -> Node {
        let factory = self.change_tracker.node_factory();
        factory.new_type_query_node(factory.deep_clone_node(node), NodeList::NIL)
    }

    // Go: ls/codeactions_fixmissingtypeannotation.go:944 typeFromArraySpreadElements
    // typeFromArraySpreadElements decomposes an array literal with spread elements into
    // separate variables, returning a tuple type of typeof references.
    // PORT: the Go closures capture the tracker's factory. They capture a
    // clone of the tracker's `Rc<EmitContext>` here, so `self` stays free for
    // `typeFromSpreads` (same factory).
    fn type_from_array_spread_elements(&mut self, node: Node, name: &str) -> Node {
        let is_in_const_context = find_ancestor(node, is_const_assertion).is_some();
        if !is_in_const_context {
            return Node::NIL;
        }
        let mut name = name.to_string();
        if name.is_empty() {
            name = "temp".to_string();
        }
        let emit_context = Rc::clone(&self.change_tracker.emit_context);
        let factory: &NodeFactory = &emit_context.factory().ast;
        self.type_from_spreads(
            node,
            &name,
            is_in_const_context,
            &|n: Node| -> Vec<Node> { n.element_list().nodes().to_vec() },
            &is_spread_element,
            &|expr: Node| -> Node { factory.new_spread_element(expr) },
            &|elements: &[Node]| -> Node {
                factory.new_array_literal_expression(factory.new_node_list(elements), true)
            },
            &|types: &[Node]| -> Node {
                let mut rest_types: Vec<Node> = Vec::with_capacity(types.len());
                for &t in types {
                    rest_types.push(factory.new_rest_type_node(t));
                }
                factory.new_tuple_type_node(factory.new_node_list(&rest_types))
            },
        )
    }

    // Go: ls/codeactions_fixmissingtypeannotation.go:979 typeFromObjectSpreadAssignment
    // typeFromObjectSpreadAssignment decomposes an object literal with spread assignments into
    // separate variables, returning an intersection type of typeof references.
    // PORT: see typeFromArraySpreadElements about the factory.
    fn type_from_object_spread_assignment(&mut self, node: Node, name: &str) -> Node {
        let is_in_const_context = find_ancestor(node, is_const_assertion).is_some();
        let mut name = name.to_string();
        if name.is_empty() {
            name = "temp".to_string();
        }
        let emit_context = Rc::clone(&self.change_tracker.emit_context);
        let factory: &NodeFactory = &emit_context.factory().ast;
        self.type_from_spreads(
            node,
            &name,
            is_in_const_context,
            &|n: Node| -> Vec<Node> {
                if n.property_list().is_some() {
                    return n.property_list().nodes().to_vec();
                }
                Vec::new()
            },
            &is_spread_assignment,
            &|expr: Node| -> Node { factory.new_spread_assignment(expr) },
            &|elements: &[Node]| -> Node {
                factory.new_object_literal_expression(factory.new_node_list(elements), true)
            },
            &|types: &[Node]| -> Node {
                factory.new_intersection_type_node(factory.new_node_list(types))
            },
        )
    }

    // Go: ls/codeactions_fixmissingtypeannotation.go:1010 typeFromSpreads
    // typeFromSpreads is the generic spread decomposition function, ported from TS's typeFromSpreads.
    // It splits a literal with spread elements into separate const variables and returns a composed type.
    // PORT: Go reads `factory` here only to pass it on; the helpers read the
    // tracker's factory themselves (see the file header).
    fn type_from_spreads(
        &mut self,
        node: Node,
        name: &str,
        is_in_const_context: bool,
        get_children: &dyn Fn(Node) -> Vec<Node>,
        is_spread: &dyn Fn(Node) -> bool,
        create_spread: &dyn Fn(Node) -> Node,
        make_node_of_kind: &dyn Fn(&[Node]) -> Node,
        final_type: &dyn Fn(&[Node]) -> Node,
    ) -> Node {
        let mut intersection_types: Vec<Node> = Vec::new();
        let mut new_spreads: Vec<Node> = Vec::new();
        let mut current_variable_properties: Vec<Node> = Vec::new();

        let statement = find_ancestor(node, is_statement);

        let children = get_children(node);
        for prop in children {
            if is_spread(prop) {
                self.finalizes_variable_part(
                    name,
                    is_in_const_context,
                    statement,
                    make_node_of_kind,
                    create_spread,
                    &mut current_variable_properties,
                    &mut intersection_types,
                    &mut new_spreads,
                );
                if is_entity_name_expression(prop.expression()) {
                    intersection_types
                        .push(self.create_type_of_from_entity_name_expression(prop.expression()));
                    new_spreads.push(prop);
                } else {
                    self.make_spread_variable(
                        name,
                        is_in_const_context,
                        statement,
                        create_spread,
                        prop.expression(),
                        &mut intersection_types,
                        &mut new_spreads,
                    );
                }
            } else {
                current_variable_properties.push(prop);
            }
        }

        if new_spreads.is_empty() {
            return Node::NIL;
        }

        self.finalizes_variable_part(
            name,
            is_in_const_context,
            statement,
            make_node_of_kind,
            create_spread,
            &mut current_variable_properties,
            &mut intersection_types,
            &mut new_spreads,
        );

        let replacement = make_node_of_kind(new_spreads.as_slice());
        self.change_tracker
            .replace_node(self.source_file, node, replacement, None);
        self.mutated_target = true;

        final_type(intersection_types.as_slice())
    }

    // Go: ls/codeactions_fixmissingtypeannotation.go:1055 makeSpreadVariable
    // makeSpreadVariable creates a const variable for a spread expression and adds it to the decomposition.
    // PORT: Go also takes `factory`; this reads the tracker's factory.
    fn make_spread_variable(
        &mut self,
        name: &str,
        is_in_const_context: bool,
        statement: Node,
        create_spread: &dyn Fn(Node) -> Node,
        expression: Node,
        intersection_types: &mut Vec<Node>,
        new_spreads: &mut Vec<Node>,
    ) {
        let temp_name = self
            .change_tracker
            .emit_context
            .factory()
            .new_unique_name_ex(
                &format!("{}_Part{}", name, new_spreads.len() + 1),
                AutoGenerateOptions {
                    flags: GeneratedIdentifierFlags::OPTIMISTIC,
                    ..AutoGenerateOptions::default()
                },
            );

        let factory = self.change_tracker.node_factory();
        let initializer: Node;
        if !is_in_const_context {
            initializer = factory.deep_clone_node(expression);
        } else {
            let const_ref =
                factory.new_type_reference_node(factory.new_identifier("const"), NodeList::NIL);
            initializer = factory.new_as_expression(factory.deep_clone_node(expression), const_ref);
        }

        let var_decl =
            factory.new_variable_declaration(temp_name, Node::NIL, Node::NIL, initializer);
        let var_decl_list = factory
            .new_variable_declaration_list(factory.new_node_list(&[var_decl]), NodeFlags::CONST);
        let var_stmt = factory.new_variable_statement(ModifierList::NIL, var_decl_list);

        if statement.is_some() {
            self.change_tracker.insert_node_before(
                self.source_file,
                statement,
                var_stmt,
                false,
                change::LeadingTriviaOption::NONE,
            );
        }

        intersection_types.push(self.create_type_of_from_entity_name_expression(temp_name));
        new_spreads.push(create_spread(temp_name));
    }

    // Go: ls/codeactions_fixmissingtypeannotation.go:1091 finalizesVariablePart
    // finalizesVariablePart finalizes accumulated non-spread properties into a variable.
    // PORT: Go also takes `factory`; `makeSpreadVariable` reads the tracker's factory.
    fn finalizes_variable_part(
        &mut self,
        name: &str,
        is_in_const_context: bool,
        statement: Node,
        make_node_of_kind: &dyn Fn(&[Node]) -> Node,
        create_spread: &dyn Fn(Node) -> Node,
        current_variable_properties: &mut Vec<Node>,
        intersection_types: &mut Vec<Node>,
        new_spreads: &mut Vec<Node>,
    ) {
        if !current_variable_properties.is_empty() {
            let expression = make_node_of_kind(current_variable_properties.as_slice());
            self.make_spread_variable(
                name,
                is_in_const_context,
                statement,
                create_spread,
                expression,
                intersection_types,
                new_spreads,
            );
            *current_variable_properties = Vec::new();
        }
    }
}

// Go: ls/codeactions_fixmissingtypeannotation.go:1109 isConstAssertion
// isConstAssertion checks if a node is an `as const` or `<const>` assertion.
fn is_const_assertion(node: Node) -> bool {
    if is_assertion_expression(node) {
        let type_node = node.type_();
        return is_const_type_reference(type_node);
    }
    false
}

impl<'a> IsolatedDeclarationsFixer<'a> {
    // Go: ls/codeactions_fixmissingtypeannotation.go:1120 relativeType
    // relativeType creates a typeof expression for a node, used in TypePrintMode.Relative.
    // Instead of spelling out the full type, returns `typeof X` for identifiers.
    // For object/array literals with spreads, decomposes into separate variables.
    fn relative_type(&mut self, node: Node) -> Node {
        if is_parameter_declaration(node) {
            return Node::NIL;
        }
        if is_shorthand_property_assignment(node) {
            return self.create_type_of_from_entity_name_expression(node.name());
        }
        if is_entity_name_expression(node) {
            return self.create_type_of_from_entity_name_expression(node);
        }
        if is_const_assertion(node) {
            return self.relative_type(node.expression());
        }
        if is_array_literal_expression(node) {
            let var_decl = find_ancestor_kind(node, SyntaxKind::VariableDeclaration);
            let mut part_name = "";
            if var_decl.is_some() && is_identifier(var_decl.name()) {
                part_name = var_decl.name().text();
            }
            return self.type_from_array_spread_elements(node, part_name);
        }
        if is_object_literal_expression(node) {
            let var_decl = find_ancestor_kind(node, SyntaxKind::VariableDeclaration);
            let mut part_name = "";
            if var_decl.is_some() && is_identifier(var_decl.name()) {
                part_name = var_decl.name().text();
            }
            return self.type_from_object_spread_assignment(node, part_name);
        }
        if is_variable_declaration(node) && node.initializer().is_some() {
            return self.relative_type(node.initializer());
        }
        if is_conditional_expression(node) {
            let cond = node;
            let true_type = self.relative_type(cond.when_true());
            if true_type.is_nil() {
                return Node::NIL;
            }
            let true_mutated = self.mutated_target;
            let false_type = self.relative_type(cond.when_false());
            if false_type.is_nil() {
                return Node::NIL;
            }
            self.mutated_target = true_mutated || self.mutated_target;
            let factory = self.change_tracker.node_factory();
            return factory.new_union_type_node(factory.new_node_list(&[true_type, false_type]));
        }
        Node::NIL
    }

    // Go: ls/codeactions_fixmissingtypeannotation.go:1173 typeToMinimizedReferenceType
    // typeToMinimizedReferenceType converts a type to a type node, then trims trailing
    // type arguments that match their defaults. Ported from TS's
    // services/codefixes/helpers.ts typeToMinimizedReferenceType.
    fn type_to_minimized_reference_type(
        &mut self,
        t: TypeId,
        enclosing_decl: Node,
        flags: NodeBuilderFlags,
    ) -> Node {
        let id_to_symbol: FxHashMap<Node, SymbolId> = FxHashMap::default();
        // !!! When truncation tracking is supported, check if the type was truncated
        // and return factory.NewKeywordTypeNode(ast.KindAnyKeyword) instead of the truncated node.
        // PORT: Go `TypeToTypeNodeEx` fills the shared `idToSymbol` map. This
        // builds the node builder as `type_to_type_node_ex_exported` does and
        // reads the filled map back from `nb.impl_` (PORTING "Programs and
        // checkers").
        let node_builder = self.checker.get_node_builder_ex(Some(id_to_symbol));
        let mut type_node = self.checker.node_builder_type_to_type_node(
            &node_builder,
            t,
            enclosing_decl,
            flags,
            InternalNodeBuilderFlags::WRITE_COMPUTED_PROPS,
            None,
        );
        let id_to_symbol = node_builder.borrow().impl_.borrow().id_to_symbol.clone();
        if type_node.is_nil() {
            return Node::NIL;
        }
        if is_type_reference_node(type_node)
            && self
                .checker
                .ty(t)
                .object_flags()
                .intersects(ObjectFlags::REFERENCE)
        {
            let type_args = self.checker.get_type_arguments_exported(t);
            let node_type_args = type_node.type_arguments();
            if !type_args.is_empty() && node_type_args.len() > 0 {
                let cutoff = end_of_required_type_parameters(self.checker, t);
                if (cutoff as usize) < node_type_args.len() {
                    // Trim trailing default type arguments
                    let factory = self.change_tracker.node_factory();
                    let trimmed_args =
                        factory.new_node_list(&node_type_args.to_vec()[..cutoff as usize]);
                    type_node = factory.update_type_reference_node(
                        type_node,
                        type_node.type_name(),
                        trimmed_args,
                    );
                }
            }
        }
        // Convert import type references (e.g. import("./path").Name) to simple type references
        // and collect symbols that need to be imported
        let (reference_type_node, importable_symbols) =
            autoimport::try_get_auto_importable_reference_from_type_node(
                &self.checker.symbols,
                type_node,
                &id_to_symbol,
            );
        if reference_type_node.is_some() {
            type_node = reference_type_node;
            self.symbols_to_import.extend(importable_symbols);
        }
        type_node
    }
}

// Go: ls/codeactions_fixmissingtypeannotation.go:1210 endOfRequiredTypeParameters
// endOfRequiredTypeParameters finds the number of type arguments that are
// actually required (i.e., differ from their defaults). Ported from TS's
// services/codefixes/helpers.ts endOfRequiredTypeParameters.
fn end_of_required_type_parameters(ch: &mut Checker, t: TypeId) -> i32 {
    let type_args = ch.get_type_arguments_exported(t);
    if type_args.is_empty() {
        return 0;
    }
    let target = ch.ty(t).target();
    // PORT: Go `target.AsInterfaceType()` returns nil for other types; the
    // Option form of the Rust accessor is `data.as_interface_type()`.
    if target.is_nil() || ch.ty(target).data.as_interface_type().is_none() {
        return type_args.len() as i32;
    }
    let (type_params, local_type_params) = {
        let interface_type = ch
            .ty(target)
            .data
            .as_interface_type()
            .expect("interface type");
        (
            interface_type.type_parameters().to_vec(),
            interface_type.local_type_parameters().to_vec(),
        )
    };
    let outer_count = type_params.len() as i32 - local_type_params.len() as i32;
    for cutoff in 0..type_args.len() {
        // Skip cutoff positions where the local type parameter has no default.
        // This matches TS's check for constraint === undefined on localTypeParameters,
        // which in practice skips type parameters without defaults (e.g. Set<T>
        // where T has no default should not have <unknown> elided).
        let local_idx = cutoff as i32 - outer_count;
        if local_idx < 0
            || local_idx >= local_type_params.len() as i32
            || !type_param_has_default(ch, local_type_params[local_idx as usize])
        {
            continue;
        }
        let filled_in = ch.fill_missing_type_arguments_exported(
            &type_args[..cutoff],
            &type_params,
            cutoff as i32,
            false,
        );
        let mut all_match = true;
        for (i, &fill) in filled_in.iter().enumerate() {
            if fill != type_args[i] {
                all_match = false;
                break;
            }
        }
        if all_match {
            return cutoff as i32;
        }
    }
    type_args.len() as i32
}

// Go: ls/codeactions_fixmissingtypeannotation.go:1247 typeParamHasDefault
// typeParamHasDefault checks if a type parameter has a default type declaration.
// PORT: `ch` is added to read the type and symbol arenas.
fn type_param_has_default(ch: &Checker, tp: TypeId) -> bool {
    let sym = ch.ty(tp).symbol();
    if sym.is_nil() {
        return false;
    }
    for &decl in ch.sym(sym).declarations.iter() {
        if is_type_parameter_declaration(decl) && decl.default_type().is_some() {
            return true;
        }
    }
    false
}

impl<'a> IsolatedDeclarationsFixer<'a> {
    // Go: ls/codeactions_fixmissingtypeannotation.go:1260 addTypeToVariableLike
    fn add_type_to_variable_like(&mut self, decl: Node) -> String {
        let type_node = self.infer_type(decl, TypeId::NIL);
        if type_node.is_nil() {
            return String::new();
        }
        if decl.type_().is_some() {
            self.change_tracker
                .replace_node(self.source_file, decl.type_(), type_node, None);
        } else {
            self.change_tracker
                .try_insert_type_annotation(self.source_file, decl, type_node);
            // Parenthesize paren-less arrow function parameters (`x => ...`) so the inserted `: T`
            // produces `(x: T) => ...` instead of the invalid `x: T => ...`. Queued after the type
            // annotation so that the `)` edit at param.End() sorts after the annotation insertion.
            if is_parameter_declaration(decl)
                && decl.parent().is_some()
                && is_arrow_function(decl.parent())
            {
                self.change_tracker
                    .parenthesize_arrow_parameters(self.source_file, decl.parent());
            }
        }
        let type_text = type_to_string_for_diag(type_node, self.source_file, self.change_tracker);
        crate::diagnostics_loc::message_localize(
            diag::Add_annotation_of_type_0,
            &self.locale,
            &args![type_text],
        )
    }
}

// Go: ls/codeactions_fixmissingtypeannotation.go:1283 typeToStringForDiag
// typeToStringForDiag converts a type node to a string for use in diagnostic descriptions.
// It reuses the change tracker's EmitContext so that generated identifier names are resolved
// consistently with the actual code edits, and passes the source file so that the printer's
// name generator can check for conflicts with existing file-level identifiers.
fn type_to_string_for_diag(type_node: Node, source_file: Node, ct: &change::Tracker) -> String {
    let saved_flags = ct.emit_context.emit_flags(type_node);
    ct.emit_context
        .set_emit_flags(type_node, saved_flags | EmitFlags::SINGLE_LINE);
    let mut p = new_printer(
        PrinterOptions {
            new_line: NewLineKind::LF,
            ..PrinterOptions::default()
        },
        PrintHandlers::default(),
        Some(Rc::clone(&ct.emit_context)),
    );
    // PORT: Go takes the writer from a pool and releases it on return; the
    // Rust writer is owned (see `get_single_line_string_writer`).
    let writer = Rc::new(RefCell::new(get_single_line_string_writer()));
    let writer_dyn: Rc<RefCell<dyn EmitTextWriter>> = writer.clone();
    p.write_exported(type_node, source_file, writer_dyn, None);
    ct.emit_context.set_emit_flags(type_node, saved_flags);
    let result = writer.borrow().string();
    if result.len() > 160 {
        // PORT: Go slices the bytes and can split a UTF-8 sequence. The
        // partial sequence becomes U+FFFD here.
        return format!("{}...", String::from_utf8_lossy(&result.as_bytes()[..157]));
    }
    result
}

// Go: ls/codeactions_fixmissingtypeannotation.go:1306 findAncestorWithMissingType
// findAncestorWithMissingType walks up the ancestor chain to find a node that
// can have a type annotation and is missing one.
fn find_ancestor_with_missing_type(node: Node) -> Node {
    find_ancestor(node, |n: Node| -> bool {
        if !CAN_HAVE_TYPE_ANNOTATION_KINDS
            .get(&n.kind())
            .copied()
            .unwrap_or(false)
        {
            return false;
        }
        if is_object_binding_pattern(n) || is_array_binding_pattern(n) {
            return is_variable_declaration(n.parent());
        }
        true
    })
}

// Go: ls/codeactions_fixmissingtypeannotation.go:1319 findBestFittingNode
// findBestFittingNode walks up from the token to find the node that best fits the diagnostic span.
fn find_best_fitting_node(node: Node, span: TextRange) -> Node {
    if node.is_nil() {
        return Node::NIL;
    }
    let mut node = node;
    while node.is_some() && node.end() < span.pos() + span.len() {
        node = node.parent();
    }
    while node.parent().is_some()
        && node.parent().pos() == node.pos()
        && node.parent().end() == node.end()
    {
        node = node.parent();
    }
    if is_identifier(node)
        && has_initializer(node.parent())
        && node.parent().initializer().is_some()
    {
        return node.parent().initializer();
    }
    if is_identifier(node) && is_shorthand_property_assignment(node.parent()) {
        return node.parent();
    }
    node
}

// Go: ls/codeactions_fixmissingtypeannotation.go:1341 isNamedDeclarationKind
// isNamedDeclarationKind matches TS's isDeclarationKind, which is narrower than Go's IsDeclaration.
// Go's IsDeclaration returns true for any node with DeclarationData (including CallExpression),
// while TS's isDeclaration only returns true for specific named declaration kinds.
fn is_named_declaration_kind(node: Node) -> bool {
    matches!(
        node.kind(),
        SyntaxKind::ArrowFunction
            | SyntaxKind::BindingElement
            | SyntaxKind::ClassDeclaration
            | SyntaxKind::ClassExpression
            | SyntaxKind::ClassStaticBlockDeclaration
            | SyntaxKind::Constructor
            | SyntaxKind::EnumDeclaration
            | SyntaxKind::EnumMember
            | SyntaxKind::ExportSpecifier
            | SyntaxKind::FunctionDeclaration
            | SyntaxKind::FunctionExpression
            | SyntaxKind::GetAccessor
            | SyntaxKind::ImportClause
            | SyntaxKind::ImportEqualsDeclaration
            | SyntaxKind::ImportSpecifier
            | SyntaxKind::InterfaceDeclaration
            | SyntaxKind::JsxAttribute
            | SyntaxKind::MethodDeclaration
            | SyntaxKind::MethodSignature
            | SyntaxKind::ModuleDeclaration
            | SyntaxKind::NamespaceExportDeclaration
            | SyntaxKind::NamespaceImport
            | SyntaxKind::NamespaceExport
            | SyntaxKind::Parameter
            | SyntaxKind::PropertyAssignment
            | SyntaxKind::PropertyDeclaration
            | SyntaxKind::PropertySignature
            | SyntaxKind::SetAccessor
            | SyntaxKind::ShorthandPropertyAssignment
            | SyntaxKind::TypeAliasDeclaration
            | SyntaxKind::TypeParameter
            | SyntaxKind::VariableDeclaration
            | SyntaxKind::JsDocTypedefTag
            | SyntaxKind::JsDocCallbackTag
            | SyntaxKind::JsDocPropertyTag
            | SyntaxKind::NamedTupleMember
    )
}

// Go: ls/codeactions_fixmissingtypeannotation.go:1361 isValueSignatureDeclaration
// isValueSignatureDeclaration checks if a node is a function-like declaration that produces a value.
fn is_value_signature_declaration(node: Node) -> bool {
    is_function_expression(node)
        || is_arrow_function(node)
        || is_method_declaration(node)
        || is_accessor(node)
        || is_function_declaration(node)
        || is_constructor_declaration(node)
}

// Go: ls/codeactions_fixmissingtypeannotation.go:1369 getIdentifierNameForNode
// getIdentifierNameForNode derives a meaningful variable name from a node expression.
// For property access expressions like `obj.foo`, returns "foo". Otherwise returns "newLocal".
// Ported from TS's getIdentifierForNode in services/refactors/helpers.ts.
fn get_identifier_name_for_node(node: Node) -> String {
    if is_property_access_expression(node) {
        let name = node.name();
        if is_identifier(name)
            && !is_private_identifier(name)
            && identifier_to_keyword_kind(name) == SyntaxKind::Unknown
        {
            return name.text().to_string();
        }
    }
    "newLocal".to_string()
}

impl<'a> IsolatedDeclarationsFixer<'a> {
    // Go: ls/codeactions_fixmissingtypeannotation.go:1381 addSymbolToExistingImport
    // addSymbolToExistingImport finds the existing import declaration for the symbol's module
    // and adds the symbol name to the named imports.
    fn add_symbol_to_existing_import(&mut self, sym: SymbolId) {
        if sym.is_nil() || self.checker.sym(sym).parent.is_nil() {
            return;
        }

        // Find the module specifier for this symbol
        let module_symbol = self.checker.sym(sym).parent;
        let symbol_name = self.checker.sym(sym).name.as_str();

        // Walk the source file's import declarations to find the one importing from the same module
        for stmt in self.source_file.statements().iter() {
            if !is_import_declaration(stmt) {
                continue;
            }
            let import_decl = stmt;
            if import_decl.import_clause().is_nil() {
                continue;
            }

            // Check if this import is from the same module
            let import_module_symbol = self
                .checker
                .get_symbol_at_location_exported(import_decl.module_specifier());
            if import_module_symbol.is_nil()
                || self
                    .checker
                    .get_merged_symbol_exported(import_module_symbol)
                    != self.checker.get_merged_symbol_exported(module_symbol)
            {
                continue;
            }

            // Found the matching import - add the symbol to named imports
            let import_clause = import_decl.import_clause();
            let named_bindings = import_clause.named_bindings();
            if named_bindings.is_some() && is_named_imports(named_bindings) {
                // Add to existing named imports
                let existing_elements = named_bindings.element_list().nodes().to_vec();
                let factory = self.change_tracker.node_factory();
                let new_specifier = factory.new_import_specifier(
                    false,
                    Node::NIL,
                    factory.new_identifier(symbol_name),
                );
                let mut new_elements = existing_elements;
                new_elements.push(new_specifier);
                let new_named_imports =
                    factory.new_named_imports(factory.new_node_list(&new_elements));
                let new_import_clause = factory.update_import_clause(
                    import_clause,
                    import_clause.phase_modifier(),
                    import_clause.name(),
                    new_named_imports,
                );
                let new_import_decl = factory.update_import_declaration(
                    import_decl,
                    import_decl.modifiers(),
                    new_import_clause,
                    import_decl.module_specifier(),
                    import_decl.attributes(),
                );
                self.change_tracker
                    .replace_node(self.source_file, stmt, new_import_decl, None);
            }
            return;
        }
    }
}

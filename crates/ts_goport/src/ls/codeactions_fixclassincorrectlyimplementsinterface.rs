use crate::ls::prelude::*;

// Port of Go `ls/codeactions_fixclassincorrectlyimplementsinterface.go`.
//
// PORT (whole file):
// - Go `GetTypeCheckerForFile` + `defer done()` is
//   `ls_program::get_type_checker_for_file` with the `Release` guard kept to
//   the end of the function. The checker is borrowed only around the code
//   that uses it. Since ts#64543 `getAllDiagnostics` runs before the
//   acquisition, because acquisitions are not reentrant.
// - Go `autoimport.ImportAdder` (nil allowed) is
//   `Option<Box<dyn autoimport::ImportAdder>>` (w3 shape); helpers borrow it
//   as `Option<&mut dyn autoimport::ImportAdder>`.

use std::sync::LazyLock;

// Go: ls/codeactions_fixclassincorrectlyimplementsinterface.go:19 fixClassIncorrectlyImplementsInterfaceFixID
const FIX_CLASS_INCORRECTLY_IMPLEMENTS_INTERFACE_FIX_ID: &str =
    "fixClassIncorrectlyImplementsInterface";

// Go: ls/codeactions_fixclassincorrectlyimplementsinterface.go:21 fixClassIncorrectlyImplementsInterfaceErrorCodes
static FIX_CLASS_INCORRECTLY_IMPLEMENTS_INTERFACE_ERROR_CODES: LazyLock<Vec<i32>> = LazyLock::new(
    || {
        vec![
            diag::Class_0_incorrectly_implements_interface_1.code() as i32,
            diag::Class_0_incorrectly_implements_class_1_Did_you_mean_to_extend_1_and_inherit_its_members_as_a_subclass.code() as i32,
        ]
    },
);

// Go: ls/codeactions_fixclassincorrectlyimplementsinterface.go:26 FixClassIncorrectlyImplementsInterfaceProvider
pub static FIX_CLASS_INCORRECTLY_IMPLEMENTS_INTERFACE_PROVIDER: LazyLock<CodeFixProvider> =
    LazyLock::new(|| CodeFixProvider {
        error_codes: FIX_CLASS_INCORRECTLY_IMPLEMENTS_INTERFACE_ERROR_CODES.clone(),
        get_code_actions: get_code_actions_to_fix_class_incorrectly_implements_interface,
        fix_ids: vec![FIX_CLASS_INCORRECTLY_IMPLEMENTS_INTERFACE_FIX_ID.to_string()],
        get_all_code_actions: Some(
            get_all_code_actions_to_fix_class_incorrectly_implements_interface,
        ),
    });

// Go: ls/codeactions_fixclassincorrectlyimplementsinterface.go:33 getCodeActionsToFixClassIncorrectlyImplementsInterface
fn get_code_actions_to_fix_class_incorrectly_implements_interface(
    context: &Context,
    fix_context: &CodeFixContext<'_>,
) -> Result<Vec<CodeAction>, GoError> {
    let class_declaration = get_class(fix_context.source_file, fix_context.span);
    if class_declaration.is_nil() {
        return Ok(Vec::new());
    }

    let implements_types = get_implements_heritage_clause_elements(class_declaration);
    let locale = locale::from_context(context);

    let (type_checker, _done) = ls_program::get_type_checker_for_file(
        fix_context.program,
        context,
        fix_context.source_file,
    );
    // Go: defer done() (`_done` releases at the end of the function)

    let mut actions: Vec<CodeAction> = Vec::new();
    for implemented_type_node in implements_types {
        let mut change_tracker = change::new_tracker(
            context,
            fix_context.program.options(),
            fix_context.ls.format_options(),
            Rc::clone(&fix_context.ls.converters),
        );
        let mut import_adder = create_import_adder(context, fix_context)?;

        {
            let mut checker_ref = type_checker.borrow_mut();
            add_changes(
                context,
                fix_context,
                &mut change_tracker,
                import_adder.as_deref_mut(),
                &mut checker_ref,
                class_declaration,
                implemented_type_node,
            );
        }
        let changes = get_changes(
            &mut change_tracker,
            import_adder.as_deref_mut(),
            fix_context.source_file,
        );
        if changes.is_empty() {
            continue;
        }

        actions.push(CodeAction {
            description: crate::diagnostics_loc::message_localize(
                diag::Implement_interface_0,
                &locale,
                &args![get_text_of_node(implemented_type_node)],
            ),
            changes,
            fix_id: FIX_CLASS_INCORRECTLY_IMPLEMENTS_INTERFACE_FIX_ID.to_string(),
            fix_all_description: crate::diagnostics_loc::message_localize(
                diag::Implement_all_unimplemented_interfaces,
                &locale,
                &args![],
            ),
        });
    }
    Ok(actions)
}

// Go: ls/codeactions_fixclassincorrectlyimplementsinterface.go:69 getAllCodeActionsToFixClassIncorrectlyImplementsInterface
fn get_all_code_actions_to_fix_class_incorrectly_implements_interface(
    context: &Context,
    fix_context: &CodeFixContext<'_>,
) -> Result<Option<CombinedCodeActions>, GoError> {
    // ts#64543: the diagnostics are read before the checker is acquired,
    // because `getAllDiagnostics` acquires a checker itself and acquisitions
    // are not reentrant.
    let all_diags = get_all_diagnostics(context, fix_context.program, fix_context.source_file);

    let (type_checker, _done) = ls_program::get_type_checker_for_file(
        fix_context.program,
        context,
        fix_context.source_file,
    );
    // Go: defer done() (`_done` releases at the end of the function)

    let mut change_tracker = change::new_tracker(
        context,
        fix_context.program.options(),
        fix_context.ls.format_options(),
        Rc::clone(&fix_context.ls.converters),
    );
    let mut import_adder = create_import_adder(context, fix_context)?;

    let mut seen_class_declarations: FxHashSet<Node> = FxHashSet::default();

    for diag in all_diags {
        if is_fixable_diagnostic(
            &diag,
            &FIX_CLASS_INCORRECTLY_IMPLEMENTS_INTERFACE_ERROR_CODES,
        ) {
            let class_declaration =
                get_class(fix_context.source_file, TextRange::new(diag.pos, diag.end));
            if class_declaration.is_nil() {
                continue;
            }
            // Go: seenClassDeclarations.AddIfAbsent(classDeclaration)
            if seen_class_declarations.insert(class_declaration) {
                let implements_types = get_implements_heritage_clause_elements(class_declaration);
                for implemented_type_node in implements_types {
                    let mut checker_ref = type_checker.borrow_mut();
                    add_changes(
                        context,
                        fix_context,
                        &mut change_tracker,
                        import_adder.as_deref_mut(),
                        &mut checker_ref,
                        class_declaration,
                        implemented_type_node,
                    );
                }
            }
        }
    }

    let changes = get_changes(
        &mut change_tracker,
        import_adder.as_deref_mut(),
        fix_context.source_file,
    );
    if changes.is_empty() {
        return Ok(None);
    }

    Ok(Some(CombinedCodeActions {
        description: crate::diagnostics_loc::message_localize(
            diag::Implement_all_unimplemented_interfaces,
            &locale::from_context(context),
            &args![],
        ),
        changes,
    }))
}

// Go: ls/codeactions_fixclassincorrectlyimplementsinterface.go:107 addChanges
// PORT: the fixer borrows `change_tracker` and `type_checker` for its whole
// life, so the Go uses of `changeTracker` and `typeChecker` next to it go
// through the fixer's fields.
fn add_changes(
    context: &Context,
    fix_context: &CodeFixContext<'_>,
    change_tracker: &mut change::Tracker,
    import_adder: Option<&mut (dyn autoimport::ImportAdder + 'static)>,
    type_checker: &mut Checker,
    class_declaration: Node,
    implemented_type_node: Node,
) {
    let mut missing_member_fixer = new_missing_member_fixer(
        change_tracker,
        fix_context.program,
        type_checker,
        fix_context.ls.user_preferences(),
        import_adder,
        locale::from_context(context),
    );
    let constructor = get_constructor(class_declaration);
    let implemented_type = missing_member_fixer
        .type_checker
        .get_type_at_location(implemented_type_node);
    let class_type = missing_member_fixer
        .type_checker
        .get_type_at_location(class_declaration);

    if missing_member_fixer
        .type_checker
        .get_number_index_type(class_type)
        .is_nil()
    {
        let number_type = missing_member_fixer.type_checker.get_number_type();
        let member = missing_member_fixer.create_index_signature_declaration_from_type(
            class_declaration,
            implemented_type,
            number_type,
        );
        if member.is_some() {
            insert_interface_member_node(
                missing_member_fixer.change_tracker,
                fix_context.source_file,
                class_declaration,
                constructor,
                member,
            );
        }
    }

    if missing_member_fixer
        .type_checker
        .get_string_index_type(class_type)
        .is_nil()
    {
        let string_type = missing_member_fixer.type_checker.get_string_type();
        let member = missing_member_fixer.create_index_signature_declaration_from_type(
            class_declaration,
            implemented_type,
            string_type,
        );
        if member.is_some() {
            insert_interface_member_node(
                missing_member_fixer.change_tracker,
                fix_context.source_file,
                class_declaration,
                constructor,
                member,
            );
        }
    }

    let missing_members = get_missing_members(
        missing_member_fixer.type_checker,
        class_declaration,
        &[implemented_type],
    );
    for member in missing_members {
        let member_nodes = missing_member_fixer.create_member_from_symbol(
            member,
            class_declaration,
            fix_context.source_file,
            Node::NIL, /*body*/
            PreserveOptionalFlags::ALL,
            false, /*abstract*/
        );
        for member_node in member_nodes {
            insert_interface_member_node(
                missing_member_fixer.change_tracker,
                fix_context.source_file,
                class_declaration,
                constructor,
                member_node,
            );
        }
    }
}

// Go: ls/codeactions_fixclassincorrectlyimplementsinterface.go:136 getChanges
fn get_changes(
    change_tracker: &mut change::Tracker,
    import_adder: Option<&mut (dyn autoimport::ImportAdder + 'static)>,
    source_file: Node,
) -> Vec<lsproto::TextEdit> {
    let (mut changes, unmappable) = change_tracker.get_changes();
    if !unmappable.is_empty() {
        return Vec::new();
    }
    // PORT: Go indexes the map; a missing file gives a nil slice.
    let mut file_changes = changes
        .shift_remove(source_file_original_file_name(source_file))
        .unwrap_or_default();
    if let Some(import_adder) = import_adder
        && import_adder.has_fixes()
    {
        file_changes.extend(import_adder.edits());
    }
    file_changes
}

// Go: ls/codeactions_fixclassincorrectlyimplementsinterface.go:148 insertInterfaceMemberNode
fn insert_interface_member_node(
    change_tracker: &mut change::Tracker,
    source_file: Node,
    class_declaration: Node,
    constructor: Node,
    member: Node,
) {
    if constructor.is_nil() {
        change_tracker.insert_member_at_start(source_file, class_declaration, member);
    } else {
        change_tracker.insert_node_after(source_file, constructor, member);
    }
}

// Go: ls/codeactions_fixclassincorrectlyimplementsinterface.go:156 getClass
fn get_class(source_file: Node, span: TextRange) -> Node {
    let token = astnav::get_token_at_position(source_file, span.pos());
    if token.is_nil() {
        return Node::NIL;
    }
    get_containing_class(token)
}

// Go: ls/codeactions_fixclassincorrectlyimplementsinterface.go:164 getConstructor
fn get_constructor(class_declaration: Node) -> Node {
    if class_declaration.is_nil() || class_declaration.member_list().is_nil() {
        return Node::NIL;
    }
    for member in class_declaration.member_list().nodes().iter() {
        if member.is_some() && is_constructor_declaration(member) {
            return member;
        }
    }
    Node::NIL
}

// Go: ls/codeactions_fixclassincorrectlyimplementsinterface.go:176 getMissingMembers
fn get_missing_members(
    type_checker: &mut Checker,
    class_declaration: Node,
    implemented_types: &[TypeId],
) -> Vec<SymbolId> {
    let inherited_members = get_inherited_members(type_checker, class_declaration);
    let mut seen_members: FxHashMap<String, SymbolId> = FxHashMap::default();

    let mut class_members = SymbolTable::NIL;
    if class_declaration.symbol().is_some() {
        class_members = type_checker.sym(class_declaration.symbol()).members;
    }

    let mut missing_members: Vec<SymbolId> = Vec::new();
    for &implemented_type in implemented_types {
        for symbol in type_checker.get_properties_of_type_exported(implemented_type) {
            if symbol.is_nil() {
                continue;
            }
            let name = type_checker.sym(symbol).name.as_str();
            if class_members.is_some() && type_checker.symbols.get(class_members, name).is_some() {
                continue;
            }
            if inherited_members.get(name).is_some_and(|s| s.is_some())
                || seen_members.get(name).is_some_and(|s| s.is_some())
            {
                continue;
            }
            let flags = type_checker.get_declaration_modifier_flags_from_symbol_exported(symbol);
            if !flags.intersects(ModifierFlags::PRIVATE) {
                seen_members.insert(name.to_string(), symbol);
                missing_members.push(symbol);
            }
        }
    }
    missing_members
}

// Go: ls/codeactions_fixclassincorrectlyimplementsinterface.go:207 getInheritedMembers
// PORT: Go returns a new `ast.SymbolTable` that only `getMissingMembers`
// reads by name. A local `FxHashMap<String, SymbolId>` stands for it, so no
// table is added to the checker's symbol arena.
fn get_inherited_members(
    type_checker: &mut Checker,
    class_declaration: Node,
) -> FxHashMap<String, SymbolId> {
    let type_node = get_class_extends_heritage_element(class_declaration);
    if type_node.is_nil() {
        return FxHashMap::default();
    }

    let base_type = type_checker.get_type_at_location(type_node);
    if base_type.is_nil() {
        return FxHashMap::default();
    }

    let mut inherited_members: FxHashMap<String, SymbolId> = FxHashMap::default();
    for symbol in type_checker.get_properties_of_type_exported(base_type) {
        if symbol.is_nil() {
            continue;
        }
        let flags = type_checker.get_declaration_modifier_flags_from_symbol_exported(symbol);
        if !flags.intersects(ModifierFlags::PRIVATE) {
            inherited_members.insert(type_checker.sym(symbol).name.as_str().to_string(), symbol);
        }
    }
    inherited_members
}

// Go: ls/codeactions_fixclassincorrectlyimplementsinterface.go:231 createImportAdder
// PORT: Go also takes `typeChecker` and passes it to `NewImportAdder`. The
// pinned w3 `new_import_adder` has no checker parameter, so this drops it.
fn create_import_adder(
    context: &Context,
    fix_context: &CodeFixContext<'_>,
) -> Result<Option<Box<dyn autoimport::ImportAdder>>, GoError> {
    let view = fix_context
        .ls
        .get_prepared_auto_import_view(fix_context.source_file)?;
    let Some(view) = view else {
        return Ok(None);
    };
    Ok(Some(autoimport::new_import_adder(
        context,
        fix_context.program,
        fix_context.source_file,
        view,
        fix_context.ls.format_options(),
        Rc::clone(&fix_context.ls.converters),
        fix_context.ls.user_preferences(),
    )))
}

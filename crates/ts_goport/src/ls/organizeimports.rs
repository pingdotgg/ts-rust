use crate::ls::prelude::*;

// Port of Go `ls/organizeimports.go`.
//
// PORT (whole file):
// - Go `[]*ast.Statement` is `Vec<Node>` (param `&[Node]`); `[][]*ast.Statement`
//   is `Vec<Vec<Node>>`.
// - Go `func(a, b string) int` comparers are `lsutil::StringComparer`
//   (`Option` where Go can hold nil); `func(s1, s2 *ast.Node) int` is
//   `lsutil::NodeComparer`.
// - Go `ast.NewNodeFactory(ast.NodeFactoryHooks{})` is `NodeFactory::new()`
//   (no hooks).
// - Go `changeTracker.SetEmitFlags` / `AddEmitFlags` (promoted from the
//   embedded `*printer.EmitContext`) are `change_tracker.emit_context.x(..)`.
// - Go `slices.SortFunc` is `gostd::slices::sort_func` (Go pdqsort);
//   `slices.SortStableFunc` is `gostd::slices::sort_stable_func`.

use crate::astdata::NodeData;
use crate::frontend::scanner::{Scanner, new_scanner};
use crate::frontend::stringutil_ls;

impl LanguageService {
    // Go: ls/organizeimports.go:24 OrganizeImports
    // OrganizeImports organizes imports by:
    //  1. Removing unused imports
    //  2. Coalescing imports from the same module
    //  3. Sorting imports
    // PORT: Go returns `map[string][]*lsproto.TextEdit`; the IndexMap keeps
    // the tracker's order (Go map order is random).
    pub fn organize_imports(
        &self,
        ctx: &Context,
        source_file: Node,
        program: &compiler::NewProgram,
        kind: &lsproto::CodeActionKind,
    ) -> IndexMap<String, Vec<lsproto::TextEdit>> {
        let mut change_tracker = change::new_tracker(
            ctx,
            program.options(),
            self.format_options(),
            self.converters.clone(),
        );
        let should_sort = *kind == lsproto::CodeActionKind::SOURCE_SORT_IMPORTS_TS
            || *kind == lsproto::CodeActionKind::SOURCE_ORGANIZE_IMPORTS_TS;
        let should_combine = should_sort;
        let should_remove = *kind == lsproto::CodeActionKind::SOURCE_REMOVE_UNUSED_IMPORTS_TS
            || *kind == lsproto::CodeActionKind::SOURCE_ORGANIZE_IMPORTS_TS;
        let top_level_import_decls =
            lsutil::filter_import_declarations(&source_file.statements().to_vec());
        let top_level_import_group_decls =
            group_by_newline_contiguous(source_file, &top_level_import_decls);

        let preferences = self.user_preferences();
        let (comparers_to_test, type_orders_to_test) = lsutil::get_detection_lists(&preferences);
        let default_comparer = comparers_to_test[0].clone();
        let sort = lsutil::resolve_organize_imports_sort(&preferences);

        let mut module_specifier_comparer: Option<lsutil::StringComparer> = None;
        let mut named_import_comparer: Option<lsutil::StringComparer> = None;
        if sort != lsutil::OrganizeImportsSort::AUTO {
            module_specifier_comparer = Some(default_comparer.clone());
            named_import_comparer = Some(default_comparer.clone());
        }
        let mut type_order = preferences.organize_imports_type_order;

        if sort == lsutil::OrganizeImportsSort::AUTO {
            let (result, _) = lsutil::detect_module_specifier_case_by_sort(
                &top_level_import_group_decls,
                &comparers_to_test,
            );
            module_specifier_comparer = result;
        }

        if type_order == lsutil::OrganizeImportsTypeOrder::AUTO
            || sort == lsutil::OrganizeImportsSort::AUTO
        {
            let (named_import_comparer2, type_order2, found) =
                lsutil::detect_named_import_organization_by_sort_exported(
                    &top_level_import_decls,
                    &comparers_to_test,
                    &type_orders_to_test,
                );
            if found {
                if named_import_comparer.is_none() || sort == lsutil::OrganizeImportsSort::AUTO {
                    named_import_comparer = named_import_comparer2;
                }
                if type_order == lsutil::OrganizeImportsTypeOrder::AUTO {
                    type_order = type_order2;
                }
            }
        }

        let comparer = OrganizeImportsComparerSettings {
            module_specifier_comparer,
            named_import_comparer,
            type_order,
        };

        for import_group_decl in &top_level_import_group_decls {
            organize_imports_worker(
                import_group_decl,
                &comparer,
                should_sort,
                should_combine,
                should_remove,
                source_file,
                program,
                &mut change_tracker,
                ctx,
            );
        }

        if *kind != lsproto::CodeActionKind::SOURCE_REMOVE_UNUSED_IMPORTS_TS {
            let top_level_export_group_decls = get_top_level_export_groups(source_file);
            for export_group_decl in &top_level_export_group_decls {
                organize_exports_worker(
                    export_group_decl,
                    &comparer,
                    source_file,
                    &mut change_tracker,
                );
            }
        }

        for stmt in source_file.statements().iter() {
            if !is_ambient_module(stmt) {
                continue;
            }

            let ambient_module = stmt;
            if ambient_module.body().is_nil() {
                continue;
            }

            let module_body = ambient_module.body();

            let ambient_module_import_decls =
                lsutil::filter_import_declarations(&module_body.statements().to_vec());
            let ambient_module_import_group_decls =
                group_by_newline_contiguous(source_file, &ambient_module_import_decls);

            for import_group_decl in &ambient_module_import_group_decls {
                organize_imports_worker(
                    import_group_decl,
                    &comparer,
                    should_sort,
                    should_combine,
                    should_remove,
                    source_file,
                    program,
                    &mut change_tracker,
                    ctx,
                );
            }

            if *kind != lsproto::CodeActionKind::SOURCE_REMOVE_UNUSED_IMPORTS_TS {
                let mut ambient_module_export_decls: Vec<Node> = Vec::new();
                for s in module_body.statements().iter() {
                    if s.kind() == SyntaxKind::ExportDeclaration {
                        ambient_module_export_decls.push(s);
                    }
                }
                organize_exports_worker(
                    &ambient_module_export_decls,
                    &comparer,
                    source_file,
                    &mut change_tracker,
                );
            }
        }

        // Unmappable files are dropped by GetChanges, so a content-mapped file whose imports cannot be
        // faithfully rewritten yields no edits rather than a corrupting one.
        let (changes, _) = change_tracker.get_changes();
        changes
    }
}

// Go: ls/organizeimports.go:120 organizeImportsComparerSettings
// PORT: a nil Go comparer func is `None`.
#[derive(Clone)]
struct OrganizeImportsComparerSettings {
    module_specifier_comparer: Option<lsutil::StringComparer>,
    named_import_comparer: Option<lsutil::StringComparer>,
    type_order: lsutil::OrganizeImportsTypeOrder,
}

/// Go `comparer(a, b)` on a `func(a, b string) int` value.
// PORT: Go calls the func value where it is used; a nil one panics then.
fn call_string_comparer(comparer: &Option<lsutil::StringComparer>, a: &str, b: &str) -> i32 {
    (comparer
        .as_ref()
        .unwrap_or_else(|| crate::core::go_nil_dereference()))(a, b)
}

// Go: ls/organizeimports.go:126 organizeImportsWorker
fn organize_imports_worker(
    old_import_decls: &[Node],
    comparer: &OrganizeImportsComparerSettings,
    should_sort: bool,
    should_combine: bool,
    should_remove: bool,
    source_file: Node,
    program: &compiler::NewProgram,
    change_tracker: &mut change::Tracker,
    ctx: &Context,
) {
    if old_import_decls.is_empty() {
        return;
    }

    // Header comment preservation is handled via LeadingTriviaOptionExclude in the change tracker below

    let module_specifier_comparer = |a: &str, b: &str| -> i32 {
        call_string_comparer(&comparer.module_specifier_comparer, a, b)
    };

    let mut processed_imports: Vec<Node> = old_import_decls.to_vec();
    // PORT: Go `defer done()` releases the checker at function exit; the
    // guard is kept to the end of the function. The borrow ends after use.
    let mut _done: Option<ls_program::Release> = None;
    if should_remove {
        let (type_checker, done) = ls_program::get_type_checker_for_file(program, ctx, source_file);
        _done = Some(done);
        let type_checker = &mut *type_checker.borrow_mut();
        processed_imports = remove_unused_imports(
            &processed_imports,
            source_file,
            type_checker,
            program,
            change_tracker,
        );
    }

    let mut new_import_decls: Vec<Node> = Vec::new();
    if should_combine {
        let mut grouped = group_by_module_specifier(&processed_imports);
        if should_sort {
            crate::gostd::slices::sort_func(&mut grouped, |a: &Vec<Node>, b: &Vec<Node>| -> i32 {
                if a.is_empty() || b.is_empty() {
                    return 0;
                }
                lsutil::compare_module_specifiers(
                    a[0].module_specifier(),
                    b[0].module_specifier(),
                    &module_specifier_comparer,
                )
            });
        }

        let specifier_comparer = lsutil::get_named_import_specifier_comparer(
            &lsutil::UserPreferences {
                organize_imports_type_order: comparer.type_order,
                ..Default::default()
            },
            comparer.named_import_comparer.clone(),
        );

        for import_group in &grouped {
            let mut coalesced = coalesce_imports_worker(
                import_group,
                &module_specifier_comparer,
                &*specifier_comparer,
                source_file,
                change_tracker,
            );
            if should_sort {
                // ts#63915: a stable sort (Go slices.SortStableFunc).
                crate::gostd::slices::sort_stable_func(
                    &mut coalesced,
                    |a: &Node, b: &Node| -> i32 {
                        lsutil::compare_imports_or_require_statements(
                            *a,
                            *b,
                            &module_specifier_comparer,
                        )
                    },
                );
            }
            new_import_decls.extend(coalesced);
        }
    } else {
        new_import_decls = processed_imports;
    }

    if should_sort && !should_combine {
        crate::gostd::slices::sort_func(&mut new_import_decls, |a: &Node, b: &Node| -> i32 {
            lsutil::compare_imports_or_require_statements(*a, *b, &module_specifier_comparer)
        });
    }

    if new_import_decls.is_empty() {
        change_tracker.delete_node_range(
            source_file,
            old_import_decls[0],
            old_import_decls[old_import_decls.len() - 1],
            change::LeadingTriviaOption::EXCLUDE, // Preserve header comment
            change::TrailingTriviaOption::INCLUDE,
        );
    } else {
        for &imp in &new_import_decls {
            change_tracker
                .emit_context
                .set_emit_flags(imp, EmitFlags::NO_LEADING_COMMENTS);
        }

        let options = change::NodeOptions {
            leading_trivia_option: change::LeadingTriviaOption::EXCLUDE, // Preserve header comment
            trailing_trivia_option: change::TrailingTriviaOption::INCLUDE,
            suffix: "\n".to_string(),
            ..Default::default()
        };

        let new_nodes: Vec<Node> = new_import_decls.clone();
        change_tracker.replace_node_with_nodes(
            source_file,
            old_import_decls[0],
            &new_nodes,
            Some(&options),
        );

        if old_import_decls.len() > 1 {
            for i in 1..old_import_decls.len() {
                change_tracker.delete(source_file, old_import_decls[i]);
            }
        }
    }
}

// Go: ls/organizeimports.go:220 groupByModuleSpecifier
fn group_by_module_specifier(imports: &[Node]) -> Vec<Vec<Node>> {
    let mut groups: FxHashMap<String, Vec<Node>> = FxHashMap::default();
    let mut order: Vec<String> = Vec::new();

    for &imp in imports {
        let specifier = lsutil::get_external_module_name(imp.module_specifier());
        if !groups.contains_key(&specifier) {
            order.push(specifier.clone());
        }
        groups.entry(specifier).or_default().push(imp);
    }

    let mut result: Vec<Vec<Node>> = Vec::with_capacity(order.len());
    for key in &order {
        result.push(groups[key].clone());
    }
    result
}

// Go: ls/organizeimports.go:239 removeUnusedImports
fn remove_unused_imports(
    old_imports: &[Node],
    source_file: Node,
    type_checker: &mut Checker,
    program: &compiler::NewProgram,
    change_tracker: &mut change::Tracker,
) -> Vec<Node> {
    let compiler_options = program.options();
    let jsx_elements_present = source_file
        .subtree_facts()
        .intersects(SubtreeFacts::SUBTREE_CONTAINS_JSX);
    let jsx_mode_needs_explicit_import =
        compiler_options.jsx == JsxEmit::REACT || compiler_options.jsx == JsxEmit::REACT_NATIVE;

    let factory = NodeFactory::new();
    let mut used_imports: Vec<Node> = Vec::with_capacity(old_imports.len());

    for &import_decl in old_imports {
        let import_clause = import_decl.import_clause();
        if import_clause.is_nil() {
            used_imports.push(import_decl);
            continue;
        }

        let clause = import_clause;
        let mut name = clause.name();
        let mut named_bindings = clause.named_bindings();

        if name.is_some()
            && !type_checker.is_declaration_used(
                source_file,
                name,
                jsx_elements_present,
                jsx_mode_needs_explicit_import,
            )
        {
            name = Node::NIL;
        }

        if named_bindings.is_some() {
            match named_bindings.kind() {
                SyntaxKind::NamespaceImport => {
                    let ns_import = named_bindings;
                    if !type_checker.is_declaration_used(
                        source_file,
                        ns_import.name(),
                        jsx_elements_present,
                        jsx_mode_needs_explicit_import,
                    ) {
                        named_bindings = Node::NIL;
                    }
                }
                SyntaxKind::NamedImports => {
                    let named_imports = named_bindings;
                    let original_bindings = named_bindings;
                    let elements = named_imports.elements().to_vec();
                    let new_elements = filter_used_import_specifiers(
                        &elements,
                        type_checker,
                        source_file,
                        jsx_elements_present,
                        jsx_mode_needs_explicit_import,
                    );
                    if new_elements.is_empty() {
                        named_bindings = Node::NIL;
                    } else if new_elements.len() < elements.len() {
                        let new_list = factory.new_node_list(&new_elements);
                        let updated_named_imports =
                            factory.update_named_imports(named_imports, new_list);
                        named_bindings = updated_named_imports;
                    }
                    if named_bindings.is_some()
                        && !node_is_synthesized(original_bindings)
                        && !range_is_on_single_line(original_bindings.loc(), source_file)
                    {
                        change_tracker
                            .emit_context
                            .set_emit_flags(named_bindings, EmitFlags::MULTI_LINE);
                    }
                }
                _ => {}
            }
        }

        if name.is_some() || named_bindings.is_some() {
            let import_decl_node = import_decl;
            let new_clause =
                factory.update_import_clause(clause, clause.phase_modifier(), name, named_bindings);
            let new_import_decl = factory.update_import_declaration(
                import_decl_node,
                import_decl_node.modifiers(),
                new_clause,
                import_decl_node.module_specifier(),
                import_decl_node.attributes(),
            );
            used_imports.push(new_import_decl);
        } else {
            let module_specifier = import_decl.module_specifier();
            if has_module_declaration_matching_specifier(source_file, module_specifier) {
                if source_file_info(source_file).is_declaration_file {
                    let import_decl_node = import_decl;
                    let new_import_decl = factory.update_import_declaration(
                        import_decl_node,
                        import_decl_node.modifiers(),
                        Node::NIL, // no import clause
                        import_decl_node.module_specifier(),
                        import_decl_node.attributes(),
                    );
                    used_imports.push(new_import_decl);
                } else {
                    used_imports.push(import_decl);
                }
            }
        }
    }

    used_imports
}

// Go: ls/organizeimports.go:320 filterUsedImportSpecifiers
fn filter_used_import_specifiers(
    elements: &[Node],
    type_checker: &mut Checker,
    source_file: Node,
    jsx_elements_present: bool,
    jsx_mode_needs_explicit_import: bool,
) -> Vec<Node> {
    let mut result: Vec<Node> = Vec::new();
    for &elem in elements {
        let spec = elem;
        if type_checker.is_declaration_used(
            source_file,
            spec.name(),
            jsx_elements_present,
            jsx_mode_needs_explicit_import,
        ) {
            result.push(elem);
        }
    }
    result
}

// Go: ls/organizeimports.go:337 hasModuleDeclarationMatchingSpecifier
fn has_module_declaration_matching_specifier(source_file: Node, module_specifier: Node) -> bool {
    if module_specifier.is_nil() || !is_string_literal(module_specifier) {
        return false;
    }
    let module_specifier_text = module_specifier.text();

    for &module_name in &source_file_info(source_file).module_augmentations {
        if is_string_literal(module_name) && module_name.text() == module_specifier_text {
            return true;
        }
    }

    false
}

// Go: ls/organizeimports.go:353 getImportAttributesKey
// getImportAttributesKey returns a key for grouping imports by their attributes.
fn get_import_attributes_key(attributes: Node) -> String {
    if attributes.is_nil() {
        return String::new();
    }

    let import_attrs = attributes;
    let mut key = String::new();
    // PORT: Go `ast.Kind.String()` (the generated stringer) is "Kind" plus
    // the kind name; `SyntaxKind::as_str` gives the name without the prefix.
    key.push_str(&format!("Kind{}", import_attrs.token().as_str()));
    key.push(' ');

    let mut attr_nodes: Vec<Node> = import_attrs.attribute_list().nodes().to_vec();
    crate::gostd::slices::sort_func(&mut attr_nodes, |a: &Node, b: &Node| -> i32 {
        let a_name = a.name().text();
        let b_name = b.name().text();
        stringutil_ls::compare_strings_case_sensitive(a_name, b_name)
    });

    for &attr_node in &attr_nodes {
        let attr = attr_node;
        key.push_str(attr.name().text());
        key.push(':');
        if is_string_literal_like(attr.value()) {
            key.push('"');
            key.push_str(attr.value().text());
            key.push('"');
        } else {
            key.push_str(go_node_text(attr.value()));
        }
        key.push(' ');
    }

    key
}

// Go: ls/organizeimports.go:389 groupByNewlineContiguous
// groupByNewlineContiguous groups declarations by blank lines between them.
fn group_by_newline_contiguous(source_file: Node, decls: &[Node]) -> Vec<Vec<Node>> {
    let text = source_file_text(source_file);
    let mut s = new_scanner();
    s.set_skip_trivia(false); // Must not skip trivia to detect newlines
    let mut groups: Vec<Vec<Node>> = Vec::new();
    let mut current_group: Vec<Node> = Vec::new();

    for &decl in decls {
        if !current_group.is_empty() && is_new_group(&text, decl, &mut s) {
            groups.push(current_group);
            current_group = Vec::new();
        }
        current_group.push(decl);
    }

    if !current_group.is_empty() {
        groups.push(current_group);
    }

    groups
}

// Go: ls/organizeimports.go:410 isNewGroup
// PORT: Go takes the source file; the caller passes its text, which the
// scanner borrows.
fn is_new_group<'a>(text: &'a str, decl: Node, s: &mut Scanner<'a>) -> bool {
    let full_start = decl.pos();
    if full_start < 0 {
        return false;
    }

    let text_len = text.len() as i32;

    if full_start >= text_len {
        return false;
    }

    let start_pos = skip_trivia(text, full_start);
    if start_pos <= full_start {
        return false;
    }

    let trivia_len = start_pos - full_start;
    // PORT: Go slices the bytes; both ends are token boundaries here.
    s.set_text(&text[full_start as usize..start_pos as usize]);

    let mut number_of_new_lines = 0;
    while s.token_start() < trivia_len {
        let token_kind = s.scan();
        if token_kind == SyntaxKind::NewLineTrivia {
            number_of_new_lines += 1;
            if number_of_new_lines >= 2 {
                return true;
            }
        }
    }

    false
}

// Go: ls/organizeimports.go:445 coalesceImportsWorker
fn coalesce_imports_worker(
    import_decls: &[Node],
    comparer: &dyn Fn(&str, &str) -> i32,
    specifier_comparer: &dyn Fn(Node, Node) -> i32,
    source_file: Node,
    change_tracker: &mut change::Tracker,
) -> Vec<Node> {
    if import_decls.is_empty() {
        return import_decls.to_vec();
    }

    let mut import_groups_by_attributes: FxHashMap<String, Vec<Node>> = FxHashMap::default();
    let mut attribute_keys: Vec<String> = Vec::new();

    for &import_decl in import_decls {
        let key = get_import_attributes_key(import_decl.attributes());
        if !import_groups_by_attributes.contains_key(&key) {
            attribute_keys.push(key.clone());
        }
        import_groups_by_attributes
            .entry(key)
            .or_default()
            .push(import_decl);
    }

    let mut coalesced_imports: Vec<Node> = Vec::new();

    for attribute_key in &attribute_keys {
        let import_group_same_attrs = &import_groups_by_attributes[attribute_key];
        let categorized = get_categorized_imports(import_group_same_attrs);

        if categorized.import_without_clause.is_some() {
            coalesced_imports.push(categorized.import_without_clause);
        }
        // ts#63915: source phase imports are not coalesced. Default-named
        // ones sort by name and come before the others (Go N'
        // organizeimports.go:476-490).
        let mut source_phase_imports = categorized.source_phase_imports;
        crate::gostd::slices::sort_stable_func(&mut source_phase_imports, |a: &Node, b: &Node| {
            let a = a.import_clause();
            let b = b.import_clause();
            if a.name().is_nil() && b.name().is_nil() {
                return 0;
            }
            if a.name().is_nil() {
                return 1;
            }
            if b.name().is_nil() {
                return -1;
            }
            specifier_comparer(a, b)
        });
        coalesced_imports.extend(source_phase_imports);

        let factory = NodeFactory::new();

        for (i, mut group) in [categorized.regular_imports, categorized.type_only_imports]
            .into_iter()
            .enumerate()
        {
            if group.is_empty() {
                continue;
            }

            let is_type_only = i == 1;

            if !is_type_only
                && group.default_imports.len() == 1
                && group.namespace_imports.len() == 1
                && group.named_imports.is_empty()
            {
                let default_import = group.default_imports[0];
                let namespace_import = group.namespace_imports[0];

                let default_clause = default_import.import_clause();
                let namespace_bindings = namespace_import.import_clause().named_bindings();

                let new_clause = factory.update_import_clause(
                    default_clause,
                    default_clause.phase_modifier(),
                    default_clause.name(),
                    namespace_bindings,
                );
                let default_decl_node = default_import;
                let new_import_decl = factory.update_import_declaration(
                    default_decl_node,
                    default_decl_node.modifiers(),
                    new_clause,
                    default_decl_node.module_specifier(),
                    default_decl_node.attributes(),
                );
                coalesced_imports.push(new_import_decl);
                continue;
            }

            crate::gostd::slices::sort_func(
                &mut group.namespace_imports,
                |a: &Node, b: &Node| -> i32 {
                    let n1 = a.import_clause().named_bindings().name();
                    let n2 = b.import_clause().named_bindings().name();
                    comparer(n1.text(), n2.text())
                },
            );

            for &ns_import in &group.namespace_imports {
                let ns_import_decl = ns_import;
                let clause = ns_import_decl.import_clause();
                let new_clause = factory.update_import_clause(
                    clause,
                    clause.phase_modifier(),
                    Node::NIL,
                    clause.named_bindings(),
                );
                let new_import_decl = factory.update_import_declaration(
                    ns_import_decl,
                    ns_import_decl.modifiers(),
                    new_clause,
                    ns_import_decl.module_specifier(),
                    ns_import_decl.attributes(),
                );
                coalesced_imports.push(new_import_decl);
            }

            let mut first_default_import = Node::NIL;
            let mut first_named_import = Node::NIL;

            if !group.default_imports.is_empty() {
                first_default_import = group.default_imports[0];
            }
            if !group.named_imports.is_empty() {
                first_named_import = group.named_imports[0];
            }

            let mut import_decl = first_default_import;
            if import_decl.is_nil() {
                import_decl = first_named_import;
            }
            if import_decl.is_nil() {
                continue;
            }

            let mut new_default_import = Node::NIL;
            let mut new_import_specifiers: Vec<Node> = Vec::new();

            if group.default_imports.len() == 1 {
                new_default_import = group.default_imports[0].import_clause().name();
            } else {
                for &default_import in &group.default_imports {
                    let default_clause = default_import.import_clause();
                    let default_name = default_clause.name();
                    let property_name = factory.new_identifier("default");
                    let import_spec =
                        factory.new_import_specifier(false, property_name, default_name);
                    new_import_specifiers.push(import_spec);
                }
            }

            new_import_specifiers.extend(get_new_import_specifiers(&group.named_imports, &factory));
            crate::gostd::slices::sort_stable_func(
                &mut new_import_specifiers,
                |a: &Node, b: &Node| -> i32 { specifier_comparer(*a, *b) },
            );

            let new_named_imports: Node;
            if new_import_specifiers.is_empty() {
                if new_default_import.is_some() {
                    new_named_imports = Node::NIL;
                } else {
                    new_named_imports = factory.new_named_imports(factory.new_node_list(&[]));
                }
            } else {
                // PORT: Go makes the list and then sets `sortedList.Loc`; a
                // `NodeList` loc is fixed when it is made, so the loc is
                // chosen first.
                if first_named_import.is_some() {
                    let first_named_bindings = first_named_import.import_clause().named_bindings();
                    let original_elements = first_named_bindings.element_list();
                    let sorted_list = if original_elements.has_trailing_comma() {
                        factory
                            .new_node_list_with_loc(&new_import_specifiers, original_elements.loc())
                    } else {
                        factory.new_node_list(&new_import_specifiers)
                    };
                    new_named_imports =
                        factory.update_named_imports(first_named_bindings, sorted_list);
                } else {
                    let sorted_list = factory.new_node_list(&new_import_specifiers);
                    new_named_imports = factory.new_named_imports(sorted_list);
                }
            }

            if source_file.is_some() && new_named_imports.is_some() && first_named_import.is_some()
            {
                let first_named_bindings = first_named_import.import_clause().named_bindings();
                if !node_is_synthesized(first_named_bindings)
                    && !range_is_on_single_line(first_named_bindings.loc(), source_file)
                {
                    change_tracker
                        .emit_context
                        .set_emit_flags(new_named_imports, EmitFlags::MULTI_LINE);
                }
            }

            if is_type_only && new_default_import.is_some() && new_named_imports.is_some() {
                let import_decl_node = import_decl;

                let default_clause = factory.new_import_clause(
                    import_decl_node.import_clause().phase_modifier(),
                    new_default_import,
                    Node::NIL,
                );
                let default_import_decl = factory.update_import_declaration(
                    import_decl_node,
                    import_decl_node.modifiers(),
                    default_clause,
                    import_decl_node.module_specifier(),
                    import_decl_node.attributes(),
                );
                coalesced_imports.push(default_import_decl);

                let mut named_decl_node = first_named_import;
                if named_decl_node.is_nil() {
                    named_decl_node = import_decl;
                }
                let named_import_decl_node = named_decl_node;
                let named_clause = factory.new_import_clause(
                    named_import_decl_node.import_clause().phase_modifier(),
                    Node::NIL,
                    new_named_imports,
                );
                let named_import_decl = factory.update_import_declaration(
                    named_import_decl_node,
                    named_import_decl_node.modifiers(),
                    named_clause,
                    named_import_decl_node.module_specifier(),
                    named_import_decl_node.attributes(),
                );
                coalesced_imports.push(named_import_decl);
            } else {
                let import_decl_node = import_decl;
                let clause_node = import_decl_node.import_clause();
                let new_clause = factory.update_import_clause(
                    clause_node,
                    clause_node.phase_modifier(),
                    new_default_import,
                    new_named_imports,
                );
                let new_import_decl = factory.update_import_declaration(
                    import_decl_node,
                    import_decl_node.modifiers(),
                    new_clause,
                    import_decl_node.module_specifier(),
                    import_decl_node.attributes(),
                );
                coalesced_imports.push(new_import_decl);
            }
        }
    }
    coalesced_imports
}

// Go: ls/organizeimports.go:635 categorizedImports
struct CategorizedImports {
    import_without_clause: Node,
    source_phase_imports: Vec<Node>,
    type_only_imports: ImportGroup,
    regular_imports: ImportGroup,
}

// Go: ls/organizeimports.go:641 importGroup
#[derive(Clone, Default)]
struct ImportGroup {
    default_imports: Vec<Node>,
    namespace_imports: Vec<Node>,
    named_imports: Vec<Node>,
}

impl ImportGroup {
    // Go: ls/organizeimports.go:643 (importGroup).isEmpty
    fn is_empty(&self) -> bool {
        self.default_imports.is_empty()
            && self.namespace_imports.is_empty()
            && self.named_imports.is_empty()
    }
}

// Go: ls/organizeimports.go:651 getCategorizedImports
fn get_categorized_imports(import_decls: &[Node]) -> CategorizedImports {
    let mut import_without_clause = Node::NIL;
    let mut source_phase_imports: Vec<Node> = Vec::new();
    let mut type_only_imports = ImportGroup::default();
    let mut regular_imports = ImportGroup::default();

    for &import_decl in import_decls {
        if import_decl.import_clause().is_nil() {
            if import_without_clause.is_nil() {
                import_without_clause = import_decl;
            }
            continue;
        }

        let clause = import_decl.import_clause();
        // ts#63915
        if clause.phase_modifier() == SyntaxKind::SourceKeyword {
            source_phase_imports.push(import_decl);
            continue;
        }
        let group = if clause.is_type_only() {
            &mut type_only_imports
        } else {
            &mut regular_imports
        };

        let name = clause.name();
        let named_bindings = clause.named_bindings();

        if name.is_some() {
            group.default_imports.push(import_decl);
        }

        if named_bindings.is_some() {
            match named_bindings.kind() {
                SyntaxKind::NamespaceImport => {
                    group.namespace_imports.push(import_decl);
                }
                SyntaxKind::NamedImports => {
                    group.named_imports.push(import_decl);
                }
                _ => {}
            }
        }
    }

    CategorizedImports {
        import_without_clause,
        source_phase_imports,
        type_only_imports,
        regular_imports,
    }
}

// Go: ls/organizeimports.go:693 getNewImportSpecifiers
fn get_new_import_specifiers(named_imports: &[Node], factory: &NodeFactory) -> Vec<Node> {
    let mut result: Vec<Node> = Vec::new();

    for &named_import in named_imports {
        let Some(elements) = try_get_named_binding_elements(named_import) else {
            continue;
        };

        for elem in elements {
            let spec = elem;

            if spec.property_name().is_some() && spec.name().is_some() {
                let property_text = spec.property_name().text();
                let name_text = spec.name().text();

                if property_text == name_text {
                    let normalized = factory.update_import_specifier(
                        spec,
                        spec.is_type_only(),
                        Node::NIL,
                        spec.name(),
                    );
                    result.push(normalized);
                    continue;
                }
            }

            result.push(elem);
        }
    }

    result
}

// Go: ls/organizeimports.go:723 tryGetNamedBindingElements
// PORT: a Go nil slice is `None`.
fn try_get_named_binding_elements(named_import: Node) -> Option<Vec<Node>> {
    if named_import.kind() != SyntaxKind::ImportDeclaration {
        return None;
    }

    let import_decl = named_import;
    if import_decl.import_clause().is_nil() {
        return None;
    }

    let clause = import_decl.import_clause();
    let named_bindings = clause.named_bindings();

    if named_bindings.is_some() && named_bindings.kind() == SyntaxKind::NamedImports {
        let named_imports_node = named_bindings;
        return Some(named_imports_node.elements().to_vec());
    }

    None
}

// Go: ls/organizeimports.go:744 getTopLevelExportGroups
fn get_top_level_export_groups(source_file: Node) -> Vec<Vec<Node>> {
    let mut top_level_export_groups: Vec<Vec<Node>> = Vec::new();
    let statements = source_file.statements().to_vec();
    let statements_len = statements.len();

    let mut i = 0;
    let mut group_index = 0;
    while i < statements_len {
        if statements[i].kind() == SyntaxKind::ExportDeclaration {
            if group_index >= top_level_export_groups.len() {
                top_level_export_groups.push(Vec::new());
            }
            let export_decl = statements[i];
            if export_decl.module_specifier().is_some() {
                top_level_export_groups[group_index].push(statements[i]);
                i += 1;
            } else {
                while i < statements_len && statements[i].kind() == SyntaxKind::ExportDeclaration {
                    top_level_export_groups[group_index].push(statements[i]);
                    i += 1;
                }
                group_index += 1;
            }
        } else {
            i += 1;
            if group_index < top_level_export_groups.len()
                && !top_level_export_groups[group_index].is_empty()
            {
                group_index += 1;
            }
        }
    }

    let mut result: Vec<Vec<Node>> = Vec::new();
    for export_group in &top_level_export_groups {
        let sub_groups = group_by_newline_contiguous(source_file, export_group);
        result.extend(sub_groups);
    }

    result
}

// Go: ls/organizeimports.go:784 organizeExportsWorker
fn organize_exports_worker(
    old_export_decls: &[Node],
    comparer: &OrganizeImportsComparerSettings,
    source_file: Node,
    change_tracker: &mut change::Tracker,
) {
    if old_export_decls.is_empty() {
        return;
    }

    let specifier_comparer_func = lsutil::get_named_import_specifier_comparer(
        &lsutil::UserPreferences {
            organize_imports_type_order: comparer.type_order,
            ..Default::default()
        },
        comparer.named_import_comparer.clone(),
    );

    let module_specifier_comparer = |a: &str, b: &str| -> i32 {
        call_string_comparer(&comparer.module_specifier_comparer, a, b)
    };

    let new_export_decls = coalesce_exports_worker(
        old_export_decls,
        &*specifier_comparer_func,
        &module_specifier_comparer,
        source_file,
        change_tracker,
    );

    if !old_export_decls.is_empty() {
        if new_export_decls.is_empty() {
            change_tracker.delete_node_range(
                source_file,
                old_export_decls[0],
                old_export_decls[old_export_decls.len() - 1],
                change::LeadingTriviaOption::EXCLUDE,
                change::TrailingTriviaOption::INCLUDE,
            );
        } else {
            for &exp in &new_export_decls {
                change_tracker
                    .emit_context
                    .add_emit_flags(exp, EmitFlags::NO_LEADING_COMMENTS);
            }

            let options = change::NodeOptions {
                leading_trivia_option: change::LeadingTriviaOption::EXCLUDE,
                trailing_trivia_option: change::TrailingTriviaOption::INCLUDE,
                suffix: "\n".to_string(),
                ..Default::default()
            };

            let new_nodes: Vec<Node> = new_export_decls.clone();
            change_tracker.replace_node_with_nodes(
                source_file,
                old_export_decls[0],
                &new_nodes,
                Some(&options),
            );

            if old_export_decls.len() > 1 {
                for i in 1..old_export_decls.len() {
                    change_tracker.delete(source_file, old_export_decls[i]);
                }
            }
        }
    }
}

// Go: ls/organizeimports.go:833 coalesceExportsWorker
fn coalesce_exports_worker(
    export_group: &[Node],
    specifier_comparer: &dyn Fn(Node, Node) -> i32,
    module_specifier_comparer: &dyn Fn(&str, &str) -> i32,
    source_file: Node,
    change_tracker: &mut change::Tracker,
) -> Vec<Node> {
    if export_group.is_empty() {
        return export_group.to_vec();
    }

    let mut exports_by_module_specifier: FxHashMap<String, Vec<Node>> = FxHashMap::default();
    let mut module_specifier_order: Vec<String> = Vec::new();

    for &export_decl in export_group {
        let export = export_decl;
        let mut module_specifier = String::new();
        if export.module_specifier().is_some() {
            module_specifier = go_node_text(export.module_specifier()).to_string();
        }
        if !exports_by_module_specifier.contains_key(&module_specifier) {
            module_specifier_order.push(module_specifier.clone());
        }
        exports_by_module_specifier
            .entry(module_specifier)
            .or_default()
            .push(export_decl);
    }

    crate::gostd::slices::sort_stable_func(
        &mut module_specifier_order,
        |a: &String, b: &String| -> i32 {
            if a.is_empty() && !b.is_empty() {
                return 1;
            }
            if !a.is_empty() && b.is_empty() {
                return -1;
            }
            module_specifier_comparer(a, b)
        },
    );

    let mut coalesced_exports: Vec<Node> = Vec::new();
    let factory = NodeFactory::new();

    for module_specifier in &module_specifier_order {
        let group = &exports_by_module_specifier[module_specifier];

        let categorized = get_categorized_exports(group);

        if categorized.export_without_clause.is_some() {
            coalesced_exports.push(categorized.export_without_clause);
        }

        for sub_group in [&categorized.named_exports, &categorized.type_only_exports] {
            if sub_group.is_empty() {
                continue;
            }

            let mut new_export_specifiers: Vec<Node> = Vec::new();
            for &export_decl in sub_group {
                let export_clause = export_decl.export_clause();
                if export_clause.is_some() && export_clause.kind() == SyntaxKind::NamedExports {
                    let named_exports = export_clause;
                    new_export_specifiers.extend(named_exports.elements().iter());
                }
            }

            crate::gostd::slices::sort_stable_func(
                &mut new_export_specifiers,
                |a: &Node, b: &Node| -> i32 { specifier_comparer(*a, *b) },
            );

            let export_decl = sub_group[0];

            let mut updated_export_clause = Node::NIL;
            if export_decl.export_clause().is_some() {
                if export_decl.export_clause().kind() == SyntaxKind::NamedExports {
                    let named_exports = export_decl.export_clause();
                    let sorted_list = factory.new_node_list(&new_export_specifiers);
                    updated_export_clause =
                        factory.update_named_exports(named_exports, sorted_list);

                    if source_file.is_some()
                        && !node_is_synthesized(named_exports)
                        && !range_is_on_single_line(named_exports.loc(), source_file)
                    {
                        change_tracker
                            .emit_context
                            .set_emit_flags(updated_export_clause, EmitFlags::MULTI_LINE);
                    }
                } else {
                    updated_export_clause = export_decl.export_clause();
                }
            }

            let new_export_decl = factory.update_export_declaration(
                export_decl,
                export_decl.modifiers(),
                export_decl.is_type_only(),
                updated_export_clause,
                export_decl.module_specifier(),
                export_decl.attributes(),
            );
            coalesced_exports.push(new_export_decl);
        }
    }

    coalesced_exports
}

// Go: ls/organizeimports.go:929 categorizedExports
struct CategorizedExports {
    export_without_clause: Node,
    named_exports: Vec<Node>,
    type_only_exports: Vec<Node>,
}

// Go: ls/organizeimports.go:935 getCategorizedExports
fn get_categorized_exports(export_group: &[Node]) -> CategorizedExports {
    let mut export_without_clause = Node::NIL;
    let mut named_exports: Vec<Node> = Vec::new();
    let mut type_only_exports: Vec<Node> = Vec::new();

    for &export_decl in export_group {
        let export = export_decl;
        if export.export_clause().is_nil() {
            if export_without_clause.is_nil() {
                export_without_clause = export_decl;
            }
        } else if export.is_type_only() {
            type_only_exports.push(export_decl);
        } else {
            named_exports.push(export_decl);
        }
    }

    CategorizedExports {
        export_without_clause,
        named_exports,
        type_only_exports,
    }
}

// Go: ast/ast.go:273 (*Node).Text
/// Go `node.Text()` of an import attribute value or an export module
/// specifier. The parser allows any expression there, and Go panics for a
/// kind with no text, so the request answers Go's InternalError.
// PORT: `Node::text` gives "" for such a kind (node.rs PORT rule). Go's
// text is `%T` of the node data, whose type name is the data schema name.
fn go_node_text(node: Node) -> &'static str {
    let unhandled = crate::ast::synthetic::with_ast_data(node, |d| match d {
        NodeData::Identifier(_)
        | NodeData::PrivateIdentifier(_)
        | NodeData::StringLiteral(_)
        | NodeData::NumericLiteral(_)
        | NodeData::BigIntLiteral(_)
        | NodeData::MetaProperty(_)
        | NodeData::NoSubstitutionTemplateLiteral(_)
        | NodeData::TemplateHead(_)
        | NodeData::TemplateMiddle(_)
        | NodeData::TemplateTail(_)
        | NodeData::JsxNamespacedName(_)
        | NodeData::RegularExpressionLiteral(_)
        | NodeData::JsDocText(_)
        | NodeData::JsDocLink(_)
        | NodeData::JsDocLinkCode(_)
        | NodeData::JsDocLinkPlain(_) => None,
        other => Some(other.schema_name()),
    });
    if let Some(data) = unhandled {
        crate::core::go_panic(format!("Unhandled case in Node.Text: *ast.{data}"));
    }
    node.text()
}

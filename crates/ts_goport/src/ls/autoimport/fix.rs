use crate::ls::autoimport::prelude::*;

// Port of Go `ls/autoimport/fix.go`.
//
// PORT (whole file):
// - Go `*Fix` values are shared (`Rc<Fix>`). Go embeds
//   `*lsproto.AutoImportFix`; it is the field `auto_import_fix`, and `Deref`
//   promotes its fields (`fix.kind`, `fix.name`) as Go does.
// - Go `*newImportBinding` values are never changed after they are made, so
//   they are `NewImportBinding` values (`Option` where Go can pass nil).
// - `(*View).GetFixes` and the methods it calls take the request checker
//   `ch`. Go stores it in the view (`v.checker`, `NewView`'s `typeChecker`,
//   ts#64178); the Rust checker is a `RefCell` that the caller already
//   borrows, so the caller passes it to each method that reads it (as the
//   pinned ImportAdder decision does for the adder).
// - Go passes `*ast.SourceFile` to program methods that take an
//   `ast.HasFileName`: `source_file_has_file_name(file)` (view.rs).
// - The change tracker's embedded Go `NodeFactory` is `ct.node_factory()`.
//   Nodes are made into locals first, in Go argument order, before the
//   tracker call that takes `&mut ct`.
// - Go `panic` text that prints a `Kind` (`KindString()`, `%v`) prints
//   `debug::kind_string` ("KindX").

use crate::frontend::compiler;
use crate::frontend::core_ls_ext::compare_booleans;
use crate::frontend::scanner::scanner_ls;
use crate::frontend::tspath;
use crate::gostd::Context;
use crate::locale;
use crate::ls::{change, lsconv, lsutil};
use crate::lsp::lsproto;
use crate::modulespecifiers;

use crate::flags_macros::go_enum;

// Go: ls/autoimport/fix.go:30 newImportBinding
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NewImportBinding {
    pub kind: lsproto::ImportKind,
    pub property_name: String,
    pub name: String,
    pub add_as_type_only: lsproto::AddAsTypeOnly,
}

// Go: ls/autoimport/fix.go:37 Fix
#[derive(Clone, Debug, Default)]
pub struct Fix {
    pub auto_import_fix: lsproto::AutoImportFix,

    pub module_specifier_kind: modulespecifiers::ResultKind,
    pub is_re_export: bool,
    pub module_file_name: String,
    pub type_only_alias_declaration: Node,
}

impl std::ops::Deref for Fix {
    type Target = lsproto::AutoImportFix;
    fn deref(&self) -> &lsproto::AutoImportFix {
        &self.auto_import_fix
    }
}

impl std::ops::DerefMut for Fix {
    fn deref_mut(&mut self) -> &mut lsproto::AutoImportFix {
        &mut self.auto_import_fix
    }
}

// Go: ls/autoimport/fix.go:46 addToExistingImportFix
#[derive(Clone, Debug, Default)]
pub struct AddToExistingImportFix {
    pub import_clause_or_binding_pattern: Node,
    // One of `defaultImport` or `namedImports` will be present
    pub default_import: Option<NewImportBinding>,
    pub named_import: Option<NewImportBinding>,
}

impl Fix {
    // Go: ls/autoimport/fix.go:56 Edits
    // Edits produces the text edits and a human-readable description for the fix. The returned bool is false
    // when the fix targets a content-mapped file and any edit could not be placed within a single verbatim
    // span, meaning it cannot be safely applied to the original text and the caller should discard it.
    pub fn edits(
        &self,
        ctx: &Context,
        file: Node,
        compiler_options: &CompilerOptions,
        format_options: &lsutil::FormatCodeSettings,
        converters: &Rc<lsconv::Converters>,
        preferences: &lsutil::UserPreferences,
    ) -> (Vec<lsproto::TextEdit>, String, bool) {
        let f = self;
        let locale = locale::from_context(ctx);
        let mut tracker = change::new_tracker(
            ctx,
            compiler_options,
            format_options.clone(),
            converters.clone(),
        );
        match f.kind {
            lsproto::AutoImportFixKind::USE_NAMESPACE => {
                let description = add_namespace_qualifier(f, &mut tracker, file, &locale);
                let (edits, safe) = file_edits(&mut tracker, file);
                (edits, description, safe)
            }
            lsproto::AutoImportFixKind::ADD_TO_EXISTING => {
                if (source_file_imports(file).len() as i32) <= f.import_index {
                    crate::core::go_panic("import index out of range".to_string());
                }
                let existing_fix = get_add_to_existing_import_fix(file, f);
                // Go: core.SingleElementSlice(existingFix.namedImport)
                let named_imports: Vec<NewImportBinding> =
                    existing_fix.named_import.iter().cloned().collect();
                add_to_existing_import(
                    &mut tracker,
                    file,
                    existing_fix.import_clause_or_binding_pattern,
                    existing_fix.default_import.as_ref(),
                    &named_imports,
                    preferences,
                );
                let (edits, safe) = file_edits(&mut tracker, file);
                (
                    edits,
                    crate::diagnostics_loc::message_localize(
                        diag::Update_import_from_0,
                        &locale,
                        &args![f.module_specifier],
                    ),
                    safe,
                )
            }
            lsproto::AutoImportFixKind::ADD_NEW => {
                let default_import = if f.import_kind == lsproto::ImportKind::DEFAULT {
                    Some(NewImportBinding {
                        name: f.name.clone(),
                        add_as_type_only: f.add_as_type_only,
                        ..Default::default()
                    })
                } else {
                    None
                };
                let named_imports: Vec<NewImportBinding> =
                    if f.import_kind == lsproto::ImportKind::NAMED {
                        vec![NewImportBinding {
                            name: f.name.clone(),
                            add_as_type_only: f.add_as_type_only,
                            ..Default::default()
                        }]
                    } else {
                        Vec::new()
                    };
                let mut namespace_like_import: Option<NewImportBinding> = None;
                // qualification := f.qualification()
                if f.import_kind == lsproto::ImportKind::NAMESPACE
                    || f.import_kind == lsproto::ImportKind::COMMON_JS
                {
                    namespace_like_import = Some(NewImportBinding {
                        kind: f.import_kind,
                        name: f.name.clone(),
                        ..Default::default()
                    });
                    // if qualification != nil && qualification.namespacePref != "" {
                    // 	namespaceLikeImport.name = qualification.namespacePref
                    // }
                }

                let quote_preference = lsutil::get_quote_preference(file, preferences);
                let declarations = if f.use_require {
                    get_new_requires(
                        &mut tracker,
                        &f.module_specifier,
                        quote_preference,
                        default_import.as_ref(),
                        &named_imports,
                        namespace_like_import.as_ref(),
                        compiler_options,
                    )
                } else {
                    get_new_imports(
                        &mut tracker,
                        &f.module_specifier,
                        quote_preference,
                        default_import.as_ref(),
                        &named_imports,
                        namespace_like_import.as_ref(),
                        compiler_options,
                        preferences,
                    )
                };

                insert_imports(
                    &mut tracker,
                    file,
                    &declarations,
                    /*blankLineBetween*/ true,
                    preferences,
                );
                // if qualification != nil {
                // 	addNamespaceQualifier(tracker, file, qualification)
                // }
                let (edits, safe) = file_edits(&mut tracker, file);
                (
                    edits,
                    crate::diagnostics_loc::message_localize(
                        diag::Add_import_from_0,
                        &locale,
                        &args![f.module_specifier],
                    ),
                    safe,
                )
            }
            lsproto::AutoImportFixKind::PROMOTE_TYPE_ONLY => {
                let promoted_declaration = promote_from_type_only(
                    &mut tracker,
                    f.type_only_alias_declaration,
                    compiler_options,
                    file,
                    preferences,
                );
                if promoted_declaration.kind() == SyntaxKind::ImportSpecifier {
                    let module_spec =
                        get_module_specifier_text(promoted_declaration.parent().parent());
                    let (edits, safe) = file_edits(&mut tracker, file);
                    return (
                        edits,
                        crate::diagnostics_loc::message_localize(
                            diag::Remove_type_from_import_of_0_from_1,
                            &locale,
                            &args![f.name, module_spec],
                        ),
                        safe,
                    );
                }
                let module_spec = get_module_specifier_text(promoted_declaration);
                let (edits, safe) = file_edits(&mut tracker, file);
                (
                    edits,
                    crate::diagnostics_loc::message_localize(
                        diag::Remove_type_from_import_declaration_from_0,
                        &locale,
                        &args![module_spec],
                    ),
                    safe,
                )
            }
            lsproto::AutoImportFixKind::JSDOC_TYPE_IMPORT => {
                let description = add_import_type(f, file, preferences, &mut tracker, &locale);
                let (edits, safe) = file_edits(&mut tracker, file);
                (edits, description, safe)
            }
            _ => crate::core::go_panic("unimplemented fix edit".to_string()),
        }
    }
}

// Go: ls/autoimport/fix.go:133 fileEdits
// fileEdits returns the edits recorded for file, along with whether they are safe to apply. GetChanges
// drops the edits of any content-mapped file that cannot be faithfully mapped back to the original text,
// so an empty result with safe == false means the fix could not be represented and must be discarded.
// PORT: Go `changes[name]` is a nil slice for a missing key; that is an empty `Vec`.
// Go `file.OriginalFileName()` is the `lsconv::Script` method of a file root `Node`.
fn file_edits(tracker: &mut change::Tracker, file: Node) -> (Vec<lsproto::TextEdit>, bool) {
    let (mut changes, unmappable) = tracker.get_changes();
    let edits = changes
        .shift_remove(lsconv::Script::original_file_name(&file))
        .unwrap_or_default();
    (edits, unmappable.is_empty())
}

// Go: ls/autoimport/fix.go:138 addImportType
pub fn add_import_type(
    f: &Fix,
    file: Node,
    preferences: &lsutil::UserPreferences,
    tracker: &mut change::Tracker,
    locale: &locale::Locale,
) -> String {
    let Some(usage_position) = f.usage_position else {
        crate::core::go_panic("UsagePosition must be set for JSDoc type import fix".to_string());
    };
    let quote_preference = lsutil::get_quote_preference(file, preferences);
    let mut quote_char = "\"";
    if quote_preference == lsutil::QuotePreference::SINGLE {
        quote_char = "'";
    }
    let import_type_prefix = format!(
        "import({}{}{}).",
        quote_char, f.module_specifier, quote_char
    );
    tracker.insert_text(file, usage_position, &import_type_prefix);
    crate::diagnostics_loc::message_localize(
        diag::Change_0_to_1,
        locale,
        &args![f.name, format!("{}{}", import_type_prefix, f.name)],
    )
}

// Go: ls/autoimport/fix.go:152 addNamespaceQualifier
pub fn add_namespace_qualifier(
    f: &Fix,
    tracker: &mut change::Tracker,
    file: Node,
    locale: &locale::Locale,
) -> String {
    if f.usage_position.is_none() || f.namespace_prefix.is_empty() {
        crate::core::go_panic("namespace fix requires usage position and prefix".to_string());
    }
    let usage_position = f.usage_position.expect("checked above");
    let qualified = format!("{}.{}", f.namespace_prefix, f.name);
    tracker.insert_text(file, usage_position, &format!("{}.", f.namespace_prefix));
    crate::diagnostics_loc::message_localize(diag::Change_0_to_1, locale, &args![f.name, qualified])
}

// Go: ls/autoimport/fix.go:161 getAddToExistingImportFix
pub fn get_add_to_existing_import_fix(file: Node, fix: &Fix) -> AddToExistingImportFix {
    if fix.kind != lsproto::AutoImportFixKind::ADD_TO_EXISTING {
        crate::core::go_panic("expected add to existing import fix".to_string());
    }
    let module_specifier = source_file_imports(file).get(fix.import_index as usize);
    let import_node = try_get_import_from_module_specifier(module_specifier);
    if import_node.is_nil() {
        crate::core::go_panic("expected import declaration".to_string());
    }
    let import_clause_or_binding_pattern;
    match import_node.kind() {
        SyntaxKind::ImportDeclaration => {
            import_clause_or_binding_pattern = import_node.import_clause();
            if import_clause_or_binding_pattern.is_nil() {
                crate::core::go_panic("expected import clause".to_string());
            }
        }
        SyntaxKind::CallExpression => {
            if !is_variable_declaration_initialized_to_require(import_node.parent()) {
                crate::core::go_panic(
                    "expected require call expression to be in variable declaration".to_string(),
                );
            }
            import_clause_or_binding_pattern = import_node.parent().name();
            if import_clause_or_binding_pattern.is_nil()
                || !is_object_binding_pattern(import_clause_or_binding_pattern)
            {
                crate::core::go_panic(
                    "expected object binding pattern in variable declaration".to_string(),
                );
            }
        }
        _ => crate::core::go_panic(
            "expected import declaration or require call expression".to_string(),
        ),
    }

    let default_import = if fix.import_kind == lsproto::ImportKind::DEFAULT {
        Some(NewImportBinding {
            kind: lsproto::ImportKind::DEFAULT,
            name: fix.name.clone(),
            add_as_type_only: fix.add_as_type_only,
            ..Default::default()
        })
    } else {
        None
    };
    let named_imports = if fix.import_kind == lsproto::ImportKind::NAMED {
        Some(NewImportBinding {
            kind: lsproto::ImportKind::NAMED,
            name: fix.name.clone(),
            add_as_type_only: fix.add_as_type_only,
            ..Default::default()
        })
    } else {
        None
    };
    AddToExistingImportFix {
        import_clause_or_binding_pattern,
        default_import,
        named_import: named_imports,
    }
}

// Go: ls/autoimport/fix.go:198 addToExistingImport
pub fn add_to_existing_import(
    ct: &mut change::Tracker,
    file: Node,
    import_clause_or_binding_pattern: Node,
    default_import: Option<&NewImportBinding>,
    named_imports: &[NewImportBinding],
    preferences: &lsutil::UserPreferences,
) {
    match import_clause_or_binding_pattern.kind() {
        SyntaxKind::ObjectBindingPattern => {
            let binding_pattern = import_clause_or_binding_pattern;
            if let Some(default_import) = default_import {
                add_element_to_binding_pattern(
                    ct,
                    file,
                    binding_pattern,
                    &default_import.name,
                    "default",
                );
            }
            for named_import in named_imports {
                add_element_to_binding_pattern(ct, file, binding_pattern, &named_import.name, "");
            }
        }
        SyntaxKind::ImportClause => {
            let import_clause = import_clause_or_binding_pattern;

            // promoteFromTypeOnly = true if we need to promote the entire original clause from type only
            // Go: core.Some(append(namedImports, defaultImport), ..) with a nil check.
            let promote_from_type_only = import_clause.is_type_only()
                && named_imports
                    .iter()
                    .map(Some)
                    .chain(std::iter::once(default_import))
                    .any(|i| match i {
                        None => false,
                        Some(i) => i.add_as_type_only == lsproto::AddAsTypeOnly::NOT_ALLOWED,
                    });

            let mut existing_specifiers: Vec<Node> = Vec::new();
            if import_clause.named_bindings().is_some()
                && import_clause.named_bindings().kind() == SyntaxKind::NamedImports
            {
                existing_specifiers = import_clause.named_bindings().elements().to_vec();
            }

            if let Some(default_import) = default_import {
                debug_assert!(
                    import_clause.name().is_nil(),
                    "Cannot add a default import to an import clause that already has one"
                );
                let pos = astnav::get_start_of_node(import_clause, file, false);
                let identifier = ct
                    .node_factory()
                    .new_identifier(default_import.name.clone());
                ct.insert_node_at(
                    file,
                    pos,
                    identifier,
                    change::NodeOptions {
                        suffix: ", ".to_string(),
                        ..Default::default()
                    },
                );
            }

            if !named_imports.is_empty() {
                let (specifier_comparer, is_sorted) =
                    lsutil::get_named_import_specifier_comparer_with_detection(
                        import_clause.parent(),
                        file,
                        preferences,
                    );
                let mut new_specifiers: Vec<Node> = named_imports
                    .iter()
                    .map(|named_import| {
                        let is_type_only = (!import_clause.is_type_only()
                            || promote_from_type_only)
                            && should_use_type_only(named_import.add_as_type_only, preferences);
                        let mut identifier = Node::NIL;
                        if !named_import.property_name.is_empty() {
                            identifier = ct
                                .node_factory()
                                .new_identifier(named_import.property_name.clone());
                        }
                        let name = ct.node_factory().new_identifier(named_import.name.clone());
                        ct.node_factory()
                            .new_import_specifier(is_type_only, identifier, name)
                    })
                    .collect();
                gostd::slices::sort_func(&mut new_specifiers, |a, b| specifier_comparer(*a, *b));
                if !existing_specifiers.is_empty() && is_sorted != Tristate::False {
                    // The sorting preference computed earlier may or may not have validated that these particular
                    // import specifiers are sorted. If they aren't, `getImportSpecifierInsertionIndex` will return
                    // nonsense. So if there are existing specifiers, even if we know the sorting preference, we
                    // need to ensure that the existing specifiers are sorted according to the preference in order
                    // to do a sorted insertion.

                    // If we're promoting the clause from type-only, we need to transform the existing imports
                    // before attempting to insert the new named imports (for comparison purposes only)
                    let mut specs_to_compare_against = existing_specifiers.clone();
                    if promote_from_type_only && !existing_specifiers.is_empty() {
                        specs_to_compare_against = existing_specifiers
                            .iter()
                            .map(|&e| {
                                let spec = e;
                                let mut property_name = Node::NIL;
                                if spec.property_name().is_some() {
                                    property_name = spec.property_name();
                                }
                                ct.node_factory().new_import_specifier(
                                    true, // isTypeOnly
                                    property_name,
                                    spec.name(),
                                )
                            })
                            .collect();
                    }

                    for &spec in &new_specifiers {
                        let insertion_index = lsutil::get_import_specifier_insertion_index(
                            &specs_to_compare_against,
                            spec,
                            &*specifier_comparer,
                        );
                        ct.insert_import_specifier_at_index(
                            file,
                            spec,
                            import_clause.named_bindings(),
                            insertion_index,
                        );
                    }
                } else if !existing_specifiers.is_empty() {
                    for &spec in &new_specifiers {
                        ct.insert_node_in_list_after(
                            file,
                            existing_specifiers[existing_specifiers.len() - 1],
                            spec,
                            NodeList::NIL,
                        );
                    }
                } else if !new_specifiers.is_empty() {
                    let list = ct.node_factory().new_node_list(&new_specifiers);
                    let named_imports = ct.node_factory().new_named_imports(list);
                    if import_clause.named_bindings().is_some() {
                        ct.replace_node(file, import_clause.named_bindings(), named_imports, None);
                    } else {
                        if import_clause.name().is_nil() {
                            crate::core::go_panic(
                                "Import clause must have either named imports or a default import"
                                    .to_string(),
                            );
                        }
                        ct.insert_node_after(file, import_clause.name(), named_imports);
                    }
                }
            }

            if promote_from_type_only {
                // Delete the 'type' keyword from the import clause
                let type_keyword = get_type_keyword_of_type_only_import(import_clause, file);
                ct.delete(file, type_keyword);

                // Add 'type' modifier to existing specifiers (not newly added ones)
                // We preserve the type-onlyness of existing specifiers regardless of whether
                // it would make a difference in emit (user preference).
                if !existing_specifiers.is_empty() {
                    for &specifier in &existing_specifiers {
                        if !specifier.is_type_only() {
                            ct.insert_modifier_before(file, SyntaxKind::TypeKeyword, specifier);
                        }
                    }
                }
            }
        }
        kind => crate::core::go_panic(format!(
            "Unsupported clause kind: {} for addToExistingImport",
            crate::gostd::debug::kind_string(kind)
        )),
    }
}

// Go: ls/autoimport/fix.go:321 getTypeKeywordOfTypeOnlyImport
fn get_type_keyword_of_type_only_import(import_clause: Node, source_file: Node) -> Node {
    debug_assert!(
        import_clause.is_type_only(),
        "import clause must be type-only"
    );
    // The first child of a type-only import clause is the 'type' keyword
    // import type { foo } from './bar'
    //        ^^^^
    let type_keyword =
        astnav::find_child_of_kind(import_clause, SyntaxKind::TypeKeyword, source_file);
    debug_assert!(
        type_keyword.is_some(),
        "type-only import clause should have a type keyword"
    );
    type_keyword
}

// Go: ls/autoimport/fix.go:331 addElementToBindingPattern
// PORT: Go `core.IfElse` evaluates both arguments, so the property name
// identifier is made even when `propertyName` is empty (and then unused).
// Go passes that identifier as the binding element's initializer.
fn add_element_to_binding_pattern(
    ct: &mut change::Tracker,
    file: Node,
    binding_pattern: Node,
    name: &str,
    property_name: &str,
) {
    let name_node = ct.node_factory().new_identifier(name);
    let property_name_node = ct.node_factory().new_identifier(property_name);
    let element = ct.node_factory().new_binding_element(
        Node::NIL,
        Node::NIL,
        name_node,
        if property_name.is_empty() {
            Node::NIL
        } else {
            property_name_node
        },
    );
    let elements = binding_pattern.element_list();
    if !elements.nodes().is_empty() {
        let last = elements.nodes().get(elements.nodes().len() - 1);
        ct.insert_node_in_list_after(file, last, element, elements);
    } else {
        let list = ct.node_factory().new_node_list(&[element]);
        let pattern = ct
            .node_factory()
            .new_binding_pattern(SyntaxKind::ObjectBindingPattern, list);
        ct.replace_node(file, binding_pattern, pattern, None);
    }
}

// Go: ls/autoimport/fix.go:346 getNewImports
pub fn get_new_imports(
    ct: &mut change::Tracker,
    module_specifier: &str,
    quote_preference: lsutil::QuotePreference,
    default_import: Option<&NewImportBinding>,
    named_imports: &[NewImportBinding],
    namespace_like_import: Option<&NewImportBinding>, // { lsproto.importKind: lsproto.ImportKind.CommonJS | lsproto.ImportKind.Namespace; }
    compiler_options: &CompilerOptions,
    preferences: &lsutil::UserPreferences,
) -> Vec<Node> {
    let token_flags = if quote_preference == lsutil::QuotePreference::SINGLE {
        TokenFlags::SINGLE_QUOTE
    } else {
        TokenFlags::NONE
    };
    let module_specifier_string_literal = ct
        .node_factory()
        .new_string_literal(module_specifier, token_flags);
    let mut statements: Vec<Node> = Vec::new();
    if default_import.is_some() || !named_imports.is_empty() {
        // `verbatimModuleSyntax` should prefer top-level `import type` -
        // even though it's not an error, it would add unnecessary runtime emit.
        let top_level_type_only = (default_import
            .is_none_or(|d| needs_type_only(d.add_as_type_only))
            && named_imports
                .iter()
                .all(|i| needs_type_only(i.add_as_type_only)))
            || ((compiler_options.verbatim_module_syntax.is_true()
                || preferences.prefer_type_only_auto_imports.is_true())
                && default_import
                    .is_none_or(|d| d.add_as_type_only != lsproto::AddAsTypeOnly::NOT_ALLOWED)
                && !named_imports
                    .iter()
                    .any(|i| i.add_as_type_only == lsproto::AddAsTypeOnly::NOT_ALLOWED));

        let mut default_import_node = Node::NIL;
        if let Some(default_import) = default_import {
            default_import_node = ct
                .node_factory()
                .new_identifier(default_import.name.clone());
        }

        let specifiers: Vec<Node> = named_imports
            .iter()
            .map(|named_import| {
                let is_type_only = !top_level_type_only
                    && should_use_type_only(named_import.add_as_type_only, preferences);
                let mut named_import_property_name = Node::NIL;
                if !named_import.property_name.is_empty() {
                    named_import_property_name = ct
                        .node_factory()
                        .new_identifier(named_import.property_name.clone());
                }
                let name = ct.node_factory().new_identifier(named_import.name.clone());
                ct.node_factory().new_import_specifier(
                    is_type_only,
                    named_import_property_name,
                    name,
                )
            })
            .collect();
        statements.push(make_import(
            ct,
            default_import_node,
            &specifiers,
            module_specifier_string_literal,
            top_level_type_only,
        ));
    }

    if let Some(namespace_like_import) = namespace_like_import {
        let declaration;
        if namespace_like_import.kind == lsproto::ImportKind::COMMON_JS {
            let is_type_only =
                should_use_type_only(namespace_like_import.add_as_type_only, preferences);
            let name = ct
                .node_factory()
                .new_identifier(namespace_like_import.name.clone());
            let module_reference = ct
                .node_factory()
                .new_external_module_reference(module_specifier_string_literal);
            declaration = ct.node_factory().new_import_equals_declaration(
                /*modifiers*/ ModifierList::NIL,
                is_type_only,
                name,
                module_reference,
            );
        } else {
            let phase_modifier =
                if should_use_type_only(namespace_like_import.add_as_type_only, preferences) {
                    SyntaxKind::TypeKeyword
                } else {
                    SyntaxKind::Unknown
                };
            let name = ct
                .node_factory()
                .new_identifier(namespace_like_import.name.clone());
            let namespace_import = ct.node_factory().new_namespace_import(name);
            let import_clause = ct.node_factory().new_import_clause(
                /*phaseModifier*/ phase_modifier,
                /*name*/ Node::NIL,
                namespace_import,
            );
            declaration = ct.node_factory().new_import_declaration(
                /*modifiers*/ ModifierList::NIL,
                import_clause,
                module_specifier_string_literal,
                /*attributes*/ Node::NIL,
            );
        }
        statements.push(declaration);
    }
    if statements.is_empty() {
        crate::core::go_panic("No statements to insert for new imports".to_string());
    }
    statements
}

// Go: ls/autoimport/fix.go:415 getNewRequires
// PORT: Go does not read `compilerOptions`.
pub fn get_new_requires(
    change_tracker: &mut change::Tracker,
    module_specifier: &str,
    quote_preference: lsutil::QuotePreference,
    default_import: Option<&NewImportBinding>,
    named_imports: &[NewImportBinding],
    namespace_like_import: Option<&NewImportBinding>,
    _compiler_options: &CompilerOptions,
) -> Vec<Node> {
    let quoted_module_specifier = change_tracker.node_factory().new_string_literal(
        module_specifier,
        if quote_preference == lsutil::QuotePreference::SINGLE {
            TokenFlags::SINGLE_QUOTE
        } else {
            TokenFlags::NONE
        },
    );
    let mut statements: Vec<Node> = Vec::new();

    // const { default: foo, bar, etc } = require('./mod');
    if default_import.is_some() || !named_imports.is_empty() {
        let mut binding_elements: Vec<Node> = Vec::new();
        for named_import in named_imports {
            let mut property_name = Node::NIL;
            if !named_import.property_name.is_empty() {
                property_name = change_tracker
                    .node_factory()
                    .new_identifier(named_import.property_name.clone());
            }
            let name = change_tracker
                .node_factory()
                .new_identifier(named_import.name.clone());
            binding_elements.push(change_tracker.node_factory().new_binding_element(
                /*dotDotDotToken*/ Node::NIL,
                property_name,
                name,
                /*initializer*/ Node::NIL,
            ));
        }
        if let Some(default_import) = default_import {
            let property_name = change_tracker.node_factory().new_identifier("default");
            let name = change_tracker
                .node_factory()
                .new_identifier(default_import.name.clone());
            let element = change_tracker.node_factory().new_binding_element(
                /*dotDotDotToken*/ Node::NIL,
                property_name,
                name,
                /*initializer*/ Node::NIL,
            );
            binding_elements.insert(0, element);
        }
        let list = change_tracker
            .node_factory()
            .new_node_list(&binding_elements);
        let pattern = change_tracker
            .node_factory()
            .new_binding_pattern(SyntaxKind::ObjectBindingPattern, list);
        let declaration = create_const_equals_require_declaration(
            change_tracker,
            pattern,
            quoted_module_specifier,
        );
        statements.push(declaration);
    }

    // const foo = require('./mod');
    if let Some(namespace_like_import) = namespace_like_import {
        let name = change_tracker
            .node_factory()
            .new_identifier(namespace_like_import.name.clone());
        let declaration =
            create_const_equals_require_declaration(change_tracker, name, quoted_module_specifier);
        statements.push(declaration);
    }

    debug_assert!(!statements.is_empty());
    statements
}

// Go: ls/autoimport/fix.go:480 createConstEqualsRequireDeclaration
fn create_const_equals_require_declaration(
    change_tracker: &mut change::Tracker,
    name: Node,
    quoted_module_specifier: Node,
) -> Node {
    let factory = change_tracker.node_factory();
    let require = factory.new_identifier("require");
    let arguments = factory.new_node_list(&[quoted_module_specifier]);
    let call = factory.new_call_expression(
        require,
        /*questionDotToken*/ Node::NIL,
        /*typeArguments*/ NodeList::NIL,
        arguments,
        NodeFlags::NONE,
    );
    let variable_declaration = factory.new_variable_declaration(
        name,
        /*exclamationToken*/ Node::NIL,
        /*type*/ Node::NIL,
        call,
    );
    let declarations = factory.new_node_list(&[variable_declaration]);
    let declaration_list = factory.new_variable_declaration_list(declarations, NodeFlags::CONST);
    factory.new_variable_statement(/*modifiers*/ ModifierList::NIL, declaration_list)
}

// Go: ls/autoimport/fix.go:503 insertImports
// PORT: Go `comparer` is a func value that is never nil here
// (`detectCaseSensitivityBySort` falls back to the first comparer); a nil
// value panics when called, as in Go.
pub fn insert_imports(
    ct: &mut change::Tracker,
    source_file: Node,
    imports: &[Node],
    blank_line_between: bool,
    preferences: &lsutil::UserPreferences,
) {
    let existing_import_statements: Vec<Node> =
        if imports[0].kind() == SyntaxKind::VariableStatement {
            source_file
                .statements()
                .iter()
                .filter(|&s| is_require_variable_statement(s))
                .collect()
        } else {
            source_file
                .statements()
                .iter()
                .filter(|&s| is_any_import_syntax(s))
                .collect()
        };
    let (comparer, is_sorted) = lsutil::get_organize_imports_string_comparer_with_detection(
        &existing_import_statements,
        preferences,
    );
    let comparer = |a: &str, b: &str| -> i32 {
        (comparer
            .as_ref()
            .unwrap_or_else(|| crate::core::go_nil_dereference()))(a, b)
    };
    let mut sorted_new_imports: Vec<Node> = imports.to_vec();
    gostd::slices::sort_func(&mut sorted_new_imports, |a, b| {
        lsutil::compare_imports_or_require_statements(*a, *b, &comparer)
    });

    if !existing_import_statements.is_empty() && is_sorted {
        // Existing imports are sorted, insert each new import at the correct position
        for &new_import in &sorted_new_imports {
            let insertion_index = lsutil::get_import_declaration_insert_index(
                &existing_import_statements,
                new_import,
                &|a: Node, b: Node| lsutil::compare_imports_or_require_statements(a, b, &comparer),
            );
            if insertion_index == 0 {
                // If the first import is top-of-file, insert after the leading comment which is likely the header.
                let mut leading_trivia_option = change::LeadingTriviaOption::NONE;
                if existing_import_statements[0] == source_file.statements().get(0) {
                    leading_trivia_option = change::LeadingTriviaOption::EXCLUDE;
                }
                ct.insert_node_before(
                    source_file,
                    existing_import_statements[0],
                    new_import,
                    false, /*blankLineBetween*/
                    leading_trivia_option,
                );
            } else {
                let prev_import = existing_import_statements[(insertion_index - 1) as usize];
                ct.insert_node_after(source_file, prev_import, new_import);
            }
        }
    } else if !existing_import_statements.is_empty() {
        ct.insert_nodes_after(
            source_file,
            existing_import_statements[existing_import_statements.len() - 1],
            &sorted_new_imports,
        );
    } else {
        ct.insert_at_top_of_file(source_file, &sorted_new_imports, blank_line_between);
    }
}

// Go: ls/autoimport/fix.go:542 makeImport
fn make_import(
    ct: &mut change::Tracker,
    default_import: Node,
    named_imports: &[Node],
    module_specifier: Node,
    is_type_only: bool,
) -> Node {
    let mut new_named_imports = Node::NIL;
    if !named_imports.is_empty() {
        let list = ct.node_factory().new_node_list(named_imports);
        new_named_imports = ct.node_factory().new_named_imports(list);
    }
    let mut import_clause = Node::NIL;
    if default_import.is_some() || new_named_imports.is_some() {
        import_clause = ct.node_factory().new_import_clause(
            if is_type_only {
                SyntaxKind::TypeKeyword
            } else {
                SyntaxKind::Unknown
            },
            default_import,
            new_named_imports,
        );
    }
    ct.node_factory().new_import_declaration(
        /*modifiers*/ ModifierList::NIL,
        import_clause,
        module_specifier,
        Node::NIL, /*attributes*/
    )
}

/// Go `unicode.IsUpper`: general category Lu.
// PORT: the Rust `Uppercase` property is Lu plus `Other_Uppercase`; the
// `Other_Uppercase` ranges are removed so the result is Go's category test
// (the same helper as in `util.rs`, which keeps it private).
fn unicode_is_upper(c: char) -> bool {
    if !c.is_uppercase() {
        return false;
    }
    !matches!(
        c as u32,
        0x2160..=0x216F | 0x24B6..=0x24CF | 0x1F130..=0x1F149 | 0x1F150..=0x1F169 | 0x1F170..=0x1F189
    )
}

/// Go `unicode.ToUpper` (simple case mapping).
// PORT: Rust `char::to_uppercase` gives the full mapping. A multi-rune full
// uppercase has no simple mapping, so the rune stays the same (the same
// helper as in `index.rs`, which keeps it private).
fn unicode_to_upper(c: char) -> char {
    let mut u = c.to_uppercase();
    match (u.next(), u.next()) {
        (Some(single), None) => single,
        _ => c,
    }
}

impl View {
    // Go: ls/autoimport/fix.go:554 GetFixes
    // PORT: `ch` is the request checker (see the file header). Go
    // `usagePosition *lsproto.Position` is `Option<lsproto::Position>`.
    pub fn get_fixes(
        &self,
        ch: &mut Checker,
        export: &Export,
        for_jsx: bool,
        is_valid_type_only_use_site: bool,
        usage_position: Option<lsproto::Position>,
    ) -> Vec<Rc<Fix>> {
        let mut fixes: Vec<Rc<Fix>> = Vec::new();
        if let Some(namespace_fix) =
            self.try_use_existing_namespace_import(ch, export, usage_position)
        {
            fixes.push(namespace_fix);
        }

        if let Some(fix) = self.try_add_to_existing_import(ch, export, is_valid_type_only_use_site)
        {
            fixes.push(fix);
            return fixes;
        }

        // !!! getNewImportFromExistingSpecifier - even worth it?

        let (module_specifier, module_specifier_kind) =
            self.get_module_specifier(export, &self.preferences);
        if module_specifier.is_empty() {
            if !fixes.is_empty() {
                return fixes;
            }
            return Vec::new();
        }

        // Check if we need a JSDoc import type fix (for JS files with type-only imports)
        let is_js = tspath::has_js_file_extension(source_file_file_name(self.importing_file));
        let imported_symbol_has_value_meaning =
            export.flags.intersects(SymbolFlags::VALUE) || export.is_unresolved_alias();
        if !imported_symbol_has_value_meaning && is_js && usage_position.is_some() {
            // For pure types in JS files, use JSDoc import type syntax
            return vec![Rc::new(Fix {
                auto_import_fix: lsproto::AutoImportFix {
                    kind: lsproto::AutoImportFixKind::JSDOC_TYPE_IMPORT,
                    module_specifier,
                    name: export.name(),
                    usage_position,
                    ..Default::default()
                },
                module_specifier_kind,
                is_re_export: export.target.module_id != export.module_id,
                module_file_name: export.module_file_name.clone(),
                ..Default::default()
            })];
        }

        let import_kind = get_import_kind(
            self.importing_file,
            export,
            &self.program,
            false, /*forceImportKeyword*/
        );
        let add_as_type_only =
            get_add_as_type_only(is_valid_type_only_use_site, export, self.program.options());

        let mut name = export.name();
        // PORT: Go `rune(name[0])` is the first byte as a rune.
        let starts_with_upper = unicode_is_upper(char::from(name.as_bytes()[0]));
        if for_jsx && !starts_with_upper {
            if export.is_renameable() {
                // PORT: Go `name[1:]` can split a UTF-8 sequence, which a Rust
                // `String` cannot hold; invalid bytes become U+FFFD.
                let mut bytes = unicode_to_upper(char::from(name.as_bytes()[0]))
                    .to_string()
                    .into_bytes();
                bytes.extend_from_slice(&name.as_bytes()[1..]);
                name = String::from_utf8_lossy(&bytes).into_owned();
            } else {
                return Vec::new();
            }
        }

        fixes.push(Rc::new(Fix {
            auto_import_fix: lsproto::AutoImportFix {
                kind: lsproto::AutoImportFixKind::ADD_NEW,
                import_kind,
                module_specifier,
                name,
                use_require: self.should_use_require(),
                add_as_type_only,
                ..Default::default()
            },
            module_specifier_kind,
            is_re_export: export.target.module_id != export.module_id,
            module_file_name: export.module_file_name.clone(),
            ..Default::default()
        }));
        fixes
    }
}

// Go: ls/autoimport/fix.go:623 getAddAsTypeOnly
// getAddAsTypeOnly determines if an import should be type-only based on usage context
fn get_add_as_type_only(
    is_valid_type_only_use_site: bool,
    export: &Export,
    compiler_options: &CompilerOptions,
) -> lsproto::AddAsTypeOnly {
    if !is_valid_type_only_use_site {
        // Can't use a type-only import if the usage is an emitting position
        return lsproto::AddAsTypeOnly::NOT_ALLOWED;
    }
    if compiler_options.verbatim_module_syntax.is_true()
        && (export.is_type_only || !export.flags.intersects(SymbolFlags::VALUE))
        || export.is_type_only && export.flags.intersects(SymbolFlags::VALUE)
    {
        // A type-only import is required for this symbol if under verbatimModuleSyntax and it's purely a type
        return lsproto::AddAsTypeOnly::REQUIRED;
    }
    lsproto::AddAsTypeOnly::ALLOWED
}

impl View {
    // Go: ls/autoimport/fix.go:636 tryUseExistingNamespaceImport
    pub fn try_use_existing_namespace_import(
        &self,
        ch: &mut Checker,
        export: &Export,
        usage_position: Option<lsproto::Position>,
    ) -> Option<Rc<Fix>> {
        if usage_position.is_none() {
            return None;
        }

        if get_import_kind(
            self.importing_file,
            export,
            &self.program,
            false, /*forceImportKeyword*/
        ) != lsproto::ImportKind::NAMED
        {
            return None;
        }

        let existing_imports = self.get_existing_imports(ch);
        let matching_declarations = existing_imports
            .get(&export.module_id)
            .cloned()
            .unwrap_or_default();
        for existing_import in &matching_declarations {
            let namespace_prefix = get_namespace_like_import_text(existing_import.node);
            if namespace_prefix.is_empty() || existing_import.module_specifier.is_empty() {
                continue;
            }
            return Some(Rc::new(Fix {
                auto_import_fix: lsproto::AutoImportFix {
                    kind: lsproto::AutoImportFixKind::USE_NAMESPACE,
                    name: export.name(),
                    module_specifier: existing_import.module_specifier.clone(),
                    import_kind: lsproto::ImportKind::NAMESPACE,
                    add_as_type_only: lsproto::AddAsTypeOnly::ALLOWED,
                    import_index: existing_import.index,
                    usage_position,
                    namespace_prefix,
                    ..Default::default()
                },
                ..Default::default()
            }));
        }

        None
    }
}

// Go: ls/autoimport/fix.go:669 getNamespaceLikeImportText
fn get_namespace_like_import_text(declaration: Node) -> String {
    match declaration.kind() {
        SyntaxKind::VariableDeclaration => {
            let name = declaration.name();
            if name.is_some() && name.kind() == SyntaxKind::Identifier {
                return name.text().to_string();
            }
            String::new()
        }
        SyntaxKind::ImportEqualsDeclaration => declaration.name().text().to_string(),
        SyntaxKind::JsDocImportTag | SyntaxKind::ImportDeclaration => {
            let import_clause = declaration.import_clause();
            if import_clause.is_some()
                && import_clause.named_bindings().is_some()
                && import_clause.named_bindings().kind() == SyntaxKind::NamespaceImport
            {
                return import_clause.named_bindings().name().text().to_string();
            }
            String::new()
        }
        _ => String::new(),
    }
}

impl View {
    // Go: ls/autoimport/fix.go:690 tryAddToExistingImport
    pub fn try_add_to_existing_import(
        &self,
        ch: &mut Checker,
        export: &Export,
        is_valid_type_only_use_site: bool,
    ) -> Option<Rc<Fix>> {
        let existing_imports = self.get_existing_imports(ch);
        let matching_declarations = existing_imports
            .get(&export.module_id)
            .cloned()
            .unwrap_or_default();
        if matching_declarations.is_empty() {
            return None;
        }

        // Can't use an es6 import for a type in JS.
        if is_source_file_js(self.importing_file)
            && !export.flags.intersects(SymbolFlags::VALUE)
            && !matching_declarations
                .iter()
                .all(|i| is_js_doc_import_tag(i.node))
        {
            return None;
        }

        let import_kind = get_import_kind(
            self.importing_file,
            export,
            &self.program,
            false, /*forceImportKeyword*/
        );
        if import_kind == lsproto::ImportKind::COMMON_JS
            || import_kind == lsproto::ImportKind::NAMESPACE
        {
            return None;
        }

        let add_as_type_only =
            get_add_as_type_only(is_valid_type_only_use_site, export, self.program.options());

        let mut best: Option<Rc<Fix>> = None;
        for existing_import in &matching_declarations {
            if existing_import.node.kind() == SyntaxKind::ImportEqualsDeclaration {
                continue;
            }

            if existing_import.node.kind() == SyntaxKind::VariableDeclaration {
                if (import_kind == lsproto::ImportKind::NAMED
                    || import_kind == lsproto::ImportKind::DEFAULT)
                    && existing_import.node.name().kind() == SyntaxKind::ObjectBindingPattern
                {
                    let fix = Rc::new(Fix {
                        auto_import_fix: lsproto::AutoImportFix {
                            kind: lsproto::AutoImportFixKind::ADD_TO_EXISTING,
                            name: export.name(),
                            import_kind,
                            import_index: existing_import.index,
                            module_specifier: existing_import.module_specifier.clone(),
                            add_as_type_only,
                            ..Default::default()
                        },
                        ..Default::default()
                    });
                    // Variable declarations are never type-only.
                    // Give preference to putting types in existing type-only imports and avoiding conversions
                    // of import statements to/from type-only.
                    if add_as_type_only == lsproto::AddAsTypeOnly::NOT_ALLOWED {
                        return Some(fix);
                    }
                    if best.is_none() {
                        best = Some(fix);
                    }
                }
                continue;
            }

            let import_clause_node = existing_import.node.import_clause();
            if import_clause_node.is_nil()
                || !is_string_literal_like(existing_import.node.module_specifier())
            {
                // Side-effect import (no import clause) - can't add to it
                continue;
            }
            let import_clause = import_clause_node;
            // ts#63915: no auto-import into a source phase import (fix.go:751).
            if import_clause.phase_modifier() == SyntaxKind::SourceKeyword {
                continue;
            }

            let named_bindings = import_clause.named_bindings();
            // A type-only import may not have both a default and named imports, so the only way a name can
            // be added to an existing type-only import is adding a named import to existing named bindings.
            if import_clause.is_type_only()
                && !(import_kind == lsproto::ImportKind::NAMED && named_bindings.is_some())
            {
                continue;
            }

            if import_kind == lsproto::ImportKind::DEFAULT
                && (import_clause.name().is_some() ||
                    // Cannot add a default import as type-only if the import already has named bindings
                    add_as_type_only == lsproto::AddAsTypeOnly::REQUIRED && named_bindings.is_some())
            {
                continue;
            }

            // Cannot add a named import to a declaration that has a namespace import
            if import_kind == lsproto::ImportKind::NAMED
                && named_bindings.is_some()
                && named_bindings.kind() == SyntaxKind::NamespaceImport
            {
                continue;
            }

            let fix = Rc::new(Fix {
                auto_import_fix: lsproto::AutoImportFix {
                    kind: lsproto::AutoImportFixKind::ADD_TO_EXISTING,
                    name: export.name(),
                    import_kind,
                    import_index: existing_import.index,
                    module_specifier: existing_import.module_specifier.clone(),
                    add_as_type_only,
                    ..Default::default()
                },
                ..Default::default()
            });

            let is_type_only = import_clause.is_type_only();
            // Give preference to putting types in existing type-only imports and avoiding conversions
            // of import statements to/from type-only.
            if (add_as_type_only != lsproto::AddAsTypeOnly::NOT_ALLOWED && is_type_only)
                || (add_as_type_only == lsproto::AddAsTypeOnly::NOT_ALLOWED && !is_type_only)
            {
                return Some(fix);
            }
            if best.is_none() {
                best = Some(fix);
            }
        }

        best
    }
}

// Go: ls/autoimport/fix.go:796 GetImportKindForImportStatement
// The completions of an import statement (`import F|`) call this. It always
// writes an `import` keyword, so an `export =` module in a JS file without an
// external module indicator gives a default import, not `require`.
pub fn get_import_kind_for_import_statement(
    importing_file: Node,
    export: &Export,
    program: &compiler::NewProgram,
) -> lsproto::ImportKind {
    get_import_kind(
        importing_file,
        export,
        program,
        true, /*forceImportKeyword*/
    )
}

// Go: ls/autoimport/fix.go:800 getImportKind
// PORT: Go `fallthrough` from the Named case into the Modifier case is the
// shared `NAMED` result.
fn get_import_kind(
    importing_file: Node,
    export: &Export,
    program: &compiler::NewProgram,
    force_import_keyword: bool,
) -> lsproto::ImportKind {
    if program.options().verbatim_module_syntax.is_true()
        && program.get_emit_module_format_of_file(&source_file_has_file_name(importing_file))
            == ModuleKind::COMMON_JS
    {
        return lsproto::ImportKind::COMMON_JS;
    }
    match export.syntax {
        ExportSyntax::DEFAULT_MODIFIER | ExportSyntax::DEFAULT_DECLARATION => {
            lsproto::ImportKind::DEFAULT
        }
        ExportSyntax::NAMED => {
            if export.export_name == INTERNAL_SYMBOL_NAME_DEFAULT {
                return lsproto::ImportKind::DEFAULT;
            }
            lsproto::ImportKind::NAMED
        }
        ExportSyntax::MODIFIER | ExportSyntax::STAR | ExportSyntax::COMMON_JS_EXPORTS_PROPERTY => {
            lsproto::ImportKind::NAMED
        }
        ExportSyntax::EQUALS | ExportSyntax::COMMON_JS_MODULE_EXPORTS | ExportSyntax::UMD => {
            // export.Syntax will be ExportSyntaxEquals for named exports/properties of an export='s target.
            if export.export_name != INTERNAL_SYMBOL_NAME_EXPORT_EQUALS {
                return lsproto::ImportKind::NAMED;
            }
            // !!! cache this?
            for statement in importing_file.statements().iter() {
                // `import foo` parses as an ImportEqualsDeclaration even though it could be an ImportDeclaration
                if is_import_equals_declaration(statement)
                    && !node_is_missing(statement.module_reference())
                {
                    return lsproto::ImportKind::COMMON_JS;
                }
            }
            // !!! this logic feels weird; we're basically trying to predict if shouldUseRequire is going to
            //     be true. The meaning of "default import" is different depending on whether we write it as
            //     a require or an es6 import. The latter, compiled to CJS, has interop built in that will
            //     avoid accessing .default, but if we write a require directly and call it a default import,
            //     we emit an unconditional .default access.
            if source_file_info(importing_file)
                .external_module_indicator
                .is_some()
                || force_import_keyword
                || !is_source_file_js(importing_file)
            {
                return lsproto::ImportKind::DEFAULT;
            }
            lsproto::ImportKind::COMMON_JS
        }
        _ => crate::core::go_panic(format!(
            "unhandled export syntax kind: {}",
            export.syntax.string()
        )),
    }
}

// Go: ls/autoimport/fix.go:840 existingImport
#[derive(Clone, Debug, Default)]
pub struct ExistingImport {
    pub node: Node,
    pub module_specifier: String,
    pub index: i32,
}

impl View {
    // Go: ls/autoimport/fix.go:846 getExistingImports
    // PORT: `ch` is Go `v.checker`, passed by the caller (see the file
    // header). Go `collections.MultiMap` is
    // `IndexMap<ModuleID, Vec<ExistingImport>>`.
    pub fn get_existing_imports(
        &self,
        ch: &mut Checker,
    ) -> Rc<IndexMap<ModuleID, Vec<ExistingImport>>> {
        if let Some(existing_imports) = self.existing_imports.borrow().as_ref() {
            return existing_imports.clone();
        }

        let imports = source_file_imports(self.importing_file);
        let mut result: IndexMap<ModuleID, Vec<ExistingImport>> =
            IndexMap::with_capacity(imports.len());

        for (i, module_specifier) in imports.iter().enumerate() {
            let node = try_get_import_from_module_specifier(module_specifier);
            if node.is_nil() {
                crate::core::go_panic(format!(
                    "error: did not expect node kind {}",
                    crate::gostd::debug::kind_string(module_specifier.kind())
                ));
            } else if is_variable_declaration_initialized_to_require(node.parent()) {
                let module_symbol = ch.resolve_external_module_name_exported(
                    module_specifier,
                    TypeId::NIL, /*importAttributesType*/
                );
                if module_symbol.is_some() {
                    let (module_id, _, ok) = try_get_module_id_and_file_name_of_module_symbol(
                        &ch.symbols,
                        module_symbol,
                    );
                    if ok {
                        result.entry(module_id).or_default().push(ExistingImport {
                            node: node.parent(),
                            module_specifier: module_specifier.text().to_string(),
                            index: i as i32,
                        });
                    }
                }
            } else if node.kind() == SyntaxKind::ImportDeclaration
                || node.kind() == SyntaxKind::ImportEqualsDeclaration
                || node.kind() == SyntaxKind::JsDocImportTag
            {
                let module_symbol = ch.get_symbol_at_location_exported(module_specifier);
                if module_symbol.is_some() {
                    let (module_id, _, ok) = try_get_module_id_and_file_name_of_module_symbol(
                        &ch.symbols,
                        module_symbol,
                    );
                    if ok {
                        result.entry(module_id).or_default().push(ExistingImport {
                            node,
                            module_specifier: module_specifier.text().to_string(),
                            index: i as i32,
                        });
                    }
                }
            }
        }
        let result = Rc::new(result);
        *self.existing_imports.borrow_mut() = Some(result.clone());
        result
    }

    // Go: ls/autoimport/fix.go:875 shouldUseRequire
    pub fn should_use_require(&self) -> bool {
        if let Some(should_use_require_for_fixes) = self.should_use_require_for_fixes.get() {
            return should_use_require_for_fixes;
        }
        let should_use_require = self.compute_should_use_require();
        self.should_use_require_for_fixes
            .set(Some(should_use_require));
        should_use_require
    }
}

// Go: ls/autoimport/fix.go:885 fileSyntaxKind
// fileSyntaxKind represents the detected module syntax of a source file.
go_enum!(FileSyntaxKind, i32 {
    AMBIGUOUS = 0; // fileSyntaxKindAmbiguous
    ESM = 1; // fileSyntaxKindESM
    CJS = 2; // fileSyntaxKindCJS
});

// Go: ls/autoimport/fix.go:897 detectSyntax
// detectSyntax returns whether a source file has unambiguous ESM or CJS syntax.
// When moduleDetection is "force", ExternalModuleIndicator may be set to the
// source file node itself rather than a genuine syntax indicator, so we fall back
// to inspecting the file's Imports() to find actual import/export declarations.
fn detect_syntax(file: Node, options: &CompilerOptions) -> FileSyntaxKind {
    let (has_esm, has_cjs) = detect_syntax_indicators(file, options);
    if has_cjs && !has_esm {
        FileSyntaxKind::CJS
    } else if has_esm && !has_cjs {
        FileSyntaxKind::ESM
    } else {
        FileSyntaxKind::AMBIGUOUS
    }
}

// Go: ls/autoimport/fix.go:913 detectSyntaxIndicators
// detectSyntaxIndicators checks whether a source file contains genuine ESM
// and/or CJS syntax. Under moduleDetection "force", the cached
// ExternalModuleIndicator may be the source file itself rather than a real
// statement, so we look at Imports() for actual import/export declarations.
fn detect_syntax_indicators(file: Node, options: &CompilerOptions) -> (bool, bool) {
    let mut has_esm = false;
    let has_cjs = source_file_info(file).common_js_module_indicator.is_some();
    if options.get_emit_module_detection_kind() != ModuleDetectionKind::FORCE {
        // ExternalModuleIndicator is reliable when moduleDetection is not "force"
        has_esm = source_file_info(file).external_module_indicator.is_some();
        return (has_esm, has_cjs);
    }
    // Under moduleDetection "force", ExternalModuleIndicator is set to
    // file.AsNode() when there is no genuine ESM syntax, so only trust it
    // when it points to a real statement node.
    let external_module_indicator = source_file_info(file).external_module_indicator;
    if external_module_indicator.is_some() && external_module_indicator != file {
        return (true, has_cjs);
    }
    // Fall back to scanning Imports() for actual import/export declarations
    // (not require() calls or dynamic imports).
    for imp in source_file_imports(file).iter() {
        if imp.flags().intersects(NodeFlags::SYNTHESIZED) {
            continue;
        }
        let parent = imp.parent();
        if parent.is_nil() {
            continue;
        }
        match parent.kind() {
            SyntaxKind::ImportDeclaration
            | SyntaxKind::JsImportDeclaration
            | SyntaxKind::ExportDeclaration => {
                return (true, has_cjs);
            }
            SyntaxKind::ExternalModuleReference => {
                // import x = require("...") — this is ESM-ish syntax
                return (true, has_cjs);
            }
            _ => {}
        }
    }
    (has_esm, has_cjs)
}

impl View {
    // Go: ls/autoimport/fix.go:947 computeShouldUseRequire
    // PORT: Go `v.program.GetSourceFiles()` gives `*ast.SourceFile` values;
    // here they are `ParsedSourceFile`s and the node is `file.root`.
    pub fn compute_should_use_require(&self) -> bool {
        // 1. TypeScript files don't use require variable declarations
        if !tspath::has_js_file_extension(source_file_file_name(self.importing_file)) {
            return false;
        }

        // 2. If the current source file is unambiguously CJS or ESM, go with that
        match detect_syntax(self.importing_file, self.program.options()) {
            FileSyntaxKind::CJS => return true,
            FileSyntaxKind::ESM => return false,
            _ => {}
        }

        // 3. Use the implied node format to determine CJS vs ESM
        //    TODO: consider removing `impliedNodeFormatForEmit`
        match self
            .program
            .get_implied_node_format_for_emit(&source_file_has_file_name(self.importing_file))
        {
            ModuleKind::COMMON_JS => return true,
            ModuleKind::ES_NEXT => return false,
            _ => {}
        }

        // 4. If there's a tsconfig/jsconfig, use its module setting
        if !self.program.options().config_file_path.is_empty() {
            return self.program.options().get_emit_module_kind() < ModuleKind::ES2015;
        }

        // 5. Match the first other JS file in the program that's unambiguously CJS or ESM
        for other_file in self.program.get_source_files() {
            if other_file.root == self.importing_file
                || !is_source_file_js(other_file.root)
                || self
                    .program
                    .is_source_file_from_external_library(other_file)
            {
                continue;
            }
            match detect_syntax(other_file.root, self.program.options()) {
                FileSyntaxKind::CJS => return true,
                FileSyntaxKind::ESM => return false,
                _ => {}
            }
        }

        // 6. Literally nothing to go on
        true
    }
}

// Go: ls/autoimport/fix.go:993 needsTypeOnly
fn needs_type_only(add_as_type_only: lsproto::AddAsTypeOnly) -> bool {
    add_as_type_only == lsproto::AddAsTypeOnly::REQUIRED
}

// Go: ls/autoimport/fix.go:997 shouldUseTypeOnly
fn should_use_type_only(
    add_as_type_only: lsproto::AddAsTypeOnly,
    preferences: &lsutil::UserPreferences,
) -> bool {
    needs_type_only(add_as_type_only)
        || add_as_type_only != lsproto::AddAsTypeOnly::NOT_ALLOWED
            && preferences.prefer_type_only_auto_imports.is_true()
}

impl View {
    // Go: ls/autoimport/fix.go:1005 CompareFixesForSorting
    // CompareFixesForSorting returns negative if `a` is better than `b`.
    // Sorting with this comparator will place the best fix first.
    // After rank sorting, fixes will be sorted by arbitrary but stable criteria
    // to ensure a deterministic order.
    pub fn compare_fixes_for_sorting(&self, a: &Fix, b: &Fix) -> i32 {
        let res = self.compare_fixes_for_ranking(a, b);
        if res != 0 {
            return res;
        }
        self.compare_module_specifiers_for_sorting(a, b)
    }

    // Go: ls/autoimport/fix.go:1015 CompareFixesForRanking
    // CompareFixesForRanking returns negative if `a` is better than `b`.
    // Sorting with this comparator will place the best fix first.
    // Fixes of equal desirability will be considered equal.
    pub fn compare_fixes_for_ranking(&self, a: &Fix, b: &Fix) -> i32 {
        let res = compare_fix_kinds(a.kind, b.kind);
        if res != 0 {
            return res;
        }
        self.compare_module_specifiers_for_ranking(a, b)
    }
}

// Go: ls/autoimport/fix.go:1022 compareFixKinds
fn compare_fix_kinds(a: lsproto::AutoImportFixKind, b: lsproto::AutoImportFixKind) -> i32 {
    a.0 - b.0
}

impl View {
    // Go: ls/autoimport/fix.go:1026 compareModuleSpecifiersForRanking
    pub fn compare_module_specifiers_for_ranking(&self, a: &Fix, b: &Fix) -> i32 {
        let comparison = compare_module_specifier_relativity(a, b, &self.preferences);
        if comparison != 0 {
            return comparison;
        }
        if a.module_specifier_kind == modulespecifiers::ResultKind::Ambient
            && b.module_specifier_kind == modulespecifiers::ResultKind::Ambient
        {
            let comparison = self.compare_node_core_module_specifiers(
                &a.module_specifier,
                &b.module_specifier,
                self.importing_file,
                &self.program,
            );
            if comparison != 0 {
                return comparison;
            }
        }
        if a.module_specifier_kind == modulespecifiers::ResultKind::Relative
            && b.module_specifier_kind == modulespecifiers::ResultKind::Relative
        {
            let comparison = compare_booleans(
                is_fix_possibly_re_exporting_importing_file(
                    a,
                    source_file_file_name(self.importing_file),
                ),
                is_fix_possibly_re_exporting_importing_file(
                    b,
                    source_file_file_name(self.importing_file),
                ),
            );
            if comparison != 0 {
                return comparison;
            }
        }
        let comparison = tspath::compare_number_of_directory_separators(
            &a.module_specifier,
            &b.module_specifier,
        );
        if comparison != 0 {
            return comparison;
        }
        0
    }

    // Go: ls/autoimport/fix.go:1049 compareModuleSpecifiersForSorting
    pub fn compare_module_specifiers_for_sorting(&self, a: &Fix, b: &Fix) -> i32 {
        let res = self.compare_module_specifiers_for_ranking(a, b);
        if res != 0 {
            return res;
        }
        // Sort ./foo before ../foo for equal-length specifiers
        if a.module_specifier.starts_with("./") && !b.module_specifier.starts_with("./") {
            return -1;
        }
        if b.module_specifier.starts_with("./") && !a.module_specifier.starts_with("./") {
            return 1;
        }
        // Go: strings.Compare (byte order)
        let comparison = a
            .module_specifier
            .as_bytes()
            .cmp(b.module_specifier.as_bytes()) as i32;
        if comparison != 0 {
            return comparison;
        }
        // Go: cmp.Compare
        let comparison = a.import_kind.cmp(&b.import_kind) as i32;
        if comparison != 0 {
            return comparison;
        }
        // !!! further tie-breakers? In practice this is only called on fixes with the same name
        0
    }

    // Go: ls/autoimport/fix.go:1070 compareNodeCoreModuleSpecifiers
    // PORT: Go does not read `importingFile` or `program`.
    pub fn compare_node_core_module_specifiers(
        &self,
        a: &str,
        b: &str,
        _importing_file: Node,
        _program: &compiler::NewProgram,
    ) -> i32 {
        if a.starts_with("node:") && !b.starts_with("node:") {
            if self.should_use_uri_style_node_core_modules.is_true() {
                return -1;
            } else if self.should_use_uri_style_node_core_modules.is_false() {
                return 1;
            }
            return 0;
        }
        if b.starts_with("node:") && !a.starts_with("node:") {
            if self.should_use_uri_style_node_core_modules.is_true() {
                return 1;
            } else if self.should_use_uri_style_node_core_modules.is_false() {
                return -1;
            }
        }
        0
    }
}

// Go: ls/autoimport/fix.go:1094 isFixPossiblyReExportingImportingFile
// This is a simple heuristic to try to avoid creating an import cycle with a barrel re-export.
// E.g., do not `import { Foo } from ".."` when you could `import { Foo } from "../Foo"`.
// This can produce false positives or negatives if re-exports cross into sibling directories
// (e.g. `export * from "../whatever"`) or are not named "index". Technically this should do
// a tspath.Path comparison, but it's not worth it to run a heuristic in such a hot path.
fn is_fix_possibly_re_exporting_importing_file(fix: &Fix, importing_file_name: &str) -> bool {
    if fix.is_re_export && is_index_file_name(&fix.module_file_name) {
        let re_export_dir = tspath::get_directory_path(&fix.module_file_name);
        return importing_file_name
            .starts_with(tspath::ensure_trailing_directory_separator(&re_export_dir).as_str());
    }
    false
}

// Go: ls/autoimport/fix.go:1102 isIndexFileName
fn is_index_file_name(file_name: &str) -> bool {
    let Some(last_slash) = file_name.rfind('/') else {
        return false;
    };
    if file_name.len() <= last_slash + 1 {
        return false;
    }
    let file_name = &file_name[last_slash + 1..];
    matches!(
        file_name,
        "index.js" | "index.jsx" | "index.d.ts" | "index.ts" | "index.tsx"
    )
}

// Go: ls/autoimport/fix.go:1115 promoteFromTypeOnly
fn promote_from_type_only(
    changes: &mut change::Tracker,
    alias_declaration: Node,
    compiler_options: &CompilerOptions,
    source_file: Node,
    preferences: &lsutil::UserPreferences,
) -> Node {
    // See comment in `doAddExistingFix` on constant with the same name.
    let convert_existing_to_type_only = compiler_options.verbatim_module_syntax;

    match alias_declaration.kind() {
        SyntaxKind::ImportSpecifier => {
            let spec = alias_declaration;
            if spec.is_type_only() {
                if spec.parent().is_some() && spec.parent().kind() == SyntaxKind::NamedImports {
                    let named_imports_node = spec.parent();
                    let elements = named_imports_node.elements().to_vec();
                    if elements.len() > 1 {
                        // Create a synthetic specifier with isTypeOnly=false to compute sorted position
                        let mut property_name = Node::NIL;
                        if spec.property_name().is_some() {
                            property_name = changes
                                .node_factory()
                                .new_identifier(spec.property_name().text());
                        }
                        let name = changes.node_factory().new_identifier(spec.name().text());
                        let new_specifier = changes.node_factory().new_import_specifier(
                            false, // isTypeOnly = false
                            property_name,
                            name,
                        );
                        let (specifier_comparer, _) =
                            lsutil::get_named_import_specifier_comparer_with_detection(
                                spec.parent().parent().parent(), // ImportDeclaration
                                source_file,
                                preferences,
                            );
                        let insertion_index = lsutil::get_import_specifier_insertion_index(
                            &elements,
                            new_specifier,
                            &*specifier_comparer,
                        );
                        let current_index = elements
                            .iter()
                            .position(|&e| e == alias_declaration)
                            .map_or(-1, |i| i as i32);
                        if insertion_index != current_index {
                            changes.delete(source_file, alias_declaration);
                            changes.insert_import_specifier_at_index(
                                source_file,
                                new_specifier,
                                spec.parent(),
                                insertion_index,
                            );
                            return alias_declaration;
                        }
                    }
                    // If no re-sorting needed, just remove the 'type' keyword
                    let first_token = lsutil::get_first_token(alias_declaration, source_file);
                    let type_keyword_pos = get_token_pos_of_node(first_token, source_file, false);
                    let target_node = if spec.property_name().is_some() {
                        spec.property_name()
                    } else {
                        spec.name()
                    };
                    let target_pos = get_token_pos_of_node(target_node, source_file, false);
                    changes.delete_range(source_file, TextRange::new(type_keyword_pos, target_pos));
                }
                alias_declaration
            } else {
                // The parent import clause is type-only
                if spec.parent().is_nil() || spec.parent().kind() != SyntaxKind::NamedImports {
                    crate::core::go_panic(
                        "ImportSpecifier parent must be NamedImports".to_string(),
                    );
                }
                if spec.parent().parent().is_nil()
                    || spec.parent().parent().kind() != SyntaxKind::ImportClause
                {
                    crate::core::go_panic("NamedImports parent must be ImportClause".to_string());
                }
                promote_import_clause(
                    changes,
                    spec.parent().parent(),
                    compiler_options,
                    source_file,
                    preferences,
                    convert_existing_to_type_only,
                    alias_declaration,
                );
                spec.parent().parent()
            }
        }

        SyntaxKind::ImportClause => {
            promote_import_clause(
                changes,
                alias_declaration,
                compiler_options,
                source_file,
                preferences,
                convert_existing_to_type_only,
                alias_declaration,
            );
            alias_declaration
        }

        SyntaxKind::NamespaceImport => {
            // Promote the parent import clause
            if alias_declaration.parent().is_nil()
                || alias_declaration.parent().kind() != SyntaxKind::ImportClause
            {
                crate::core::go_panic("NamespaceImport parent must be ImportClause".to_string());
            }
            promote_import_clause(
                changes,
                alias_declaration.parent(),
                compiler_options,
                source_file,
                preferences,
                convert_existing_to_type_only,
                alias_declaration,
            );
            alias_declaration.parent()
        }

        SyntaxKind::ImportEqualsDeclaration => {
            // Remove the 'type' keyword (which is the second token: 'import' 'type' name '=' ...)
            let import_eq_decl = alias_declaration;
            // The type keyword is after 'import' and before the name
            let sf_text = source_file_text(source_file);
            let mut scan = scanner_ls::get_scanner_for_source_file(
                source_file,
                &sf_text,
                import_eq_decl.pos(),
            );
            // Skip 'import' keyword to get to 'type'
            scan.scan();
            delete_type_keyword(changes, source_file, scan.token_start());
            alias_declaration
        }
        kind => crate::core::go_panic(format!(
            "Unexpected alias declaration kind: {}",
            crate::gostd::debug::kind_string(kind)
        )),
    }
}

// Go: ls/autoimport/fix.go:1208 promoteImportClause
// promoteImportClause removes the type keyword from an import clause
fn promote_import_clause(
    changes: &mut change::Tracker,
    import_clause: Node,
    compiler_options: &CompilerOptions,
    source_file: Node,
    preferences: &lsutil::UserPreferences,
    convert_existing_to_type_only: Tristate,
    alias_declaration: Node,
) {
    // Delete the 'type' keyword
    if import_clause.phase_modifier() == SyntaxKind::TypeKeyword {
        delete_type_keyword(changes, source_file, import_clause.pos());
    }

    // Handle .ts extension conversion to .js if necessary
    if compiler_options.allow_importing_ts_extensions.is_false() {
        let module_specifier = try_get_module_specifier_from_declaration(import_clause.parent());
        if module_specifier.is_some() {
            // Note: We can't check ResolvedUsingTsExtension without program, so we'll skip this optimization
            // The fix will still work, just might not change .ts to .js extensions in all cases
        }
    }

    // Handle verbatimModuleSyntax conversion
    // If convertExistingToTypeOnly is true, we need to add 'type' to other specifiers
    // in the same import declaration
    if convert_existing_to_type_only.is_true() {
        let named_imports = import_clause.named_bindings();
        if named_imports.is_some() && named_imports.kind() == SyntaxKind::NamedImports {
            let elements = named_imports.elements().to_vec();
            if elements.len() > 1 {
                // Check if the list is sorted and if we need to reorder
                let (_, is_sorted) = lsutil::get_named_import_specifier_comparer_with_detection(
                    import_clause.parent(),
                    source_file,
                    preferences,
                );

                // If the alias declaration is an ImportSpecifier and the list is sorted,
                // move it to index 0 (since it will be the only non-type-only import)
                if !is_sorted.is_false() && // isSorted !== false
                    alias_declaration.is_some() &&
                    alias_declaration.kind() == SyntaxKind::ImportSpecifier
                {
                    // Find the index of the alias declaration
                    let mut alias_index: i32 = -1;
                    for (i, &element) in elements.iter().enumerate() {
                        if element == alias_declaration {
                            alias_index = i as i32;
                            break;
                        }
                    }
                    // If not already at index 0, move it there
                    if alias_index > 0 {
                        // Delete the specifier from its current position
                        changes.delete(source_file, alias_declaration);
                        // Insert it at index 0
                        changes.insert_import_specifier_at_index(
                            source_file,
                            alias_declaration,
                            named_imports,
                            0,
                        );
                    }
                }

                // Add 'type' keyword to all other import specifiers that aren't already type-only
                for &element in &elements {
                    let spec = element;
                    // Skip the specifier being promoted (if aliasDeclaration is an ImportSpecifier)
                    if alias_declaration.is_some()
                        && alias_declaration.kind() == SyntaxKind::ImportSpecifier
                        && element == alias_declaration
                    {
                        continue;
                    }
                    // Skip if already type-only
                    if !spec.is_type_only() {
                        changes.insert_modifier_before(
                            source_file,
                            SyntaxKind::TypeKeyword,
                            element,
                        );
                    }
                }
            }
        }
    }
}

// Go: ls/autoimport/fix.go:1289 deleteTypeKeyword
// deleteTypeKeyword deletes the 'type' keyword token starting at the given position,
// including any trailing whitespace.
fn delete_type_keyword(changes: &mut change::Tracker, source_file: Node, start_pos: i32) {
    let sf_text = source_file_text(source_file);
    let scan = scanner_ls::get_scanner_for_source_file(source_file, &sf_text, start_pos);
    if scan.token() != SyntaxKind::TypeKeyword {
        return;
    }
    let type_start = scan.token_start();
    let mut type_end = scan.token_end();
    // Skip trailing whitespace
    let text_text = source_file_text(source_file);
    let text = text_text.as_bytes();
    while (type_end as usize) < text.len()
        && (text[type_end as usize] == b' ' || text[type_end as usize] == b'\t')
    {
        type_end += 1;
    }
    changes.delete_range(source_file, TextRange::new(type_start, type_end));
}

// Go: ls/autoimport/fix.go:1304 getModuleSpecifierText
fn get_module_specifier_text(promoted_declaration: Node) -> String {
    if promoted_declaration.kind() == SyntaxKind::ImportEqualsDeclaration {
        let import_equals_declaration = promoted_declaration;
        if is_external_module_reference(import_equals_declaration.module_reference()) {
            let expr = import_equals_declaration.module_reference().expression();
            if expr.is_some() {
                if is_string_literal_like(expr) {
                    return expr.text().to_string();
                }
                return get_text_of_node(expr);
            }
        }
        return get_text_of_node(import_equals_declaration.module_reference());
    }
    let module_specifier = promoted_declaration.parent().module_specifier();
    if is_string_literal_like(module_specifier) {
        return module_specifier.text().to_string();
    }
    get_text_of_node(module_specifier)
}

// Go: ls/autoimport/fix.go:1326 compareModuleSpecifierRelativity
// returns `-1` if `a` is better than `b`
fn compare_module_specifier_relativity(
    a: &Fix,
    b: &Fix,
    preferences: &modulespecifiers::UserPreferences,
) -> i32 {
    match preferences.import_module_specifier_preference {
        modulespecifiers::ImportModuleSpecifierPreference::NonRelative
        | modulespecifiers::ImportModuleSpecifierPreference::ProjectRelative => compare_booleans(
            a.module_specifier_kind == modulespecifiers::ResultKind::Relative,
            b.module_specifier_kind == modulespecifiers::ResultKind::Relative,
        ),
        _ => 0,
    }
}

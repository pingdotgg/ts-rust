use crate::ls::prelude::*;

// Port of Go `ls/string_completions.go`.
//
// PORT (whole file):
// - Go `*compiler.Program` is `&compiler::NewProgram` (plan
//   D-PROGRAM). Package.json lookups and the typings cache location are
//   `NewProgram` methods, run on the dispatch thread.
// - Go passes `program` where a `modulespecifiers.ModuleSpecifierGenerationHost`
//   is needed. The host for the installed program is
//   `modulespecifiers::ProgramHost` (installed program, plan D-LS1).
// - Go `*ast.SourceFile` passed as an `ast.HasFileName` is
//   `new_has_file_name(file name, path)` (Go `ast.NewHasFileName`).
// - Go `file.Path()` is `source_file_info(file).path` (a `String`).
// - `*checker.StringLiteralType` is `TypeId`. Go `t.AsLiteralType().Value().(string)`
//   panics when the value is not a string; so does the Rust `let ... else`.
// - Go `moduleCompletionNameAndKindSet.names` is a Go map, and the results are
//   collected with `slices.Collect(maps.Values(...))`. It is an `IndexMap`
//   in insertion order. PORT: Go map order is random; the oracle compares
//   that output without order.
// - Go `fromPaths *pathCompletions` (tsgo#4712) is `Option<PathCompletions>`.
//   It is set even when it has no entries.
// - Go byte-offset string cuts whose boundary is not guaranteed by a prefix
//   check use `String::from_utf8_lossy` on the bytes (Go keeps raw bytes;
//   Rust text must stay valid UTF-8).

use crate::astnav;
use crate::flags_macros::go_enum;
use crate::frontend::scanner::get_leading_comment_ranges;
use crate::frontend::stringutil_ls::{COMPARISON_GREATER_THAN, COMPARISON_LESS_THAN, Comparison};
use crate::frontend::{compiler, module, packagejson, tsoptions, tspath};
use crate::gostd::Context;
use crate::ls::lsutil;
use crate::lsp::lsproto;
use crate::modulespecifiers;
use crate::spanmap::Fidelity;

// Go: ls/string_completions.go:30 completionsFromTypes
#[derive(Clone, Debug, Default)]
struct CompletionsFromTypes {
    types: Vec<TypeId>,
    is_new_identifier: bool,
}

// Go: ls/string_completions.go:35 completionsFromProperties
#[derive(Clone, Debug, Default)]
struct CompletionsFromProperties {
    symbols: Vec<SymbolId>,
    has_index_signature: bool,
}

// Go: ls/string_completions.go:40 pathCompletion
#[derive(Clone, Debug)]
struct PathCompletion {
    name: String,
    kind: lsutil::ScriptElementKind,
    extension: String,
}

// Go: ls/string_completions.go:46 pathCompletions
// PORT: Go `replacementSpan *lsproto.Range`; nil is `None`.
#[derive(Clone, Debug, Default)]
struct PathCompletions {
    entries: Vec<PathCompletion>,
    replacement_span: Option<lsproto::Range>,
}

// Go: ls/string_completions.go:51 stringLiteralCompletions
// PORT: Go `*T` fields are `Option<T>`.
#[derive(Clone, Debug, Default)]
struct StringLiteralCompletions {
    from_types: Option<CompletionsFromTypes>,
    from_properties: Option<CompletionsFromProperties>,
    from_paths: Option<PathCompletions>,
}

impl LanguageService {
    // Go: ls/string_completions.go:57 getStringLiteralCompletions
    pub fn get_string_literal_completions(
        &self,
        ctx: &Context,
        file: Node,
        position: i32,
        context_token: Node,
        checker: &mut Checker,
        compiler_options: &CompilerOptions,
        include_symbols: bool,
    ) -> Option<CompletionList> {
        if is_in_reference_comment(file, position) {
            let completion = self.get_triple_slash_reference_completions(
                file,
                position,
                self.get_program(),
                checker,
            );
            return self.convert_path_completions(ctx, completion.as_ref(), file, position);
        }
        if is_in_string(file, position, context_token) {
            if context_token.is_nil() || !is_string_literal_like(context_token) {
                return None;
            }
            let entries = self.get_string_literal_completion_entries(
                ctx,
                file,
                context_token,
                position,
                checker,
            );
            return self.convert_string_literal_completions(
                ctx,
                entries,
                context_token,
                file,
                position,
                checker,
                compiler_options,
                include_symbols,
            );
        }
        None
    }

    // Go: ls/string_completions.go:95 convertStringLiteralCompletions
    fn convert_string_literal_completions(
        &self,
        ctx: &Context,
        completion: Option<StringLiteralCompletions>,
        context_token: Node,
        file: Node,
        position: i32,
        type_checker: &mut Checker,
        options: &CompilerOptions,
        include_symbols: bool,
    ) -> Option<CompletionList> {
        let Some(completion) = completion else {
            return None;
        };

        let optional_replacement_range =
            self.create_range_from_string_literal_like_content(file, context_token, position);
        if completion.from_paths.is_some() {
            return self.convert_path_completions(
                ctx,
                completion.from_paths.as_ref(),
                file,
                position,
            );
        } else if let Some(completion) = completion.from_properties {
            // PORT: every `completionDataData` field is written; the ones Go
            // leaves out get their Go zero value.
            let data = CompletionDataData {
                symbols: completion.symbols,
                auto_imports: Vec::new(),
                completion_kind: CompletionKind::STRING,
                is_in_snippet_scope: false,
                property_access_to_convert: Node::NIL,
                is_new_identifier_location: completion.has_index_signature,
                location: file,
                keyword_filters: KeywordCompletionFilters::NONE,
                literals: Vec::new(),
                symbol_to_origin_info_map: FxHashMap::default(),
                symbol_to_sort_text_map: FxHashMap::default(),
                recommended_completion: SymbolId::NIL,
                previous_token: Node::NIL,
                context_token,
                jsx_initializer: JsxInitializer::default(),
                inside_js_doc_tag_type_expression: false,
                is_type_only_location: false,
                is_jsx_identifier_expected: false,
                is_right_of_open_tag: false,
                is_right_of_dot_or_question_dot: false,
                import_statement_completion: None,
                has_unresolved_auto_imports: false,
                default_commit_characters: None,
            };
            let (_, mut items) = match self.get_completion_entries_from_symbols(
                ctx,
                type_checker,
                &data,
                context_token, /*replacementToken*/
                position,
                file,
                options,
                include_symbols, /*includeSymbols*/
            ) {
                Ok(result) => result,
                Err(err) => crate::core::go_panic(err.error()),
            };
            let default_commit_characters =
                get_default_commit_characters(completion.has_index_signature);
            let item_defaults = self.set_item_defaults(
                ctx,
                position,
                file,
                &mut items,
                Some(&default_commit_characters),
                optional_replacement_range,
            );
            return Some(CompletionList {
                is_incomplete: false,
                item_defaults,
                apply_kind: None,
                items,
            });
        } else if let Some(completion) = completion.from_types {
            let quote_char = if context_token.kind() == SyntaxKind::NoSubstitutionTemplateLiteral {
                QuoteChar::BACKTICK
            } else if context_token.text().starts_with('\'') {
                QuoteChar::SINGLE_QUOTE
            } else {
                QuoteChar::DOUBLE_QUOTE
            };
            let mut items: Vec<CompletionItem> = completion
                .types
                .iter()
                .map(|&t| {
                    let Some(LiteralValue::String(value)) =
                        type_checker.ty(t).as_literal_type().value()
                    else {
                        panic!("interface conversion: interface {{}} is not string");
                    };
                    let name = escape_string(value, quote_char);
                    let lsp_item = self.create_lsp_completion_item(
                        ctx,
                        &name,
                        "", /*insertText*/
                        "", /*filterText*/
                        SORT_TEXT_LOCATION_PRIORITY,
                        lsutil::ScriptElementKind::STRING,
                        lsutil::ScriptElementKindModifier::NONE,
                        self.get_replacement_range_for_context_token(file, context_token, position),
                        None, /*commitCharacters*/
                        None, /*labelDetails*/
                        file,
                        position,
                        false, /*isMemberCompletion*/
                        false, /*isSnippet*/
                        false, /*hasAction*/
                        false, /*preselect*/
                        "",    /*source*/
                        None,  /*autoImportEntryData*/
                        None,  /*additionalTextEdits*/
                        None,  /*detail*/
                    );
                    CompletionItem {
                        completion_item: lsp_item,
                        symbol: SymbolId::NIL,
                    }
                })
                .collect();
            let default_commit_characters =
                get_default_commit_characters(completion.is_new_identifier);
            let item_defaults = self.set_item_defaults(
                ctx,
                position,
                file,
                &mut items,
                Some(&default_commit_characters),
                None, /*optionalReplacementSpan*/
            );
            return Some(CompletionList {
                is_incomplete: false,
                item_defaults,
                apply_kind: None,
                items,
            });
        }
        None
    }

    // Go: ls/string_completions.go:206 convertPathCompletions
    // PORT: Go `completion *pathCompletions`; nil is `None`.
    fn convert_path_completions(
        &self,
        ctx: &Context,
        completion: Option<&PathCompletions>,
        file: Node,
        position: i32,
    ) -> Option<CompletionList> {
        let Some(completion) = completion else {
            return None;
        };
        let is_new_identifier_location = true; // The user may type in a path that doesn't yet exist, creating a "new identifier" with respect to the collection of identifiers the server is aware of.
        let default_commit_characters = get_default_commit_characters(is_new_identifier_location);
        let mut items: Vec<CompletionItem> = completion
            .entries
            .iter()
            .map(|path_completion| {
                let mut detail = path_completion.name.clone();
                if !path_completion
                    .name
                    .ends_with(path_completion.extension.as_str())
                {
                    detail += &path_completion.extension;
                }
                let lsp_item = self.create_lsp_completion_item(
                    ctx,
                    &path_completion.name,
                    "", /*insertText*/
                    "", /*filterText*/
                    SORT_TEXT_LOCATION_PRIORITY,
                    path_completion.kind,
                    kind_modifiers_from_extension(&path_completion.extension),
                    completion.replacement_span,
                    None, /*commitCharacters*/
                    None, /*labelDetails*/
                    file,
                    position,
                    false, /*isMemberCompletion*/
                    false, /*isSnippet*/
                    false, /*hasAction*/
                    false, /*preselect*/
                    "",    /*source*/
                    None,  /*autoImportEntryData*/
                    None,  /*additionalTextEdits*/
                    Some(detail),
                );
                CompletionItem {
                    completion_item: lsp_item,
                    symbol: SymbolId::NIL,
                }
            })
            .collect();
        let item_defaults = self.set_item_defaults(
            ctx,
            position,
            file,
            &mut items,
            Some(&default_commit_characters),
            None, /*optionalReplacementSpan*/
        );
        Some(CompletionList {
            is_incomplete: false,
            item_defaults,
            apply_kind: None,
            items,
        })
    }

    // Go: ls/string_completions.go:263 getStringLiteralCompletionEntries
    fn get_string_literal_completion_entries(
        &self,
        _ctx: &Context,
        file: Node,
        node: Node,
        position: i32,
        type_checker: &mut Checker,
    ) -> Option<StringLiteralCompletions> {
        let parent = walk_up_parentheses(node.parent());
        match parent.kind() {
            SyntaxKind::LiteralType => {
                let grandparent = walk_up_parentheses(parent.parent());
                if grandparent.kind() == SyntaxKind::ImportType {
                    return self.get_string_literal_completions_from_module_names(
                        file,
                        node,
                        self.get_program(),
                        type_checker,
                    );
                }
                from_unionable_literal_type(grandparent, parent, position, type_checker)
            }
            SyntaxKind::PropertyAssignment => {
                if is_object_literal_expression(parent.parent()) && parent.name() == node {
                    // Get quoted name of properties of the object literal expression
                    // i.e. interface ConfigFiles {
                    //          'jspm:dev': string
                    //      }
                    //      let files: ConfigFiles = {
                    //          '/*completion position*/'
                    //      }
                    //
                    //      function foo(c: ConfigFiles) {}
                    //      foo({
                    //          '/*completion position*/'
                    //      });
                    return Some(StringLiteralCompletions {
                        from_properties: string_literal_completions_for_object_literal(
                            type_checker,
                            parent.parent(),
                        ),
                        ..Default::default()
                    });
                }
                if find_ancestor(parent.parent(), is_call_like_expression).is_some() {
                    let mut uniques = FxHashSet::default();
                    let t = type_checker.get_contextual_type_exported(node, ContextFlags::NONE);
                    let mut string_literal_types =
                        get_string_literal_types(t, Some(&mut uniques), type_checker);
                    let t = type_checker
                        .get_contextual_type_exported(node, ContextFlags::IGNORE_NODE_INFERENCES);
                    string_literal_types.extend(get_string_literal_types(
                        t,
                        Some(&mut uniques),
                        type_checker,
                    ));
                    return to_string_literal_completions_from_types(string_literal_types);
                }
                Some(StringLiteralCompletions {
                    from_types: from_contextual_type(ContextFlags::NONE, node, type_checker),
                    ..Default::default()
                })
            }
            SyntaxKind::ElementAccessExpression => {
                let expression = parent.expression();
                let argument_expression = parent.argument_expression();
                if node == skip_parentheses(argument_expression) {
                    // Get all names of properties on the expression
                    // i.e. interface A {
                    //      'prop1': string
                    // }
                    // let a: A;
                    // a['/*completion position*/']
                    let t = type_checker.get_type_at_location(expression);
                    return Some(StringLiteralCompletions {
                        from_properties: Some(string_literal_completions_from_properties(
                            t,
                            type_checker,
                        )),
                        ..Default::default()
                    });
                }
                None
            }
            // PORT: Go lists `CallExpression`, `NewExpression` and `JsxAttribute`
            // in their own case and falls through to the module name case when
            // the string is a `require`/`import` argument. Both cases share
            // this arm; the first `if` is the Go first case.
            SyntaxKind::CallExpression
            | SyntaxKind::NewExpression
            | SyntaxKind::JsxAttribute
            | SyntaxKind::ImportDeclaration
            | SyntaxKind::ExportDeclaration
            | SyntaxKind::ExternalModuleReference
            | SyntaxKind::JsDocImportTag => {
                if matches!(
                    parent.kind(),
                    SyntaxKind::CallExpression
                        | SyntaxKind::NewExpression
                        | SyntaxKind::JsxAttribute
                ) && !is_require_call_argument(node)
                    && !is_import_call(parent)
                {
                    let argument_node = if parent.kind() == SyntaxKind::JsxAttribute {
                        parent.parent()
                    } else {
                        node
                    };
                    let argument_info = get_argument_info_for_completions(
                        argument_node,
                        position,
                        file,
                        type_checker,
                    );
                    // Get string literal completions from specialized signatures of the target
                    // i.e. declare function f(a: 'A');
                    // f("/*completion position*/")
                    let Some(argument_info) = argument_info else {
                        return None;
                    };

                    let result = get_string_literal_completions_from_signature(
                        argument_info.invocation,
                        node,
                        &argument_info,
                        type_checker,
                    );
                    if result.is_some() {
                        return Some(StringLiteralCompletions {
                            from_types: result,
                            ..Default::default()
                        });
                    }
                    return Some(StringLiteralCompletions {
                        from_types: from_contextual_type(ContextFlags::NONE, node, type_checker),
                        ..Default::default()
                    });
                }
                // fallthrough: is `require("")` or `require(""` or `import("")`

                // Get all known external module names or complete a path to a module
                // i.e. import * as ns from "/*completion position*/";
                //      var y = import("/*completion position*/");
                //      import x = require("/*completion position*/");
                //      var y = require("/*completion position*/");
                //      export * from "/*completion position*/";
                self.get_string_literal_completions_from_module_names(
                    file,
                    node,
                    self.get_program(),
                    type_checker,
                )
            }
            SyntaxKind::CaseClause => {
                let clauses = parent.parent().clauses().nodes().to_vec();
                let tracker = new_case_clause_tracker(type_checker, &clauses);
                let contextual_types =
                    from_contextual_type(ContextFlags::IGNORE_NODE_INFERENCES, node, type_checker);
                let Some(contextual_types) = contextual_types else {
                    return None;
                };
                let literals: Vec<TypeId> = contextual_types
                    .types
                    .iter()
                    .copied()
                    .filter(|&t| {
                        // PORT: Go passes the `any` value; nil makes `hasValue`
                        // panic with this text.
                        let value =
                            type_checker
                                .ty(t)
                                .as_literal_type()
                                .value()
                                .unwrap_or_else(|| {
                                    crate::core::go_panic("Unsupported type: <nil>".to_string())
                                });
                        !tracker.has_value(value)
                    })
                    .collect();
                Some(StringLiteralCompletions {
                    from_types: Some(CompletionsFromTypes {
                        types: literals,
                        is_new_identifier: false,
                    }),
                    ..Default::default()
                })
            }
            SyntaxKind::ImportSpecifier | SyntaxKind::ExportSpecifier => {
                // Complete string aliases in `import { "|" } from` and `export { "|" } from`
                let specifier = parent;
                let property_name = specifier.property_name();
                if property_name.is_some() && node != property_name {
                    return None; // Don't complete in `export { "..." as "|" } from`
                }
                let named_imports_or_exports = specifier.parent();
                let module_specifier =
                    if named_imports_or_exports.kind() == SyntaxKind::NamedImports {
                        named_imports_or_exports.parent().parent()
                    } else {
                        named_imports_or_exports.parent()
                    };
                if module_specifier.is_nil() {
                    return None;
                }
                let module_specifier_symbol =
                    type_checker.get_symbol_at_location_exported(module_specifier);
                if module_specifier_symbol.is_nil() {
                    return None;
                }
                let exports =
                    type_checker.get_exports_and_properties_of_module(module_specifier_symbol);
                let existing: FxHashSet<String> = named_imports_or_exports
                    .elements()
                    .iter()
                    .map(|n| n.property_name_or_name().text().to_string())
                    .collect();
                let uniques: Vec<SymbolId> = exports
                    .into_iter()
                    .filter(|&e| {
                        let name = type_checker.sym(e).name.as_str();
                        name != INTERNAL_SYMBOL_NAME_DEFAULT && !existing.contains(name)
                    })
                    .collect();
                Some(StringLiteralCompletions {
                    from_properties: Some(CompletionsFromProperties {
                        symbols: uniques,
                        has_index_signature: false,
                    }),
                    ..Default::default()
                })
            }
            SyntaxKind::BinaryExpression => {
                if parent.operator_token().kind() == SyntaxKind::InKeyword {
                    let t = type_checker.get_type_at_location(parent.right());
                    let properties = get_properties_for_completion(t, type_checker);
                    return Some(StringLiteralCompletions {
                        from_properties: Some(CompletionsFromProperties {
                            symbols: properties
                                .into_iter()
                                .filter(|&s| {
                                    let value_declaration = type_checker.sym(s).value_declaration;
                                    value_declaration.is_nil()
                                        || !is_private_identifier_class_element_declaration(
                                            value_declaration,
                                        )
                                })
                                .collect(),
                            has_index_signature: false,
                        }),
                        ..Default::default()
                    });
                }
                Some(StringLiteralCompletions {
                    from_types: from_contextual_type(ContextFlags::NONE, node, type_checker),
                    ..Default::default()
                })
            }
            _ => {
                let result =
                    from_contextual_type(ContextFlags::IGNORE_NODE_INFERENCES, node, type_checker);
                if result.is_some() {
                    return Some(StringLiteralCompletions {
                        from_types: result,
                        ..Default::default()
                    });
                }
                Some(StringLiteralCompletions {
                    from_types: from_contextual_type(ContextFlags::NONE, node, type_checker),
                    ..Default::default()
                })
            }
        }
    }
}

// Go: ls/string_completions.go:440 fromContextualType
fn from_contextual_type(
    context_flags: ContextFlags,
    node: Node,
    type_checker: &mut Checker,
) -> Option<CompletionsFromTypes> {
    // Get completion for string literal from string literal type
    // i.e. var x: "hi" | "hello" = "/*completion position*/"
    let t = get_contextual_type_from_parent(node, type_checker, context_flags);
    to_completions_from_types(get_string_literal_types(t, None, type_checker))
}

// Go: ls/string_completions.go:446 toCompletionsFromTypes
fn to_completions_from_types(types: Vec<TypeId>) -> Option<CompletionsFromTypes> {
    if types.is_empty() {
        return None;
    }
    Some(CompletionsFromTypes {
        types,
        is_new_identifier: false,
    })
}

// Go: ls/string_completions.go:456 toStringLiteralCompletionsFromTypes
fn to_string_literal_completions_from_types(
    types: Vec<TypeId>,
) -> Option<StringLiteralCompletions> {
    let result = to_completions_from_types(types)?;
    Some(StringLiteralCompletions {
        from_types: Some(result),
        ..Default::default()
    })
}

// Go: ls/string_completions.go:466 fromUnionableLiteralType
fn from_unionable_literal_type(
    grandparent: Node,
    parent: Node,
    position: i32,
    type_checker: &mut Checker,
) -> Option<StringLiteralCompletions> {
    match grandparent.kind() {
        SyntaxKind::CallExpression
        | SyntaxKind::ExpressionWithTypeArguments
        | SyntaxKind::JsxOpeningElement
        | SyntaxKind::JsxSelfClosingElement
        | SyntaxKind::NewExpression
        | SyntaxKind::TaggedTemplateExpression
        | SyntaxKind::TypeReference => {
            let type_argument = find_ancestor(parent, |n| n.parent() == grandparent);
            if type_argument.is_some() {
                let t = type_checker.get_type_argument_constraint_exported(type_argument);
                return Some(StringLiteralCompletions {
                    from_types: Some(CompletionsFromTypes {
                        types: get_string_literal_types(t, None, type_checker),
                        is_new_identifier: false,
                    }),
                    ..Default::default()
                });
            }
            None
        }
        SyntaxKind::IndexedAccessType => {
            // Get all apparent property names
            // i.e. interface Foo {
            //          foo: string;
            //          bar: string;
            //      }
            //      let x: Foo["/*completion position*/"]
            let index_type = grandparent.index_type();
            let object_type = grandparent.object_type();
            if !index_type.loc().contains_inclusive(position) {
                return None;
            }
            let t = type_checker.get_type_from_type_node_exported(object_type);
            Some(StringLiteralCompletions {
                from_properties: Some(string_literal_completions_from_properties(t, type_checker)),
                ..Default::default()
            })
        }
        SyntaxKind::UnionType => {
            let result = from_unionable_literal_type(
                walk_up_parentheses(grandparent.parent()),
                parent,
                position,
                type_checker,
            );
            let Some(result) = result else {
                return None;
            };
            let already_used_types =
                get_already_used_types_in_string_literal_union(grandparent, parent);
            if let Some(result) = result.from_properties {
                Some(StringLiteralCompletions {
                    from_properties: Some(CompletionsFromProperties {
                        symbols: result
                            .symbols
                            .into_iter()
                            .filter(|&s| {
                                let name = type_checker.sym(s).name.as_str();
                                !already_used_types.iter().any(|used| used == name)
                            })
                            .collect(),
                        has_index_signature: result.has_index_signature,
                    }),
                    ..Default::default()
                })
            } else if let Some(result) = result.from_types {
                Some(StringLiteralCompletions {
                    from_types: Some(CompletionsFromTypes {
                        types: result
                            .types
                            .into_iter()
                            .filter(|&t| {
                                let Some(LiteralValue::String(value)) =
                                    type_checker.ty(t).as_literal_type().value()
                                else {
                                    panic!("interface conversion: interface {{}} is not string");
                                };
                                !already_used_types.iter().any(|used| used == value)
                            })
                            .collect(),
                        is_new_identifier: false,
                    }),
                    ..Default::default()
                })
            } else {
                None
            }
        }
        SyntaxKind::PropertySignature => {
            let t = get_constraint_of_type_argument_property(grandparent, type_checker);
            Some(StringLiteralCompletions {
                from_types: Some(CompletionsFromTypes {
                    types: get_string_literal_types(t, None, type_checker),
                    is_new_identifier: false,
                }),
                ..Default::default()
            })
        }
        _ => None,
    }
}

// Go: ls/string_completions.go:555 stringLiteralCompletionsForObjectLiteral
fn string_literal_completions_for_object_literal(
    type_checker: &mut Checker,
    object_literal_expression: Node,
) -> Option<CompletionsFromProperties> {
    let contextual_type =
        type_checker.get_contextual_type_exported(object_literal_expression, ContextFlags::NONE);
    if contextual_type.is_nil() {
        return None;
    }

    let completions_type = type_checker.get_contextual_type_exported(
        object_literal_expression,
        ContextFlags::IGNORE_NODE_INFERENCES,
    );
    let symbols = get_properties_for_object_expression(
        contextual_type,
        completions_type,
        object_literal_expression,
        type_checker,
    );

    Some(CompletionsFromProperties {
        symbols,
        has_index_signature: has_index_signature(contextual_type, type_checker),
    })
}

// Go: ls/string_completions.go:578 stringLiteralCompletionsFromProperties
fn string_literal_completions_from_properties(
    t: TypeId,
    type_checker: &mut Checker,
) -> CompletionsFromProperties {
    let symbols = type_checker
        .get_apparent_properties(t)
        .into_iter()
        .filter(|&s| {
            let value_declaration = type_checker.sym(s).value_declaration;
            !(value_declaration.is_some()
                && is_private_identifier_class_element_declaration(value_declaration))
        })
        .collect();
    CompletionsFromProperties {
        symbols,
        has_index_signature: has_index_signature(t, type_checker),
    }
}

impl LanguageService {
    // Go: ls/string_completions.go:587 getStringLiteralCompletionsFromModuleNames
    // PORT: Go returns `*stringLiteralCompletions`; nil is `None`.
    fn get_string_literal_completions_from_module_names(
        &self,
        file: Node,
        node: Node,
        program: &compiler::NewProgram,
        checker: &mut Checker,
    ) -> Option<StringLiteralCompletions> {
        let (replacement_span, ok) =
            self.path_completion_replacement_span(file, module_name_fragment_range(file, node));
        if !ok {
            return None;
        }
        let name_and_kinds = self
            .get_string_literal_completions_from_module_names_worker(file, node, program, checker);
        Some(StringLiteralCompletions {
            from_paths: Some(PathCompletions {
                entries: to_path_completions(name_and_kinds),
                replacement_span,
            }),
            ..Default::default()
        })
    }
}

/// The range of the module name `node` of `file` that the path completions
/// replace: Go `getDirectoryFragmentRange(node.Text(), textStart)` in
/// `getStringLiteralCompletionsFromModuleNames` (string_completions.go:593).
// PORT: Go adds byte offsets of `node.Text()`, the literal's value, to
// `textStart`, a byte offset of the source. The value's port form can differ
// from the source's (an escape, a WTF-8 surrogate fused into one unit), and
// a marker unit (see `GO_STRING_MARKER`) has more port bytes than Go bytes.
// So the range is Go's in Go bytes, then port offsets of the source.
fn module_name_fragment_range(file: Node, node: Node) -> Option<TextRange> {
    let text_start = astnav::get_start_of_node(node, file, false /*includeJSDoc*/) + 1;
    let source = source_file_text(file);
    get_directory_fragment_range(
        &go_string_bytes(node.text()),
        go_byte_offset(&source, text_start),
    )
    .map(|range| {
        TextRange::new(
            port_byte_offset(&source, range.pos()),
            port_byte_offset(&source, range.end()),
        )
    })
}

// Go: ls/string_completions.go:612 toPathCompletions
fn to_path_completions(names: Vec<ModuleCompletionNameAndKind>) -> Vec<PathCompletion> {
    names
        .into_iter()
        .map(|name_and_kind| PathCompletion {
            name: name_and_kind.name,
            kind: modulet_to_script_element_kind(name_and_kind.kind),
            extension: name_and_kind.extension,
        })
        .collect()
}

impl LanguageService {
    // Go: ls/string_completions.go:622 pathCompletionReplacementSpan
    // PORT: Go `textRange *core.TextRange` and the `*lsproto.Range` result;
    // nil is `None`.
    fn path_completion_replacement_span(
        &self,
        file: Node,
        text_range: Option<TextRange>,
    ) -> (Option<lsproto::Range>, bool) {
        let Some(text_range) = text_range else {
            return (None, true);
        };
        let (lsp_range, fidelity): (lsproto::Range, Fidelity) =
            self.create_lsp_range_from_bounds(text_range.pos(), text_range.end(), file);
        if !fidelity.is_exact() {
            return (None, false);
        }
        (Some(lsp_range), true)
    }
}

// Go: ls/string_completions.go:633 moduletToScriptElementKind
fn modulet_to_script_element_kind(kind: ModuleCompletionKind) -> lsutil::ScriptElementKind {
    match kind {
        ModuleCompletionKind::DIRECTORY => return lsutil::ScriptElementKind::DIRECTORY,
        ModuleCompletionKind::FILE => return lsutil::ScriptElementKind::SCRIPT_ELEMENT,
        ModuleCompletionKind::EXTERNAL_MODULE_NAME => {
            return lsutil::ScriptElementKind::EXTERNAL_MODULE_NAME;
        }
        _ => {}
    }
    crate::core::go_panic(format!("Unknown moduleCompletionKind: {}", kind.0));
}

// Go: ls/string_completions.go:645 isAnyDirectorySeparator
fn is_any_directory_separator(r: char) -> bool {
    r == '/' || r == '\\'
}

// Go: ls/string_completions.go:650 getDirectoryFragmentRange
// Replace everything after the last directory separator that appears
// PORT: Go works on the bytes of `text`. `text` is the Go bytes
// (`go_string_bytes`), and `text_start` and the range are Go byte offsets.
// A separator is ASCII, so the last separator byte is Go's last separator
// rune.
fn get_directory_fragment_range(text: &[u8], text_start: i32) -> Option<TextRange> {
    let index = text
        .iter()
        .rposition(|&b| is_any_directory_separator(char::from(b)));
    let mut offset = 0;
    if let Some(index) = index {
        offset = index + 1;
    }
    let length = text.len() - offset;
    if length == 0 {
        return None;
    }
    Some(TextRange::new(
        text_start + offset as i32,
        text_start + offset as i32 + length as i32,
    ))
}

impl LanguageService {
    // Go: ls/string_completions.go:663 getStringLiteralCompletionsFromModuleNamesWorker
    fn get_string_literal_completions_from_module_names_worker(
        &self,
        file: Node,
        node: Node,
        program: &compiler::NewProgram,
        checker: &mut Checker,
    ) -> Vec<ModuleCompletionNameAndKind> {
        let literal_value = tspath::normalize_slashes(node.text());
        let mut mode = RESOLUTION_MODE_NONE;
        if is_string_literal_like(node) {
            mode = program.get_mode_for_usage_location(
                &new_has_file_name(source_file_file_name(file), &source_file_info(file).path),
                node,
            );
        }

        let script_path = tspath::Path(source_file_info(file).path.clone());
        let script_directory = script_path.get_directory_path();
        let options = program.options();
        let extension_options = self.get_extension_options(
            options,
            ReferenceKind::MODULE_SPECIFIER,
            file,
            mode,
            Some(&mut *checker),
        );

        if is_path_relative_to_script(&literal_value)
            || (options.paths.as_ref().map_or(0, |p| p.len()) == 0
                && (tspath::is_rooted_disk_path(&literal_value) || tspath::is_url(&literal_value)))
        {
            self.get_completion_entries_for_relative_modules(
                &literal_value,
                script_directory.as_str(),
                program,
                &script_path,
                &extension_options,
            )
        } else {
            self.get_completion_entries_for_non_relative_modules(
                &literal_value,
                script_directory.as_str(),
                mode,
                program,
                checker,
                &extension_options,
            )
        }
    }

    // Go: ls/string_completions.go:707 getCompletionEntriesForNonRelativeModules
    // Check all of the declared modules and those in node modules. Possible sources of modules:
    //
    //	Modules that are found by the type checker
    //	Modules found via patterns from "paths" compiler option
    //	Modules from node_modules (i.e. those listed in package.json)
    //	    This includes all files that are found in node_modules/moduleName/ with acceptable file extensions
    //
    // PORT: the Go closures share `result` and `seenPackageScope`. The
    // closures here take them as `&mut` parameters. Go reassigns
    // `ancestorLookup` to a wrapper that calls the old value; the Rust
    // variable is a boxed `FnMut` that is replaced the same way.
    fn get_completion_entries_for_non_relative_modules(
        &self,
        fragment: &str,
        script_path: &str,
        mode: ResolutionMode,
        program: &compiler::NewProgram,
        type_checker: &mut Checker,
        extension_options: &ExtensionOptions,
    ) -> Vec<ModuleCompletionNameAndKind> {
        let compiler_options = program.options();
        let paths = &compiler_options.paths;

        let mut result = ModuleCompletionNameAndKindSet::default();
        let module_resolution = compiler_options.get_module_resolution_kind();

        if let Some(paths) = paths
            && !paths.is_empty()
        {
            let absolute = compiler_options.get_paths_base_path(&program.get_current_directory());
            self.add_completion_entries_from_paths(
                &mut result,
                program,
                fragment,
                &absolute,
                extension_options,
                paths,
            );
        }

        let fragment_directory = get_fragment_directory(fragment);
        for ambient_name in
            get_ambient_module_completions(fragment, &fragment_directory, type_checker)
        {
            result.add(ModuleCompletionNameAndKind {
                name: ambient_name,
                kind: ModuleCompletionKind::EXTERNAL_MODULE_NAME,
                extension: String::new(),
            });
        }

        self.get_completion_entries_from_typings(
            program,
            script_path,
            &fragment_directory,
            extension_options,
            &mut result,
        );

        if module_resolution_uses_node_modules(module_resolution) {
            // If looking for a global package name, don't just include everything in `node_modules` because that includes dependencies' own dependencies.
            // (But do if we didn't find anything, e.g. 'package.json' missing.)
            let mut found_global = false;
            if fragment_directory.is_empty() {
                for module_name in self.enumerate_node_modules_visible_to_script(script_path) {
                    let module_result = ModuleCompletionNameAndKind {
                        name: module_name,
                        kind: ModuleCompletionKind::EXTERNAL_MODULE_NAME,
                        extension: String::new(),
                    };
                    if !result.names.contains_key(&module_result.name) {
                        found_global = true;
                        result.add(module_result);
                    }
                }
            }
            if !found_global {
                let resolve_package_json_exports =
                    compiler_options.get_resolve_package_json_exports();
                let resolve_package_json_imports =
                    compiler_options.get_resolve_package_json_imports();
                let mut seen_package_scope = false;
                let conditions = module::get_conditions(compiler_options, mode);

                // Returns true if the search should stop.
                let exports_or_imports_lookup = &|result: &mut ModuleCompletionNameAndKindSet,
                                                  lookup_table: Option<
                    &packagejson::ExportsOrImports,
                >,
                                                  fragment: &str,
                                                  base_directory: &str,
                                                  is_exports: bool,
                                                  is_imports: bool|
                 -> bool {
                    let lookup_table = match lookup_table {
                        Some(lookup_table)
                            if lookup_table.type_ == packagejson::JSONValueType::OBJECT =>
                        {
                            lookup_table
                        }
                        _ => {
                            return lookup_table.is_some_and(|lookup_table| {
                                lookup_table.type_ != packagejson::JSONValueType::NOT_PRESENT
                            });
                        }
                    };
                    let keys: Vec<String> = lookup_table.as_object().keys().cloned().collect();
                    self.add_completion_entries_from_paths_or_exports_or_imports(
                        result,
                        program,
                        is_exports,
                        is_imports,
                        fragment,
                        base_directory,
                        extension_options,
                        keys,
                        &|key: &str| -> Vec<String> {
                            let Some(key_value) = lookup_table.as_object().get(key) else {
                                return Vec::new();
                            };
                            let pattern =
                                get_pattern_from_first_matching_condition(key_value, &conditions);
                            if pattern.is_empty() {
                                return Vec::new();
                            }
                            if key.ends_with('/') && pattern.ends_with('/') {
                                return vec![pattern + "*"];
                            }
                            vec![pattern]
                        },
                        &module::compare_pattern_keys,
                    );
                    true
                };

                let imports_lookup = &|result: &mut ModuleCompletionNameAndKindSet,
                                       seen_package_scope: &mut bool,
                                       directory: &str| {
                    if resolve_package_json_imports && !*seen_package_scope {
                        let package_file = tspath::combine_paths(directory, &["package.json"]);
                        let package_json_info = program.get_package_json_info(&package_file);
                        if let Some(package_json_info) = package_json_info
                            && package_json_info.exists()
                        {
                            *seen_package_scope = true;
                            // PORT: `Exists()` means `Contents` is set.
                            let contents = package_json_info.get_contents().unwrap();
                            exports_or_imports_lookup(
                                result,
                                Some(&contents.fields.path_fields.imports),
                                fragment,
                                directory,
                                false, /*isExports*/
                                true,  /*isImports*/
                            );
                        }
                    }
                };

                let mut ancestor_lookup: Box<
                    dyn FnMut(&mut ModuleCompletionNameAndKindSet, &mut bool, &str) -> ((), bool)
                        + '_,
                > = Box::new(
                    move |result: &mut ModuleCompletionNameAndKindSet,
                          seen_package_scope: &mut bool,
                          ancestor: &str|
                          -> ((), bool) {
                        let node_modules = tspath::combine_paths(ancestor, &["node_modules"]);
                        if self.host.directory_exists(&node_modules) {
                            self.get_completion_entries_for_directory_fragment(
                                fragment,
                                &node_modules,
                                extension_options,
                                program,
                                false, /* moduleSpecifierIsRelative */
                                "",
                                result,
                            );
                        }
                        imports_lookup(result, seen_package_scope, ancestor);
                        ((), false)
                    },
                );

                if !fragment_directory.is_empty() && resolve_package_json_exports {
                    let mut node_modules_directory_or_imports_lookup = ancestor_lookup;
                    ancestor_lookup = Box::new(
                        move |result: &mut ModuleCompletionNameAndKindSet,
                              seen_package_scope: &mut bool,
                              ancestor: &str|
                              -> ((), bool) {
                            let all_components = tspath::get_path_components(fragment, "");
                            let mut components: &[String] = &all_components[1..]; // shift off empty root
                            if components.is_empty() {
                                node_modules_directory_or_imports_lookup(
                                    result,
                                    seen_package_scope,
                                    ancestor,
                                );
                                return ((), false);
                            }
                            let mut package_path = components[0].clone();
                            components = &components[1..];
                            if package_path.starts_with('@') {
                                if components.is_empty() {
                                    node_modules_directory_or_imports_lookup(
                                        result,
                                        seen_package_scope,
                                        ancestor,
                                    );
                                    return ((), false);
                                }
                                let sub_name = &components[0];
                                components = &components[1..];
                                package_path = tspath::combine_paths(&package_path, &[sub_name]);
                            }
                            if resolve_package_json_imports && package_path.starts_with('#') {
                                imports_lookup(result, seen_package_scope, ancestor);
                                return ((), false);
                            }
                            let package_directory =
                                tspath::combine_paths(ancestor, &["node_modules", &package_path]);
                            let package_file =
                                tspath::combine_paths(&package_directory, &["package.json"]);
                            let package_json_info = program.get_package_json_info(&package_file);
                            if let Some(package_json_info) = package_json_info
                                && package_json_info.exists()
                            {
                                let mut fragment_subpath = components.join("/");
                                if !components.is_empty()
                                    && tspath::has_trailing_directory_separator(fragment)
                                {
                                    fragment_subpath += "/";
                                }
                                // PORT: `Exists()` means `Contents` is set.
                                let contents = package_json_info.get_contents().unwrap();
                                if exports_or_imports_lookup(
                                    result,
                                    Some(&contents.fields.path_fields.exports),
                                    &fragment_subpath,
                                    &package_directory,
                                    true,  /*isExports*/
                                    false, /*isImports*/
                                ) {
                                    return ((), false);
                                }
                            }
                            node_modules_directory_or_imports_lookup(
                                result,
                                seen_package_scope,
                                ancestor,
                            );
                            ((), false)
                        },
                    );
                }

                let global_cache_location = program.get_global_typings_cache_location();
                tspath::for_each_ancestor_directory_stopping_at_global_cache(
                    &global_cache_location,
                    script_path,
                    |ancestor| ancestor_lookup(&mut result, &mut seen_package_scope, ancestor),
                );
            }
        }

        // PORT: Go map order is random; the oracle compares that output without order.
        result.names.into_values().collect()
    }
}

// Go: ls/string_completions.go:875 getFragmentDirectory
fn get_fragment_directory(fragment: &str) -> String {
    if !contains_slash(fragment) {
        return String::new();
    }
    if tspath::has_trailing_directory_separator(fragment) {
        return fragment.to_string();
    }
    tspath::get_directory_path(fragment)
}

// Go: ls/string_completions.go:885 getPatternFromFirstMatchingCondition
fn get_pattern_from_first_matching_condition(
    target: &packagejson::ExportsOrImports,
    conditions: &[String],
) -> String {
    if target.type_ == packagejson::JSONValueType::STRING {
        return target.as_string().to_string();
    }
    if target.type_ == packagejson::JSONValueType::OBJECT {
        let obj = target.as_object();
        for condition in obj.keys() {
            if condition == "default"
                || conditions.iter().any(|c| c == condition)
                || (conditions.iter().any(|c| c == "types")
                    && module::is_applicable_versioned_types_key(condition))
            {
                if let Some(pattern) = obj.get(condition) {
                    return get_pattern_from_first_matching_condition(pattern, conditions);
                }
            }
        }
    }
    String::new()
}

// Go: ls/string_completions.go:904 getAmbientModuleCompletions
fn get_ambient_module_completions(
    fragment: &str,
    fragment_directory: &str,
    type_checker: &mut Checker,
) -> Vec<String> {
    let ambient_modules = type_checker.get_ambient_modules();
    let mut non_relative_module_names = Vec::new();
    for sym in ambient_modules {
        let module_name = get_ambient_module_name(&type_checker.symbols, sym);
        // PORT: Go `strings.HasPrefix` on Go bytes (`go_has_prefix`). The
        // fragment of a closed file can end in the raw bytes of a cut char,
        // which are a prefix of the char's bytes in Go only.
        if go_has_prefix(&module_name, fragment) && !module_name.contains('*') {
            non_relative_module_names.push(module_name);
        }
    }

    if !fragment_directory.is_empty() {
        let module_name_with_separator =
            tspath::ensure_trailing_directory_separator(fragment_directory);
        for module_name in non_relative_module_names.iter_mut() {
            if let Some(rest) = module_name.strip_prefix(module_name_with_separator.as_str()) {
                *module_name = rest.to_string();
            }
        }
    }
    non_relative_module_names
}

// Go: ls/string_completions.go:923 getAmbientModuleName
// PORT: reading the symbol takes the symbol arena (as for Go `ast`
// functions that take a symbol).
fn get_ambient_module_name(symbols: &SymbolArena, symbol: SymbolId) -> String {
    let declaration = get_non_augmentation_declaration(symbols, symbol);
    if declaration.is_some() && is_module_with_string_literal_name(declaration) {
        return declaration.name().text().to_string();
    }
    strip_quotes(symbols.sym(symbol).name.as_str())
}

impl LanguageService {
    // Go: ls/string_completions.go:931 getCompletionEntriesFromTypings
    fn get_completion_entries_from_typings(
        &self,
        program: &compiler::NewProgram,
        script_path: &str,
        fragment_directory: &str,
        extension_options: &ExtensionOptions,
        result: &mut ModuleCompletionNameAndKindSet,
    ) {
        let options = program.options();
        let mut seen: FxHashMap<String, bool> = FxHashMap::default();

        let (type_roots, _) = options.get_effective_type_roots(&program.get_current_directory());

        for root in &type_roots {
            self.get_completion_entries_from_typings_directories(
                root,
                options,
                fragment_directory,
                extension_options,
                program,
                &mut seen,
                result,
            );
        }

        let global_cache_location = program.get_global_typings_cache_location();
        tspath::for_each_ancestor_directory_stopping_at_global_cache(
            &global_cache_location,
            script_path,
            |directory| {
                let types_dir = tspath::combine_paths(directory, &["node_modules/@types"]);
                self.get_completion_entries_from_typings_directories(
                    &types_dir,
                    options,
                    fragment_directory,
                    extension_options,
                    program,
                    &mut seen,
                    result,
                );
                ((), false)
            },
        );
    }

    // Go: ls/string_completions.go:955 getCompletionEntriesFromTypingsDirectories
    fn get_completion_entries_from_typings_directories(
        &self,
        directory: &str,
        options: &CompilerOptions,
        fragment_directory: &str,
        extension_options: &ExtensionOptions,
        program: &compiler::NewProgram,
        seen: &mut FxHashMap<String, bool>,
        result: &mut ModuleCompletionNameAndKindSet,
    ) {
        if !self.host.directory_exists(directory) {
            return;
        }

        for type_directory_name in self.get_directories(directory) {
            let package_name = module::unmangle_scoped_package_name(&type_directory_name);
            if let Some(types) = &options.types
                && !types.is_empty()
                && !types.contains(&package_name)
            {
                continue;
            }

            if fragment_directory.is_empty() {
                if !seen.get(&package_name).copied().unwrap_or(false) {
                    result.add(ModuleCompletionNameAndKind {
                        name: package_name.clone(),
                        kind: ModuleCompletionKind::EXTERNAL_MODULE_NAME,
                        extension: String::new(),
                    });
                    seen.insert(package_name, true);
                }
            } else {
                let base_directory = tspath::combine_paths(directory, &[&type_directory_name]);
                let remaining_fragment = try_remove_directory_prefix(
                    fragment_directory,
                    &package_name,
                    program.use_case_sensitive_file_names(),
                );
                if let Some(remaining_fragment) = remaining_fragment {
                    self.get_completion_entries_for_directory_fragment(
                        &remaining_fragment,
                        &base_directory,
                        extension_options,
                        program,
                        false,
                        "",
                        result,
                    );
                }
            }
        }
    }
}

// Go: ls/string_completions.go:1000 tryRemoveDirectoryPrefix
// tsgo#4900: `TrimFilePathPrefix` cuts the prefix by runes, not by its byte
// length.
fn try_remove_directory_prefix(
    path: &str,
    prefix: &str,
    use_case_sensitive_file_names: bool,
) -> Option<String> {
    let without_prefix =
        tspath::trim_file_path_prefix(path, prefix, use_case_sensitive_file_names)?;
    if without_prefix.starts_with('/') || without_prefix.starts_with('\\') {
        return Some(without_prefix[1..].to_string());
    }
    Some(without_prefix.into_owned())
}

impl LanguageService {
    // Go: ls/string_completions.go:1011 enumerateNodeModulesVisibleToScript
    fn enumerate_node_modules_visible_to_script(&self, script_path: &str) -> Vec<String> {
        let mut result = Vec::new();
        let global_cache_location = self.program.get_global_typings_cache_location();

        tspath::for_each_ancestor_directory_stopping_at_global_cache(
            &global_cache_location,
            script_path,
            |directory| {
                let package_json_path = tspath::combine_paths(directory, &["package.json"]);
                let package_json_info = self.program.get_package_json_info(&package_json_path);
                if let Some(package_json_info) = package_json_info
                    && package_json_info.exists()
                    && let Some(contents) = package_json_info.get_contents()
                {
                    contents
                        .fields
                        .range_dependencies(|name, _version, _dependency_field| {
                            if !name.starts_with("@types/") {
                                result.push(name.to_string());
                            }
                            true
                        });
                }
                ((), false)
            },
        );

        result
    }

    // Go: ls/string_completions.go:1032 getExtensionOptions
    // PORT: Go `checker *checker.Checker` can be nil: `Option<&mut Checker>`.
    fn get_extension_options(
        &self,
        options: &CompilerOptions,
        reference_kind: ReferenceKind,
        file: Node,
        mode: ResolutionMode,
        checker: Option<&mut Checker>,
    ) -> ExtensionOptions {
        let extensions_to_search = get_supported_extensions_for_module_resolution(
            options,
            &self
                .get_program()
                .command_line()
                .content_mapper_extensions(),
            checker,
        );

        ExtensionOptions {
            extensions_to_search,
            reference_kind,
            importing_source_file: file,
            ending_preference: self.user_preferences().import_module_specifier_ending,
            resolution_mode: mode,
        }
    }
}

// Go: ls/string_completions.go:1050 getSupportedExtensionsForModuleResolution
fn get_supported_extensions_for_module_resolution(
    options: &CompilerOptions,
    extra_extensions: &[String],
    checker: Option<&mut Checker>,
) -> Vec<String> {
    /* file extensions from ambient modules declarations e.g. *.css */
    let mut extensions: Vec<String> = Vec::new();
    if let Some(checker) = checker {
        let ambient_modules = checker.get_ambient_modules();
        for module in ambient_modules {
            let name = get_ambient_module_name(&checker.symbols, module);
            if !name.starts_with("*.") || name.contains('/') {
                continue;
            }
            extensions.push(name[1..].to_string());
        }
    }
    let supported_extensions = tsoptions::get_supported_extensions(options, extra_extensions);
    for ext in supported_extensions {
        extensions.extend(ext);
    }
    let module_resolution = options.get_module_resolution_kind();
    if module_resolution_uses_node_modules(module_resolution) {
        return tsoptions::get_supported_extensions_with_json_if_resolve_json_module(
            Some(options),
            vec![extensions],
        )
        .into_iter()
        .flatten()
        .collect();
    }
    extensions
}

// Go: ls/string_completions.go:1074 moduleResolutionUsesNodeModules
fn module_resolution_uses_node_modules(module_resolution: ModuleResolutionKind) -> bool {
    module_resolution >= ModuleResolutionKind::NODE16
        && module_resolution <= ModuleResolutionKind::NODE_NEXT
        || module_resolution == ModuleResolutionKind::BUNDLER
}

// Go: ls/string_completions.go:1080 isPathRelativeToScript
// Returns true if the path is explicitly relative (i.e. relative to . or ..)
fn is_path_relative_to_script(path: &str) -> bool {
    path.starts_with("./") || path.starts_with("../")
}

impl LanguageService {
    // Go: ls/string_completions.go:1084 getCompletionEntriesForRelativeModules
    fn get_completion_entries_for_relative_modules(
        &self,
        literal_value: &str,
        script_directory: &str,
        program: &compiler::NewProgram,
        script_path: &tspath::Path,
        extension_options: &ExtensionOptions,
    ) -> Vec<ModuleCompletionNameAndKind> {
        let options = program.options();
        if let Some(root_dirs) = &options.root_dirs
            && !root_dirs.is_empty()
        {
            self.get_completion_entries_for_directory_fragment_with_root_dirs(
                root_dirs,
                literal_value,
                script_directory,
                program,
                script_path.as_str(),
                extension_options,
            )
        } else {
            let mut result = ModuleCompletionNameAndKindSet::default();
            self.get_completion_entries_for_directory_fragment(
                literal_value,
                script_directory,
                extension_options,
                program,
                true, /*moduleSpecifierIsRelative*/
                script_path.as_str(),
                &mut result,
            );
            // PORT: Go map order is random; the oracle compares that output without order.
            result.names.into_values().collect()
        }
    }

    // Go: ls/string_completions.go:1115 getCompletionEntriesForDirectoryFragmentWithRootDirs
    fn get_completion_entries_for_directory_fragment_with_root_dirs(
        &self,
        root_dirs: &[String],
        fragment: &str,
        script_directory: &str,
        program: &compiler::NewProgram,
        exclude: &str,
        extension_options: &ExtensionOptions,
    ) -> Vec<ModuleCompletionNameAndKind> {
        let options = program.options();
        let base_path = if !options.project.is_empty() {
            options.project.clone()
        } else {
            program.get_current_directory()
        };
        let ignore_case = !program.use_case_sensitive_file_names();
        let base_directories = get_base_directories_from_root_dirs(
            root_dirs,
            &base_path,
            script_directory,
            ignore_case,
        );

        let mut all_completions = Vec::new();
        for base_directory in &base_directories {
            let mut result = ModuleCompletionNameAndKindSet::default();
            self.get_completion_entries_for_directory_fragment(
                fragment,
                base_directory,
                extension_options,
                program,
                true, /*moduleSpecifierIsRelative*/
                exclude,
                &mut result,
            );
            // PORT: Go map order is random; the oracle compares that output without order.
            for entry in result.names.into_values() {
                all_completions.push(entry);
            }
        }

        // Deduplicate based on name, kind, and extension
        deduplicate_module_completions(all_completions)
    }
}

// Go: ls/string_completions.go:1155 getBaseDirectoriesFromRootDirs
// getBaseDirectoriesFromRootDirs takes a script path and returns paths for all potential folders
// that could be merged with its containing folder via the "rootDirs" compiler option.
fn get_base_directories_from_root_dirs(
    root_dirs: &[String],
    base_path: &str,
    script_directory: &str,
    ignore_case: bool,
) -> Vec<String> {
    // Make all paths absolute/normalized if they are not already
    let normalized_root_dirs: Vec<String> = root_dirs
        .iter()
        .map(|root_directory| {
            let normalized_path = if tspath::is_rooted_disk_path(root_directory) {
                root_directory.clone()
            } else {
                tspath::combine_paths(base_path, &[root_directory])
            };
            tspath::ensure_trailing_directory_separator(&tspath::normalize_path(&normalized_path))
        })
        .collect();

    // Determine the path to the directory containing the script relative to the root directory it is contained within
    let mut relative_directory = String::new();
    let compare_paths_options = tspath::ComparePathsOptions {
        use_case_sensitive_file_names: !ignore_case,
        current_directory: base_path.to_string(),
    };
    for root_directory in &normalized_root_dirs {
        if tspath::contains_path(root_directory, script_directory, &compare_paths_options) {
            if root_directory.len() > script_directory.len() {
                relative_directory = String::new();
            } else {
                // PORT: Go `scriptDirectory[len(rootDirectory):]` cuts bytes; see the file header.
                relative_directory =
                    String::from_utf8_lossy(&script_directory.as_bytes()[root_directory.len()..])
                        .into_owned();
            }
            break;
        }
    }

    // Now find a path for each potential directory that is to be merged with the one containing the script
    let mut directories = Vec::new();
    for root_directory in &normalized_root_dirs {
        directories.push(
            tspath::remove_trailing_directory_separator(&tspath::combine_paths(
                root_directory,
                &[&relative_directory],
            ))
            .to_string(),
        );
    }
    directories.push(tspath::remove_trailing_directory_separator(script_directory).to_string());

    deduplicate_strings(directories)
}

// Go: ls/string_completions.go:1195 deduplicateStrings
fn deduplicate_strings(slice: Vec<String>) -> Vec<String> {
    if slice.len() <= 1 {
        return slice;
    }
    let mut seen: FxHashSet<String> = FxHashSet::default();
    let mut result = Vec::new();
    for s in slice {
        if !seen.contains(&s) {
            seen.insert(s.clone());
            result.push(s);
        }
    }
    result
}

// Go: ls/string_completions.go:1210 deduplicateModuleCompletions
fn deduplicate_module_completions(
    completions: Vec<ModuleCompletionNameAndKind>,
) -> Vec<ModuleCompletionNameAndKind> {
    if completions.len() <= 1 {
        return completions;
    }
    #[derive(PartialEq, Eq, Hash)]
    struct Key {
        name: String,
        kind: ModuleCompletionKind,
        extension: String,
    }
    let mut seen: FxHashSet<Key> = FxHashSet::default();
    let mut result = Vec::new();
    for c in completions {
        let k = Key {
            name: c.name.clone(),
            kind: c.kind,
            extension: c.extension.clone(),
        };
        if !seen.contains(&k) {
            seen.insert(k);
            result.push(c);
        }
    }
    result
}

// Go: ls/string_completions.go:1231 moduleCompletionKind
go_enum!(ModuleCompletionKind, i32 {
    DIRECTORY = 0; // moduleCompletionKindDirectory
    FILE = 1; // moduleCompletionKindFile
    EXTERNAL_MODULE_NAME = 2; // moduleCompletionKindExternalModuleName
});

// Go: ls/string_completions.go:1239 moduleCompletionNameAndKind
#[derive(Clone, Debug, Default)]
struct ModuleCompletionNameAndKind {
    name: String,
    kind: ModuleCompletionKind,
    extension: String,
}

// Go: ls/string_completions.go:1245 moduleCompletionNameAndKindSet
// PORT: Go map; `IndexMap` in insertion order (see the file header).
#[derive(Clone, Debug, Default)]
struct ModuleCompletionNameAndKindSet {
    names: IndexMap<String, ModuleCompletionNameAndKind>,
}

impl ModuleCompletionNameAndKindSet {
    // Go: ls/string_completions.go:1249 add
    fn add(&mut self, entry: ModuleCompletionNameAndKind) {
        let existing = self.names.get(&entry.name);
        if existing.is_none_or(|existing| existing.kind < entry.kind) {
            self.names.insert(entry.name.clone(), entry);
        }
    }
}

// Go: ls/string_completions.go:1256 extensionOptions
#[derive(Clone, Debug)]
struct ExtensionOptions {
    extensions_to_search: Vec<String>,
    reference_kind: ReferenceKind,
    importing_source_file: Node,
    ending_preference: modulespecifiers::ImportModuleSpecifierEndingPreference,
    resolution_mode: ResolutionMode,
}

// Go: ls/string_completions.go:1264 referenceKind
go_enum!(ReferenceKind, i32 {
    FILE_NAME = 0; // referenceKindFileName
    MODULE_SPECIFIER = 1; // referenceKindModuleSpecifier
});

impl LanguageService {
    // Go: ls/string_completions.go:1272 getCompletionEntriesForDirectoryFragment
    // Given a path ending at a directory, gets the completions for the path.
    // PORT: Go returns `result`, the pointer it was given. Callers read the set
    // they passed in, so this returns nothing.
    fn get_completion_entries_for_directory_fragment(
        &self,
        fragment: &str,
        script_directory: &str,
        extension_options: &ExtensionOptions,
        program: &compiler::NewProgram,
        module_specifier_is_relative: bool,
        exclude: &str,
        result: &mut ModuleCompletionNameAndKindSet,
    ) {
        let mut fragment = tspath::normalize_slashes(fragment);

        // Remove the basename from the path.
        // We don't use the basename to filter completions: the client is responsible for that filtering.
        if !tspath::has_trailing_directory_separator(&fragment) {
            fragment = tspath::get_directory_path(&fragment);
        }

        if fragment.is_empty() {
            fragment = ".".to_string();
        }

        fragment = tspath::ensure_trailing_directory_separator(&fragment);

        let base_directory = tspath::resolve_path(script_directory, &[&fragment]);
        if !module_specifier_is_relative {
            // Check for a version redirect.
            let package_json_directory =
                program.get_nearest_ancestor_directory_with_package_json(&base_directory);
            if !package_json_directory.is_empty() {
                let package_json_path =
                    tspath::combine_paths(&package_json_directory, &["package.json"]);
                let package_json_info = program.get_package_json_info(&package_json_path);
                if let Some(package_json_info) = package_json_info
                    && let Some(contents) = package_json_info.get_contents()
                    && contents.fields.path_fields.types_versions.type_
                        == packagejson::JSONValueType::OBJECT
                {
                    let version_paths = contents.get_version_paths(None);
                    let paths = version_paths.get_paths();
                    if let Some(paths) = paths
                        && !paths.is_empty()
                    {
                        // PORT: Go `baseDirectory[len(...):]` cuts bytes; see the file header.
                        let path_in_package = String::from_utf8_lossy(
                            &base_directory.as_bytes()
                                [tspath::ensure_trailing_directory_separator(
                                    &package_json_directory,
                                )
                                .len()..],
                        )
                        .into_owned();
                        if self.add_completion_entries_from_paths(
                            result,
                            program,
                            &path_in_package,
                            &package_json_directory,
                            extension_options,
                            paths,
                        ) {
                            // One of the `versionPaths` was matched, which will block relative resolution
                            // to files and folders from here.
                            // All reachable paths given the pattern match are already added.
                            return;
                        }
                    }
                }
            }
        }

        if !self.host.directory_exists(&base_directory) {
            return;
        }

        // Enumerate all available files.
        let files = self.read_directory(
            &base_directory,
            &extension_options.extensions_to_search,
            &["./*".to_string()], /*include*/
        );

        for file_path in &files {
            if tspath::compare_paths(
                exclude,
                file_path,
                &tspath::ComparePathsOptions {
                    use_case_sensitive_file_names: program.use_case_sensitive_file_names(),
                    current_directory: program.get_current_directory(),
                },
            ) == 0
            {
                continue; // Avoid self-imports
            }

            let (name, extension) = get_filename_with_extension_option(
                &tspath::get_base_file_name(file_path),
                program,
                extension_options,
                false, /*isExportsOrImportsWildcard*/
            );
            result.add(ModuleCompletionNameAndKind {
                name,
                kind: ModuleCompletionKind::FILE,
                extension,
            });
        }

        // Get folder completion as well.
        let directories = self.get_directories(&base_directory);

        for directory in &directories {
            let directory_name = tspath::get_base_file_name(directory);
            if directory_name != "@types" {
                result.add(ModuleCompletionNameAndKind {
                    name: directory_name,
                    kind: ModuleCompletionKind::DIRECTORY,
                    extension: String::new(),
                });
            }
        }
    }

    // Go: ls/string_completions.go:1373 addCompletionEntriesFromPaths
    // Returns true if `fragment` was a match for any `paths`
    // (which should indicate whether any other path completions should be offered).
    fn add_completion_entries_from_paths(
        &self,
        result: &mut ModuleCompletionNameAndKindSet,
        program: &compiler::NewProgram,
        fragment: &str,
        base_directory: &str,
        extension_options: &ExtensionOptions,
        paths: &IndexMap<String, Option<Vec<String>>>,
    ) -> bool {
        let get_patterns_for_keys =
            |key: &str| -> Vec<String> { paths.get(key).cloned().flatten().unwrap_or_default() };
        let compare_paths = |a: &str, b: &str| -> Comparison {
            let pattern_a = module::try_parse_pattern(a);
            let pattern_b = module::try_parse_pattern(b);
            let mut length_a = a.len() as i32;
            if pattern_a.star_index != -1 {
                length_a = pattern_a.star_index;
            }
            let mut length_b = b.len() as i32;
            if pattern_b.star_index != -1 {
                length_b = pattern_b.star_index;
            }
            length_b.cmp(&length_a) as Comparison
        };
        self.add_completion_entries_from_paths_or_exports_or_imports(
            result,
            program,
            false, /*isExports*/
            false, /*isImports*/
            fragment,
            base_directory,
            extension_options,
            paths.keys().cloned().collect(),
            &get_patterns_for_keys,
            &compare_paths,
        )
    }

    // Go: ls/string_completions.go:1413 addCompletionEntriesFromPathsOrExportsOrImports
    // Returns true if `fragment` was a match for any `paths`
    // (which should indicate whether any other path completions should be offered).
    // PORT: Go `keys iter.Seq[string]` is collected into a `Vec` by the caller;
    // the maps it reads are not changed while it runs.
    fn add_completion_entries_from_paths_or_exports_or_imports(
        &self,
        result: &mut ModuleCompletionNameAndKindSet,
        program: &compiler::NewProgram,
        is_exports: bool,
        is_imports: bool,
        fragment: &str,
        base_directory: &str,
        extension_options: &ExtensionOptions,
        keys: Vec<String>,
        get_patterns_for_key: &dyn Fn(&str) -> Vec<String>,
        compare_paths: &dyn Fn(&str, &str) -> Comparison,
    ) -> bool {
        struct PathResult {
            results: Vec<ModuleCompletionNameAndKind>,
            matched: bool,
        }
        let mut path_results: Vec<PathResult> = Vec::new();
        let mut matched_path: Option<String> = None;
        for key in keys {
            if key == "." {
                continue;
            }
            let mut normalized_key = key.strip_prefix("./").unwrap_or(&key).to_string(); // Remove leading "./"
            if (is_exports || is_imports) && key.ends_with('/') {
                // Normalize trailing "/" to "/*"
                normalized_key += "*";
            }
            let patterns = get_patterns_for_key(&key);
            if !patterns.is_empty() {
                let path_pattern = module::try_parse_pattern(&normalized_key);
                if !path_pattern.is_valid() {
                    continue;
                }
                let is_match = path_pattern.matches(fragment);
                let mut is_longest_match = false;
                if is_match {
                    is_longest_match = match &matched_path {
                        None => true,
                        Some(matched_path) => {
                            compare_paths(&normalized_key, matched_path) == COMPARISON_LESS_THAN
                        }
                    };
                }
                if is_longest_match {
                    // If this is a higher priority match than anything we've seen so far, previous results from matches are invalid, e.g.
                    // for `import {} from "some-package/|"` with a typesVersions:
                    // {
                    //   "bar/*": ["bar/*"], // <-- 1. We add 'bar', but 'bar/*' doesn't match yet.
                    //   "*": ["dist/*"],    // <-- 2. We match here and add files from dist. 'bar' is still ok because it didn't come from a match.
                    //   "foo/*": ["foo/*"]  // <-- 3. We matched '*' earlier and added results from dist, but if 'foo/*' also matched,
                    // }                               results in dist would not be visible. 'bar' still stands because it didn't come from a match.
                    //                                 This is especially important if `dist/foo` is a folder, because if we fail to clear results
                    //                                 added by the '*' match, after typing `"some-package/foo/|"` we would get file results from both
                    //                                 ./dist/foo and ./foo, when only the latter will actually be resolvable.
                    //                                 See pathCompletionsTypesVersionsWildcard6.ts.
                    matched_path = Some(normalized_key.clone());
                    path_results.retain(|pr| !pr.matched);
                }
                if path_pattern.star_index == -1
                    || matched_path.is_none()
                    || compare_paths(&normalized_key, matched_path.as_deref().unwrap())
                        != COMPARISON_GREATER_THAN
                {
                    path_results.push(PathResult {
                        matched: is_match,
                        results: self.get_completions_for_path_mapping(
                            &normalized_key,
                            &patterns,
                            fragment,
                            base_directory,
                            is_exports,
                            is_imports,
                            extension_options,
                            program,
                        ),
                    });
                }
            }
        }

        for pr in path_results {
            for res in pr.results {
                result.add(res);
            }
        }

        matched_path.is_some()
    }

    // Go: ls/string_completions.go:1500 getCompletionsForPathMapping
    fn get_completions_for_path_mapping(
        &self,
        path: &str,
        patterns: &[String],
        fragment: &str,
        package_directory: &str,
        is_exports: bool,
        is_imports: bool,
        extension_options: &ExtensionOptions,
        program: &compiler::NewProgram,
    ) -> Vec<ModuleCompletionNameAndKind> {
        let mut fragment_directory = get_fragment_directory(fragment);
        if !fragment_directory.is_empty() {
            fragment_directory = tspath::ensure_trailing_directory_separator(&fragment_directory);
        }
        let just_path_mapping_name = |name: &str,
                                      kind: ModuleCompletionKind,
                                      extension: &str|
         -> Vec<ModuleCompletionNameAndKind> {
            // PORT: on Go bytes, as `get_ambient_module_completions`.
            if go_has_prefix(name, fragment) {
                let mut name = tspath::remove_trailing_directory_separator(name).to_string();
                // Go: strings.TrimPrefix, on Go bytes.
                if !fragment_directory.is_empty() && go_has_prefix(&name, &fragment_directory) {
                    name = go_slice(&name, go_len(&fragment_directory), go_len(&name)).into_owned();
                }
                return vec![ModuleCompletionNameAndKind {
                    name,
                    kind,
                    extension: extension.to_string(),
                }];
            }
            Vec::new()
        };

        let parsed_path = module::try_parse_pattern(path);
        if !parsed_path.is_valid() {
            return Vec::new();
        }
        // No stars in the pattern.
        if parsed_path.star_index == -1 {
            // For a path mapping "foo": ["/x/y/z.ts"], add "foo" itself as a completion.
            let pattern = patterns.first().cloned().unwrap_or_default();
            let extension = get_file_extension(&pattern);
            return just_path_mapping_name(path, ModuleCompletionKind::FILE, &extension);
        }

        let star_index = parsed_path.star_index as usize;
        let path_prefix = &parsed_path.text[..star_index];
        let path_suffix = &parsed_path.text[star_index + 1..];
        // PORT: the prefix checks and cuts below are on Go bytes, as
        // `get_ambient_module_completions`: a `paths` key keeps the raw bytes
        // of its tsconfig text, so its prefix can end inside a char that the
        // fragment has whole.
        if !go_has_prefix(fragment, path_prefix) {
            // Fragment doesn't match the path mapping prefix at all:
            // we cannot extend it via this path.
            if !go_has_prefix(path_prefix, fragment) {
                return Vec::new();
            }
            let star_is_full_path_component = path.ends_with("/*");
            if star_is_full_path_component {
                return just_path_mapping_name(
                    path_prefix,
                    ModuleCompletionKind::DIRECTORY,
                    "", /*extension*/
                );
            }
            // If path is e.g. `foo/bar/*`, and fragment is `foo/b`, then remaining directory prefix is `bar/`,
            let remaining_directory_prefix = go_slice(
                path_prefix,
                go_len(&fragment_directory),
                go_len(path_prefix),
            )
            .into_owned();
            let mut completions = Vec::new();
            for pattern in patterns {
                let mut modules = self.get_modules_for_paths_pattern(
                    "", /*fragment*/
                    package_directory,
                    pattern,
                    is_exports,
                    is_imports,
                    extension_options,
                    program,
                );
                for module in &mut modules {
                    let suffix = if module.kind == ModuleCompletionKind::FILE {
                        path_suffix
                    } else {
                        ""
                    };
                    module.name = remaining_directory_prefix.clone() + &module.name + suffix;
                }
                completions.extend(modules);
            }
            return completions;
        }
        let remaining_fragment = go_slice(fragment, go_len(path_prefix), go_len(fragment));
        let mut remaining_directory_fragment = String::new();
        if !go_has_prefix(&fragment_directory, path_prefix) {
            remaining_directory_fragment = go_slice(
                path_prefix,
                go_len(&fragment_directory),
                go_len(path_prefix),
            )
            .into_owned();
        }
        // Go: core.FlatMap
        let mut result = Vec::new();
        for pattern in patterns {
            let mut modules = self.get_modules_for_paths_pattern(
                &remaining_fragment,
                package_directory,
                pattern,
                is_exports,
                is_imports,
                extension_options,
                program,
            );
            for module in &mut modules {
                let suffix = if module.kind == ModuleCompletionKind::FILE {
                    path_suffix
                } else {
                    ""
                };
                module.name = remaining_directory_fragment.clone() + &module.name + suffix;
            }
            if !modules.is_empty() {
                result.extend(modules);
            }
        }
        result
    }
}

// Go: ls/string_completions.go:1598 getFileExtension
fn get_file_extension(file_name: &str) -> String {
    let mut extension = tspath::try_get_extension_from_path(file_name).to_string();
    if extension.is_empty() {
        extension = tspath::get_any_extension_from_path(
            file_name,
            &[],   /*extensions*/
            false, /*ignoreCase*/
        );
    }
    extension
}

impl LanguageService {
    // Go: ls/string_completions.go:1611 getModulesForPathsPattern
    // The input fragment is relative to the path pattern's prefix:
    // e.g. if path = "bar/_*/baz", and fragment = "bar/_dir", then fragment is "dir".
    // The names are relative to the path pattern's prefix and fragment directory :
    // e.g. if path = "bar/_*/baz", and fragment = "bar/_dir/a", and we find result "abd",
    // the result should be interpreted as "bar/_dir/abd".
    fn get_modules_for_paths_pattern(
        &self,
        fragment: &str,
        package_directory: &str,
        pattern: &str,
        is_exports: bool,
        is_imports: bool,
        extension_options: &ExtensionOptions,
        program: &compiler::NewProgram,
    ) -> Vec<ModuleCompletionNameAndKind> {
        let parsed = module::try_parse_pattern(pattern);
        if !parsed.is_valid() || parsed.star_index == -1 {
            return Vec::new();
        }

        let star_index = parsed.star_index as usize;
        let prefix = &parsed.text[..star_index];
        let suffix = &parsed.text[star_index + 1..];

        // The prefix has two effective parts: the directory path and the base component after the filepath that is not a
        // full directory component. For example: directory/path/of/prefix/base*
        let normalized_prefix = tspath::resolve_path(prefix, &[]);
        let normalized_prefix_directory;
        let normalized_prefix_base;
        if tspath::has_trailing_directory_separator(prefix) {
            normalized_prefix_directory = normalized_prefix;
            normalized_prefix_base = String::new();
        } else {
            normalized_prefix_directory = tspath::get_directory_path(&normalized_prefix);
            normalized_prefix_base = tspath::get_base_file_name(&normalized_prefix);
        }

        let fragment_has_path = contains_slash(fragment);
        let mut fragment_directory = String::new();
        if fragment_has_path {
            if tspath::has_trailing_directory_separator(fragment) {
                fragment_directory = fragment.to_string();
            } else {
                fragment_directory = tspath::get_directory_path(fragment);
            }
        }

        let options = program.options();
        let ignore_case = !program.use_case_sensitive_file_names();
        let out_dir = &options.out_dir;
        let declaration_dir = &options.declaration_dir;

        // Try and expand the prefix to include any path from the fragment so that we can limit the readDirectory call
        let expanded_prefix_directory = if fragment_has_path {
            tspath::combine_paths(
                &normalized_prefix_directory,
                &[&(normalized_prefix_base.clone() + &fragment_directory)],
            )
        } else {
            normalized_prefix_directory
        };
        // Need to normalize after combining: If we combinePaths("a", "../b"), we want "b" and not "a/../b".
        let base_directory = tspath::normalize_path(&tspath::combine_paths(
            package_directory,
            &[&expanded_prefix_directory],
        ));

        let mut possible_input_base_directory_for_out_dir = String::new();
        let mut possible_input_base_directory_for_declaration_dir = String::new();
        if is_imports {
            if !out_dir.is_empty() {
                possible_input_base_directory_for_out_dir =
                    get_possible_original_input_path_without_changing_ext(
                        &base_directory,
                        ignore_case,
                        out_dir,
                        &|| program.common_source_directory(),
                    );
            }
            if !declaration_dir.is_empty() {
                possible_input_base_directory_for_declaration_dir =
                    get_possible_original_input_path_without_changing_ext(
                        &base_directory,
                        ignore_case,
                        declaration_dir,
                        &|| program.common_source_directory(),
                    );
            }
        }

        let normalized_suffix = tspath::normalize_path(suffix);

        let mut declaration_extension = String::new();
        let mut input_extensions: Vec<String> = Vec::new();
        if !normalized_suffix.is_empty() {
            declaration_extension = tspath::get_declaration_emit_extension_for_path(
                &("_".to_string() + &normalized_suffix),
            );
            input_extensions = tspath::get_possible_original_input_extension_for_extension(
                &("_".to_string() + &normalized_suffix),
            );
        }

        let mut matching_suffixes: Vec<String> = Vec::new();
        if !declaration_extension.is_empty() {
            matching_suffixes.push(tspath::change_extension(
                &normalized_suffix,
                &declaration_extension,
            ));
        }
        for ext in &input_extensions {
            matching_suffixes.push(tspath::change_extension(&normalized_suffix, ext));
        }
        matching_suffixes.push(normalized_suffix.clone());

        // If we have a suffix, then we read the directory all the way down to avoid returning completions for
        // directories that don't contain files that would match the suffix. A previous comment here was concerned
        // about the case where `normalizedSuffix` includes a `?` character, which should be interpreted literally,
        // but will match any single character as part of the `include` pattern in `tryReadDirectory`. This is not
        // a problem, because (in the extremely unusual circumstance where the suffix has a `?` in it) a `?`
        // interpreted as "any character" can only return *too many* results as compared to the literal
        // interpretation, so we can filter those superfluous results out via `trimPrefixAndSuffix` as we've always
        // done.
        let include_globs: Vec<String> = if !normalized_suffix.is_empty() {
            matching_suffixes
                .iter()
                .map(|suffix| "**/*".to_string() + suffix)
                .collect()
        } else {
            vec!["./*".to_string()]
        };

        let is_exports_or_imports_wildcard = (is_exports || is_imports) && pattern.ends_with("/*");

        let trim_prefix_and_suffix = |path: &str, prefix_str: &str| -> String {
            for suffix in &matching_suffixes {
                let inner =
                    without_start_and_end(&tspath::normalize_path(path), prefix_str, suffix);
                let Some(inner) = inner else {
                    continue;
                };
                return remove_leading_directory_separator(&inner).to_string();
            }
            String::new()
        };

        let get_matches_with_prefix = |directory: &str| -> Vec<ModuleCompletionNameAndKind> {
            let complete_prefix = if fragment_has_path {
                directory.to_string()
            } else {
                tspath::ensure_trailing_directory_separator(directory) + &normalized_prefix_base
            };

            let matches = self.read_directory(
                directory,
                &extension_options.extensions_to_search,
                &include_globs,
            );

            let mut result = Vec::new();
            for match_ in &matches {
                let trimmed_with_pattern = trim_prefix_and_suffix(match_, &complete_prefix);
                if !trimmed_with_pattern.is_empty() {
                    if contains_slash(&trimmed_with_pattern) {
                        let path_components = tspath::get_path_components(
                            remove_leading_directory_separator(&trimmed_with_pattern),
                            "",
                        );
                        if path_components.len() > 1 {
                            result.push(ModuleCompletionNameAndKind {
                                name: path_components[1].clone(),
                                kind: ModuleCompletionKind::DIRECTORY,
                                extension: String::new(),
                            });
                        }
                    } else {
                        let (name, mut extension) = get_filename_with_extension_option(
                            &trimmed_with_pattern,
                            program,
                            extension_options,
                            is_exports_or_imports_wildcard,
                        );
                        if extension.is_empty() {
                            extension = get_file_extension(match_);
                        }
                        result.push(ModuleCompletionNameAndKind {
                            name,
                            kind: ModuleCompletionKind::FILE,
                            extension,
                        });
                    }
                }
            }
            result
        };

        let get_directory_matches = |directory_name: &str| -> Vec<ModuleCompletionNameAndKind> {
            let directories = self.get_directories(directory_name);
            let mut result = Vec::new();
            for dir in directories {
                if dir != "node_modules" {
                    result.push(ModuleCompletionNameAndKind {
                        name: dir,
                        kind: ModuleCompletionKind::DIRECTORY,
                        extension: String::new(),
                    });
                }
            }
            result
        };

        let mut matches = Vec::new();
        matches.extend(get_matches_with_prefix(&base_directory));

        if !possible_input_base_directory_for_out_dir.is_empty() {
            matches.extend(get_matches_with_prefix(
                &possible_input_base_directory_for_out_dir,
            ));
        }
        if !possible_input_base_directory_for_declaration_dir.is_empty() {
            matches.extend(get_matches_with_prefix(
                &possible_input_base_directory_for_declaration_dir,
            ));
        }

        // If we had a suffix, we already recursively searched for all possible files that could match
        // it and returned the directories leading to those files. Otherwise, assume any directory could
        // have something valid to import.
        if normalized_suffix.is_empty() {
            matches.extend(get_directory_matches(&base_directory));
            if !possible_input_base_directory_for_out_dir.is_empty() {
                matches.extend(get_directory_matches(
                    &possible_input_base_directory_for_out_dir,
                ));
            }
            if !possible_input_base_directory_for_declaration_dir.is_empty() {
                matches.extend(get_directory_matches(
                    &possible_input_base_directory_for_declaration_dir,
                ));
            }
        }

        matches
    }
}

// Go: ls/string_completions.go:1822 containsSlash
fn contains_slash(fragment: &str) -> bool {
    fragment.contains(tspath::DIRECTORY_SEPARATOR as char)
}

// Go: ls/string_completions.go:1826 withoutStartAndEnd
fn without_start_and_end(s: &str, start: &str, end: &str) -> Option<String> {
    if s.starts_with(start) && s.ends_with(end) && s.len() >= start.len() + end.len() {
        return Some(s[start.len()..s.len() - end.len()].to_string());
    }
    None
}

// Go: ls/string_completions.go:1834 removeLeadingDirectorySeparator
fn remove_leading_directory_separator(path: &str) -> &str {
    path.strip_prefix(tspath::DIRECTORY_SEPARATOR as char)
        .unwrap_or(path)
}

// Go: ls/string_completions.go:1838 getPossibleOriginalInputPathWithoutChangingExt
fn get_possible_original_input_path_without_changing_ext(
    file_path: &str,
    ignore_case: bool,
    output_dir: &str,
    get_common_source_directory: &dyn Fn() -> String,
) -> String {
    if !output_dir.is_empty() {
        return tspath::resolve_path(
            &get_common_source_directory(),
            &[&tspath::get_relative_path_from_directory(
                output_dir,
                file_path,
                &tspath::ComparePathsOptions {
                    use_case_sensitive_file_names: !ignore_case,
                    ..Default::default()
                },
            )],
        );
    }
    file_path.to_string()
}

// Go: ls/string_completions.go:1855 getFilenameWithExtensionOption
fn get_filename_with_extension_option(
    name: &str,
    program: &compiler::NewProgram,
    extension_options: &ExtensionOptions,
    is_exports_or_imports_wildcard: bool,
) -> (String, String) {
    let non_js_result =
        modulespecifiers::try_get_real_file_name_for_non_js_declaration_file_name(name);
    if !non_js_result.is_empty() {
        let extension = tspath::try_get_extension_from_path(&non_js_result).to_string();
        return (non_js_result, extension);
    }
    if extension_options.reference_kind == ReferenceKind::FILE_NAME {
        return (
            name.to_string(),
            tspath::try_get_extension_from_path(name).to_string(),
        );
    }

    // PORT: Go passes `program` as the host; see the file header.
    let mut allowed_endings = modulespecifiers::get_allowed_endings_in_preferred_order(
        &modulespecifiers::UserPreferences {
            import_module_specifier_ending: extension_options.ending_preference,
            ..Default::default()
        },
        &modulespecifiers::ProgramHost,
        program.options(),
        &extension_options.importing_source_file,
        "", /*oldImportSpecifier*/
        extension_options.resolution_mode,
    );

    if is_exports_or_imports_wildcard {
        // If we're completing `import {} from "foo/|"` and subpaths are available via `"exports": { "./*": "./src/*" }`,
        // the completion must be a (potentially extension-swapped) file name. Dropping extensions and index files is not allowed.
        allowed_endings.retain(|e| {
            *e != modulespecifiers::ModuleSpecifierEnding::Minimal
                && *e != modulespecifiers::ModuleSpecifierEnding::Index
        });
    }

    if !allowed_endings.is_empty()
        && allowed_endings[0] == modulespecifiers::ModuleSpecifierEnding::TsExtension
    {
        if tspath::file_extension_is_one_of(name, tspath::SUPPORTED_TS_IMPLEMENTATION_EXTENSIONS) {
            return (
                name.to_string(),
                tspath::try_get_extension_from_path(name).to_string(),
            );
        }
        let output_extension = module::try_get_js_extension_for_file(name, program.options());
        if !output_extension.is_empty() {
            return (
                tspath::change_extension(name, output_extension),
                output_extension.to_string(),
            );
        }
        return (
            name.to_string(),
            tspath::try_get_extension_from_path(name).to_string(),
        );
    }

    if !is_exports_or_imports_wildcard
        && !allowed_endings.is_empty()
        && (allowed_endings[0] == modulespecifiers::ModuleSpecifierEnding::Minimal
            || allowed_endings[0] == modulespecifiers::ModuleSpecifierEnding::Index)
        && tspath::file_extension_is_one_of(
            name,
            &[
                tspath::EXTENSION_JS,
                tspath::EXTENSION_JSX,
                tspath::EXTENSION_TS,
                tspath::EXTENSION_TSX,
                tspath::EXTENSION_DTS,
            ],
        )
    {
        return (
            tspath::remove_file_extension(name).to_string(),
            tspath::try_get_extension_from_path(name).to_string(),
        );
    }

    let output_extension = module::try_get_js_extension_for_file(name, program.options());
    if !output_extension.is_empty() {
        return (
            tspath::change_extension(name, output_extension),
            output_extension.to_string(),
        );
    }
    (
        name.to_string(),
        tspath::try_get_extension_from_path(name).to_string(),
    )
}

// Go: ls/string_completions.go:1911 walkUpParentheses
fn walk_up_parentheses(node: Node) -> Node {
    match node.kind() {
        SyntaxKind::ParenthesizedType => walk_up_parenthesized_types(node),
        SyntaxKind::ParenthesizedExpression => walk_up_parenthesized_expressions(node),
        _ => node,
    }
}

// Go: ls/string_completions.go:1922 getStringLiteralTypes
// PORT: Go `uniques *collections.Set[string]` can be nil: `Option<&mut ..>`;
// nil makes a fresh set as in Go.
fn get_string_literal_types(
    t: TypeId,
    uniques: Option<&mut FxHashSet<String>>,
    type_checker: &mut Checker,
) -> Vec<TypeId> {
    if t.is_nil() {
        return Vec::new();
    }
    let mut own_uniques = FxHashSet::default();
    let uniques = match uniques {
        Some(uniques) => uniques,
        None => &mut own_uniques,
    };
    let t = skip_constraint(t, type_checker);
    if type_checker.ty(t).is_union() {
        let mut types = Vec::new();
        for element_type in type_checker.ty(t).types().to_vec() {
            types.extend(get_string_literal_types(
                element_type,
                Some(&mut *uniques),
                type_checker,
            ));
        }
        return types;
    }
    if type_checker.ty(t).is_string_literal() && !type_checker.ty(t).is_enum_literal() {
        let Some(LiteralValue::String(value)) = type_checker.ty(t).as_literal_type().value() else {
            panic!("interface conversion: interface {{}} is not string");
        };
        if uniques.insert(value.clone()) {
            return vec![t];
        }
    }
    Vec::new()
}

// Go: ls/string_completions.go:1943 getAlreadyUsedTypesInStringLiteralUnion
fn get_already_used_types_in_string_literal_union(union: Node, current: Node) -> Vec<String> {
    let types_list = union.types();
    if types_list.is_nil() {
        return Vec::new();
    }
    let mut values = Vec::new();
    for type_node in types_list.nodes().iter() {
        if type_node != current
            && is_literal_type_node(type_node)
            && is_string_literal(type_node.literal())
        {
            values.push(type_node.literal().text().to_string());
        }
    }
    values
}

// Go: ls/string_completions.go:1958 hasIndexSignature
fn has_index_signature(t: TypeId, type_checker: &mut Checker) -> bool {
    type_checker.get_string_index_type(t).is_some()
        || type_checker.get_number_index_type(t).is_some()
}

// Go: ls/string_completions.go:1966 isRequireCallArgument
// Matches
//
//	require(""
//	require("")
fn is_require_call_argument(node: Node) -> bool {
    is_call_expression(node.parent())
        && !node.parent().arguments().is_empty()
        && node.parent().arguments().get(0) == node
        && is_identifier(node.parent().expression())
        && node.parent().expression().text() == "require"
}

// Go: ls/string_completions.go:1971 kindModifiersFromExtension
fn kind_modifiers_from_extension(extension: &str) -> lsutil::ScriptElementKindModifier {
    match extension {
        tspath::EXTENSION_DTS => lsutil::ScriptElementKindModifier::DTS,
        tspath::EXTENSION_JS => lsutil::ScriptElementKindModifier::JS,
        tspath::EXTENSION_JSON => lsutil::ScriptElementKindModifier::JSON,
        tspath::EXTENSION_JSX => lsutil::ScriptElementKindModifier::JSX,
        tspath::EXTENSION_TS => lsutil::ScriptElementKindModifier::TS,
        tspath::EXTENSION_TSX => lsutil::ScriptElementKindModifier::TSX,
        tspath::EXTENSION_DMTS => lsutil::ScriptElementKindModifier::DMTS,
        tspath::EXTENSION_MJS => lsutil::ScriptElementKindModifier::MJS,
        tspath::EXTENSION_MTS => lsutil::ScriptElementKindModifier::MTS,
        tspath::EXTENSION_DCTS => lsutil::ScriptElementKindModifier::DCTS,
        tspath::EXTENSION_CJS => lsutil::ScriptElementKindModifier::CJS,
        tspath::EXTENSION_CTS => lsutil::ScriptElementKindModifier::CTS,
        tspath::EXTENSION_TS_BUILD_INFO => crate::core::go_panic(format!(
            "Extension {} is unsupported.",
            tspath::EXTENSION_TS_BUILD_INFO
        )),
        _ => lsutil::ScriptElementKindModifier::NONE,
    }
}

// Go: ls/string_completions.go:2004 getStringLiteralCompletionsFromSignature
fn get_string_literal_completions_from_signature(
    call: Node,
    arg: Node,
    argument_info: &ArgumentInfoForCompletions,
    type_checker: &mut Checker,
) -> Option<CompletionsFromTypes> {
    let mut is_new_identifier = false;
    let mut uniques: FxHashSet<String> = FxHashSet::default();
    let editing_argument;
    if is_jsx_opening_like_element(call) {
        editing_argument = find_ancestor(arg.parent(), is_jsx_attribute);
        if editing_argument.is_nil() {
            crate::core::go_panic(
                "Expected jsx opening-like element to have a jsx attribute as ancestor."
                    .to_string(),
            );
        }
    } else {
        editing_argument = arg;
    }
    let candidates = type_checker
        .get_candidate_signatures_for_string_literal_completions(call, editing_argument);
    let mut types = Vec::new();
    for candidate in candidates {
        if !type_checker.sig(candidate).has_rest_parameter()
            && argument_info.argument_count > type_checker.sig(candidate).parameters().len() as i32
        {
            continue;
        }
        let mut t =
            type_checker.get_type_parameter_at_position(candidate, argument_info.argument_index);
        if is_jsx_opening_like_element(call) {
            let prop_type = type_checker
                .get_type_of_property_of_type_exported(t, editing_argument.name().text());
            if prop_type.is_some() {
                t = prop_type;
            }
        }
        is_new_identifier = is_new_identifier || type_checker.ty(t).is_string();
        types.extend(get_string_literal_types(
            t,
            Some(&mut uniques),
            type_checker,
        ));
    }
    if !types.is_empty() {
        return Some(CompletionsFromTypes {
            types,
            is_new_identifier,
        });
    }
    None
}

impl LanguageService {
    // Go: ls/string_completions.go:2046 getStringLiteralCompletionDetails
    // PORT: Go mutates `*item` and returns it; the item moves in and out.
    pub fn get_string_literal_completion_details(
        &self,
        ctx: &Context,
        checker: &mut Checker,
        item: lsproto::CompletionItem,
        name: &str,
        file: Node,
        position: i32,
        context_token: Node,
        doc_format: lsproto::MarkupKind,
    ) -> lsproto::CompletionItem {
        if context_token.is_nil() || !is_string_literal_like(context_token) {
            return item;
        }
        let completions =
            self.get_string_literal_completion_entries(ctx, file, context_token, position, checker);
        let Some(completions) = completions else {
            return item;
        };
        self.string_literal_completion_details(
            item,
            name,
            context_token,
            position,
            &completions,
            file,
            checker,
            doc_format,
        )
    }

    // Go: ls/string_completions.go:2072 stringLiteralCompletionDetails
    fn string_literal_completion_details(
        &self,
        item: lsproto::CompletionItem,
        name: &str,
        location: Node,
        position: i32,
        completion: &StringLiteralCompletions,
        _file: Node,
        checker: &mut Checker,
        doc_format: lsproto::MarkupKind,
    ) -> lsproto::CompletionItem {
        if completion.from_paths.is_some() {
            // Path completions have eagerly-resolved details so the client can show an accurate icon
            // for items of file kind based on the file extension provided in the item detail.
            return item;
        } else if let Some(properties) = &completion.from_properties {
            for &symbol in &properties.symbols {
                if checker.sym(symbol).name.as_str() == name {
                    return self.create_completion_details_for_symbol(
                        item, symbol, checker, location, position, doc_format,
                    );
                }
            }
        } else if let Some(types) = &completion.from_types {
            for &t in &types.types {
                let Some(LiteralValue::String(value)) = checker.ty(t).as_literal_type().value()
                else {
                    panic!("interface conversion: interface {{}} is not string");
                };
                if value == name {
                    return create_completion_details(
                        item, name, "", /*documentation*/
                        doc_format,
                    );
                }
            }
        }
        item
    }
}

// Go: ls/string_completions.go:2105 isInReferenceComment
fn is_in_reference_comment(file: Node, position: i32) -> bool {
    let comment_range = is_in_comment(
        file,
        position,
        astnav::get_token_at_position(file, position),
    );
    let Some(comment_range) = comment_range else {
        return false;
    };
    let comment_text =
        &source_file_text(file)[comment_range.pos() as usize..comment_range.end() as usize];
    has_triple_slash_prefix(comment_text)
}

// Go: ls/string_completions.go:2114 hasTripleSlashPrefix
fn has_triple_slash_prefix(comment_text: &str) -> bool {
    comment_text.starts_with("///") && comment_text[3..].trim().starts_with('<')
}

// Go: ls/string_completions.go:2134 parseTripleSlashDirectiveFragment
// Matches a triple slash reference directive with an incomplete string literal for its path.
// Used to determine if the caret is currently within the string literal and capture the literal
// fragment for completions.
// For example, this matches
//
// /// <reference path="fragment
//
// but not
//
// /// <reference path="fragment"

// Returns (prefix, kind, toComplete, ok) where:
//   - prefix is everything up to and including the opening quote
//   - kind is either "path" or "types"
//   - toComplete is the fragment after the opening quote
//   - ok indicates whether the match was successful
fn parse_triple_slash_directive_fragment(text: &str) -> (&str, &'static str, &str, bool) {
    let mut rest = text;
    if !rest.starts_with("///") {
        return ("", "", "", false);
    }

    rest = &rest["///".len()..];
    rest = rest.trim_start_matches(is_white_space_like);

    // <reference
    if !rest.starts_with("<reference") {
        return ("", "", "", false);
    }
    rest = &rest["<reference".len()..];

    // PORT: Go `rune(rest[0])` reads one byte as a code point; so does `as char`.
    if rest.is_empty() || !is_white_space_like(rest.as_bytes()[0] as char) {
        return ("", "", "", false);
    }
    rest = rest.trim_start_matches(is_white_space_like);

    // path or types
    let kind;
    if rest.starts_with("path") {
        kind = "path";
        rest = &rest["path".len()..];
    } else if rest.starts_with("types") {
        kind = "types";
        rest = &rest["types".len()..];
    } else {
        return ("", "", "", false);
    }

    // Skip optional whitespace, then must have "="
    rest = rest.trim_start_matches(is_white_space_like);
    if !rest.starts_with('=') {
        return ("", "", "", false);
    }
    rest = &rest[1..];

    // Skip optional whitespace, then must have opening quote (' or ")
    rest = rest.trim_start_matches(is_white_space_like);
    if rest.is_empty() || (rest.as_bytes()[0] != b'\'' && rest.as_bytes()[0] != b'"') {
        return ("", "", "", false);
    }
    rest = &rest[1..];

    // The toComplete part is everything after the opening quote
    if rest.contains(['\'', '"']) {
        return ("", "", "", false);
    }
    let to_complete = rest;
    let prefix = &text[..text.len() - to_complete.len()];
    (prefix, kind, to_complete, true)
}

impl LanguageService {
    // Go: ls/string_completions.go:2188 getTripleSlashReferenceCompletions
    fn get_triple_slash_reference_completions(
        &self,
        file: Node,
        position: i32,
        program: &compiler::NewProgram,
        _checker: &mut Checker,
    ) -> Option<PathCompletions> {
        let compiler_options = program.options();
        let token = astnav::get_token_at_position(file, position);
        let comment_ranges = get_leading_comment_ranges(
            &NodeFactory::default(),
            &source_file_text(file),
            token.pos(),
        );

        let mut found_range = None;
        for comment_range in &comment_ranges {
            if position >= comment_range.pos() && position <= comment_range.end() {
                found_range = Some(comment_range);
                break;
            }
        }
        let Some(found_range) = found_range else {
            return None;
        };

        // PORT: `position` can cut a char (see `go_text_slice`). Go keeps
        // the cut bytes at the end of `text`. They are not white space,
        // quotes or separators, and the basename of `toComplete` is not
        // used, so the parse and the directory are Go's.
        let file_text = source_file_text(file);
        let text = go_text_slice(&file_text, found_range.pos(), position);
        let (prefix, kind, to_complete, ok) = parse_triple_slash_directive_fragment(&text);
        if !ok {
            return None;
        }
        // PORT: Go's range ends at `position`, the end of `text`. The invalid
        // byte units of a cut char are longer than its Go bytes, so the end
        // is `position`, not the end of `to_complete` in port bytes. The
        // start is a Go offset before the cut, mapped back to the port.
        let fragment_range = get_directory_fragment_range(
            &go_string_bytes(to_complete),
            go_byte_offset(&file_text, found_range.pos()) + go_len(prefix) as i32,
        )
        .map(|range| TextRange::new(port_byte_offset(&file_text, range.pos()), position));
        let (replacement_span, ok) = self.path_completion_replacement_span(file, fragment_range);
        if !ok {
            return None;
        }

        let script_path = tspath::get_directory_path(&source_file_info(file).path);

        // PORT: Go `var names` is nil unless `kind` matches; `kind` is always
        // "path" or "types" here.
        let names: Vec<ModuleCompletionNameAndKind> = match kind {
            "path" => {
                let extension_options = self.get_extension_options(
                    compiler_options,
                    ReferenceKind::FILE_NAME,
                    file,
                    RESOLUTION_MODE_NONE,
                    None, /*checker*/
                );
                let mut result = ModuleCompletionNameAndKindSet::default();
                self.get_completion_entries_for_directory_fragment(
                    to_complete,
                    &script_path,
                    &extension_options,
                    program,
                    true, /*moduleSpecifierIsRelative*/
                    &source_file_info(file).path,
                    &mut result,
                );
                // PORT: Go map order is random; the oracle compares that output without order.
                result.names.into_values().collect()
            }
            "types" => {
                let extension_options = self.get_extension_options(
                    compiler_options,
                    ReferenceKind::MODULE_SPECIFIER,
                    file,
                    RESOLUTION_MODE_NONE,
                    None, /*checker*/
                );
                let mut result = ModuleCompletionNameAndKindSet::default();
                self.get_completion_entries_from_typings(
                    program,
                    &script_path,
                    &get_fragment_directory(to_complete),
                    &extension_options,
                    &mut result,
                );
                // PORT: Go map order is random; the oracle compares that output without order.
                result.names.into_values().collect()
            }
            _ => Vec::new(),
        };

        Some(PathCompletions {
            entries: to_path_completions(names),
            replacement_span,
        })
    }
}

// Go: ls/string_completions_test.go (tsgo#4900)
#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write;

    // Go: ls/string_completions_test.go:17 TestTryRemoveDirectoryPrefixCaseFoldingShrinksPrefix
    // Each Kelvin sign '\u212A' below case-folds to the single-byte 'k', so the raw
    // prefix is longer in bytes (15) than path (12), even though path's canonical
    // form is case-insensitively prefixed by prefix's canonical form.
    #[test]
    fn test_try_remove_directory_prefix_case_folding_shrinks_prefix() {
        let prefix = "/a/\u{212A}\u{212A}\u{212A}\u{212A}";
        let path = "/a/kkkk/x.ts";
        let actual =
            try_remove_directory_prefix(path, prefix, false /*useCaseSensitiveFileNames*/);
        assert_eq!(actual.as_deref(), Some("x.ts"));
    }

    /// The triple-slash fragment of `getTripleSlashReferenceCompletions`
    /// is Go's when the position cuts a char, where Go slices the comment's
    /// bytes and keeps the cut bytes in `toComplete`. The expected lines are
    /// from a Go test at pin N with Go 1.27.1 (`utf8cmp1/tools/gomodel` in
    /// the lane dir, an overlay test file in package `ls`) that runs, at
    /// each position: `parseTripleSlashDirectiveFragment(text[:p])` (ok,
    /// `len(prefix)`, kind and the `toComplete` bytes),
    /// `getDirectoryFragmentRange(toComplete, len(prefix))` and the bytes of
    /// `getFragmentDirectory(toComplete)`. Go's range always ends at the
    /// position, as the port's range does.
    #[test]
    fn triple_slash_fragment_cut_chars_as_go() {
        let go = "\
T0 20 false 0 \"\"  nil \n\
T0 21 true 21 \"path\"  nil \n\
T0 22 true 21 \"path\" 2e 21-22 \n\
T0 23 true 21 \"path\" 2e2f nil 2e2f\n\
T0 24 true 21 \"path\" 2e2fc3 23-24 2e\n\
T0 25 true 21 \"path\" 2e2fc3a9 23-25 2e\n\
T0 26 true 21 \"path\" 2e2fc3a92f nil 2e2fc3a92f\n\
T0 27 true 21 \"path\" 2e2fc3a92ff0 26-27 2e2fc3a9\n\
T0 28 true 21 \"path\" 2e2fc3a92ff09f 26-28 2e2fc3a9\n\
T0 29 true 21 \"path\" 2e2fc3a92ff09f98 26-29 2e2fc3a9\n\
T0 30 true 21 \"path\" 2e2fc3a92ff09f9880 26-30 2e2fc3a9\n\
T1 21 false 0 \"\"  nil \n\
T1 22 true 22 \"types\"  nil \n\
T1 23 true 22 \"types\" c3 22-23 \n\
T1 24 true 22 \"types\" c3a9 22-24 \n\
T2 3 false 0 \"\"  nil \n\
T2 4 false 0 \"\"  nil \n\
T2 5 false 0 \"\"  nil \n\
T2 6 false 0 \"\"  nil \n\
";
        let cases = [
            ("/// <reference path=\"./\u{E9}/\u{1F600}", 20, 30),
            ("/// <reference types=\"\u{E9}", 21, 24),
            ("///\u{3000}<reference path=\"a", 3, 6),
        ];
        let hex = |s: &str| {
            go_string_bytes(s)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        };
        let mut port = String::new();
        for (i, (text, from, to)) in cases.into_iter().enumerate() {
            for p in from..=to {
                let slice = go_text_slice(text, 0, p);
                let (prefix, kind, to_complete, ok) = parse_triple_slash_directive_fragment(&slice);
                let range = get_directory_fragment_range(
                    &go_string_bytes(to_complete),
                    go_len(prefix) as i32,
                )
                .map_or("nil".to_string(), |range| format!("{}-{p}", range.pos()));
                writeln!(
                    port,
                    "T{i} {p} {ok} {} {kind:?} {} {range} {}",
                    prefix.len(),
                    hex(to_complete),
                    hex(&get_fragment_directory(to_complete))
                )
                .unwrap();
            }
        }
        assert_eq!(port, go);
    }

    /// The range that module name completions replace is Go's: Go adds byte
    /// offsets of the literal's value to the source offset of its text, so
    /// a value whose port form differs from the source's (an escape, a WTF-8
    /// surrogate that the value fuses into one unit) or that has a marker
    /// unit must be counted in Go bytes. The expected lines are from a Go
    /// test at pin N with Go 1.27.1 (`followups17/tools/gomodel` in the lane
    /// dir, an overlay test file in package `ls`) that prints
    /// `getDirectoryFragmentRange(node.Text(), textStart)` of
    /// `getStringLiteralCompletionsFromModuleNames` for the module specifier,
    /// in Go bytes. It numbers these texts after its 13 texts of
    /// `findallreferences_p1::tests::range_of_a_string_literal_counts_go_bytes`.
    #[test]
    fn module_name_fragment_range_counts_go_bytes() {
        use crate::frontend::parser::{SourceFileParseOptions, parse_source_file};
        use crate::frontend::tspath::Path;
        use crate::scanner_util::go_string_from_bytes;
        let go = "\
M13 22-23
M14 21-25
M15 22-23
M16 21-25
M17 21-25
M18 22-26
M19 22-26
M20 21-23
M21 20-22
M22 21-23
M23 21-22
M24 21-26
";
        let texts: [&[u8]; 12] = [
            b"import a from \"./d\xed\xa0\x80/f\";\n",
            b"import b from \"./sub/a\xed\xa0\x80\";\n",
            b"import c from \"./d\\uD800/f\";\n",
            b"import d from \"./sub/a\\uD800\";\n",
            b"import e from \"./sub/a\\uFDD0\";\n",
            b"import f from \"./d\\uFDD0/a\\uFDD0\";\n",
            b"import g from \"./d\xef\xb7\x90/a\xef\xb7\x90\";\n",
            b"import h from \"./sub/a\xff\";\n",
            b"import i from \"./d\xff/a\xff\";\n",
            b"import j from \"./sub/\\xFF\";\n",
            b"import k from \"./sub/\xff\n",
            b"import l from \"./sub/\\uD83D\\uDE00x\";\n",
        ];
        let mut port = String::new();
        for (i, bytes) in texts.into_iter().enumerate() {
            let text = go_string_from_bytes(bytes.to_vec());
            let file = parse_source_file(
                &SourceFileParseOptions {
                    file_name: "/a.ts".to_string(),
                    path: Path("/a.ts".to_string()),
                    ..Default::default()
                },
                crate::ast::FileText::new(text.clone(), false),
                ScriptKind::TS,
            );
            let specifier = file.root.statements().get(0).module_specifier();
            let range = module_name_fragment_range(file.root, specifier).map_or(
                "nil".to_string(),
                |range| {
                    format!(
                        "{}-{}",
                        go_byte_offset(&text, range.pos()),
                        go_byte_offset(&text, range.end())
                    )
                },
            );
            writeln!(port, "M{} {range}", i + 13).unwrap();
        }
        assert_eq!(port, go);
    }
}

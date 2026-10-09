use crate::ls::prelude::*;

use crate::spanmap::Feature;

// Go: ls/signaturehelp.go:25 SignatureHelpTriggerCharacters
// SignatureHelpTriggerCharacters and SignatureHelpRetriggerCharacters are the characters that trigger and
// re-trigger signature help. They are advertised both in the static server capabilities and in the dynamic
// content-mapper registration, so they live here to keep those two declarations in sync.
// PORT: Go `[]string` package variables; here constant slices.
pub const SIGNATURE_HELP_TRIGGER_CHARACTERS: &[&str] = &["(", ",", "<"];
// Go: ls/signaturehelp.go:26 SignatureHelpRetriggerCharacters
pub const SIGNATURE_HELP_RETRIGGER_CHARACTERS: &[&str] = &[")"];

// Go: ls/signaturehelp.go:29 callInvocation
#[derive(Clone, Copy, Debug)]
pub struct CallInvocation {
    pub node: Node,
}

// Go: ls/signaturehelp.go:33 typeArgsInvocation
#[derive(Clone, Copy, Debug)]
pub struct TypeArgsInvocation {
    pub called: Node,
}

// Go: ls/signaturehelp.go:37 contextualInvocation
#[derive(Clone, Copy, Debug)]
pub struct ContextualInvocation {
    pub signature: SignatureId,
    pub node: Node, // Just for enclosingDeclaration for printing types
    pub symbol: SymbolId,
}

// Go: ls/signaturehelp.go:43 invocation
// PORT: Go `*callInvocation` (and the other two) are nil-able pointers; here `Option`.
#[derive(Clone, Copy, Debug, Default)]
pub struct Invocation {
    pub call_invocation: Option<CallInvocation>,
    pub type_args_invocation: Option<TypeArgsInvocation>,
    pub contextual_invocation: Option<ContextualInvocation>,
}

// Go: ls/signaturehelp.go:76 signatureHelpTriggerReasonKind (local type of GetSignatureHelpItems)
const SIGNATURE_HELP_TRIGGER_REASON_KIND_NONE: i32 = 0; // was undefined
const SIGNATURE_HELP_TRIGGER_REASON_KIND_INVOKED: i32 = 1; // was "invoked"
const SIGNATURE_HELP_TRIGGER_REASON_KIND_CHARACTER_TYPED: i32 = 2; // was "characterTyped"
const SIGNATURE_HELP_TRIGGER_REASON_KIND_RETRIGGERED: i32 = 3; // was "retrigger"

impl LanguageService {
    // Go: ls/signaturehelp.go:49 ProvideSignatureHelp
    // PORT: Go passes `documentURI` by value and `context` as a nil-able
    // pointer; here a reference and an `Option` reference.
    pub fn provide_signature_help(
        &self,
        ctx: &Context,
        document_uri: &lsproto::DocumentUri,
        position: lsproto::Position,
        context: Option<&lsproto::SignatureHelpContext>,
    ) -> Result<lsproto::SignatureHelpResponse, GoError> {
        let (program, source_file) = self.get_program_and_file(document_uri);
        let positions = lsconv::from_lsp_position_for_source_file(
            &self.converters,
            source_file,
            position,
            Feature::SIGNATURE_HELP,
        );
        for projection in &positions {
            if !projection.fidelity.is_single_segment() {
                continue;
            }
            let items = self.get_signature_help_items(
                ctx,
                projection.position,
                program,
                projection.script,
                context,
            );
            if items.is_some() {
                return Ok(lsproto::SignatureHelpOrNull {
                    signature_help: items,
                });
            }
        }
        Ok(lsproto::SignatureHelpOrNull::default())
    }

    // Go: ls/signaturehelp.go:75 GetSignatureHelpItems
    pub fn get_signature_help_items(
        &self,
        ctx: &Context,
        position: i32,
        program: &compiler::NewProgram,
        source_file: Node,
        context: Option<&lsproto::SignatureHelpContext>,
    ) -> Option<lsproto::SignatureHelp> {
        // Go: defer done(). `done` releases the checker when it drops at the end of scope.
        let (checker, done) = ls_program::get_type_checker_for_file(program, ctx, source_file);
        let type_checker = &mut *checker.borrow_mut();

        // Decide whether to show signature help
        let starting_token = astnav::find_preceding_token(source_file, position);
        if starting_token.is_nil() {
            // We are at the beginning of the file
            return None;
        }

        // Emulate VS Code's toTsTriggerReason.
        let mut trigger_reason_kind = SIGNATURE_HELP_TRIGGER_REASON_KIND_NONE;
        if let Some(context) = context {
            if context.trigger_kind == lsproto::SignatureHelpTriggerKind::TRIGGER_CHARACTER {
                if context.trigger_character.is_some() {
                    if context.is_retrigger {
                        trigger_reason_kind = SIGNATURE_HELP_TRIGGER_REASON_KIND_RETRIGGERED;
                    } else {
                        trigger_reason_kind = SIGNATURE_HELP_TRIGGER_REASON_KIND_CHARACTER_TYPED;
                    }
                } else {
                    trigger_reason_kind = SIGNATURE_HELP_TRIGGER_REASON_KIND_INVOKED;
                }
            } else if context.trigger_kind == lsproto::SignatureHelpTriggerKind::CONTENT_CHANGE {
                if context.is_retrigger {
                    trigger_reason_kind = SIGNATURE_HELP_TRIGGER_REASON_KIND_RETRIGGERED;
                } else {
                    trigger_reason_kind = SIGNATURE_HELP_TRIGGER_REASON_KIND_CHARACTER_TYPED;
                }
            } else if context.trigger_kind == lsproto::SignatureHelpTriggerKind::INVOKED {
                trigger_reason_kind = SIGNATURE_HELP_TRIGGER_REASON_KIND_INVOKED;
            } else {
                trigger_reason_kind = SIGNATURE_HELP_TRIGGER_REASON_KIND_INVOKED;
            }
        }

        // Only need to be careful if the user typed a character and signature help wasn't showing.
        let only_use_syntactic_owners =
            trigger_reason_kind == SIGNATURE_HELP_TRIGGER_REASON_KIND_CHARACTER_TYPED;

        // Bail out quickly in the middle of a string or comment, don't provide signature help unless the user explicitly requested it.
        if only_use_syntactic_owners
            && (is_in_string(source_file, position, starting_token)
                || is_in_comment(source_file, position, starting_token).is_some())
        {
            return None;
        }

        let is_manually_invoked = trigger_reason_kind == SIGNATURE_HELP_TRIGGER_REASON_KIND_INVOKED;
        let argument_info = get_containing_argument_info(
            starting_token,
            source_file,
            type_checker,
            is_manually_invoked,
            position,
        )?;

        if ctx.err().is_some() {
            return None;
        }

        // Extra syntactic and semantic filtering of signature help
        let candidate_info = get_candidate_or_type_info(
            &argument_info,
            type_checker,
            source_file,
            starting_token,
            only_use_syntactic_owners,
        );

        if ctx.err().is_some() {
            return None;
        }

        let Some(candidate_info) = candidate_info else {
            // For JS files, try a fallback that searches all source files for declarations
            // with matching names that have call signatures. This is a heuristic for untyped JS code.
            if is_source_file_js(source_file) {
                return self.create_js_signature_help_items(
                    ctx,
                    &argument_info,
                    program,
                    type_checker,
                );
            }
            return None;
        };

        // return typeChecker.runWithCancellationToken(cancellationToken, typeChecker =>
        if let Some(inner) = &candidate_info.candidate_info {
            return self.create_signature_help_items(
                ctx,
                &inner.candidates,
                inner.resolved_signature,
                &argument_info,
                source_file,
                type_checker,
                only_use_syntactic_owners,
            );
        }
        create_type_help_items(
            ctx,
            candidate_info.type_info,
            &argument_info,
            source_file,
            type_checker,
        )
    }
}

// Go: ls/signaturehelp.go:169 createTypeHelpItems
pub fn create_type_help_items(
    ctx: &Context,
    symbol: SymbolId,
    argument_info: &ArgumentListInfo,
    source_file: Node,
    c: &mut Checker,
) -> Option<lsproto::SignatureHelp> {
    let type_parameters =
        c.get_local_type_parameters_of_class_or_interface_or_type_alias_exported(symbol);
    // PORT: Go tests `typeParameters == nil`. Go returns nil exactly when no
    // type parameter was appended, so an empty Vec is the same test.
    if type_parameters.is_empty() {
        return None;
    }
    let item = get_type_help_item(
        symbol,
        &type_parameters,
        get_enclosing_declaration_from_invocation(&argument_info.invocation),
        source_file,
        c,
    );

    // Check client capabilities for activeParameter handling
    let caps = lsproto::get_client_capabilities(ctx);
    let sig_info_caps = &caps.text_document.signature_help.signature_information;
    let supports_per_signature_active_param = sig_info_caps.active_parameter_support;

    // Converting signatureHelpParameter to *lsproto.ParameterInformation
    let parameters: Vec<Option<lsproto::ParameterInformation>> = item
        .parameters
        .iter()
        .map(|param| Some(param.parameter_info.clone()))
        .collect();

    let mut sig_info = lsproto::SignatureInformation {
        label: item.label.clone(),
        documentation: None,
        parameters: Some(parameters),
        ..Default::default()
    };

    // If client supports per-signature activeParameter, set it on SignatureInformation
    if supports_per_signature_active_param && !item.parameters.is_empty() {
        sig_info.active_parameter = Some(lsproto::UintegerOrNull {
            uinteger: Some(argument_info.argument_index as u32),
        });
    }

    let mut help = lsproto::SignatureHelp {
        signatures: vec![Some(sig_info)],
        active_signature: Some(0),
        ..Default::default()
    };

    // If client doesn't support per-signature activeParameter, set it on the top-level SignatureHelp
    if !supports_per_signature_active_param && !item.parameters.is_empty() {
        help.active_parameter = Some(lsproto::UintegerOrNull {
            uinteger: Some(argument_info.argument_index as u32),
        });
    }

    Some(help)
}

// Go: ls/signaturehelp.go:211 getTypeHelpItem
pub fn get_type_help_item(
    symbol: SymbolId,
    type_parameter: &[TypeId],
    enclosing_declaration: Node,
    source_file: Node,
    c: &mut Checker,
) -> SignatureInformation {
    // ts#64649: one emit context and one node builder for the item.
    let emit_context = new_emit_context();
    let builder = Rc::new(RefCell::new(new_node_builder(c, emit_context.clone())));
    let mut printer = new_printer(
        PrinterOptions {
            new_line: NewLineKind::LF,
            ..Default::default()
        },
        PrintHandlers::default(),
        Some(emit_context),
    );

    let mut parameters: Vec<SignatureHelpParameter> = Vec::with_capacity(type_parameter.len());
    for &type_param in type_parameter {
        parameters.push(create_signature_help_parameter_for_type_parameter(
            type_param,
            source_file,
            enclosing_declaration,
            c,
            &builder,
            &mut printer,
        ));
    }

    // Creating display label
    let mut display_parts = String::new();
    display_parts.push_str(&c.symbol_to_string_exported(symbol));
    if !parameters.is_empty() {
        display_parts.push_str(token_to_string(SyntaxKind::LessThanToken));
        for (i, type_parameter) in parameters.iter().enumerate() {
            if i > 0 {
                display_parts.push_str(", ");
            }
            display_parts.push_str(
                type_parameter
                    .parameter_info
                    .label
                    .string
                    .as_deref()
                    .unwrap_or_else(|| crate::core::go_nil_dereference()),
            );
        }
        display_parts.push_str(token_to_string(SyntaxKind::GreaterThanToken));
    }

    SignatureInformation {
        label: display_parts,
        documentation: None,
        parameters,
        is_variadic: false,
        colorized_runs: Vec::new(),
    }
}

impl LanguageService {
    // Go: ls/signaturehelp.go:244 createJSSignatureHelpItems
    // createJSSignatureHelpItems is a fallback for JavaScript files when normal signature help
    // doesn't produce results. It searches all source files for declarations with matching names
    // that have call signatures.
    pub fn create_js_signature_help_items(
        &self,
        ctx: &Context,
        argument_info: &ArgumentListInfo,
        program: &compiler::NewProgram,
        c: &mut Checker,
    ) -> Option<lsproto::SignatureHelp> {
        if argument_info.invocation.contextual_invocation.is_some() {
            return None;
        }
        // See if we can find some symbol with the call expression name that has call signatures.
        let expression = get_expression_from_invocation(argument_info);
        if !is_property_access_expression(expression) {
            return None;
        }
        let name = expression.name().text();
        if name.is_empty() {
            return None;
        }

        for sf in program.get_source_files() {
            let result = self.find_signature_help_from_named_declarations(
                ctx,
                sf.root,
                name,
                argument_info,
                c,
            );
            if result.is_some() {
                return result;
            }
        }
        None
    }

    // Go: ls/signaturehelp.go:267 findSignatureHelpFromNamedDeclarations
    pub fn find_signature_help_from_named_declarations(
        &self,
        ctx: &Context,
        source_file: Node,
        name: &str,
        argument_info: &ArgumentListInfo,
        c: &mut Checker,
    ) -> Option<lsproto::SignatureHelp> {
        // PORT: Go uses a recursive closure `visit` that captures `result`. Here
        // it is a nested function that takes the captured values.
        #[allow(clippy::too_many_arguments)]
        fn visit(
            l: &LanguageService,
            ctx: &Context,
            node: Node,
            source_file: Node,
            name: &str,
            argument_info: &ArgumentListInfo,
            c: &mut Checker,
            result: &mut Option<lsproto::SignatureHelp>,
        ) -> bool {
            if result.is_some() {
                return true;
            }
            if get_declaration_name(node) == name {
                let symbol = node.symbol();
                if symbol.is_some() {
                    let t = c.get_type_of_symbol_at_location(symbol, node);
                    if t.is_some() {
                        let call_signatures = c.get_call_signatures(t);
                        if !call_signatures.is_empty() {
                            *result = l.create_signature_help_items(
                                ctx,
                                &call_signatures,
                                call_signatures[0],
                                argument_info,
                                source_file,
                                c,
                                true, /*useFullPrefix*/
                            );
                            if result.is_some() {
                                return true;
                            }
                        }
                    }
                }
            }
            node.for_each_child(|child| {
                visit(l, ctx, child, source_file, name, argument_info, c, result)
            });
            result.is_some()
        }
        let mut result: Option<lsproto::SignatureHelp> = None;
        visit(
            self,
            ctx,
            source_file,
            source_file,
            name,
            argument_info,
            c,
            &mut result,
        );
        result
    }

    // Go: ls/signaturehelp.go:295 createSignatureHelpItems
    #[allow(clippy::too_many_arguments)]
    pub fn create_signature_help_items(
        &self,
        ctx: &Context,
        candidates: &[SignatureId],
        resolved_signature: SignatureId,
        argument_info: &ArgumentListInfo,
        source_file: Node,
        c: &mut Checker,
        use_full_prefix: bool,
    ) -> Option<lsproto::SignatureHelp> {
        let caps = lsproto::get_client_capabilities(ctx);
        let doc_format = lsproto::preferred_markup_kind(
            &caps
                .text_document
                .signature_help
                .signature_information
                .documentation_format,
        );
        let vs_capability = caps.vs_supports_visual_studio_extensions;

        let enclosing_declaration =
            get_enclosing_declaration_from_invocation(&argument_info.invocation);
        if enclosing_declaration.is_nil() {
            return None;
        }
        let mut call_target_symbol: SymbolId;
        if let Some(contextual_invocation) = &argument_info.invocation.contextual_invocation {
            call_target_symbol = contextual_invocation.symbol;
        } else {
            call_target_symbol =
                c.get_symbol_at_location_exported(get_expression_from_invocation(argument_info));
            if call_target_symbol.is_nil()
                && use_full_prefix
                && c.sig(resolved_signature).declaration.is_some()
            {
                call_target_symbol = c.sig(resolved_signature).declaration.symbol();
            }
        }

        let mut call_target_display_parts = String::new();
        // A contextual signature for an anonymous inline function type (e.g. a callback
        // argument) has a synthetic symbol whose name is an internal marker such as
        // "\xFEtype". There is no meaningful name to show, so render the signature with
        // no prefix (as we already do when there is no call target symbol) rather than
        // leaking the internal name.
        if call_target_symbol.is_some()
            && !c
                .sym(call_target_symbol)
                .name
                .starts_with(INTERNAL_SYMBOL_NAME_PREFIX)
        {
            if use_full_prefix {
                call_target_display_parts.push_str(&c.symbol_to_string_ex(
                    call_target_symbol,
                    source_file,
                    SymbolFlags::NONE,
                    SymbolFormatFlags::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE,
                ));
            } else {
                call_target_display_parts
                    .push_str(&c.symbol_to_string_exported(call_target_symbol));
            }
        }
        let mut items: Vec<Vec<SignatureInformation>> = Vec::with_capacity(candidates.len());
        for &candidate_signature in candidates {
            items.push(self.get_signature_help_item(
                candidate_signature,
                argument_info.is_type_parameter_list,
                &call_target_display_parts,
                call_target_symbol,
                enclosing_declaration,
                source_file,
                c,
                &doc_format,
                vs_capability,
            ));
        }

        let mut selected_item_index: i32 = 0;
        let mut item_seen: i32 = 0;
        for i in 0..items.len() {
            let item = &items[i];
            if candidates[i] == resolved_signature {
                selected_item_index = item_seen;
                if item.len() > 1 {
                    let mut count: i32 = 0;
                    for j in item {
                        if j.is_variadic
                            || j.parameters.len() as i32 >= argument_info.argument_count
                        {
                            selected_item_index = item_seen + count;
                            break;
                        }
                        count += 1;
                    }
                }
            }
            item_seen += item.len() as i32;
        }

        debug_assert!(selected_item_index != -1);
        let mut flattened_signatures: Vec<SignatureInformation> = Vec::new();
        for item in items {
            flattened_signatures.extend(item);
        }
        if flattened_signatures.is_empty() {
            return None;
        }

        // Check client capabilities for activeParameter handling
        let sig_info_caps = &caps.text_document.signature_help.signature_information;
        let supports_per_signature_active_param = sig_info_caps.active_parameter_support;
        let supports_null_active_param = sig_info_caps.no_active_parameter_support;

        // Converting []signatureInformation to []*lsproto.SignatureInformation
        let mut signature_information: Vec<Option<lsproto::SignatureInformation>> =
            Vec::with_capacity(flattened_signatures.len());
        for item in &flattened_signatures {
            let parameters: Vec<Option<lsproto::ParameterInformation>> = item
                .parameters
                .iter()
                .map(|param| Some(param.parameter_info.clone()))
                .collect();
            let mut documentation: Option<lsproto::StringOrMarkupContent> = None;
            if let Some(item_documentation) = &item.documentation {
                documentation = Some(lsproto::StringOrMarkupContent {
                    markup_content: Some(lsproto::MarkupContent {
                        kind: doc_format.clone(),
                        value: item_documentation.clone(),
                    }),
                    ..Default::default()
                });
            }
            let mut sig_info = lsproto::SignatureInformation {
                label: item.label.clone(),
                documentation,
                parameters: Some(parameters),
                ..Default::default()
            };

            // Set VS-specific colorized label if we have classified runs
            if !item.colorized_runs.is_empty() {
                sig_info.vs_colorized_label = Some(lsproto::VSClassifiedTextElement {
                    runs: item.colorized_runs.iter().cloned().map(Some).collect(),
                    ..Default::default()
                });
            }

            // If client supports per-signature activeParameter, set it on each SignatureInformation
            if supports_per_signature_active_param {
                sig_info.active_parameter = self.compute_active_parameter(
                    item,
                    argument_info.argument_index,
                    supports_null_active_param,
                );
            }

            signature_information.push(Some(sig_info));
        }

        let mut help = lsproto::SignatureHelp {
            signatures: signature_information,
            active_signature: Some(selected_item_index as u32),
            ..Default::default()
        };

        // If client doesn't support per-signature activeParameter, set it on the top-level SignatureHelp
        if !supports_per_signature_active_param {
            let active_signature = &flattened_signatures[selected_item_index as usize];
            help.active_parameter = self.compute_active_parameter(
                active_signature,
                argument_info.argument_index,
                supports_null_active_param,
            );
        }

        Some(help)
    }

    // Go: ls/signaturehelp.go:419 computeActiveParameter
    // computeActiveParameter calculates the active parameter index for a signature,
    // handling variadic signatures and null support appropriately.
    pub fn compute_active_parameter(
        &self,
        sig: &SignatureInformation,
        argument_index: i32,
        supports_null: bool,
    ) -> Option<lsproto::UintegerOrNull> {
        let param_count = sig.parameters.len() as i32;
        if param_count == 0 {
            // No parameters, return nil (omit the field)
            return None;
        }

        let mut active_param = argument_index as u32;

        if sig.is_variadic {
            let first_rest = sig
                .parameters
                .iter()
                .position(|p| p.is_rest)
                .map_or(-1, |i| i as i32);
            if -1 < first_rest && first_rest < param_count - 1 {
                // Middle rest parameter - we can't accurately highlight, so indicate "no active parameter"
                if supports_null {
                    return Some(lsproto::UintegerOrNull::default()); // null means "no parameter is active"
                }
                // Client doesn't support null, use out-of-range index (defaults to 0 per LSP spec)
                return Some(lsproto::UintegerOrNull {
                    uinteger: Some(param_count as u32),
                });
            }
            // Clamp to last parameter for trailing rest parameters
            if active_param > (param_count - 1) as u32 {
                active_param = (param_count - 1) as u32;
            }
        }

        Some(lsproto::UintegerOrNull {
            uinteger: Some(active_param),
        })
    }

    // Go: ls/signaturehelp.go:451 getSignatureHelpItem
    #[allow(clippy::too_many_arguments)]
    pub fn get_signature_help_item(
        &self,
        candidate: SignatureId,
        is_type_parameter_list: bool,
        call_target_symbol: &str,
        call_target_sym: SymbolId,
        enclosing_declaration: Node,
        source_file: Node,
        c: &mut Checker,
        doc_format: &lsproto::MarkupKind,
        vs_capability: bool,
    ) -> Vec<SignatureInformation> {
        // ts#64649: one emit context for the parameters and the return type.
        let emit_context = new_emit_context();
        let infos = if is_type_parameter_list {
            self.item_info_for_type_parameters(
                candidate,
                c,
                enclosing_declaration,
                source_file,
                doc_format,
                vs_capability,
                &emit_context,
            )
        } else {
            self.item_info_for_parameters(
                candidate,
                c,
                enclosing_declaration,
                source_file,
                doc_format,
                vs_capability,
                &emit_context,
            )
        };

        let suffix_dpw = return_type_to_display_parts(
            candidate,
            c,
            enclosing_declaration,
            source_file,
            vs_capability,
            &emit_context,
        );

        // Generate documentation from the signature's declaration
        let mut documentation: Option<String> = None;
        let declaration = c.sig(candidate).declaration;
        if declaration.is_some() {
            let doc = get_documentation_from_declaration(
                &self.documentation_location_mapper(Feature::SIGNATURE_HELP),
                c,
                SymbolId::NIL,
                declaration,
                Node::NIL,
                doc_format,
                true, /*commentOnly*/
            );
            if !doc.is_empty() {
                documentation = Some(doc);
            }
        }

        let mut result: Vec<SignatureInformation> = Vec::with_capacity(infos.len());
        for info in &infos {
            let label_dpw = new_display_parts_writer(vs_capability);
            if !call_target_symbol.is_empty() {
                label_dpw
                    .borrow_mut()
                    .write_symbol(call_target_symbol, call_target_sym);
            }
            label_dpw.borrow_mut().write_from(&info.writer.borrow());
            label_dpw.borrow_mut().write_from(&suffix_dpw.borrow());

            // PORT: the display parts writer records symbol runs while the
            // printer runs and classifies them from the checker when the runs
            // are read.
            let label = label_dpw.borrow().string();
            let colorized_runs = label_dpw.borrow().get_runs(c);
            result.push(SignatureInformation {
                label,
                documentation: documentation.clone(),
                parameters: info.parameters.clone(),
                is_variadic: info.is_variadic,
                colorized_runs,
            });
        }
        result
    }
}

// Go: ls/signaturehelp.go:491 returnTypeToDisplayParts
pub fn return_type_to_display_parts(
    candidate_signature: SignatureId,
    c: &mut Checker,
    enclosing_declaration: Node,
    source_file: Node,
    vs_capability: bool,
    emit_context: &Rc<EmitContext>,
) -> Rc<RefCell<DisplayPartsWriter>> {
    let dpw = new_display_parts_writer(vs_capability);

    // Add ": " prefix
    dpw.borrow_mut().write_punctuation(": ");

    let predicate = c.get_type_predicate_of_signature_exported(candidate_signature);
    if predicate.is_some() {
        let text = c.type_predicate_to_string_exported(predicate);
        dpw.borrow_mut().write(&text);
    } else {
        let return_type = c.get_return_type_of_signature_exported(candidate_signature);
        let type_node = c.type_to_type_node_exported(
            return_type,
            enclosing_declaration,
            SIGNATURE_HELP_NODE_BUILDER_FLAGS,
            None,
        );
        if type_node.is_some() {
            let mut p = new_printer(
                PrinterOptions {
                    new_line: NewLineKind::LF,
                    ..Default::default()
                },
                PrintHandlers::default(),
                Some(emit_context.clone()),
            );
            // Use a temporary writer for p.Write since the printer calls Clear() on its writer
            let temp_dpw = new_display_parts_writer(vs_capability);
            p.write_exported(type_node, source_file, temp_dpw.clone(), None);
            dpw.borrow_mut().write_from(&temp_dpw.borrow());
        } else {
            let text = c.type_to_string_exported(return_type);
            dpw.borrow_mut().write(&text);
        }
    }
    dpw
}

impl LanguageService {
    // Go: ls/signaturehelp.go:516 itemInfoForTypeParameters
    pub fn item_info_for_type_parameters(
        &self,
        candidate_signature: SignatureId,
        c: &mut Checker,
        enclosing_declaration: Node,
        source_file: Node,
        doc_format: &lsproto::MarkupKind,
        vs_capability: bool,
        emit_context: &Rc<EmitContext>,
    ) -> Vec<SignatureHelpItemInfo> {
        // ts#64649: the caller's emit context and one node builder.
        let builder = Rc::new(RefCell::new(new_node_builder(c, emit_context.clone())));
        let mut p = new_printer(
            PrinterOptions {
                new_line: NewLineKind::LF,
                ..Default::default()
            },
            PrintHandlers::default(),
            Some(emit_context.clone()),
        );

        let type_parameters: Vec<TypeId> = if c.sig(candidate_signature).target.is_some() {
            let target = c.sig(candidate_signature).target;
            c.sig(target).type_parameters.clone()
        } else {
            c.sig(candidate_signature).type_parameters.clone()
        };
        let mut signature_help_type_parameters: Vec<SignatureHelpParameter> =
            Vec::with_capacity(type_parameters.len());
        for &type_parameter in &type_parameters {
            signature_help_type_parameters.push(
                create_signature_help_parameter_for_type_parameter(
                    type_parameter,
                    source_file,
                    enclosing_declaration,
                    c,
                    &builder,
                    &mut p,
                ),
            );
        }

        let mut this_parameter: Vec<SignatureHelpParameter> = Vec::new();
        let candidate_this_parameter = c.sig(candidate_signature).this_parameter;
        if candidate_this_parameter.is_some() {
            this_parameter = vec![self.create_signature_help_parameter_for_parameter(
                candidate_this_parameter,
                enclosing_declaration,
                &builder,
                &mut p,
                source_file,
                c,
                doc_format,
            )];
        }

        // Creating type parameter display label
        let dpw = new_display_parts_writer(vs_capability);

        let less_than_token = token_to_string(SyntaxKind::LessThanToken);
        dpw.borrow_mut().write_punctuation(less_than_token);
        for (i, type_parameter) in signature_help_type_parameters.iter().enumerate() {
            if i > 0 {
                dpw.borrow_mut().write_punctuation(", ");
            }
            let label = type_parameter
                .parameter_info
                .label
                .string
                .clone()
                .unwrap_or_else(|| crate::core::go_nil_dereference());
            dpw.borrow_mut()
                .write_classified(&label, lsproto::ClassificationTypeName::TYPE_PARAMETER_NAME);
        }
        let greater_than_token = token_to_string(SyntaxKind::GreaterThanToken);
        dpw.borrow_mut().write_punctuation(greater_than_token);

        // Creating display label for parameters like, (a: string, b: number)
        let lists = c.get_expanded_parameters_exported(candidate_signature, false);
        if !lists.is_empty() {
            let open_paren = token_to_string(SyntaxKind::OpenParenToken);
            dpw.borrow_mut().write_punctuation(open_paren);
        }

        let mut result: Vec<SignatureHelpItemInfo> = Vec::with_capacity(lists.len());
        for parameter_list in &lists {
            let param_dpw = new_display_parts_writer(vs_capability);
            param_dpw.borrow_mut().write_from(&dpw.borrow());

            // PORT: Go appends to `thisParameter` and never reads the result;
            // the per-parameter work below still runs for its checker effects.
            let mut parameters = this_parameter.clone();
            for (j, &param) in parameter_list.iter().enumerate() {
                let param_node = c.node_builder_symbol_to_parameter_declaration(
                    &builder,
                    param,
                    enclosing_declaration,
                    SIGNATURE_HELP_NODE_BUILDER_FLAGS,
                    InternalNodeBuilderFlags::NONE,
                    None,
                );

                if j > 0 {
                    param_dpw.borrow_mut().write_punctuation(", ");
                }
                // Use a temporary writer for p.Write since the printer calls Clear() on its writer
                let temp_dpw = new_display_parts_writer(vs_capability);
                p.write_exported(param_node, source_file, temp_dpw.clone(), None);
                let param_label = temp_dpw.borrow().string();
                param_dpw.borrow_mut().write_from(&temp_dpw.borrow());

                let parameter = self.create_signature_help_parameter_from_label(
                    param,
                    &param_label,
                    c,
                    doc_format,
                );
                parameters.push(parameter);
            }
            let close_paren = token_to_string(SyntaxKind::CloseParenToken);
            param_dpw.borrow_mut().write_punctuation(close_paren);

            result.push(SignatureHelpItemInfo {
                is_variadic: false,
                parameters: signature_help_type_parameters.clone(),
                writer: param_dpw,
            });
        }
        result
    }

    // Go: ls/signaturehelp.go:592 itemInfoForParameters
    pub fn item_info_for_parameters(
        &self,
        candidate_signature: SignatureId,
        c: &mut Checker,
        enclosing_declaratipn: Node,
        source_file: Node,
        doc_format: &lsproto::MarkupKind,
        vs_capability: bool,
        emit_context: &Rc<EmitContext>,
    ) -> Vec<SignatureHelpItemInfo> {
        // ts#64649: the caller's emit context and one node builder.
        let builder = Rc::new(RefCell::new(new_node_builder(c, emit_context.clone())));
        let mut p = new_printer(
            PrinterOptions {
                new_line: NewLineKind::LF,
                ..Default::default()
            },
            PrintHandlers::default(),
            Some(emit_context.clone()),
        );

        let candidate_type_parameters = c.sig(candidate_signature).type_parameters.clone();
        let mut signature_help_type_parameters: Vec<SignatureHelpParameter> =
            Vec::with_capacity(candidate_type_parameters.len());
        if !candidate_type_parameters.is_empty() {
            for &type_parameter in &candidate_type_parameters {
                signature_help_type_parameters.push(
                    create_signature_help_parameter_for_type_parameter(
                        type_parameter,
                        source_file,
                        enclosing_declaratipn,
                        c,
                        &builder,
                        &mut p,
                    ),
                );
            }
        }

        // Creating display label for type parameters like, <T, U>
        let dpw = new_display_parts_writer(vs_capability);

        if !signature_help_type_parameters.is_empty() {
            let less_than_token = token_to_string(SyntaxKind::LessThanToken);
            dpw.borrow_mut().write_punctuation(less_than_token);
            for (i, type_parameter) in signature_help_type_parameters.iter().enumerate() {
                if i > 0 {
                    dpw.borrow_mut().write_punctuation(", ");
                }
                let label = type_parameter
                    .parameter_info
                    .label
                    .string
                    .clone()
                    .unwrap_or_else(|| crate::core::go_nil_dereference());
                dpw.borrow_mut()
                    .write_classified(&label, lsproto::ClassificationTypeName::TYPE_PARAMETER_NAME);
            }
            let greater_than_token = token_to_string(SyntaxKind::GreaterThanToken);
            dpw.borrow_mut().write_punctuation(greater_than_token);
        }

        // Creating display parts for parameters. For example, (a: string, b: number)
        let lists = c.get_expanded_parameters_exported(candidate_signature, false);
        if !lists.is_empty() {
            let open_paren = token_to_string(SyntaxKind::OpenParenToken);
            dpw.borrow_mut().write_punctuation(open_paren);
        }

        let mut result: Vec<SignatureHelpItemInfo> = Vec::with_capacity(lists.len());
        for parameter_list in &lists {
            let mut parameters: Vec<SignatureHelpParameter> =
                Vec::with_capacity(parameter_list.len());
            let param_dpw = new_display_parts_writer(vs_capability);
            param_dpw.borrow_mut().write_from(&dpw.borrow());

            for (j, &param) in parameter_list.iter().enumerate() {
                let param_node = c.node_builder_symbol_to_parameter_declaration(
                    &builder,
                    param,
                    enclosing_declaratipn,
                    SIGNATURE_HELP_NODE_BUILDER_FLAGS,
                    InternalNodeBuilderFlags::NONE,
                    None,
                );

                if j > 0 {
                    param_dpw.borrow_mut().write_punctuation(", ");
                }
                // Use a temporary writer for p.Write since the printer calls Clear() on its writer
                let temp_dpw = new_display_parts_writer(vs_capability);
                p.write_exported(param_node, source_file, temp_dpw.clone(), None);
                let param_label = temp_dpw.borrow().string();
                param_dpw.borrow_mut().write_from(&temp_dpw.borrow());

                let parameter = self.create_signature_help_parameter_from_label(
                    param,
                    &param_label,
                    c,
                    doc_format,
                );
                parameters.push(parameter);
            }
            let close_paren = token_to_string(SyntaxKind::CloseParenToken);
            param_dpw.borrow_mut().write_punctuation(close_paren);

            // Go: the isVariadic closure (signaturehelp.go:600), called here.
            let is_variadic = if !c.has_effective_rest_parameter_exported(candidate_signature) {
                false
            } else if lists.len() == 1 {
                true
            } else {
                match parameter_list.last() {
                    Some(&last) => {
                        last.is_some()
                            && c.sym(last)
                                .check_flags
                                .intersects(CheckFlags::REST_PARAMETER)
                    }
                    None => false,
                }
            };

            result.push(SignatureHelpItemInfo {
                is_variadic,
                parameters,
                writer: param_dpw,
            });
        }
        result
    }
}

// Go: ls/signaturehelp.go:671 signatureHelpNodeBuilderFlags
pub const SIGNATURE_HELP_NODE_BUILDER_FLAGS: NodeBuilderFlags =
    NodeBuilderFlags::OMIT_PARAMETER_MODIFIERS
        .union(NodeBuilderFlags::IGNORE_ERRORS)
        .union(NodeBuilderFlags::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE);

impl LanguageService {
    // Go: ls/signaturehelp.go:674 createSignatureHelpParameterFromLabel
    // createSignatureHelpParameterFromLabel creates a signatureHelpParameter from a pre-computed label string.
    pub fn create_signature_help_parameter_from_label(
        &self,
        parameter: SymbolId,
        label: &str,
        c: &mut Checker,
        doc_format: &lsproto::MarkupKind,
    ) -> SignatureHelpParameter {
        let check_flags = c.sym(parameter).check_flags;
        let is_optional = check_flags.intersects(CheckFlags::OPTIONAL_PARAMETER);
        let is_rest = check_flags.intersects(CheckFlags::REST_PARAMETER);
        let mut documentation: Option<lsproto::StringOrMarkupContent> = None;
        let value_declaration = c.sym(parameter).value_declaration;
        if value_declaration.is_some() {
            let doc = get_documentation_from_declaration(
                &self.documentation_location_mapper(Feature::SIGNATURE_HELP),
                c,
                SymbolId::NIL,
                value_declaration,
                Node::NIL,
                doc_format,
                true, /*commentOnly*/
            );
            if !doc.is_empty() {
                documentation = Some(lsproto::StringOrMarkupContent {
                    markup_content: Some(lsproto::MarkupContent {
                        kind: doc_format.clone(),
                        value: doc,
                    }),
                    ..Default::default()
                });
            }
        }
        SignatureHelpParameter {
            parameter_info: lsproto::ParameterInformation {
                label: lsproto::StringOrTuple {
                    string: Some(label.to_string()),
                    ..Default::default()
                },
                documentation,
            },
            is_rest,
            is_optional,
        }
    }

    // Go: ls/signaturehelp.go:699 createSignatureHelpParameterForParameter
    pub fn create_signature_help_parameter_for_parameter(
        &self,
        parameter: SymbolId,
        enclosing_declaratipn: Node,
        builder: &Rc<RefCell<NodeBuilder>>,
        p: &mut Printer,
        source_file: Node,
        c: &mut Checker,
        doc_format: &lsproto::MarkupKind,
    ) -> SignatureHelpParameter {
        // ts#64649: the caller's node builder.
        let parameter_node = c.node_builder_symbol_to_parameter_declaration(
            builder,
            parameter,
            enclosing_declaratipn,
            SIGNATURE_HELP_NODE_BUILDER_FLAGS,
            InternalNodeBuilderFlags::NONE,
            None,
        );
        let display = p.emit(parameter_node, source_file);
        self.create_signature_help_parameter_from_label(parameter, &display, c, doc_format)
    }
}

// Go: ls/signaturehelp.go:705 createSignatureHelpParameterForTypeParameter
pub fn create_signature_help_parameter_for_type_parameter(
    t: TypeId,
    source_file: Node,
    enclosing_declaration: Node,
    c: &mut Checker,
    builder: &Rc<RefCell<NodeBuilder>>,
    p: &mut Printer,
) -> SignatureHelpParameter {
    // ts#64649: the caller's node builder.
    let type_parameter_node = c.node_builder_type_parameter_to_declaration(
        builder,
        t,
        enclosing_declaration,
        SIGNATURE_HELP_NODE_BUILDER_FLAGS,
        InternalNodeBuilderFlags::NONE,
        None,
    );
    let display = p.emit(type_parameter_node, source_file);
    SignatureHelpParameter {
        parameter_info: lsproto::ParameterInformation {
            label: lsproto::StringOrTuple {
                string: Some(display),
                ..Default::default()
            },
            ..Default::default()
        },
        is_rest: false,
        is_optional: false,
    }
}

// Go: ls/signaturehelp.go:713 signatureInformation
// Represents the signature of something callable. A signature
// can have a label, like a function-name, a doc-comment, and
// a set of parameters.
#[derive(Clone, Debug, Default)]
pub struct SignatureInformation {
    // The Label of this signature. Will be shown in
    // the UI.
    pub label: String,
    // The human-readable doc-comment of this signature. Will be shown
    // in the UI but can be omitted.
    pub documentation: Option<String>,
    // The Parameters of this signature.
    pub parameters: Vec<SignatureHelpParameter>,
    // Needed only here, not in lsp
    pub is_variadic: bool,
    // Classified text runs for VS colorized label
    pub colorized_runs: Vec<lsproto::VSClassifiedTextRun>,
}

// Go: ls/signaturehelp.go:728 signatureHelpItemInfo
pub struct SignatureHelpItemInfo {
    pub is_variadic: bool,
    pub parameters: Vec<SignatureHelpParameter>,
    pub writer: Rc<RefCell<DisplayPartsWriter>>,
}

// Go: ls/signaturehelp.go:734 signatureHelpParameter
// PORT: Go shares the `*lsproto.ParameterInformation` pointer between copies;
// nothing mutates it after creation, so a value is used.
#[derive(Clone, Debug, Default)]
pub struct SignatureHelpParameter {
    pub parameter_info: lsproto::ParameterInformation,
    pub is_rest: bool,
    pub is_optional: bool,
}

// Go: ls/signaturehelp.go:740 getEnclosingDeclarationFromInvocation
pub fn get_enclosing_declaration_from_invocation(invocation: &Invocation) -> Node {
    if let Some(call_invocation) = &invocation.call_invocation {
        call_invocation.node
    } else if let Some(type_args_invocation) = &invocation.type_args_invocation {
        type_args_invocation.called
    } else {
        invocation
            .contextual_invocation
            .as_ref()
            .unwrap_or_else(|| crate::core::go_nil_dereference())
            .node
    }
}

// Go: ls/signaturehelp.go:750 getExpressionFromInvocation
pub fn get_expression_from_invocation(argument_info: &ArgumentListInfo) -> Node {
    if let Some(call_invocation) = &argument_info.invocation.call_invocation {
        return get_invoked_expression(call_invocation.node);
    }
    argument_info
        .invocation
        .type_args_invocation
        .as_ref()
        .unwrap_or_else(|| crate::core::go_nil_dereference())
        .called
}

// Go: ls/signaturehelp.go:757 candidateInfo
#[derive(Clone, Debug, Default)]
pub struct CandidateInfo {
    pub candidates: Vec<SignatureId>,
    pub resolved_signature: SignatureId,
}

// Go: ls/signaturehelp.go:762 CandidateOrTypeInfo
#[derive(Clone, Debug, Default)]
pub struct CandidateOrTypeInfo {
    pub candidate_info: Option<CandidateInfo>,
    pub type_info: SymbolId,
}

// Go: ls/signaturehelp.go:767 getCandidateOrTypeInfo
pub fn get_candidate_or_type_info(
    info: &ArgumentListInfo,
    c: &mut Checker,
    source_file: Node,
    starting_token: Node,
    only_use_syntactic_owners: bool,
) -> Option<CandidateOrTypeInfo> {
    if let Some(call_invocation) = info.invocation.call_invocation {
        if only_use_syntactic_owners
            && !is_syntactic_owner(starting_token, call_invocation.node, source_file)
        {
            return None;
        }

        let (resolved_signature, candidates) =
            c.get_resolved_signature_for_signature_help(call_invocation.node, info.argument_count);
        if candidates.is_empty() {
            return None;
        }

        return Some(CandidateOrTypeInfo {
            candidate_info: Some(CandidateInfo {
                candidates,
                resolved_signature,
            }),
            type_info: SymbolId::NIL,
        });
    }
    if let Some(type_args_invocation) = info.invocation.type_args_invocation {
        let called = type_args_invocation.called;
        let mut container = called;
        if is_identifier(called) {
            container = called.parent();
        }

        if only_use_syntactic_owners
            && !contains_preceding_token(starting_token, source_file, container)
        {
            return None;
        }

        let candidates = get_possible_generic_signatures(called, info.argument_count, c);
        if !candidates.is_empty() {
            let resolved_signature = candidates[0];
            return Some(CandidateOrTypeInfo {
                candidate_info: Some(CandidateInfo {
                    candidates,
                    resolved_signature,
                }),
                type_info: SymbolId::NIL,
            });
        }

        let symbol = c.get_symbol_at_location_exported(called);
        if symbol.is_some() {
            return Some(CandidateOrTypeInfo {
                candidate_info: None,
                type_info: symbol,
            });
        }

        // This can happen in the case of an unresolved symbol.
        return None;
    }

    if let Some(contextual_invocation) = info.invocation.contextual_invocation {
        return Some(CandidateOrTypeInfo {
            candidate_info: Some(CandidateInfo {
                candidates: vec![contextual_invocation.signature],
                resolved_signature: contextual_invocation.signature,
            }),
            type_info: SymbolId::NIL,
        });
    }
    // Go: debug.AssertNever(info.invocation)
    // PORT: Go `%v` of the `*invocation`, whose three fields are nil here.
    crate::gostd::debug::assert_never("&{<nil> <nil> <nil>}", None);
}

// Go: ls/signaturehelp.go:828 isSyntacticOwner
pub fn is_syntactic_owner(starting_token: Node, node: Node, source_file: Node) -> bool {
    if !is_call_or_new_expression(node) {
        return false;
    }
    let invocation_children = get_children_from_non_js_doc_node(node, source_file);
    match starting_token.kind() {
        SyntaxKind::OpenParenToken | SyntaxKind::CommaToken => {
            invocation_children.contains(&starting_token)
        }
        SyntaxKind::LessThanToken => {
            contains_preceding_token(starting_token, source_file, node.expression())
        }
        _ => false,
    }
}

// Go: ls/signaturehelp.go:843 containsPrecedingToken
pub fn contains_preceding_token(starting_token: Node, source_file: Node, container: Node) -> bool {
    let pos = starting_token.pos();
    // There's a possibility that `startingToken.parent` contains only `startingToken` and
    // missing nodes, none of which are valid to be returned by `findPrecedingToken`. In that
    // case, the preceding token we want is actually higher up the tree—almost definitely the
    // next parent, but theoretically the situation with missing nodes might be happening on
    // multiple nested levels.
    let mut current_parent = starting_token.parent();
    while current_parent.is_some() {
        let preceding_token = astnav::find_preceding_token_ex(
            source_file,
            pos,
            current_parent,
            true, /*excludeJSDoc*/
        );
        if preceding_token.is_some() {
            return range_contains_range(container.loc(), preceding_token.loc());
        }
        current_parent = current_parent.parent();
    }
    false
}

// Go: ls/signaturehelp.go:861 getContainingArgumentInfo
pub fn get_containing_argument_info(
    node: Node,
    source_file: Node,
    checker: &mut Checker,
    is_manually_invoked: bool,
    position: i32,
) -> Option<ArgumentListInfo> {
    let mut first_argument_info: Option<ArgumentListInfo> = None;
    let mut n = node;
    while !is_source_file(n) && (is_manually_invoked || !is_block(n)) {
        // If the node is not a subspan of its parent, this is a big problem.
        // There have been crashes that might be caused by this violation.
        debug_assert!(
            range_contains_range(n.parent().loc(), n.loc()),
            "Not a subspan. Child: {:?}, parent: {:?}",
            n.kind(),
            n.parent().kind()
        );
        let argument_info = get_immediately_containing_argument_or_contextual_parameter_info(
            n,
            position,
            source_file,
            checker,
        );
        if let Some(argument_info) = argument_info {
            // For contextual invocations (e.g., arrow functions with contextual types),
            // always return immediately without checking the position.
            // This ensures that when inside a callback's parameter list, we show the callback's
            // signature, not the outer call's signature.
            if argument_info.invocation.contextual_invocation.is_some() {
                return Some(argument_info);
            }

            // Remember the first (innermost) argument info we find
            if first_argument_info.is_none() {
                first_argument_info = Some(argument_info);
            }

            // If the position is at the end boundary of an argument list, keep the
            // innermost call. This covers cases like foo(bar("x"|)) where the cursor is
            // still inside the inner invocation, just before its closing paren.
            if argument_info.arguments_span.end() == position {
                return Some(argument_info);
            }

            // If any call's span contains the position, return it.
            // We walk from inner to outer, so this naturally prefers the innermost call
            // when multiple calls contain the position.
            if argument_info.arguments_span.contains(position) {
                return Some(argument_info);
            }
        }
        n = n.parent();
    }

    // No call's span contains the position. Fall back to the innermost call we found.
    // This covers boundary positions that are still syntactically associated with that
    // invocation, such as being at the end of the argument list or on the close paren.
    first_argument_info
}

// Go: ls/signaturehelp.go:904 getImmediatelyContainingArgumentOrContextualParameterInfo
pub fn get_immediately_containing_argument_or_contextual_parameter_info(
    node: Node,
    position: i32,
    source_file: Node,
    checker: &mut Checker,
) -> Option<ArgumentListInfo> {
    let result = try_get_parameter_info(node, source_file, checker);
    if result.is_none() {
        return get_immediately_containing_argument_info(node, position, source_file, checker);
    }
    result
}

// Go: ls/signaturehelp.go:912 argumentListInfo
// PORT: Go `*invocation` is a pointer that nothing mutates after creation; a value here.
#[derive(Clone, Copy, Debug)]
pub struct ArgumentListInfo {
    pub is_type_parameter_list: bool,
    pub invocation: Invocation,
    pub arguments_span: TextRange,
    pub argument_index: i32,
    /** argumentCount is the *apparent* number of arguments. */
    pub argument_count: i32,
}

// Go: ls/signaturehelp.go:923 getImmediatelyContainingArgumentInfo
// Returns relevant information for the argument list and the current argument if we are
// in the argument of an invocation; returns undefined otherwise.
pub fn get_immediately_containing_argument_info(
    node: Node,
    position: i32,
    source_file: Node,
    c: &mut Checker,
) -> Option<ArgumentListInfo> {
    let parent = node.parent();
    if is_call_or_new_expression(parent) {
        // There are 3 cases to handle:
        //   1. The token introduces a list, and should begin a signature help session
        //   2. The token is either not associated with a list, or ends a list, so the session should end
        //   3. The token is buried inside a list, and should give signature help
        //
        // The following are examples of each:
        //
        //    Case 1:
        //          foo<#T, U>(#a, b)    -> The token introduces a list, and should begin a signature help session
        //    Case 2:
        //          fo#o<T, U>#(a, b)#   -> The token is either not associated with a list, or ends a list, so the session should end
        //    Case 3:
        //          foo<T#, U#>(a#, #b#) -> The token is buried inside a list, and should give signature help
        // Find out if 'node' is an argument, a type argument, or neither
        let info = get_argument_or_parameter_list_info(node, source_file, c)?;
        let list = info.list;
        let argument_index = info.argument_index;
        let argument_count = info.argument_count;
        let arguments_span = info.arguments_span;
        let mut is_type_parameter_list = false;
        let parent_type_argument_list = parent.type_argument_list();
        if parent_type_argument_list.is_some() && parent_type_argument_list.pos() == list.pos() {
            is_type_parameter_list = true;
        }
        return Some(ArgumentListInfo {
            is_type_parameter_list,
            invocation: Invocation {
                call_invocation: Some(CallInvocation { node: parent }),
                ..Default::default()
            },
            arguments_span,
            argument_index,
            argument_count,
        });
    } else if is_no_substitution_template_literal(node) && is_tagged_template_expression(parent) {
        // Check if we're actually inside the template;
        // otherwise we'll fall out and return undefined.
        if is_inside_template_literal(node, position, source_file) {
            return get_argument_list_info_for_template(parent, 0, source_file);
        }
        return None;
    } else if is_template_head(node)
        && parent.parent().kind() == SyntaxKind::TaggedTemplateExpression
    {
        let template_expression = parent;
        let tag_expression = template_expression.parent();

        let mut argument_index = 1;
        if is_inside_template_literal(node, position, source_file) {
            argument_index = 0;
        }
        return get_argument_list_info_for_template(tag_expression, argument_index, source_file);
    } else if is_template_span(parent) && is_tagged_template_expression(parent.parent().parent()) {
        let template_span = parent;
        let tag_expression = parent.parent().parent();

        // If we're just after a template tail, don't show signature help.
        if is_template_tail(node) && !is_inside_template_literal(node, position, source_file) {
            return None;
        }

        let span_index = index_of_node(
            template_span.parent().template_spans().nodes(),
            template_span,
        );
        let argument_index =
            get_argument_index_for_template_piece(span_index, node, position, source_file);

        return get_argument_list_info_for_template(tag_expression, argument_index, source_file);
    } else if is_jsx_opening_like_element(parent) {
        // Provide a signature help for JSX opening element or JSX self-closing element.
        // This is not guarantee that JSX tag-name is resolved into stateless function component. (that is done in "getSignatureHelpItems")
        // i.e
        //      export function MainButton(props: ButtonProps, context: any): JSX.Element { ... }
        //      <MainButton /*signatureHelp*/
        let attribute_span_start = parent.attributes().loc().pos();
        let attribute_span_end =
            skip_trivia(&source_file_text(source_file), parent.attributes().end());
        // PORT: Go passes the span length as the end of `core.NewTextRange`; kept as in Go.
        return Some(ArgumentListInfo {
            is_type_parameter_list: false,
            invocation: Invocation {
                call_invocation: Some(CallInvocation { node: parent }),
                ..Default::default()
            },
            arguments_span: TextRange::new(
                attribute_span_start,
                attribute_span_end - attribute_span_start,
            ),
            argument_index: 0,
            argument_count: 1,
        });
    } else {
        let type_arg_info = get_possible_type_arguments_info(node, source_file);
        if let Some(type_arg_info) = type_arg_info {
            let called = type_arg_info.called;
            let n_type_arguments = type_arg_info.n_type_arguments;
            let invoc = TypeArgsInvocation { called };
            let argument_range = TextRange::new(called.loc().pos(), node.end());
            return Some(ArgumentListInfo {
                is_type_parameter_list: true,
                invocation: Invocation {
                    type_args_invocation: Some(invoc),
                    ..Default::default()
                },
                arguments_span: argument_range,
                argument_index: n_type_arguments,
                argument_count: n_type_arguments + 1,
            });
        }
    }
    None
}

// Go: ls/signaturehelp.go:1029 getArgumentIndexForTemplatePiece
// spanIndex is either the index for a given template span.
// This does not give appropriate results for a NoSubstitutionTemplateLiteral
pub fn get_argument_index_for_template_piece(
    span_index: i32,
    node: Node,
    position: i32,
    source_file: Node,
) -> i32 {
    // Because the TemplateStringsArray is the first argument, we have to offset each substitution expression by 1.
    // There are three cases we can encounter:
    //      1. We are precisely in the template literal (argIndex = 0).
    //      2. We are in or to the right of the substitution expression (argIndex = spanIndex + 1).
    //      3. We are directly to the right of the template literal, but because we look for the token on the left,
    //          not enough to put us in the substitution expression; we should consider ourselves part of
    //          the *next* span's expression by offsetting the index (argIndex = (spanIndex + 1) + 1).
    //
    // Example: f  `# abcd $#{#  1 + 1#  }# efghi ${ #"#hello"#  }  #  `
    //              ^       ^ ^       ^   ^          ^ ^      ^     ^
    // Case:        1       1 3       2   1          3 2      2     1
    debug_assert!(
        position >= node.loc().pos(),
        "Assumed 'position' could not occur before node."
    );
    if is_template_literal_token(node) {
        if is_inside_template_literal(node, position, source_file) {
            return 0;
        }
        return span_index + 2;
    }
    span_index + 1
}

// Go: ls/signaturehelp.go:1051 getAdjustedNode
pub fn get_adjusted_node(node: Node) -> Node {
    match node.kind() {
        SyntaxKind::OpenParenToken | SyntaxKind::CommaToken => node,
        _ => find_ancestor(node.parent(), |n| {
            if is_parameter_declaration(n) {
                true
            } else if is_binding_element(n)
                || is_object_binding_pattern(n)
                || is_array_binding_pattern(n)
            {
                false
            } else {
                false
            }
        }),
    }
}

// Go: ls/signaturehelp.go:1067 contextualSignatureLocationInfo
#[derive(Clone, Copy, Debug)]
pub struct ContextualSignatureLocationInfo {
    pub contextual_type: TypeId,
    pub argument_index: i32,
    pub argument_count: i32,
    pub arguments_span: TextRange,
}

// Go: ls/signaturehelp.go:1074 getSpreadElementCount
pub fn get_spread_element_count(node: Node, c: &mut Checker) -> i32 {
    let spread_type = c.get_type_at_location(node.expression());
    if c.is_tuple_type_exported(spread_type) {
        let target = c.ty(spread_type).target();
        // PORT: Go tests `tupleType == nil`. Its type assertion panics instead
        // of returning nil, as the Rust accessor does, so the test is dropped.
        let tuple_type = c.ty(target).as_tuple_type();
        let element_flags = tuple_type.element_flags();
        let fixed_length = tuple_type.fixed_length();
        if fixed_length == 0 {
            return 0;
        }

        let first_optional_index = element_flags
            .iter()
            .position(|f| !f.intersects(ElementFlags::REQUIRED))
            .map_or(-1, |i| i as i32);
        if first_optional_index < 0 {
            return fixed_length;
        }
        return first_optional_index;
    }
    0
}

// Go: ls/signaturehelp.go:1098 getArgumentIndex
pub fn get_argument_index(
    node: Node,
    arguments: NodeList,
    source_file: Node,
    c: &mut Checker,
) -> i32 {
    get_argument_index_or_count(
        &get_token_from_node_list(arguments, node.parent(), source_file),
        node,
        c,
    )
}

// Go: ls/signaturehelp.go:1102 getArgumentCount
pub fn get_argument_count(
    node: Node,
    arguments: NodeList,
    source_file: Node,
    c: &mut Checker,
) -> i32 {
    get_argument_index_or_count(
        &get_token_from_node_list(arguments, node.parent(), source_file),
        Node::NIL,
        c,
    )
}

// Go: ls/signaturehelp.go:1106 getArgumentIndexOrCount
pub fn get_argument_index_or_count(arguments: &[Node], node: Node, c: &mut Checker) -> i32 {
    let mut argument_index: i32 = 0;
    let mut skip_comma = false;
    for &arg in arguments {
        if node.is_some() && arg == node {
            if !skip_comma && arg.kind() == SyntaxKind::CommaToken {
                argument_index += 1;
            }
            return argument_index;
        }
        if is_spread_element(arg) {
            argument_index += get_spread_element_count(arg, c);
            skip_comma = true;
            continue;
        }
        if arg.kind() != SyntaxKind::CommaToken {
            argument_index += 1;
            skip_comma = true;
            continue;
        }
        if skip_comma {
            skip_comma = false;
            continue;
        }
        argument_index += 1;
    }
    if node.is_some() {
        return argument_index;
    }
    // The argument count for a list is normally the number of non-comma children it has.
    // For example, if you have "Foo(a,b)" then there will be three children of the arg
    // list 'a' '<comma>' 'b'. So, in this case the arg count will be 2. However, there
    // is a small subtlety. If you have "Foo(a,)", then the child list will just have
    // 'a' '<comma>'. So, in the case where the last child is a comma, we increase the
    // arg count by one to compensate.
    let mut argument_count = argument_index;
    if !arguments.is_empty() && arguments[arguments.len() - 1].kind() == SyntaxKind::CommaToken {
        argument_count = argument_index + 1;
    }
    argument_count
}

// Go: ls/signaturehelp.go:1148 argumentOrParameterListInfo
#[derive(Clone, Copy, Debug)]
pub struct ArgumentOrParameterListInfo {
    pub list: NodeList,
    pub argument_index: i32,
    pub argument_count: i32,
    pub arguments_span: TextRange,
}

// Go: ls/signaturehelp.go:1155 getArgumentOrParameterListInfo
pub fn get_argument_or_parameter_list_info(
    node: Node,
    source_file: Node,
    c: &mut Checker,
) -> Option<ArgumentOrParameterListInfo> {
    let info = get_argument_or_parameter_list_and_index(node, source_file, c)?;
    let list = info.list;
    let argument_index = info.argument_index;
    let argument_count = get_argument_count(node, list, source_file, c);
    let arguments_span = get_applicable_span_for_arguments(list, node, source_file);
    Some(ArgumentOrParameterListInfo {
        list,
        argument_index,
        argument_count,
        arguments_span,
    })
}

// Go: ls/signaturehelp.go:1172 getApplicableSpanForArguments
pub fn get_applicable_span_for_arguments(
    argument_list: NodeList,
    node: Node,
    source_file: Node,
) -> TextRange {
    // We use full start and skip trivia on the end because we want to include trivia on
    // both sides. For example,
    //
    //    foo(   /*comment */     a, b, c      /*comment*/     )
    //        |                                               |
    //
    // The applicable span is from the first bar to the second bar (inclusive,
    // but not including parentheses).
    if argument_list.is_nil() && node.is_some() {
        // If the user has just opened a list, and there are no arguments.
        // For example, foo(    )
        //                  |  |
        // The span should include positions inside the parentheses.
        let span_start = node.end();
        let mut span_end = skip_trivia(&source_file_text(source_file), node.end());
        span_end = ensure_minimum_span_size(span_start, span_end);
        return TextRange::new(span_start, span_end);
    }
    let applicable_span_start = argument_list.pos();
    let mut applicable_span_end = skip_trivia(&source_file_text(source_file), argument_list.end());

    // If the argument list is empty (Pos == End), extend the span to include at least
    // one position. This handles foo(|) where the cursor is right after the opening paren.
    applicable_span_end = ensure_minimum_span_size(applicable_span_start, applicable_span_end);

    TextRange::new(applicable_span_start, applicable_span_end)
}

// Go: ls/signaturehelp.go:1204 ensureMinimumSpanSize
// ensureMinimumSpanSize ensures that a span includes at least one position.
// TextRange.Contains uses a half-open interval, so an empty span would not contain
// the cursor immediately after typing an opening paren in a call like foo(bar(|)).
pub fn ensure_minimum_span_size(start: i32, end: i32) -> i32 {
    if end <= start {
        return start + 1;
    }
    end
}

// Go: ls/signaturehelp.go:1211 argumentOrParameterListAndIndex
#[derive(Clone, Copy, Debug)]
pub struct ArgumentOrParameterListAndIndex {
    pub list: NodeList,
    pub argument_index: i32,
}

// Go: ls/signaturehelp.go:1216 getArgumentOrParameterListAndIndex
pub fn get_argument_or_parameter_list_and_index(
    node: Node,
    source_file: Node,
    c: &mut Checker,
) -> Option<ArgumentOrParameterListAndIndex> {
    if node.kind() == SyntaxKind::LessThanToken || node.kind() == SyntaxKind::OpenParenToken {
        // Find the list that starts right *after* the < or ( token.
        // If the user has just opened a list, consider this item 0.
        let list = get_child_list_that_starts_with_opener_token(node.parent(), node);
        Some(ArgumentOrParameterListAndIndex {
            list,
            argument_index: 0,
        })
    } else {
        // findListItemInfo can return undefined if we are not in parent's argument list
        // or type argument list. This includes cases where the cursor is:
        //   - To the right of the closing parenthesis, non-substitution template, or template tail.
        //   - Between the type arguments and the arguments (greater than token)
        //   - On the target of the call (parent.func)
        //   - On the 'new' keyword in a 'new' expression
        let list = find_containing_list(node, source_file);
        if list.is_nil() {
            return None;
        }
        Some(ArgumentOrParameterListAndIndex {
            list,
            // Find the index of the argument that contains the node.
            argument_index: get_argument_index(node, list, source_file, c),
        })
    }
}

// Go: ls/signaturehelp.go:1244 getChildListThatStartsWithOpenerToken
pub fn get_child_list_that_starts_with_opener_token(parent: Node, opener_token: Node) -> NodeList {
    if is_call_expression(parent) {
        let parent_call_expression = parent;
        if opener_token.kind() == SyntaxKind::LessThanToken {
            return parent_call_expression.type_argument_list();
        }
        return parent_call_expression.argument_list();
    } else if is_new_expression(parent) {
        let parent_new_expression = parent;
        if opener_token.kind() == SyntaxKind::LessThanToken {
            return parent_new_expression.type_argument_list();
        }
        return parent_new_expression.argument_list();
    }
    NodeList::NIL
}

// Go: ls/signaturehelp.go:1261 tryGetParameterInfo
pub fn try_get_parameter_info(
    starting_token: Node,
    source_file: Node,
    c: &mut Checker,
) -> Option<ArgumentListInfo> {
    let node = get_adjusted_node(starting_token);
    if node.is_nil() {
        return None;
    }
    let info = get_contextual_signature_location_info(node, source_file, c)?;

    // for optional function condition
    let non_nullable_contextual_type = c.get_non_nullable_type(info.contextual_type);
    if non_nullable_contextual_type.is_nil() {
        return None;
    }

    let symbol = c.ty(non_nullable_contextual_type).symbol;
    if symbol.is_nil() {
        return None;
    }

    let signatures =
        c.get_signatures_of_type_exported(non_nullable_contextual_type, SignatureKind::CALL);
    if signatures.is_empty() {
        return None;
    }
    let signature = signatures[signatures.len() - 1];

    let contextual_invocation = ContextualInvocation {
        signature,
        node: starting_token,
        symbol: choose_better_symbol(&c.symbols, symbol),
    };
    Some(ArgumentListInfo {
        is_type_parameter_list: false,
        invocation: Invocation {
            contextual_invocation: Some(contextual_invocation),
            ..Default::default()
        },
        arguments_span: info.arguments_span,
        argument_index: info.argument_index,
        argument_count: info.argument_count,
    })
}

// Go: ls/signaturehelp.go:1302 chooseBetterSymbol
// PORT: symbol data lives in the checker arena, so the arena is the first parameter.
pub fn choose_better_symbol(symbols: &SymbolArena, s: SymbolId) -> SymbolId {
    if symbols.sym(s).name.as_str() == INTERNAL_SYMBOL_NAME_TYPE {
        for &d in symbols.sym(s).declarations.iter() {
            if is_function_type_node(d) && can_have_symbol(d.parent()) {
                return d.parent().symbol();
            }
        }
    }
    s
}

// Go: ls/signaturehelp.go:1313 getContextualSignatureLocationInfo
pub fn get_contextual_signature_location_info(
    node: Node,
    source_file: Node,
    c: &mut Checker,
) -> Option<ContextualSignatureLocationInfo> {
    let parent = node.parent();
    match parent.kind() {
        SyntaxKind::ParenthesizedExpression
        | SyntaxKind::MethodDeclaration
        | SyntaxKind::FunctionExpression
        | SyntaxKind::ArrowFunction => {
            let info = get_argument_or_parameter_list_info(node, source_file, c)?;
            let argument_index = info.argument_index;
            let argument_count = info.argument_count;
            let arguments_span = info.arguments_span;

            let contextual_type = if is_method_declaration(parent) {
                c.get_contextual_type_for_object_literal_element_exported(
                    parent,
                    ContextFlags::NONE,
                )
            } else {
                c.get_contextual_type_exported(parent, ContextFlags::NONE)
            };
            if contextual_type.is_some() {
                return Some(ContextualSignatureLocationInfo {
                    contextual_type,
                    argument_index,
                    argument_count,
                    arguments_span,
                });
            }
            None
        }
        SyntaxKind::BinaryExpression => {
            let highest_binary = get_highest_binary(parent);
            let contextual_type =
                c.get_contextual_type_exported(highest_binary, ContextFlags::NONE);
            if node.kind() != SyntaxKind::OpenParenToken {
                let argument_index = count_binary_expression_parameters(parent) - 1;
                let argument_count = count_binary_expression_parameters(highest_binary);
                if contextual_type.is_some() {
                    return Some(ContextualSignatureLocationInfo {
                        contextual_type,
                        argument_index,
                        argument_count,
                        arguments_span: TextRange::new(parent.pos(), parent.end()),
                    });
                }
                return None;
            }
            None
        }
        _ => None,
    }
}

// Go: ls/signaturehelp.go:1361 getHighestBinary
pub fn get_highest_binary(b: Node) -> Node {
    if is_binary_expression(b.parent()) {
        return get_highest_binary(b.parent());
    }
    b
}

// Go: ls/signaturehelp.go:1368 countBinaryExpressionParameters
pub fn count_binary_expression_parameters(b: Node) -> i32 {
    if is_binary_expression(b.left()) {
        return count_binary_expression_parameters(b.left()) + 1;
    }
    2
}

// Go: ls/signaturehelp.go:1375 getTokenFromNodeList
pub fn get_token_from_node_list(
    node_list: NodeList,
    node_list_parent: Node,
    source_file: Node,
) -> Vec<Node> {
    if node_list.is_nil() || node_list_parent.is_nil() {
        return Vec::new();
    }
    let mut left = node_list.pos();
    let mut node_list_index: usize = 0;
    let mut tokens: Vec<Node> = Vec::new();
    while left < node_list.end() {
        let nodes = node_list.nodes();
        if nodes.len() > node_list_index && left == nodes.get(node_list_index).pos() {
            tokens.push(nodes.get(node_list_index));
            left = nodes.get(node_list_index).end();
            node_list_index += 1;
        } else {
            let sf_text = source_file_text(source_file);
            let scanner = scanner_ls::get_scanner_for_source_file(source_file, &sf_text, left);
            let token = scanner.token();
            let token_full_start = scanner.token_full_start();
            let token_end = scanner.token_end();
            tokens.push(source_file_get_or_create_token(
                source_file,
                token,
                token_full_start,
                token_end,
                node_list_parent,
                scanner.token_flags(),
            ));
            left = token_end;
        }
    }
    tokens
}

// Go: ls/signaturehelp.go:1399 getArgumentListInfoForTemplate
pub fn get_argument_list_info_for_template(
    tag_expression: Node,
    argument_index: i32,
    source_file: Node,
) -> Option<ArgumentListInfo> {
    // argumentCount is either 1 or (numSpans + 1) to account for the template strings array argument.
    let mut argument_count = 1;
    if !is_no_substitution_template_literal(tag_expression.template()) {
        argument_count = tag_expression.template().template_spans().nodes().len() as i32 + 1;
    }
    if argument_index != 0 {
        debug_assert!(argument_index < argument_count);
    }
    Some(ArgumentListInfo {
        is_type_parameter_list: false,
        invocation: Invocation {
            call_invocation: Some(CallInvocation {
                node: tag_expression,
            }),
            ..Default::default()
        },
        argument_index,
        argument_count,
        arguments_span: get_applicable_range_for_tagged_template(tag_expression, source_file),
    })
}

// Go: ls/signaturehelp.go:1417 getApplicableRangeForTaggedTemplate
pub fn get_applicable_range_for_tagged_template(
    tagged_template: Node,
    source_file: Node,
) -> TextRange {
    let template = tagged_template.template();
    let applicable_span_start = get_token_pos_of_node(template, source_file, false);
    let mut applicable_span_end = template.end();

    // We need to adjust the end position for the case where the template does not have a tail.
    // Otherwise, we will not show signature help past the expression.
    // For example,
    //
    //      ` ${ 1 + 1 foo(10)
    //       |       |
    // This is because a Missing node has no width. However, what we actually want is to include trivia
    // leading up to the next token in case the user is about to type in a TemplateMiddle or TemplateTail.
    if template.kind() == SyntaxKind::TemplateExpression {
        let template_spans = template.template_spans();
        // Go: templateSpans.Nodes[len(templateSpans.Nodes)-1]
        let last_span = template_spans.nodes().last().unwrap_or_else(|| {
            crate::core::go_panic("runtime error: index out of range [-1]".to_string())
        });
        if last_span.literal().end() - last_span.literal().pos() == 0 {
            applicable_span_end = skip_trivia(&source_file_text(source_file), applicable_span_end);
        }
    }

    // PORT: Go passes the span length as the end of `core.NewTextRange`; kept as in Go.
    TextRange::new(
        applicable_span_start,
        applicable_span_end - applicable_span_start,
    )
}

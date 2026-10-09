//! Go `internal/compiler/program.go` lines 724 to 1289: compiler option
//! checks (`verifyCompilerOptions`), project reference checks and emit
//! blocking.

use crate::frontend::prelude::*;

/// The Go `core.Memoize` locals of `verifyCompilerOptions`.
// PORT: Go computes them lazily. They only read the config file syntax and
// have no side effects, so they are computed once at the start.
struct OptionsSyntax {
    // Go `sourceFile()`. `Node::NIL` for the Go nil.
    source_file: Node,
    // Go `configFilePath()`.
    config_file_path: String,
    // Go `getCompilerOptionsPropertySyntax()`. `None` for the Go nil.
    compiler_options_property: Option<Node>,
    // Go `getCompilerOptionsObjectLiteralSyntax()`. `Node::NIL` for the Go nil.
    compiler_options_object_literal: Node,
}

impl OptionsSyntax {
    fn new(program: &NewProgram) -> Self {
        let source_file = match &program.opts.config.config_file {
            None => Node::NIL,
            Some(config_file) => config_file.source_file,
        };
        let config_file_path = if source_file.is_some() {
            source_file_file_name(source_file).to_string()
        } else {
            String::new()
        };
        let compiler_options_property =
            for_each_tsconfig_prop_array(source_file, "compilerOptions", Some);
        let mut compiler_options_object_literal = Node::NIL;
        if let Some(compiler_options_property) = compiler_options_property
            && compiler_options_property.initializer().is_some()
            && is_object_literal_expression(compiler_options_property.initializer())
        {
            compiler_options_object_literal = compiler_options_property.initializer();
        }
        OptionsSyntax {
            source_file,
            config_file_path,
            compiler_options_property,
            compiler_options_object_literal,
        }
    }
}

/// Go `json.Marshal` (internal/json) of a `string`.
// PORT: U17 owns internal/json, which has no string entry point. This is the
// go-json-experiment `jsonwire.AppendQuote` with the default Marshal flags
// (`append_json_quote`).
fn marshal_json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    append_json_quote(&mut out, value);
    out
}

// PORT: the Go closures of `verifyCompilerOptions` append to
// `p.programDiagnostics` and return the `*ast.Diagnostic`. Here they are
// methods that return the index of the new diagnostic in
// `program_diagnostics`, so a caller can still add a message chain.
impl NewProgram {
    // Go: program.go:942 createOptionDiagnosticInObjectLiteralSyntax (closure)
    #[allow(clippy::too_many_arguments)]
    fn create_option_diagnostic_in_object_literal_syntax(
        &mut self,
        syntax: &OptionsSyntax,
        object_literal: Node,
        on_key: bool,
        key1: &str,
        key2: &str,
        message: &'static Message,
        args: &[String],
    ) -> Option<usize> {
        let diag = for_each_property_assignment(
            object_literal,
            key1,
            |property: Node| {
                Some(create_diagnostic_for_node_in_source_file(
                    syntax.source_file,
                    if on_key {
                        property.name()
                    } else {
                        property.initializer()
                    },
                    message,
                    args.to_vec(),
                ))
            },
            &[key2],
        );
        diag.map(|diag| {
            self.program_diagnostics.push(diag);
            self.program_diagnostics.len() - 1
        })
    }

    // Go: program.go:952 createCompilerOptionsDiagnostic (closure)
    fn create_compiler_options_diagnostic(
        &mut self,
        syntax: &OptionsSyntax,
        message: &'static Message,
        args: &[String],
    ) -> usize {
        let diag = if let Some(compiler_options_property) = syntax.compiler_options_property {
            create_diagnostic_for_node_in_source_file(
                syntax.source_file,
                compiler_options_property.name(),
                message,
                args.to_vec(),
            )
        } else {
            new_compiler_diagnostic(message, args.to_vec())
        };
        self.program_diagnostics.push(diag);
        self.program_diagnostics.len() - 1
    }

    // Go: program.go:964 createDiagnosticForOption (closure)
    fn create_diagnostic_for_option(
        &mut self,
        syntax: &OptionsSyntax,
        on_key: bool,
        option1: &str,
        option2: &str,
        message: &'static Message,
        args: &[String],
    ) -> usize {
        let diag = self.create_option_diagnostic_in_object_literal_syntax(
            syntax,
            syntax.compiler_options_object_literal,
            on_key,
            option1,
            option2,
            message,
            args,
        );
        match diag {
            Some(diag) => diag,
            None => self.create_compiler_options_diagnostic(syntax, message, args),
        }
    }

    // Go: program.go:972 createDiagnosticForOptionName (closure)
    fn create_diagnostic_for_option_name(
        &mut self,
        syntax: &OptionsSyntax,
        message: &'static Message,
        option1: &str,
        option2: &str,
        args: &[String],
    ) {
        let mut new_args: Vec<String> = Vec::with_capacity(args.len() + 2);
        new_args.push(option1.to_string());
        new_args.push(option2.to_string());
        new_args.extend_from_slice(args);
        self.create_diagnostic_for_option(
            syntax, true, /*onKey*/
            option1, option2, message, &new_args,
        );
    }

    // Go: program.go:979 createOptionValueDiagnostic (closure)
    fn create_option_value_diagnostic(
        &mut self,
        syntax: &OptionsSyntax,
        option1: &str,
        message: &'static Message,
        args: &[String],
    ) {
        self.create_diagnostic_for_option(syntax, false /*onKey*/, option1, "", message, args);
    }

    // Go: program.go:983 createRemovedOptionDiagnostic (closure)
    fn create_removed_option_diagnostic(
        &mut self,
        syntax: &OptionsSyntax,
        name: &str,
        value: &str,
        use_instead: &str,
    ) {
        let (message, args) = if value.is_empty() {
            (
                diag::Option_0_has_been_removed_Please_remove_it_from_your_configuration,
                args![name],
            )
        } else {
            (
                diag::Option_0_1_has_been_removed_Please_remove_it_from_your_configuration,
                args![name, value],
            )
        };

        let diag =
            self.create_diagnostic_for_option(syntax, value.is_empty(), name, "", message, &args);
        if !use_instead.is_empty() {
            self.program_diagnostics[diag].add_message_chain(Some(new_compiler_diagnostic(
                diag::Use_0_instead,
                args![use_instead],
            )));
        }
    }

    // Go: program.go:1126 createDiagnosticForOptionPaths (closure)
    // PORT: Go `forEachOptionPathsSyntax` (program.go:1122) is inlined; it is
    // `ForEachPropertyAssignment(getCompilerOptionsObjectLiteralSyntax(), "paths", callback)`.
    fn create_diagnostic_for_option_paths(
        &mut self,
        syntax: &OptionsSyntax,
        on_key: bool,
        key: &str,
        message: &'static Message,
        args: &[String],
    ) -> usize {
        let diag = for_each_property_assignment(
            syntax.compiler_options_object_literal,
            "paths",
            |path_prop: Node| {
                if is_object_literal_expression(path_prop.initializer()) {
                    return self.create_option_diagnostic_in_object_literal_syntax(
                        syntax,
                        path_prop.initializer(),
                        on_key,
                        key,
                        "",
                        message,
                        args,
                    );
                }
                None
            },
            &[],
        );
        match diag {
            Some(diag) => diag,
            None => self.create_compiler_options_diagnostic(syntax, message, args),
        }
    }

    // Go: program.go:1139 createDiagnosticForOptionPathKeyValue (closure)
    fn create_diagnostic_for_option_path_key_value(
        &mut self,
        syntax: &OptionsSyntax,
        key: &str,
        value_index: usize,
        message: &'static Message,
        args: &[String],
    ) -> usize {
        let diag = for_each_property_assignment(
            syntax.compiler_options_object_literal,
            "paths",
            |path_prop: Node| {
                if is_object_literal_expression(path_prop.initializer()) {
                    return for_each_property_assignment(
                        path_prop.initializer(),
                        key,
                        |key_props: Node| {
                            let initializer = key_props.initializer();
                            if is_array_literal_expression(initializer) {
                                let elements = initializer.element_list();
                                if elements.is_some() && elements.nodes().len() > value_index {
                                    let diag = create_diagnostic_for_node_in_source_file(
                                        syntax.source_file,
                                        elements.nodes().get(value_index),
                                        message,
                                        args.to_vec(),
                                    );
                                    self.program_diagnostics.push(diag);
                                    return Some(self.program_diagnostics.len() - 1);
                                }
                            }
                            None
                        },
                        &[],
                    );
                }
                None
            },
            &[],
        );
        match diag {
            Some(diag) => diag,
            None => self.create_compiler_options_diagnostic(syntax, message, args),
        }
    }

    // Go: program.go:909 (*Program).verifyCompilerOptions
    pub fn verify_compiler_options(&mut self) {
        // PORT: Go holds a `*core.CompilerOptions`. The `Rc` is cloned so
        // `self` can be borrowed mutably.
        let options: Rc<CompilerOptions> = self.opts.config.compiler_options().clone();
        let syntax = OptionsSyntax::new(self);
        let syntax = &syntax;

        // Removed in TS7

        if !options.base_url.is_empty() {
            // BaseUrl will have been turned absolute by this point.
            let mut use_instead = String::new();
            if !syntax.config_file_path.is_empty() {
                // ts#64159 (program.go:1006): the file system's case
                // sensitivity (N: the zero-value `comparePathsOptions`).
                let mut relative = get_relative_path_from_file(
                    &syntax.config_file_path,
                    &options.base_url,
                    &self.case_sensitivity(),
                );
                if !(relative.starts_with("./") || relative.starts_with("../")) {
                    relative = format!("./{relative}");
                }
                let suggestion = combine_paths(&relative, &["*"]);
                use_instead = format!(
                    r#""paths": {{"*": [{}]}}"#,
                    marshal_json_string(&suggestion)
                );
            }
            self.create_removed_option_diagnostic(syntax, "baseUrl", "", &use_instead);
        }

        if !options.out_file.is_empty() {
            self.create_removed_option_diagnostic(syntax, "outFile", "", "");
        }

        if options.target == ScriptTarget::ES5 {
            self.create_removed_option_diagnostic(syntax, "target", "ES5", "");
        }

        if options.module == ModuleKind::AMD {
            self.create_removed_option_diagnostic(syntax, "module", "AMD", "");
        }
        if options.module == ModuleKind::SYSTEM {
            self.create_removed_option_diagnostic(syntax, "module", "System", "");
        }
        if options.module == ModuleKind::UMD {
            self.create_removed_option_diagnostic(syntax, "module", "UMD", "");
        }

        if options.module_resolution == ModuleResolutionKind::CLASSIC {
            self.create_removed_option_diagnostic(syntax, "moduleResolution", "Classic", "");
        }

        if options.always_strict.is_false() {
            self.create_removed_option_diagnostic(syntax, "alwaysStrict", "false", "");
        }

        if options.es_module_interop.is_false() {
            self.create_removed_option_diagnostic(syntax, "esModuleInterop", "false", "");
        }

        if options.allow_synthetic_default_imports.is_false() {
            self.create_removed_option_diagnostic(
                syntax,
                "allowSyntheticDefaultImports",
                "false",
                "",
            );
        }

        if options.module_resolution == ModuleResolutionKind::NODE10 {
            self.create_removed_option_diagnostic(syntax, "moduleResolution", "node10", "");
        }

        if !options.downlevel_iteration.is_unknown() {
            self.create_removed_option_diagnostic(syntax, "downlevelIteration", "", "");
        }

        if options.strict_property_initialization.is_true()
            && !options.get_strict_option_value(options.strict_null_checks)
        {
            self.create_diagnostic_for_option_name(
                syntax,
                diag::Option_0_cannot_be_specified_without_specifying_option_1,
                "strictPropertyInitialization",
                "strictNullChecks",
                &[],
            );
        }
        if options.exact_optional_property_types.is_true()
            && !options.get_strict_option_value(options.strict_null_checks)
        {
            self.create_diagnostic_for_option_name(
                syntax,
                diag::Option_0_cannot_be_specified_without_specifying_option_1,
                "exactOptionalPropertyTypes",
                "strictNullChecks",
                &[],
            );
        }

        if options.isolated_declarations.is_true() {
            if options.get_allow_js() {
                self.create_diagnostic_for_option_name(
                    syntax,
                    diag::Option_0_cannot_be_specified_with_option_1,
                    "allowJs",
                    "isolatedDeclarations",
                    &[],
                );
            }
            if !options.get_emit_declarations() {
                self.create_diagnostic_for_option_name(
                    syntax,
                    diag::Option_0_cannot_be_specified_without_specifying_option_1_or_option_2,
                    "isolatedDeclarations",
                    "declaration",
                    &args!["composite"],
                );
            }
        }

        if options.inline_source_map.is_true() {
            if options.source_map.is_true() {
                self.create_diagnostic_for_option_name(
                    syntax,
                    diag::Option_0_cannot_be_specified_with_option_1,
                    "sourceMap",
                    "inlineSourceMap",
                    &[],
                );
            }
            if !options.map_root.is_empty() {
                self.create_diagnostic_for_option_name(
                    syntax,
                    diag::Option_0_cannot_be_specified_with_option_1,
                    "mapRoot",
                    "inlineSourceMap",
                    &[],
                );
            }
        }

        if options.composite.is_true() {
            if options.declaration.is_false() {
                self.create_diagnostic_for_option_name(
                    syntax,
                    diag::Composite_projects_may_not_disable_declaration_emit,
                    "declaration",
                    "",
                    &[],
                );
            }
            if options.incremental.is_false() {
                self.create_diagnostic_for_option_name(
                    syntax,
                    diag::Composite_projects_may_not_disable_incremental_compilation,
                    "declaration",
                    "",
                    &[],
                );
            }
        }

        if options.ts_build_info_file.is_empty()
            && options.incremental.is_true()
            && options.config_file_path.is_empty()
        {
            self.create_compiler_options_diagnostic(
                syntax,
                diag::Option_incremental_is_only_valid_with_a_known_configuration_file_like_tsconfig_json_or_when_tsBuildInfoFile_is_explicitly_provided,
                &[],
            );
        }

        self.verify_project_references();

        if options.composite.is_true() {
            let mut root_paths: FxHashSet<Path> = FxHashSet::default();
            for file_name in self.opts.config.file_names() {
                root_paths.insert(self.to_path(file_name));
            }

            // PORT: the diagnostics are collected first, then added, so the
            // loop over `self.files` does not overlap the mutable borrow.
            let mut not_listed: Vec<Rc<ProcessingDiagnostic>> = Vec::new();
            for file in &self.files {
                // ts#64407
                let root_path = match file.canonical_source_file() {
                    Some(canonical) => canonical.path().clone(),
                    None => file.path().clone(),
                };
                // #4699: Go `sourceFileMayBeEmitted(file, p, false, false)`.
                if source_file_may_be_emitted(file, self, false, false)
                    && !root_paths.contains(&root_path)
                {
                    not_listed.push(Rc::new(ProcessingDiagnostic {
                        kind: ProcessingDiagnosticKind::EXPLAINING_FILE_INCLUDE,
                        data: ProcessingDiagnosticData::IncludeExplaining(IncludeExplainingDiagnostic {
                            file: file.path().clone(),
                            diagnostic_reason: None,
                            message: diag::File_0_is_not_listed_within_the_file_list_of_project_1_Projects_must_list_all_files_or_use_an_include_pattern,
                            args: args![file.file_name(), syntax.config_file_path],
                        }),
                    }));
                }
            }
            self.processed_files
                .include_processor
                .add_processing_diagnostic(not_listed);
        }

        // PORT: Go `options.Paths.Entries()` walks the ordered map. A nil
        // substitution slice is `None`.
        if let Some(paths) = &options.paths {
            for (key, value) in paths {
                // !!! This code does not handle cases where where the path mappings have the wrong types,
                // as that information is mostly lost during the parsing process.
                if !has_zero_or_one_asterisk_character(key) {
                    self.create_diagnostic_for_option_paths(
                        syntax,
                        true, /*onKey*/
                        key,
                        diag::Pattern_0_can_have_at_most_one_Asterisk_character,
                        &args![key],
                    );
                }
                match value {
                    None => {
                        self.create_diagnostic_for_option_paths(
                            syntax,
                            false, /*onKey*/
                            key,
                            diag::Substitutions_for_pattern_0_should_be_an_array,
                            &args![key],
                        );
                    }
                    Some(value) if value.is_empty() => {
                        self.create_diagnostic_for_option_paths(
                            syntax,
                            false, /*onKey*/
                            key,
                            diag::Substitutions_for_pattern_0_shouldn_t_be_an_empty_array,
                            &args![key],
                        );
                    }
                    Some(_) => {}
                }
                // Go ranges over a nil slice as zero items.
                for (i, subst) in value.iter().flatten().enumerate() {
                    if !has_zero_or_one_asterisk_character(subst) {
                        self.create_diagnostic_for_option_path_key_value(
                            syntax,
                            key,
                            i,
                            diag::Substitution_0_in_pattern_1_can_have_at_most_one_Asterisk_character,
                            &args![subst, key],
                        );
                    }
                    if !path_is_relative(subst) && !path_is_absolute(subst) {
                        self.create_diagnostic_for_option_path_key_value(
                            syntax,
                            key,
                            i,
                            diag::Non_relative_paths_are_not_allowed_Did_you_forget_a_leading_Slash,
                            &[],
                        );
                    }
                }
            }
        }

        if options.source_map.is_false_or_unknown()
            && options.inline_source_map.is_false_or_unknown()
        {
            if options.inline_sources.is_true() {
                self.create_diagnostic_for_option_name(
                    syntax,
                    diag::Option_0_can_only_be_used_when_either_option_inlineSourceMap_or_option_sourceMap_is_provided,
                    "inlineSources",
                    "",
                    &[],
                );
            }
            if !options.source_root.is_empty() {
                self.create_diagnostic_for_option_name(
                    syntax,
                    diag::Option_0_can_only_be_used_when_either_option_inlineSourceMap_or_option_sourceMap_is_provided,
                    "sourceRoot",
                    "",
                    &[],
                );
            }
        }

        if !options.map_root.is_empty()
            && !(options.source_map.is_true() || options.declaration_map.is_true())
        {
            // Error to specify --mapRoot without --sourcemap
            self.create_diagnostic_for_option_name(
                syntax,
                diag::Option_0_cannot_be_specified_without_specifying_option_1_or_option_2,
                "mapRoot",
                "sourceMap",
                &args!["declarationMap"],
            );
        }

        if !options.declaration_dir.is_empty() && !options.get_emit_declarations() {
            self.create_diagnostic_for_option_name(
                syntax,
                diag::Option_0_cannot_be_specified_without_specifying_option_1_or_option_2,
                "declarationDir",
                "declaration",
                &args!["composite"],
            );
        }

        if options.declaration_map.is_true() && !options.get_emit_declarations() {
            self.create_diagnostic_for_option_name(
                syntax,
                diag::Option_0_cannot_be_specified_without_specifying_option_1_or_option_2,
                "declarationMap",
                "declaration",
                &args!["composite"],
            );
        }

        if options.lib.is_some() && options.no_lib.is_true() {
            self.create_diagnostic_for_option_name(
                syntax,
                diag::Option_0_cannot_be_specified_with_option_1,
                "lib",
                "noLib",
                &[],
            );
        }

        if (options.isolated_modules.is_true() || options.verbatim_module_syntax.is_true())
            && options.preserve_const_enums.is_false()
        {
            self.create_diagnostic_for_option_name(
                syntax,
                diag::Option_preserveConstEnums_cannot_be_disabled_when_0_is_enabled,
                if options.verbatim_module_syntax.is_true() {
                    "verbatimModuleSyntax"
                } else {
                    "isolatedModules"
                },
                "preserveConstEnums",
                &[],
            );
        }

        if !options.out_dir.is_empty()
            || !options.root_dir.is_empty()
            || !options.source_root.is_empty()
            || !options.map_root.is_empty()
            || (options.get_emit_declarations() && !options.declaration_dir.is_empty())
        {
            // !!! sheetal checkSourceFilesBelongToPath - for root Dir and configFile - explaining why file is in the program
            let dir = self.common_source_directory();
            if !options.out_dir.is_empty()
                && dir.is_empty()
                && self
                    .files
                    .iter()
                    .any(|f| get_root_length(f.file_name()) > 1)
            {
                self.create_diagnostic_for_option_name(
                    syntax,
                    diag::Cannot_find_the_common_subdirectory_path_for_the_input_files,
                    "outDir",
                    "",
                    &[],
                );
            }
        }

        if !options.no_emit.is_true()
            && !options.composite.is_true()
            && options.root_dir.is_empty()
            && !options.config_file_path.is_empty()
            && (!options.out_dir.is_empty()
                || (options.get_emit_declarations() && !options.declaration_dir.is_empty())
                || !options.out_file.is_empty())
        {
            // Check if rootDir inferred changed and issue diagnostic
            let dir = self.common_source_directory();
            let mut emitted_files: Vec<String> = Vec::new();
            for file in &self.files {
                // #4699: Go `sourceFileMayBeEmitted(file, p, false, false)`.
                if !file.is_declaration_file && source_file_may_be_emitted(file, self, false, false)
                {
                    emitted_files.push(file.file_name().to_string());
                }
            }
            // ts#64159 (program.go:1245): the base directory (rule R1), and
            // the directories compare as rooted text (rule R3).
            let dir59 = get_computed_common_source_directory(
                &emitted_files,
                &self.base_directory(),
                self.use_case_sensitive_file_names(),
            );
            if !dir59.is_empty()
                && compare_rooted_text(&dir, &dir59, self.use_case_sensitive_file_names()) != 0
            {
                // change in layout
                let option1 = if !options.out_file.is_empty() {
                    "outFile"
                } else if !options.out_dir.is_empty() {
                    "outDir"
                } else {
                    "declarationDir"
                };
                let option2 = if options.out_file.is_empty() && !options.out_dir.is_empty() {
                    "declarationDir"
                } else {
                    ""
                };
                let diag = self.create_diagnostic_for_option(
                    syntax,
                    true, /*onKey*/
                    option1,
                    option2,
                    diag::The_common_source_directory_of_0_is_1_The_rootDir_setting_must_be_explicitly_set_to_this_or_another_path_to_adjust_your_output_s_file_layout,
                    &args![
                        get_base_file_name(&options.config_file_path),
                        // ts#64159 (program.go:1261): the relative path with the
                        // file system's case sensitivity, or the absolute one on
                        // another root (rule R4), which
                        // `get_relative_path_from_file` already gives.
                        get_relative_path_from_file(&options.config_file_path, &dir59, &self.case_sensitivity())
                    ],
                );
                self.program_diagnostics[diag].add_message_chain(Some(new_compiler_diagnostic(
                    diag::Visit_https_Colon_Slash_Slashaka_ms_Slashts6_for_migration_information,
                    args![],
                )));
            }
        }

        if options.check_js.is_true() && !options.get_allow_js() {
            self.create_diagnostic_for_option_name(
                syntax,
                diag::Option_0_cannot_be_specified_without_specifying_option_1,
                "checkJs",
                "allowJs",
                &[],
            );
        }

        if options.emit_declaration_only.is_true() && !options.get_emit_declarations() {
            self.create_diagnostic_for_option_name(
                syntax,
                diag::Option_0_cannot_be_specified_without_specifying_option_1_or_option_2,
                "emitDeclarationOnly",
                "declaration",
                &args!["composite"],
            );
        }

        if options.emit_decorator_metadata.is_true()
            && options.experimental_decorators.is_false_or_unknown()
        {
            self.create_diagnostic_for_option_name(
                syntax,
                diag::Option_0_cannot_be_specified_without_specifying_option_1,
                "emitDecoratorMetadata",
                "experimentalDecorators",
                &[],
            );
        }

        if !options.jsx_factory.is_empty() {
            if !options.react_namespace.is_empty() {
                self.create_diagnostic_for_option_name(
                    syntax,
                    diag::Option_0_cannot_be_specified_with_option_1,
                    "reactNamespace",
                    "jsxFactory",
                    &[],
                );
            }
            if options.jsx == JsxEmit::REACT_JSX || options.jsx == JsxEmit::REACT_JSX_DEV {
                self.create_diagnostic_for_option_name(
                    syntax,
                    diag::Option_0_cannot_be_specified_when_option_jsx_is_1,
                    "jsxFactory",
                    &options.jsx.to_string(),
                    &[],
                );
            }
            if !parse_isolated_entity_name(&options.jsx_factory).is_some() {
                self.create_option_value_diagnostic(
                    syntax,
                    "jsxFactory",
                    diag::Invalid_value_for_jsxFactory_0_is_not_a_valid_identifier_or_qualified_name,
                    &args![options.jsx_factory],
                );
            }
        } else if !options.react_namespace.is_empty()
            // PORT: Go `scanner.IsIdentifierText`. Two ports exist; the path
            // is explicit to avoid a glob import conflict.
            && !crate::scanner_util::is_identifier_text(&options.react_namespace, LanguageVariant::STANDARD)
        {
            self.create_option_value_diagnostic(
                syntax,
                "reactNamespace",
                diag::Invalid_value_for_reactNamespace_0_is_not_a_valid_identifier,
                &args![options.react_namespace],
            );
        }

        if !options.jsx_fragment_factory.is_empty() {
            if options.jsx_factory.is_empty() {
                self.create_diagnostic_for_option_name(
                    syntax,
                    diag::Option_0_cannot_be_specified_without_specifying_option_1,
                    "jsxFragmentFactory",
                    "jsxFactory",
                    &[],
                );
            }
            if options.jsx == JsxEmit::REACT_JSX || options.jsx == JsxEmit::REACT_JSX_DEV {
                self.create_diagnostic_for_option_name(
                    syntax,
                    diag::Option_0_cannot_be_specified_when_option_jsx_is_1,
                    "jsxFragmentFactory",
                    &options.jsx.to_string(),
                    &[],
                );
            }
            if !parse_isolated_entity_name(&options.jsx_fragment_factory).is_some() {
                self.create_option_value_diagnostic(
                    syntax,
                    "jsxFragmentFactory",
                    diag::Invalid_value_for_jsxFragmentFactory_0_is_not_a_valid_identifier_or_qualified_name,
                    &args![options.jsx_fragment_factory],
                );
            }
        }

        if !options.react_namespace.is_empty()
            && (options.jsx == JsxEmit::REACT_JSX || options.jsx == JsxEmit::REACT_JSX_DEV)
        {
            self.create_diagnostic_for_option_name(
                syntax,
                diag::Option_0_cannot_be_specified_when_option_jsx_is_1,
                "reactNamespace",
                &options.jsx.to_string(),
                &[],
            );
        }

        if !options.jsx_import_source.is_empty() && options.jsx == JsxEmit::REACT {
            self.create_diagnostic_for_option_name(
                syntax,
                diag::Option_0_cannot_be_specified_when_option_jsx_is_1,
                "jsxImportSource",
                &options.jsx.to_string(),
                &[],
            );
        }

        let module_kind = options.get_emit_module_kind();

        if options.allow_importing_ts_extensions.is_true()
            && !(options.no_emit.is_true()
                || options.emit_declaration_only.is_true()
                || options.rewrite_relative_import_extensions.is_true())
        {
            self.create_option_value_diagnostic(
                syntax,
                "allowImportingTsExtensions",
                diag::Option_allowImportingTsExtensions_can_only_be_used_when_one_of_noEmit_emitDeclarationOnly_or_rewriteRelativeImportExtensions_is_set,
                &[],
            );
        }

        let module_resolution = options.get_module_resolution_kind();
        if options.resolve_package_json_exports.is_true()
            && !module_resolution_supports_package_json_exports_and_imports(module_resolution)
        {
            self.create_diagnostic_for_option_name(
                syntax,
                diag::Option_0_can_only_be_used_when_moduleResolution_is_set_to_node16_nodenext_or_bundler,
                "resolvePackageJsonExports",
                "",
                &[],
            );
        }
        if options.resolve_package_json_imports.is_true()
            && !module_resolution_supports_package_json_exports_and_imports(module_resolution)
        {
            self.create_diagnostic_for_option_name(
                syntax,
                diag::Option_0_can_only_be_used_when_moduleResolution_is_set_to_node16_nodenext_or_bundler,
                "resolvePackageJsonImports",
                "",
                &[],
            );
        }
        if options.custom_conditions.is_some()
            && !module_resolution_supports_package_json_exports_and_imports(module_resolution)
        {
            self.create_diagnostic_for_option_name(
                syntax,
                diag::Option_0_can_only_be_used_when_moduleResolution_is_set_to_node16_nodenext_or_bundler,
                "customConditions",
                "",
                &[],
            );
        }

        if module_resolution == ModuleResolutionKind::BUNDLER
            && !emit_module_kind_is_non_node_esm(module_kind)
            && module_kind != ModuleKind::PRESERVE
            && module_kind != ModuleKind::COMMON_JS
        {
            self.create_option_value_diagnostic(
                syntax,
                "moduleResolution",
                diag::Option_0_can_only_be_used_when_module_is_set_to_preserve_commonjs_or_es2015_or_later,
                &args!["bundler"],
            );
        }

        if ModuleKind::NODE16 <= module_kind
            && module_kind <= ModuleKind::NODE_NEXT
            && !(ModuleResolutionKind::NODE16 <= module_resolution
                && module_resolution <= ModuleResolutionKind::NODE_NEXT)
        {
            let module_kind_name = module_kind.to_string();
            let module_resolution_name = match module_kind_to_module_resolution_kind(module_kind) {
                (v, true) => v.to_string(),
                (_, false) => "Node16".to_string(),
            };
            self.create_option_value_diagnostic(
                syntax,
                "moduleResolution",
                diag::Option_moduleResolution_must_be_set_to_0_or_left_unspecified_when_option_module_is_set_to_1,
                &args![module_resolution_name, module_kind_name],
            );
        } else if ModuleResolutionKind::NODE16 <= module_resolution
            && module_resolution <= ModuleResolutionKind::NODE_NEXT
            && !(ModuleKind::NODE16 <= module_kind && module_kind <= ModuleKind::NODE_NEXT)
        {
            let module_resolution_name = module_resolution.to_string();
            self.create_option_value_diagnostic(
                syntax,
                "module",
                diag::Option_module_must_be_set_to_0_when_option_moduleResolution_is_set_to_1,
                &args![module_resolution_name, module_resolution_name],
            );
        }

        // !!! The below needs filesByName, which is not equivalent to p.filesByPath.

        // If the emit is enabled make sure that every output file is unique and not overwriting any of the input files
        if !options.no_emit.is_true() && !options.suppress_output_path_check.is_true() {
            // PORT: Go calls `verifyEmitFilePath` inside the
            // `ForEachEmittedFile` callback. The callback only reads, so the
            // names are collected first and then checked in the same order.
            let mut emit_file_names: Vec<String> = Vec::new();
            // #4699: Go `p.getSourceFilesToEmit(nil, false, false)`.
            let source_files_to_emit = self.get_source_files_to_emit(None, false, false);
            for_each_emitted_file(
                self,
                &options,
                |emit_file_names_of_file: &OutputPaths,
                 _source_file: Option<&Rc<ParsedSourceFile>>| {
                    emit_file_names.push(emit_file_names_of_file.js_file_path().to_string());
                    emit_file_names
                        .push(emit_file_names_of_file.source_map_file_path().to_string());
                    emit_file_names
                        .push(emit_file_names_of_file.declaration_file_path().to_string());
                    emit_file_names
                        .push(emit_file_names_of_file.declaration_map_path().to_string());
                    false
                },
                &source_files_to_emit,
                false,
            );
            emit_file_names.push(self.opts.config.get_build_info_file_name());

            let mut emit_files_seen: FxHashSet<String> = FxHashSet::default();
            for emit_file_name in &emit_file_names {
                self.verify_emit_file_path(syntax, &mut emit_files_seen, emit_file_name);
            }
        }
    }

    // Go: program.go:1375 verifyEmitFilePath (closure)
    // Verify that all the emit files are unique and don't overwrite input files
    fn verify_emit_file_path(
        &mut self,
        syntax: &OptionsSyntax,
        emit_files_seen: &mut FxHashSet<String>,
        emit_file_name: &str,
    ) {
        if !emit_file_name.is_empty() {
            let emit_file_path = self.to_path(emit_file_name);
            // Report error if the output overwrites input file
            if self.files_by_path.contains_key(&emit_file_path) {
                let mut diag = new_compiler_diagnostic(
                    diag::Cannot_write_file_0_because_it_would_overwrite_input_file,
                    args![emit_file_name],
                );
                if syntax.config_file_path.is_empty() {
                    // The program is from either an inferred project or an external project
                    diag.add_message_chain(Some(new_compiler_diagnostic(
                        diag::Adding_a_tsconfig_json_file_will_help_organize_projects_that_contain_both_TypeScript_and_JavaScript_files_Learn_more_at_https_Colon_Slash_Slashaka_ms_Slashtsconfig,
                        args![],
                    )));
                }
                self.block_emitting_of_file(emit_file_name, diag);
            }

            let emit_file_key = if !self.host().fs().use_case_sensitive_file_names() {
                to_file_name_lower_case(&emit_file_path)
            } else {
                emit_file_path.to_string()
            };

            // Report error if multiple files write into same file
            if emit_files_seen.contains(&emit_file_key) {
                // Already seen the same emit file - report error
                self.block_emitting_of_file(
                    emit_file_name,
                    new_compiler_diagnostic(
                        diag::Cannot_write_file_0_because_it_would_be_overwritten_by_multiple_input_files,
                        args![emit_file_name],
                    ),
                );
            } else {
                emit_files_seen.insert(emit_file_key);
            }
        }
    }

    // Go: program.go:1409 (*Program).blockEmittingOfFile
    pub fn block_emitting_of_file(&mut self, emit_file_name: &str, diag: Diagnostic) {
        let path = self.to_path(emit_file_name);
        self.has_emit_blocking_diagnostics.insert(path);
        self.program_diagnostics.push(diag);
    }

    // Go: program.go:1414 (*Program).IsEmitBlocked
    pub fn is_emit_blocked(&self, emit_file_name: &str) -> bool {
        self.has_emit_blocking_diagnostics
            .contains(&self.to_path(emit_file_name))
    }

    // Go: program.go:1418 (*Program).verifyProjectReferences
    pub fn verify_project_references(&mut self) {
        let build_info_file_name = if !self.options().suppress_output_path_check.is_true() {
            self.opts.config.get_build_info_file_name()
        } else {
            String::new()
        };
        // PORT: the range callback runs while the mapper is borrowed, so the
        // diagnostics and blocked paths are collected in order and applied
        // after the range ends.
        let mut diagnostics: Vec<Diagnostic> = Vec::new();
        let mut blocked_paths: Vec<Path> = Vec::new();
        // Go: program.go:1420 createDiagnosticForReference (closure)
        let create_diagnostic_for_reference =
            |diagnostics: &mut Vec<Diagnostic>,
             config: &ParsedCommandLine,
             index: usize,
             message: &'static Message,
             args: Vec<String>| {
                let diag =
                    create_diagnostic_at_reference_syntax(config, index, message, args.clone())
                        .unwrap_or_else(|| new_compiler_diagnostic(message, args));
                diagnostics.push(diag);
            };

        self.range_resolved_project_reference(
            |_path: &Path,
             config: Option<&Rc<ParsedCommandLine>>,
             parent: Option<&Rc<ParsedCommandLine>>,
             index: usize| {
                // PORT: Go dereferences a nil parent and panics; so does this.
                let parent = parent.expect("project reference has no parent config");
                let ref_ = &parent.project_references()[index];
                // !!! Deprecated in 5.0 and removed since 5.5
                // verifyRemovedProjectReference(ref, parent, index);
                let Some(config) = config else {
                    create_diagnostic_for_reference(
                        &mut diagnostics,
                        parent,
                        index,
                        diag::File_0_not_found,
                        args![ref_.path],
                    );
                    return true;
                };
                let ref_options = config.compiler_options();
                if (!ref_options.composite.is_true() || ref_options.no_emit.is_true())
                    && !parent.file_names().is_empty()
                {
                    if !ref_options.composite.is_true() {
                        create_diagnostic_for_reference(
                            &mut diagnostics,
                            parent,
                            index,
                            diag::Referenced_project_0_must_have_setting_composite_Colon_true,
                            args![ref_.path],
                        );
                    }
                    if ref_options.no_emit.is_true() {
                        create_diagnostic_for_reference(
                            &mut diagnostics,
                            parent,
                            index,
                            diag::Referenced_project_0_may_not_disable_emit,
                            args![ref_.path],
                        );
                    }
                }
                if !build_info_file_name.is_empty() && build_info_file_name == config.get_build_info_file_name() {
                    create_diagnostic_for_reference(
                        &mut diagnostics,
                        parent,
                        index,
                        diag::Cannot_write_file_0_because_it_will_overwrite_tsbuildinfo_file_generated_by_referenced_project_1,
                        args![build_info_file_name, ref_.path],
                    );
                    blocked_paths.push(self.to_path(&build_info_file_name));
                }
                true
            },
        );
        self.program_diagnostics.extend(diagnostics);
        self.has_emit_blocking_diagnostics.extend(blocked_paths);
    }
}

// Go: program.go:1455 hasZeroOrOneAsteriskCharacter
pub fn has_zero_or_one_asterisk_character(str: &str) -> bool {
    let mut seen_asterisk = false;
    for ch in str.chars() {
        if ch == '*' {
            if !seen_asterisk {
                seen_asterisk = true;
            } else {
                // have already seen asterisk
                return false;
            }
        }
    }
    true
}

// Go: program.go:1470 moduleResolutionSupportsPackageJsonExportsAndImports
pub fn module_resolution_supports_package_json_exports_and_imports(
    module_resolution: ModuleResolutionKind,
) -> bool {
    module_resolution >= ModuleResolutionKind::NODE16
        && module_resolution <= ModuleResolutionKind::NODE_NEXT
        || module_resolution == ModuleResolutionKind::BUNDLER
}

// Go: program.go:1475 emitModuleKindIsNonNodeESM
pub fn emit_module_kind_is_non_node_esm(module_kind: ModuleKind) -> bool {
    module_kind >= ModuleKind::ES2015 && module_kind <= ModuleKind::ES_NEXT
}

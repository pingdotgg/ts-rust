//! Port of Go `ls/sourcedefinition.go`.
//!
//! PORT: Go `*compiler.Program` is `&compiler::NewProgram`.
//! `getOrParseSourceFile` parses and binds a file outside the program as Go
//! does; the file is published with no program first (node reads of a file
//! need its published Go file).

use crate::ls::prelude::*;

use crate::spanmap::Feature;

impl LanguageService {
    // Go: ls/sourcedefinition.go:24 ProvideSourceDefinition
    pub fn provide_source_definition(
        &self,
        ctx: &Context,
        document_uri: &lsproto::DocumentUri,
        position: lsproto::Position,
    ) -> Result<lsproto::DefinitionResponse, GoError> {
        let (program, file) = self.get_program_and_file(document_uri);
        let positions = lsconv::from_lsp_position_for_source_file(
            &self.converters,
            file,
            position,
            Feature::DEFINITION,
        );
        let mut results = Vec::with_capacity(positions.len());
        for mapped in &positions {
            if mapped.fidelity.is_single_segment() {
                let result = self.provide_source_definition_at_position(
                    ctx,
                    program,
                    mapped.script,
                    mapped.position,
                )?;
                results.push(result);
            }
        }
        Ok(combine_definition_responses(
            results,
            lsproto::get_client_capabilities(ctx)
                .text_document
                .definition
                .link_support,
        ))
    }

    // Go: ls/sourcedefinition.go:44 provideSourceDefinitionAtPosition
    // PORT: Go `core.TextPos` is `i32`.
    pub fn provide_source_definition_at_position(
        &self,
        ctx: &Context,
        program: &compiler::NewProgram,
        file: Node,
        text_pos: i32,
    ) -> Result<lsproto::DefinitionResponse, GoError> {
        let caps = lsproto::get_client_capabilities(ctx);
        let client_supports_link = caps.text_document.definition.link_support;

        let pos = text_pos;
        let mut resolver = self.new_source_def_resolver(program, source_file_file_name(file));
        let node = astnav::get_touching_property_name(file, pos);

        if node.kind() == SyntaxKind::SourceFile {
            // Triple-slash directives are comments, not AST nodes, so
            // GetTouchingPropertyName returns the SourceFile node.
            let (declarations, ref_) = resolver.resolve_triple_slash_reference(file, pos, program);
            if !declarations.is_empty() {
                // PORT: Go reads `ref.Pos()` through the pointer; a nil
                // pointer panics there.
                let ref_ = ref_.unwrap_or_else(|| crate::core::go_nil_dereference());
                let (origin_selection_range, _) =
                    self.create_lsp_range_from_bounds(ref_.range.pos(), ref_.range.end(), file);
                return Ok(self.create_definition_locations(
                    origin_selection_range,
                    client_supports_link,
                    &declarations,
                    None, /*reference*/
                    Feature::DEFINITION,
                ));
            }
            return Ok(lsproto::LocationOrLocationsOrDefinitionLinksOrNull::default());
        }

        let (origin_selection_range, _) = self.create_lsp_range_from_node(node, file);

        // ts#63915: a source phase import gives the plain definition.
        let containing_module_specifier = find_containing_module_specifier(node);
        if containing_module_specifier.is_some()
            && is_source_phase_import(containing_module_specifier.parent())
        {
            return Ok(self.provide_definition_at_position(
                ctx,
                program,
                file,
                text_pos,
                client_supports_link,
            ));
        }

        // If the cursor is directly on a module specifier string, resolve to the
        // implementation file's entry point.
        if node == containing_module_specifier {
            // PORT: Go passes the file as an `ast.HasFileName`.
            let specifier_mode = program.get_mode_for_usage_location(
                &autoimport::source_file_has_file_name(file),
                containing_module_specifier,
            );
            let implementation_file =
                resolver.resolve_implementation(containing_module_specifier.text(), specifier_mode);
            if !implementation_file.is_empty() {
                let source_file = resolver.get_or_parse_source_file(&implementation_file);
                if source_file.is_some() {
                    return Ok(self.create_definition_locations(
                        origin_selection_range,
                        client_supports_link,
                        &get_source_definition_entry_declarations(source_file),
                        None,
                        Feature::DEFINITION,
                    ));
                }
            }
            return Ok(self.provide_definition_at_position(
                ctx,
                program,
                file,
                text_pos,
                client_supports_link,
            ));
        }

        // Phase 1: Syntactic fast path — when the cursor is inside an
        // import/require/export, forward-resolve the module specifier to an
        // implementation file and search it directly. This avoids acquiring
        // the type checker entirely when the fast path succeeds.
        let mut resolved_impl_file = String::new();
        if containing_module_specifier.is_some() {
            let specifier_mode = program.get_mode_for_usage_location(
                &autoimport::source_file_has_file_name(file),
                containing_module_specifier,
            );
            resolved_impl_file =
                resolver.resolve_implementation(containing_module_specifier.text(), specifier_mode);
        }

        if !resolved_impl_file.is_empty() {
            let names = get_candidate_source_declaration_names(node, Node::NIL);
            let module_results =
                resolver.search_implementation_file(node, &resolved_impl_file, &names);
            if !module_results.is_empty() {
                if !is_part_of_type_node(node)
                    && !is_part_of_type_only_import_or_export_declaration(node)
                    || has_concrete_source_declarations(&module_results)
                {
                    return Ok(self.create_definition_locations(
                        origin_selection_range,
                        client_supports_link,
                        &unique_declaration_nodes(&module_results),
                        None,
                        Feature::DEFINITION,
                    ));
                }
            }
        }

        // Phase 2: Type checker path — acquire the checker for the original file
        // and use its declarations and module specifier to map to source
        // implementations. This is the only point where the checker is used;
        // after this, only the NoDts module resolver and file parsing are needed.
        let (checker_declarations, module_specifier) =
            get_source_def_checker_info(ctx, program, file, node);

        // Phase 3: Map checker results to source definitions.
        let declarations = resolver.resolve_from_checker_info(
            node,
            &resolved_impl_file,
            &checker_declarations,
            &module_specifier,
        );
        if declarations.is_empty() {
            // If we resolved an implementation file from an import/export but
            // couldn't find specific declarations, fall back to the file entry
            // point rather than the standard definition provider — unless the
            // checker found declarations that are all type-only (e.g. interfaces),
            // in which case the .d.ts definition is more appropriate.
            if containing_module_specifier.is_some()
                && !resolved_impl_file.is_empty()
                && !has_concrete_source_declarations(&checker_declarations)
            {
                let source_file = resolver.get_or_parse_source_file(&resolved_impl_file);
                if source_file.is_some() {
                    return Ok(self.create_definition_locations(
                        origin_selection_range,
                        client_supports_link,
                        &get_source_definition_entry_declarations(source_file),
                        None,
                        Feature::DEFINITION,
                    ));
                }
            }
            return Ok(self.provide_definition_at_position(
                ctx,
                program,
                file,
                text_pos,
                client_supports_link,
            ));
        }
        Ok(self.create_definition_locations(
            origin_selection_range,
            client_supports_link,
            &declarations,
            None, /*reference*/
            Feature::DEFINITION,
        ))
    }
}

// Go: ls/sourcedefinition.go:135 sourceDefResolver
// sourceDefResolver resolves source definitions by mapping .d.ts declarations
// to their implementation files (.js/.ts). It uses the NoDts module resolver
// and file parsing for resolution, but never acquires the type checker or
// the original program; all checker-dependent work is done before results
// are passed in.
// PORT: Go `*sourceDefResolver` is owned by its one request; methods that
// fill `parsedFiles` take `&mut self`. Go `parsedFiles` is a nil map until
// the first store, so it is an `Option`.
pub struct SourceDefResolver<'a> {
    pub ls: &'a LanguageService,
    pub fs: Rc<dyn vfs::Fs>,
    pub options: &'a CompilerOptions,
    pub get_source_file: Box<dyn Fn(&str) -> Node + 'a>,
    pub resolve_from: String,
    pub resolver: module::DefaultResolver,
    pub parsed_files: Option<FxHashMap<String, Node>>,
}

impl LanguageService {
    // Go: ls/sourcedefinition.go:158 newSourceDefResolver
    pub fn new_source_def_resolver<'a>(
        &'a self,
        program: &'a compiler::NewProgram,
        resolve_from: &str,
    ) -> SourceDefResolver<'a> {
        let options = program.options();
        let mut no_dts_options = options.clone();
        no_dts_options.no_dts_resolution = Tristate::True;
        // ts#64159: the resolution host is the program's file system with the
        // program's base directory (Go N' `sourceDefResolutionHost`,
        // sourcedefinition.go:145, :172; R1).
        // PORT: `CompilerResolutionHost` with that current directory is Go's
        // `sourceDefResolutionHost`.
        let host = program.host().clone();
        let resolution_host: Rc<dyn module::ResolutionHost> =
            Rc::new(compiler::CompilerResolutionHost {
                fs: host.fs(),
                host,
                current_directory: program.base_directory(),
            });
        SourceDefResolver {
            ls: self,
            fs: program.host().fs(),
            options,
            // PORT: Go stores the method value `program.GetSourceFile`; the
            // file root node stands for Go `*ast.SourceFile` (nil when absent).
            get_source_file: Box::new(move |file_name: &str| {
                program
                    .get_source_file(file_name)
                    .map_or(Node::NIL, |source_file| source_file.root)
            }),
            resolve_from: resolve_from.to_string(),
            resolver: module::new_resolver(module::ResolverOptions {
                host: Some(resolution_host),
                compiler_options: Some(Rc::new(no_dts_options)),
                typings_location: program.get_global_typings_cache_location(),
                extra_extensions: program.command_line().content_mapper_extensions(),
                ..Default::default()
            }),
            parsed_files: None,
        }
    }
}

impl SourceDefResolver<'_> {
    // Go: ls/sourcedefinition.go:183 resolveFromCheckerInfo
    // resolveFromCheckerInfo maps type-checker declarations to source
    // implementations. It uses only the NoDts module resolver and file parsing;
    // the type checker and original request file are not needed.
    pub fn resolve_from_checker_info(
        &mut self,
        node: Node,
        resolved_impl_file: &str,
        checker_declarations: &[Node],
        module_specifier: &str,
    ) -> Vec<Node> {
        let mut resolved_impl_file = resolved_impl_file.to_string();
        // If we don't yet have a forward-resolved implementation file, try to
        // recover a module specifier from the checker (e.g. from the import that
        // brought the symbol into scope, or from the root of an access expression).
        if resolved_impl_file.is_empty() && !module_specifier.is_empty() {
            let mode = self.infer_implied_node_format(&self.resolve_from);
            resolved_impl_file = self.resolve_implementation(module_specifier, mode);
        }

        // For property access where the checker found no declarations (e.g.
        // mapped types), search the implementation file for the property name.
        if checker_declarations.is_empty() && !resolved_impl_file.is_empty() {
            let names = get_candidate_source_declaration_names(node, Node::NIL);
            // PORT: Go tests `results != nil`. `searchImplementationFile`
            // returns nil or a non-empty slice, so this is `!is_empty()`.
            let results = self.search_implementation_file(node, &resolved_impl_file, &names);
            if !results.is_empty() {
                return unique_declaration_nodes(&results);
            }
        }

        let mut declarations: Vec<Node> = Vec::new();
        for &declaration in checker_declarations {
            declarations.extend(self.map_declaration_to_source(
                node,
                declaration,
                &resolved_impl_file,
            ));
        }
        let declarations = unique_declaration_nodes(&declarations);
        if has_concrete_source_declarations(&declarations) {
            return declarations;
        }
        Vec::new()
    }
}

// Go: ls/sourcedefinition.go:219 getSourceDefCheckerInfo
// getSourceDefCheckerInfo acquires the type checker for the given file and
// returns the definition declarations for node along with the module specifier
// of the import that brought the symbol into scope (empty if not applicable).
pub fn get_source_def_checker_info(
    ctx: &Context,
    program: &compiler::NewProgram,
    file: Node,
    node: Node,
) -> (Vec<Node>, String) {
    // Go: `defer done()`; `_done` releases the lease at the end of the scope.
    let (checker, _done) = ls_program::get_type_checker_for_file(program, ctx, file);
    let c = &mut *checker.borrow_mut();

    let mut declarations = get_declarations_from_location(c, node);
    let is_property_name = node.parent().is_some()
        && is_access_expression(node.parent())
        && node.parent().name() == node;
    if declarations.is_empty() && is_property_name {
        let left = node.parent().expression();
        if left.is_some() {
            let left_type = c.get_type_at_location(left);
            let prop = c.get_property_of_type_exported(left_type, node.text());
            if prop.is_some() {
                declarations = c.sym(prop).declarations.to_vec();
            }
        }
    }
    let called_declaration = try_get_signature_declaration(c, node);
    if called_declaration.is_some() {
        let mut non_function_declarations: Vec<Node> = declarations
            .into_iter()
            .filter(|&node| !is_function_like(node))
            .collect();
        non_function_declarations.push(called_declaration);
        declarations = non_function_declarations;
    }

    // Extract module specifier from the import that brought this symbol into
    // scope. For property access (obj.prop), walk up the access chain to the
    // root expression's symbol.
    let mut module_specifier = String::new();
    let mut resolve_node = node;
    if is_property_name {
        let mut expr = node.parent().expression();
        while expr.is_some() && is_access_expression(expr) {
            expr = expr.expression();
        }
        if expr.is_some() {
            resolve_node = expr;
        }
    }
    let sym = c.get_symbol_at_location_exported(resolve_node);
    if sym.is_some() {
        let sym_declarations = c.sym(sym).declarations.to_vec();
        for d in sym_declarations {
            if !is_import_specifier(d)
                && !is_import_clause(d)
                && !is_namespace_import(d)
                && !is_import_equals_declaration(d)
            {
                continue;
            }
            let spec = try_get_module_specifier_from_declaration(d);
            if spec.is_some() {
                module_specifier = spec.text().to_string();
                break;
            }
        }
    }

    (declarations, module_specifier)
}

impl SourceDefResolver<'_> {
    // Go: ls/sourcedefinition.go:275 resolveTripleSlashReference
    // resolveTripleSlashReference handles /// <reference path/types="..."/> directives.
    // For path references to .js files, it returns the entry declarations directly.
    // For path references to .d.ts files or type references, it uses the NoDts
    // resolver to find the corresponding implementation file.
    // PORT: Go returns `*ast.FileReference`; a nil one is `None`.
    pub fn resolve_triple_slash_reference(
        &mut self,
        file: Node,
        pos: i32,
        program: &compiler::NewProgram,
    ) -> (Vec<Node>, Option<FileReference>) {
        let Some(ref_) = get_reference_at_position(file, pos, program) else {
            return (Vec::new(), None);
        };
        if ref_.file.is_nil() {
            return (Vec::new(), None);
        }

        // If the referenced file is already an implementation file, return it directly.
        if !source_file_info(ref_.file).is_declaration_file {
            return (
                get_source_definition_entry_declarations(ref_.file),
                ref_.reference.clone(),
            );
        }

        // The referenced file is a .d.ts. Try to find the implementation file
        // using the NoDts module resolver via findImplementationFileFromDtsFileName.
        let dts_file_name = source_file_file_name(ref_.file);
        let preferred_mode = self.infer_implied_node_format(dts_file_name);
        let implementation_file =
            self.find_implementation_file_from_dts_file_name(dts_file_name, preferred_mode);
        if implementation_file.is_empty() {
            return (Vec::new(), None);
        }

        let source_file = self.get_or_parse_source_file(&implementation_file);
        if source_file.is_nil() {
            return (Vec::new(), None);
        }
        (
            get_source_definition_entry_declarations(source_file),
            ref_.reference.clone(),
        )
    }

    // Go: ls/sourcedefinition.go:305 searchImplementationFile
    // searchImplementationFile searches an implementation file for declarations
    // matching the given names. Returns nil when no declarations matched; callers
    // fall through to the checker path or to the standard definition provider.
    pub fn search_implementation_file(
        &mut self,
        original_node: Node,
        implementation_file: &str,
        names: &[String],
    ) -> Vec<Node> {
        if implementation_file.is_empty() {
            return Vec::new();
        }
        let source_file = self.get_or_parse_source_file(implementation_file);
        if source_file.is_nil() {
            return Vec::new();
        }
        if is_default_import_name(original_node) {
            // For default imports, only search for "default" declarations to avoid
            // matching unrelated declarations with the same identifier name.
            let default_declarations = self.find_declarations_in_file(
                implementation_file,
                &["default".to_string()],
                &mut FxHashSet::default(),
            );
            if !default_declarations.is_empty() {
                return filter_preferred_source_declarations(original_node, default_declarations);
            }
            return get_source_definition_entry_declarations(source_file);
        }
        let declarations =
            self.find_declarations_in_file(implementation_file, names, &mut FxHashSet::default());
        if !declarations.is_empty() {
            return filter_preferred_source_declarations(original_node, declarations);
        }
        Vec::new()
    }
}

// Go: ls/sourcedefinition.go:333 isDefaultImportName
pub fn is_default_import_name(node: Node) -> bool {
    if node.is_nil()
        || node.parent().is_nil()
        || !is_import_clause(node.parent())
        || node.parent().name() != node
        || node.parent().parent().is_nil()
    {
        return false;
    }
    is_default_import(node.parent().parent())
}

// Go: ls/sourcedefinition.go:340 getSourceDefinitionEntryNode
pub fn get_source_definition_entry_node(source_file: Node) -> Node {
    let statements = source_file.statements();
    if !statements.is_empty() {
        return statements.get(0);
    }
    source_file
}

// Go: ls/sourcedefinition.go:347 getSourceDefinitionEntryDeclarations
pub fn get_source_definition_entry_declarations(source_file: Node) -> Vec<Node> {
    vec![get_source_definition_entry_node(source_file)]
}

impl SourceDefResolver<'_> {
    // Go: ls/sourcedefinition.go:351 mapDeclarationToSource
    pub fn map_declaration_to_source(
        &mut self,
        original_node: Node,
        declaration: Node,
        resolved_impl_file: &str,
    ) -> Vec<Node> {
        let (file, start_pos) = get_file_and_start_pos_from_declaration(declaration);
        let file_name = source_file_file_name(file);

        if let Some(mapped) = self.ls.try_get_source_position(file_name, start_pos) {
            let source_file = self.get_or_parse_source_file(&mapped.file_name);
            if source_file.is_some() {
                return vec![find_closest_declaration_node(source_file, mapped.pos)];
            }
        }

        if !tspath::is_declaration_file_name(file_name) {
            return vec![declaration];
        }

        let mut implementation_file = resolved_impl_file.to_string();
        if implementation_file.is_empty() {
            // Reverse-resolve .d.ts path to implementation file. This path is only
            // reached for declarations with no associated module specifier (e.g.
            // globals, ambient declarations, or when forward resolution failed).
            let dts_file_name = source_file_file_name(get_source_file_of_node(declaration));
            let preferred_mode = self.infer_implied_node_format(dts_file_name);
            implementation_file =
                self.find_implementation_file_from_dts_file_name(dts_file_name, preferred_mode);
        }

        let names = get_candidate_source_declaration_names(original_node, declaration);
        self.search_implementation_file(original_node, &implementation_file, &names)
    }

    // Go: ls/sourcedefinition.go:382 findImplementationFileFromDtsFileName
    pub fn find_implementation_file_from_dts_file_name(
        &self,
        dts_file_name: &str,
        preferred_mode: ResolutionMode,
    ) -> String {
        let js_ext = module::try_get_js_extension_for_file(dts_file_name, self.options);
        if !js_ext.is_empty() {
            let candidate = tspath::change_extension(dts_file_name, js_ext);
            if self.fs.file_exists(&candidate) {
                return candidate;
            }
        }

        let Some(parts) = modulespecifiers::get_node_module_path_parts(dts_file_name) else {
            return String::new();
        };

        // ts#64159: Go N' bails out on `parts.HasNestedNodeModules`
        // (sourcedefinition.go:398). N bailed out on any second
        // `/node_modules/`, also when it was the package's own name.
        if has_nested_node_modules(dts_file_name, &parts) {
            return String::new();
        }
        // ts#64159: a file directly in node_modules (or in a scope directory)
        // has no package root, so it has no implementation file (Go N'
        // `parts.IsDirectNodeModulesFile`, sourcedefinition.go:401). N sliced
        // with a package root index of -1 and panicked.
        if parts.package_root_index == -1 {
            return String::new();
        }

        let package_name_path_part = &dts_file_name
            [(parts.top_level_package_name_index + 1) as usize..parts.package_root_index as usize];
        let package_name = module::get_package_name_from_types_package_name(
            &module::unmangle_scoped_package_name(package_name_path_part),
        );
        if package_name.is_empty() {
            return String::new();
        }

        let path_to_file_in_package = &dts_file_name[(parts.package_root_index + 1) as usize..];

        // Try resolving as a package subpath first (e.g. "pkg/dist/utils"), then
        // fall back to the bare package name (e.g. "pkg"). This covers both main
        // entrypoints and deep imports without needing to inspect package.json
        // entrypoints.
        if !path_to_file_in_package.is_empty() {
            let specifier = format!(
                "{}/{}",
                package_name,
                tspath::remove_file_extension(path_to_file_in_package)
            );
            let implementation_file = self.resolve_implementation(&specifier, preferred_mode);
            if !implementation_file.is_empty() {
                return implementation_file;
            }
        }
        self.resolve_implementation(&package_name, preferred_mode)
    }

    // Go: ls/sourcedefinition.go:423 resolveImplementation
    pub fn resolve_implementation(
        &self,
        module_name: &str,
        preferred_mode: ResolutionMode,
    ) -> String {
        self.resolve_implementation_from(module_name, &self.resolve_from, preferred_mode)
    }

    // Go: ls/sourcedefinition.go:430 resolveImplementationFrom
    pub fn resolve_implementation_from(
        &self,
        module_name: &str,
        resolve_from_file: &str,
        preferred_mode: ResolutionMode,
    ) -> String {
        let mut modes: Vec<ResolutionMode> = vec![preferred_mode];
        if preferred_mode != ModuleKind::ES_NEXT {
            modes.push(ModuleKind::ES_NEXT);
        }
        if preferred_mode != ModuleKind::COMMON_JS {
            modes.push(ModuleKind::COMMON_JS);
        }

        for mode in modes {
            // PORT: Go `resolved != nil` always holds; the resolver returns an
            // `Arc<ResolvedModule>`.
            let (resolved, _, _) =
                self.resolver
                    .resolve_module_name(module_name, resolve_from_file, mode, None);
            if resolved.is_resolved()
                && !tspath::is_declaration_file_name(&resolved.resolved_file_name)
            {
                return resolved.resolved_file_name.clone();
            }
        }
        String::new()
    }

    // Go: ls/sourcedefinition.go:452 getOrParseSourceFile
    // PORT: the parsed file is published with no program
    // (`program::publish_parsed_files`), then bound into the binder lineage
    // (`program::bind_file_outside_program`), which is Go `BindSourceFile`.
    // The text is leaked, as in the parse cache.
    pub fn get_or_parse_source_file(&mut self, file_name: &str) -> Node {
        let source_file = (self.get_source_file)(file_name);
        if source_file.is_some() {
            return source_file;
        }
        if let Some(parsed_files) = &self.parsed_files {
            if let Some(&source_file) = parsed_files.get(file_name) {
                return source_file;
            }
        }
        let mut source_file = Node::NIL;
        let (text, ok) = self.ls.read_file(file_name);
        if ok {
            // The file is published for good, so its nodes belong to the thread.
            let _base = crate::ast::enter_base_synthetic_owner();
            let text: &'static str = match text.as_static() {
                Some(text) => text,
                None => Box::leak(Box::from(&*text)),
            };
            let file = Rc::new(crate::frontend::parser::parse_source_file(
                &crate::frontend::parser::SourceFileParseOptions {
                    file_name: file_name.to_string(),
                    path: self.ls.to_path(file_name),
                    ..Default::default()
                },
                text,
                // A declaration map's `sources` entries are arbitrary strings, so the
                // file name here may not have a recognized extension.
                crate::frontend::core_ext::ensure_script_kind_from_file_name(file_name),
            ));
            crate::program::note_parsed_source_file(&file);
            crate::program::publish_parsed_files(&self.ls.program.get_current_directory());
            crate::program::bind_file_outside_program(file.root);
            source_file = file.root;
        }
        self.parsed_files
            .get_or_insert_with(FxHashMap::default)
            .insert(file_name.to_string(), source_file);
        source_file
    }

    // Go: ls/sourcedefinition.go:479 inferImpliedNodeFormat
    // inferImpliedNodeFormat determines the module format for a source file that may not be
    // in the program, using the file extension and nearest package.json "type" field.
    pub fn infer_implied_node_format(&self, file_name: &str) -> ResolutionMode {
        let mut package_json_type = String::new();
        // PORT: Go `scope.Exists()` is false for a nil scope.
        if let Some(scope) = self
            .resolver
            .get_package_scope_for_path(&tspath::get_directory_path(file_name))
        {
            if scope.exists() {
                let contents = scope
                    .contents
                    .as_ref()
                    .unwrap_or_else(|| crate::core::go_nil_dereference());
                let (value, ok) = contents.header_fields.type_.get_value();
                if ok {
                    package_json_type = value;
                }
            }
        }
        get_implied_node_format_for_file(file_name, &package_json_type)
    }
}

// Go: ls/sourcedefinition.go:489 findContainingModuleSpecifier
pub fn find_containing_module_specifier(node: Node) -> Node {
    let mut current = node;
    while current.is_some() {
        if is_any_import_or_re_export(current)
            || is_require_call(current, true /*requireStringLiteralLikeArgument*/)
            || is_import_call(current)
        {
            // PORT: `lsutil` also defines `get_external_module_name`; Go calls
            // the ast one.
            let module_specifier = crate::ast::get_external_module_name(current);
            if module_specifier.is_some() && is_string_literal_like(module_specifier) {
                return module_specifier;
            }
        }
        current = current.parent();
    }
    Node::NIL
}

impl SourceDefResolver<'_> {
    // Go: ls/sourcedefinition.go:500 findDeclarationsInFile
    // PORT: Go `seen *collections.Set[string]` is `&mut FxHashSet<String>`.
    pub fn find_declarations_in_file(
        &mut self,
        file_name: &str,
        names: &[String],
        seen: &mut FxHashSet<String>,
    ) -> Vec<Node> {
        if file_name.is_empty() || names.is_empty() {
            return Vec::new();
        }
        if !seen.insert(file_name.to_string()) {
            return Vec::new();
        }

        let source_file = self.get_or_parse_source_file(file_name);
        if source_file.is_nil() {
            return Vec::new();
        }

        let declarations = find_declaration_nodes_by_name(source_file, names);
        if !declarations.is_empty() && has_concrete_source_declarations(&declarations) {
            return declarations;
        }

        let mut forwarded: Vec<Node> = Vec::new();
        for forwarded_file in self.get_forwarded_implementation_files(source_file) {
            forwarded.extend(self.find_declarations_in_file(&forwarded_file, names, seen));
        }
        if !forwarded.is_empty() {
            if has_concrete_source_declarations(&forwarded) {
                return unique_declaration_nodes(&forwarded);
            }
            let mut combined = declarations.clone();
            combined.extend(forwarded);
            return unique_declaration_nodes(&combined);
        }
        declarations
    }

    // Go: ls/sourcedefinition.go:535 getForwardedImplementationFiles
    pub fn get_forwarded_implementation_files(&self, source_file: Node) -> Vec<String> {
        let preferred_mode = self.infer_implied_node_format(source_file_file_name(source_file));

        let mut files: Vec<String> = Vec::new();
        for imp in source_file_imports(source_file).iter() {
            let module_name = imp.text();
            let implementation_file = self.resolve_implementation_from(
                module_name,
                source_file_file_name(source_file),
                preferred_mode,
            );
            if !implementation_file.is_empty() {
                files.push(implementation_file);
            }
        }
        deduplicate(files)
    }
}

// Go: ls/sourcedefinition.go:548 getCandidateSourceDeclarationNames
pub fn get_candidate_source_declaration_names(
    original_node: Node,
    declaration: Node,
) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    if declaration.is_some() {
        let name = get_name_of_declaration(declaration);
        if name.is_some() {
            let text = get_text_of_property_name(name);
            if !text.is_empty() {
                names.push(text);
            }
        }
        if declaration.kind() == SyntaxKind::ExportAssignment {
            names.push("default".to_string());
        }
        if (is_function_declaration(declaration) || is_class_declaration(declaration))
            && declaration
                .modifier_flags()
                .contains(ModifierFlags::EXPORT_DEFAULT)
        {
            names.push("default".to_string());
        }
        if is_import_specifier(declaration) || is_export_specifier(declaration) {
            let prop_name = declaration.property_name();
            if prop_name.is_some() {
                names.push(prop_name.text().to_string());
            }
        }
    }
    if original_node.is_some() {
        if is_identifier(original_node) || is_private_identifier(original_node) {
            names.push(original_node.text().to_string());
        }
        if is_default_import_name(original_node) {
            names.push("default".to_string());
        }
        if original_node.parent().is_some() {
            if is_import_specifier(original_node.parent())
                || is_export_specifier(original_node.parent())
            {
                let prop_name = original_node.parent().property_name();
                if prop_name.is_some() {
                    names.push(prop_name.text().to_string());
                }
            }
        }
    }
    names
}

// Go: ls/sourcedefinition.go:586 findDeclarationNodesByName
pub fn find_declaration_nodes_by_name(source_file: Node, names: &[String]) -> Vec<Node> {
    let names: Vec<String> = deduplicate(
        names
            .iter()
            .filter(|name| !name.is_empty())
            .cloned()
            .collect(),
    );
    if names.is_empty() {
        return Vec::new();
    }

    let mut wanted: FxHashSet<String> = FxHashSet::default();
    let mut want_default = false;
    for name in &names {
        if name == "default" {
            want_default = true;
            continue;
        }
        wanted.insert(name.clone());
    }

    struct Candidate {
        node: Node,
        depth: i32,
    }

    // PORT: the Go `visit` closure captures `wanted`, `wantDefault`,
    // `candidates` and `minDepth`; here they travel in one state value
    // through a recursive function.
    struct VisitState<'a> {
        wanted: &'a FxHashSet<String>,
        want_default: bool,
        candidates: Vec<Candidate>,
        min_depth: i32,
    }

    fn visit(node: Node, state: &mut VisitState<'_>) -> bool {
        let mut matched = false;
        let name = get_name_of_declaration(node);
        if name.is_some() {
            let text = get_text_of_property_name(name);
            if !text.is_empty() && state.wanted.contains(&text) {
                matched = true;
            }
        }
        if state.want_default && node.kind() == SyntaxKind::ExportAssignment {
            matched = true;
        }
        if state.want_default
            && (is_function_declaration(node) || is_class_declaration(node))
            && node
                .modifier_flags()
                .contains(ModifierFlags::EXPORT_DEFAULT)
        {
            matched = true;
        }
        if matched {
            let depth = get_container_depth(node);
            state.candidates.push(Candidate { node, depth });
            if depth < state.min_depth {
                state.min_depth = depth;
            }
        }
        node.for_each_child(|child| visit(child, state))
    }

    let mut state = VisitState {
        wanted: &wanted,
        want_default,
        candidates: Vec::new(),
        // PORT: Go `math.MaxInt`; depths are small, so `i32::MAX` compares the same.
        min_depth: i32::MAX,
    };
    source_file.for_each_child(|child| visit(child, &mut state));

    // Only keep declarations at the shallowest depth, like getTopMostDeclarationNamesInFile.
    let mut declarations: Vec<Node> = Vec::new();
    for c in &state.candidates {
        if c.depth == state.min_depth {
            declarations.push(c.node);
        }
    }
    unique_declaration_nodes(&declarations)
}

// Go: ls/sourcedefinition.go:648 getContainerDepth
// getContainerDepth counts the number of container nodes above a declaration,
// matching the behavior of getDepth in getTopMostDeclarationNamesInFile.
pub fn get_container_depth(node: Node) -> i32 {
    let mut depth = 0;
    let mut current = node;
    while current.is_some() {
        current = get_container_node(current);
        depth += 1;
    }
    depth
}

// Go: ls/sourcedefinition.go:658 filterPreferredSourceDeclarations
pub fn filter_preferred_source_declarations(
    original_node: Node,
    declarations: Vec<Node>,
) -> Vec<Node> {
    if declarations.len() <= 1 || original_node.is_nil() {
        return declarations;
    }
    let preferred = get_property_like_source_declarations(original_node, &declarations);
    if !preferred.is_empty() {
        return preferred;
    }
    let preferred: Vec<Node> = declarations
        .iter()
        .copied()
        .filter(|&node| is_concrete_source_declaration(node))
        .collect();
    if !preferred.is_empty() {
        return preferred;
    }
    declarations
}

// Go: ls/sourcedefinition.go:671 getPropertyLikeSourceDeclarations
pub fn get_property_like_source_declarations(
    original_node: Node,
    declarations: &[Node],
) -> Vec<Node> {
    if original_node.parent().is_nil()
        || !is_access_expression(original_node.parent())
        || original_node.parent().name() != original_node
    {
        return Vec::new();
    }
    declarations
        .iter()
        .copied()
        .filter(|node| {
            matches!(
                node.kind(),
                SyntaxKind::PropertyAssignment
                    | SyntaxKind::ShorthandPropertyAssignment
                    | SyntaxKind::PropertyDeclaration
                    | SyntaxKind::PropertySignature
                    | SyntaxKind::MethodDeclaration
                    | SyntaxKind::MethodSignature
                    | SyntaxKind::GetAccessor
                    | SyntaxKind::SetAccessor
                    | SyntaxKind::EnumMember
            )
        })
        .collect()
}

// Go: ls/sourcedefinition.go:693 hasConcreteSourceDeclarations
pub fn has_concrete_source_declarations(declarations: &[Node]) -> bool {
    declarations
        .iter()
        .any(|&node| is_concrete_source_declaration(node))
}

// Go: ls/sourcedefinition.go:697 isConcreteSourceDeclaration
pub fn is_concrete_source_declaration(node: Node) -> bool {
    if !is_declaration(node) || node.kind() == SyntaxKind::ExportAssignment {
        return false;
    }
    if (is_binary_expression(node) || is_call_expression(node))
        && get_assignment_declaration_kind(node) != JSDeclarationKind::NONE
    {
        return false;
    }
    !matches!(
        node.kind(),
        SyntaxKind::Parameter
            | SyntaxKind::TypeParameter
            | SyntaxKind::BindingElement
            | SyntaxKind::ImportClause
            | SyntaxKind::ImportSpecifier
            | SyntaxKind::NamespaceImport
            | SyntaxKind::ExportSpecifier
            | SyntaxKind::PropertyAccessExpression
            | SyntaxKind::ElementAccessExpression
    )
}

// Go: ls/sourcedefinition.go:720 uniqueDeclarationNodes
pub fn unique_declaration_nodes(nodes: &[Node]) -> Vec<Node> {
    #[derive(PartialEq, Eq, Hash)]
    struct DeclarationKey {
        file_name: String,
        loc: TextRange,
    }
    let mut seen: FxHashSet<DeclarationKey> = FxHashSet::default();
    let mut result: Vec<Node> = Vec::with_capacity(nodes.len());
    for &node in nodes {
        if node.is_nil() {
            continue;
        }
        let file_name = source_file_file_name(get_source_file_of_node(node)).to_string();
        let key = DeclarationKey {
            file_name,
            loc: node.loc(),
        };
        if !seen.insert(key) {
            continue;
        }
        result.push(node);
    }
    result
}

// Go: ls/sourcedefinition.go:741 findClosestDeclarationNode
pub fn find_closest_declaration_node(source_file: Node, pos: i32) -> Node {
    let node = astnav::get_touching_property_name(source_file, pos);
    let mut current = node;
    while current.is_some() {
        if is_declaration(current) || current.kind() == SyntaxKind::ExportAssignment {
            return current;
        }
        current = current.parent();
    }
    get_source_definition_entry_node(source_file)
}

// Go N' `NodeModulePathParts.HasNestedNodeModules` (modulespecifiers/util.go:315-318):
// a `/node_modules/` segment after the first package root. The root comes after
// the scope of a scoped package, so `node_modules/node_modules/x/a.d.ts` and
// `node_modules/@s/node_modules/a.d.ts` (a package named `node_modules`) are not nested.
// PORT: the port keeps the index form of `NodeModulePathParts`, whose
// `package_root_index` is the root of the last package. This finds the first
// root from `top_level_package_name_index`, as the Go parse states do (:301-314).
fn has_nested_node_modules(file_name: &str, parts: &modulespecifiers::NodeModulePathParts) -> bool {
    let name_start = (parts.top_level_package_name_index + 1) as usize;
    let next_slash = |from: usize| file_name[from..].find('/').map(|i| from + i);
    let mut root = next_slash(name_start);
    if file_name.as_bytes().get(name_start) == Some(&b'@') {
        root = root.and_then(|scope_end| next_slash(scope_end + 1));
    }
    root.is_some_and(|root| file_name[root..].contains("/node_modules/"))
}

#[cfg(test)]
mod tests {
    use super::has_nested_node_modules;
    use crate::modulespecifiers::get_node_module_path_parts;

    fn nested(file_name: &str) -> bool {
        let parts = get_node_module_path_parts(file_name).expect("node_modules path");
        has_nested_node_modules(file_name, &parts)
    }

    // Go: modulespecifiers/specifiers_test.go:16 TestGetNodeModulePathParts (ts#64159),
    // the `hasNestedNodeModules` values, and the ls skeptic's sourcedef2 probe paths.
    #[test]
    fn test_has_nested_node_modules() {
        assert!(!nested("/workspace/node_modules/pkg/lib/index.d.ts"));
        assert!(!nested("/node_modules/@scope/pkg/index.d.ts"));
        assert!(!nested("c:/node_modules/pkg/index.d.ts"));
        assert!(nested(
            "/workspace/node_modules/pkg/node_modules/@scope/dep/index.d.ts"
        ));
        assert!(nested("/p/node_modules/@s/a/node_modules/b/index.d.ts"));
        // A package named `node_modules` (unscoped and scoped) is not nested.
        assert!(!nested("/p/node_modules/node_modules/x/types/index.d.ts"));
        assert!(!nested("/p/node_modules/@sc/node_modules/t/s.d.ts"));
        assert!(nested("/p/node_modules/node_modules/node_modules/x/a.d.ts"));
        // Files directly in node_modules or in a scope directory have no root.
        assert!(!nested("/workspace/node_modules/pkg"));
        assert!(!nested("/workspace/node_modules/@scope"));
        assert!(!nested("/workspace/node_modules/@scope/a.d.ts"));
    }
}

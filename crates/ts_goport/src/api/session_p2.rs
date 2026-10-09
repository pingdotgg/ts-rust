use crate::api::prelude::*;

// Port of Go `internal/api/session.go`, lines 1272-2484: the
// `resolve*PropertyOf*` helpers, the checker query handlers, node building
// and printing, emit (tsgo#4699), intrinsic types, diagnostics, `resolveNodeHandle`,
// `computeSnapshotChanges`, `Close`, and the references, signature usage and
// completion handlers. The file header of `session_p1.rs` holds the PORT
// notes for both files (registry entries keep the owning checker; handles
// cross to the setup checker only through `checker_symbol`, `checker_type`
// and `checker_signature`).

use crate::api::encoder;
use crate::api::requestfilesystem;
use crate::emitter::emitter::EmitOnly;
use crate::emitter::program_emit::{self, EmitOptions, EmitResult, WriteFile, WriteFileData};
use crate::execute::incremental::emit_files::fs_error_text;
use crate::frontend::compiler;
use crate::frontend::core_context::{self, CheckerLifetime};
use crate::frontend::core_ls_ext::{diff_maps, diff_ordered_maps};
use crate::frontend::json_ext::AnyValue;
use crate::frontend::parser::ParsedSourceFile;
use crate::frontend::tspath;
use crate::frontend::vfs::Fs as _;
use crate::gostd::{self, Context, GoError, errors};
use crate::program::ls_program;
use crate::project;
use std::sync::{Arc, Mutex, PoisonError};

impl Session {
    // Go: api/session.go:2971 resolveTypePropertyOfType
    // resolveTypePropertyOfType resolves a type property of type `Type` and returns a type response.
    // PORT: the getter reads the type in the arena of its checker.
    // ts#64397: takes the context and uses the setup checker.
    pub fn resolve_type_property_of_type(
        &self,
        ctx: &Context,
        params: &GetTypePropertyParams,
        getter: &dyn Fn(&Checker, TypeId) -> TypeId,
    ) -> Result<Option<TypeResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, t) = setup
            .sd
            .resolve_type_handle(&params.project, params.type_)?;
        let t = checker_type(&setup.checker, &owner, t);

        let result = getter(&setup.checker.borrow(), t);
        if result.is_nil() {
            return Ok(None);
        }

        Ok(setup
            .sd
            .new_type_response(&setup.project_id, &setup.checker, result))
    }

    // Go: api/session.go:2992 resolveTypeArrayPropertyOfType
    // resolveTypeArrayPropertyOfType resolves a type property of an array of types and returns an array of type responses.
    // ts#64397: takes the context and uses the setup checker.
    pub fn resolve_type_array_property_of_type(
        &self,
        ctx: &Context,
        params: &GetTypePropertyParams,
        getter: &dyn Fn(&Checker, TypeId) -> Vec<TypeId>,
    ) -> Result<Vec<Option<TypeResponse>>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, t) = setup
            .sd
            .resolve_type_handle(&params.project, params.type_)?;
        let t = checker_type(&setup.checker, &owner, t);

        let types = getter(&setup.checker.borrow(), t);
        if types.is_empty() {
            return Ok(Vec::new());
        }

        let mut results = Vec::with_capacity(types.len());
        for sub in types {
            results.push(
                setup
                    .sd
                    .new_type_response(&setup.project_id, &setup.checker, sub),
            );
        }
        Ok(results)
    }

    // Go: api/session.go:3017 resolveSymbolPropertyOfType
    // resolveSymbolPropertyOfType resolves a type property of type `Symbol` and returns a symbol response.
    pub fn resolve_symbol_property_of_type(
        &self,
        params: &GetTypePropertyParams,
        getter: &dyn Fn(&Checker, TypeId) -> SymbolId,
    ) -> Result<Option<SymbolResponse>, GoError> {
        let sd = self.get_snapshot_data(params.snapshot)?;

        let (checker, t) = sd.resolve_type_handle(&params.project, params.type_)?;
        // Node handles in the answer read lazy JSDoc (session_p1.rs header).
        let _program = ls_program::enter_version(checker.borrow().program);

        let result = getter(&checker.borrow(), t);
        if result.is_nil() {
            return Ok(None);
        }
        Ok(sd.new_symbol_response(&checker, result, &params.project))
    }

    // Go: api/session.go:405 resolveSymbolReference (ts#64518)
    // resolveSymbolReference resolves a symbol without a semantic context. A file reference holds the
    // exact cached AST until the returned release function is called; a snapshot reference also returns
    // the snapshot and canonical project that own the symbol.
    // PORT: the Go release function is the lease guard in the result; it
    // releases when the result drops. A file symbol is read from a copy of
    // the binder lineage (`program::lineage_for_checker`), which holds every
    // live bound file; Go reads the `*ast.Symbol` with no checker.
    pub fn resolve_symbol_reference(
        &self,
        reference: &SymbolReference,
    ) -> Result<ResolvedSymbolReference, GoError> {
        let client_error = |text: String| {
            Err(errors::errorf(
                format!("{}: {text}", *ERR_CLIENT_ERROR),
                vec![ERR_CLIENT_ERROR.clone()],
            ))
        };
        if reference.kind == SymbolOwnerKind::FILE {
            let Some(file) = reference
                .file
                .as_ref()
                .filter(|_| reference.snapshot.0 == 0 && reference.project.0.is_empty())
            else {
                return client_error("invalid file symbol reference".to_string());
            };
            let lease = LeaseGuard(Some(self.acquire_cached_source_file(file)?));
            let symbols = crate::program::lineage_for_checker();
            let root = lease
                .0
                .as_ref()
                .map_or(Node::NIL, |lease| lease.source_file());
            let symbol = get_source_file_symbol_index(&symbols, root)
                .get(&reference.id)
                .copied();
            let Some(symbol) = symbol else {
                // Go: lease.Release() (the guard drops here).
                return client_error(format!(
                    "symbol {} not found in source file",
                    reference.id.0
                ));
            };
            Ok(ResolvedSymbolReference::File {
                symbols,
                symbol,
                _lease: lease,
            })
        } else if reference.kind == SymbolOwnerKind::SNAPSHOT {
            if reference.file.is_some()
                || reference.snapshot.0 == 0
                || reference.project.0.is_empty()
            {
                return client_error("invalid snapshot symbol reference".to_string());
            }
            let sd = self.get_snapshot_data(reference.snapshot)?;
            let (checker, symbol) = sd.resolve_symbol_handle(reference.id)?;
            Ok(ResolvedSymbolReference::Snapshot {
                sd,
                project: reference.project.clone(),
                checker,
                symbol,
            })
        } else {
            client_error(format!(
                "invalid symbol reference kind {}",
                reference.kind.0
            ))
        }
    }

    // Go: api/session.go:2164 handleGetSymbolOfDeclaration (ts#64571)
    // PORT: the binder symbol is read from a copy of the binder lineage, as
    // in `resolve_symbol_reference`.
    pub fn handle_get_symbol_of_declaration(
        &self,
        params: &GetSymbolOfDeclarationParams,
    ) -> Result<SymbolResponse, GoError> {
        let client_error = |text: String| {
            Err(errors::errorf(
                format!("{}: {text}", *ERR_CLIENT_ERROR),
                vec![ERR_CLIENT_ERROR.clone()],
            ))
        };
        let lease = self.acquire_cached_source_file(&params.file)?;
        // Go: defer lease.Release()
        let lease = LeaseGuard(Some(lease));
        let source_file = lease
            .0
            .as_ref()
            .map_or(Node::NIL, |lease| lease.source_file());

        let table = encoder::get_node_index_table(source_file);
        if params.index == 0 || params.index as usize >= table.nodes.len() {
            return client_error(format!(
                "declaration node index {} is out of range",
                params.index
            ));
        }
        let node = table.nodes[params.index as usize];
        if node.is_nil() || !is_declaration(node) {
            return client_error(format!("node index {} is not a declaration", params.index));
        }
        let symbol = node.symbol();
        if symbol.is_nil() {
            return client_error(format!(
                "declaration node index {} has no binder symbol",
                params.index
            ));
        }
        let symbols = crate::program::lineage_for_checker();
        Ok(lease.file_symbol_response(&symbols, symbol))
    }

    // Go: api/session.go:3318 resolveSymbolPropertyOfSymbol
    // resolveSymbolTablePropertyOfSymbol resolves a symbol property of type `Symbol` and returns a symbol response.
    // ts#64518: the symbol comes from its reference, and a file-owned answer
    // needs no snapshot.
    pub fn resolve_symbol_property_of_symbol(
        &self,
        params: &GetSymbolPropertyParams,
        getter: &dyn Fn(&SymbolArena, SymbolId) -> SymbolId,
    ) -> Result<Option<SymbolResponse>, GoError> {
        let resolved = self.resolve_symbol_reference(&params.symbol)?;
        match &resolved {
            ResolvedSymbolReference::File {
                symbols,
                symbol,
                _lease: lease,
            } => {
                let result = getter(symbols, *symbol);
                if result.is_nil() {
                    return Ok(None);
                }
                Ok(Some(lease.file_symbol_response(symbols, result)))
            }
            ResolvedSymbolReference::Snapshot {
                sd,
                project,
                checker,
                symbol,
            } => {
                // Node handles in the answer read lazy JSDoc (session_p1.rs header).
                let _program = ls_program::enter_version(checker.borrow().program);
                let result = getter(&checker.borrow().symbols, *symbol);
                if result.is_nil() {
                    return Ok(None);
                }
                Ok(sd.new_symbol_response(checker, result, project))
            }
        }
    }

    // Go: api/session.go:3337 resolveSymbolTablePropertyOfSymbol
    // resolveSymbolTablePropertyOfSymbol resolves a symbol property of type `SymbolTable` and returns an array of symbol responses.
    // Results are sorted using the checker's canonical symbol ordering so that API consumers receive
    // a stable, deterministic order instead of Go's randomized map iteration order.
    pub fn resolve_symbol_table_property_of_symbol(
        &self,
        ctx: &Context,
        params: &GetSymbolPropertyParams,
        getter: &dyn Fn(&SymbolArena, SymbolId) -> SymbolTable,
    ) -> Result<Vec<Option<SymbolResponse>>, GoError> {
        let resolved = self.resolve_symbol_reference(&params.symbol)?;
        let (sd, project, checker, symbol) = match &resolved {
            ResolvedSymbolReference::File {
                symbols,
                symbol,
                _lease: lease,
            } => {
                let symbol_table = getter(symbols, *symbol);
                if symbol_table.is_nil() || symbols.len(symbol_table) == 0 {
                    return Ok(Vec::new());
                }
                let mut subs = symbols.values(symbol_table);
                if subs.len() > 1 {
                    // Binder tables of a file-owned symbol only contain symbols from the same file, so they
                    // can be ordered by declaration position without a checker.
                    let file = get_source_file_of_symbol(symbols, *symbol);
                    crate::gostd::slices::sort_func(&mut subs, |&left, &right| {
                        go_assert!(get_source_file_of_symbol(symbols, left) == file);
                        go_assert!(get_source_file_of_symbol(symbols, right) == file);
                        let (l, r) = (symbols.sym(left), symbols.sym(right));
                        let left_has_declaration = !l.declarations.is_empty();
                        let right_has_declaration = !r.declarations.is_empty();
                        if left_has_declaration != right_has_declaration {
                            return if left_has_declaration { -1 } else { 1 };
                        }
                        let order = if left_has_declaration {
                            l.declarations[0].pos().cmp(&r.declarations[0].pos())
                        } else {
                            std::cmp::Ordering::Equal
                        }
                        .then_with(|| {
                            go_symbol_name(symbols, left).cmp(&go_symbol_name(symbols, right))
                        })
                        .then_with(|| {
                            get_symbol_id(symbols, left).cmp(&get_symbol_id(symbols, right))
                        });
                        order as i32
                    });
                }
                return Ok(subs
                    .into_iter()
                    .map(|sub| Some(lease.file_symbol_response(symbols, sub)))
                    .collect());
            }
            ResolvedSymbolReference::Snapshot {
                sd,
                project,
                checker,
                symbol,
            } => (sd, project, checker, *symbol),
        };
        // Node handles in the answer read lazy JSDoc (session_p1.rs header).
        let _program = ls_program::enter_version(checker.borrow().program);

        let symbol_table = getter(&checker.borrow().symbols, symbol);
        let table_len = checker.borrow().symbols.len(symbol_table);
        if symbol_table.is_nil() || table_len == 0 {
            return Ok(Vec::new());
        }
        let subs = checker.borrow().symbols.values(symbol_table);
        if table_len == 1 {
            return Ok(vec![sd.new_symbol_response(checker, subs[0], project)]);
        }

        // Tables of snapshot-owned symbols may contain symbols from several files, so they use the
        // checker's ordering.
        let setup = self.setup_checker(ctx, params.symbol.snapshot, &params.symbol.project)?;

        // PORT: the setup checker only sorts. Each entry keeps the symbol of
        // `checker` for its answer, which Go reads with no checker, as the
        // one-entry answer above does.
        let mut symbols: Vec<(SymbolId, SymbolId)> = Vec::with_capacity(table_len);
        for sub in subs {
            symbols.push((checker_symbol(&setup.checker, checker, sub), sub));
        }
        // Go: api/session.go:3402 slices.SortFunc(symbols, setup.checker.CompareSymbols)
        // PORT: `CompareSymbols` is not a total order (see
        // `sort_symbol_sort_keys`), so this is Go's pdqsort, not std `sort_by`,
        // which can panic.
        {
            let mut c = setup.checker.borrow_mut();
            crate::gostd::slices::sort_func(&mut symbols, |&(a, _), &(b, _)| {
                c.compare_symbols_exported(a, b)
            });
        }

        let mut results = Vec::with_capacity(symbols.len());
        for (_, sub) in symbols {
            results.push(sd.new_symbol_response(checker, sub, &setup.project_id));
        }
        Ok(results)
    }

    // Go: api/session.go:3099 resolveSymbolArrayPropertyOfSignature
    // resolveSymbolArrayPropertyOfSignature resolves a signature property of an array of symbols and returns an array of symbol responses.
    pub fn resolve_symbol_array_property_of_signature(
        &self,
        params: &GetSignaturePropertyParams,
        getter: &dyn Fn(&Checker, SignatureId) -> Vec<SymbolId>,
    ) -> Result<Vec<Option<SymbolResponse>>, GoError> {
        let sd = self.get_snapshot_data(params.snapshot)?;

        let (checker, sig) = sd.resolve_signature_handle(&params.project, params.signature)?;
        // Node handles in the answer read lazy JSDoc (session_p1.rs header).
        let _program = ls_program::enter_version(checker.borrow().program);

        let symbols = getter(&checker.borrow(), sig);
        if symbols.is_empty() {
            return Ok(Vec::new());
        }

        let mut results = Vec::with_capacity(symbols.len());
        for sym in symbols {
            results.push(sd.new_symbol_response(&checker, sym, &params.project));
        }
        Ok(results)
    }

    // Go: api/session.go:3123 resolveSymbolPropertyOfSignature
    // resolveSymbolPropertyOfSignature resolves a signature property of type `Symbol` and returns a symbol response.
    pub fn resolve_symbol_property_of_signature(
        &self,
        params: &GetSignaturePropertyParams,
        getter: &dyn Fn(&Checker, SignatureId) -> SymbolId,
    ) -> Result<Option<SymbolResponse>, GoError> {
        let sd = self.get_snapshot_data(params.snapshot)?;

        let (checker, sig) = sd.resolve_signature_handle(&params.project, params.signature)?;
        // Node handles in the answer read lazy JSDoc (session_p1.rs header).
        let _program = ls_program::enter_version(checker.borrow().program);

        let result = getter(&checker.borrow(), sig);
        if result.is_nil() {
            return Ok(None);
        }
        Ok(sd.new_symbol_response(&checker, result, &params.project))
    }

    // Go: api/session.go:3141 resolveTypeArrayPropertyOfSignature
    // ts#64397: takes the context and uses the setup checker.
    pub fn resolve_type_array_property_of_signature(
        &self,
        ctx: &Context,
        params: &GetSignaturePropertyParams,
        getter: &dyn Fn(&Checker, SignatureId) -> Vec<TypeId>,
    ) -> Result<Vec<Option<TypeResponse>>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, sig) = setup
            .sd
            .resolve_signature_handle(&params.project, params.signature)?;
        let sig = checker_signature(&setup.checker, &owner, sig);

        let types = getter(&setup.checker.borrow(), sig);
        if types.is_empty() {
            return Ok(Vec::new());
        }

        let mut results = Vec::with_capacity(types.len());
        for sub in types {
            results.push(
                setup
                    .sd
                    .new_type_response(&setup.project_id, &setup.checker, sub),
            );
        }
        Ok(results)
    }

    // Go: api/session.go:3165 resolveSignaturePropertyOfSignature
    pub fn resolve_signature_property_of_signature(
        &self,
        params: &GetSignaturePropertyParams,
        getter: &dyn Fn(&Checker, SignatureId) -> SignatureId,
    ) -> Result<Option<SignatureResponse>, GoError> {
        let sd = self.get_snapshot_data(params.snapshot)?;

        let (checker, sig) = sd.resolve_signature_handle(&params.project, params.signature)?;
        // Node handles in the answer read lazy JSDoc (session_p1.rs header).
        let _program = ls_program::enter_version(checker.borrow().program);

        let result = getter(&checker.borrow(), sig);
        if result.is_nil() {
            return Ok(None);
        }
        Ok(sd.new_signature_response(&params.project, &checker, result))
    }

    // Go: api/session.go:3185 handleGetContextualType
    // handleGetContextualType returns the contextual type for a node.
    pub fn handle_get_contextual_type(
        &self,
        ctx: &Context,
        params: &GetContextualTypeParams,
    ) -> Result<Option<TypeResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let node = setup
            .sd
            .resolve_node_handle(&setup.program, &params.location)?;
        if node.is_nil() {
            return Ok(None);
        }

        let t = setup
            .checker
            .borrow_mut()
            .get_contextual_type_exported(node, ContextFlags::NONE);
        if t.is_nil() {
            return Ok(None);
        }

        Ok(setup
            .sd
            .new_type_response(&setup.project_id, &setup.checker, t))
    }

    // Go: api/session.go handleGetContextualTypeForArgument (ts#64264)
    pub fn handle_get_contextual_type_for_argument(
        &self,
        ctx: &Context,
        params: &GetContextualTypeForArgumentParams,
    ) -> Result<Option<TypeResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let node = setup
            .sd
            .resolve_node_handle(&setup.program, &params.location)?;
        let t = setup
            .checker
            .borrow_mut()
            .get_contextual_type_for_argument_at_index_exported(node, params.index);
        Ok(setup
            .sd
            .new_type_response(&setup.project_id, &setup.checker, t))
    }

    // Go: api/session.go handleGetAwaitedType (ts#64264)
    pub fn handle_get_awaited_type(
        &self,
        ctx: &Context,
        params: &CheckerTypeParams,
    ) -> Result<Option<TypeResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, t) = setup.resolve_type_handle(params.type_)?;
        let t = checker_type(&setup.checker, &owner, t);
        let awaited = setup.checker.borrow_mut().get_awaited_type_exported(t);
        Ok(setup
            .sd
            .new_type_response(&setup.project_id, &setup.checker, awaited))
    }

    // Go: api/session.go:3239 handleGetBaseTypeOfLiteralType
    // handleGetBaseTypeOfLiteralType returns the base type of a literal type (e.g. number for 42).
    pub fn handle_get_base_type_of_literal_type(
        &self,
        ctx: &Context,
        params: &GetBaseTypeOfLiteralTypeParams,
    ) -> Result<Option<TypeResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, t) = setup.resolve_type_handle(params.type_)?;
        let t = checker_type(&setup.checker, &owner, t);

        let result = setup
            .checker
            .borrow_mut()
            .get_base_type_of_literal_type_exported(t);
        Ok(setup
            .sd
            .new_type_response(&setup.project_id, &setup.checker, result))
    }

    // Go: api/session.go:3255 handleGetNonNullableType
    // handleGetNonNullableType returns the type with null and undefined removed.
    pub fn handle_get_non_nullable_type(
        &self,
        ctx: &Context,
        params: &GetTypePropertyParams,
    ) -> Result<Option<TypeResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, t) = setup.resolve_type_handle(params.type_)?;
        let t = checker_type(&setup.checker, &owner, t);

        let result = setup.checker.borrow_mut().get_non_nullable_type(t);
        Ok(setup
            .sd
            .new_type_response(&setup.project_id, &setup.checker, result))
    }

    // Go: api/session.go:3271 handleGetTypeFromTypeNode
    // handleGetTypeFromTypeNode returns the type for a type node.
    pub fn handle_get_type_from_type_node(
        &self,
        ctx: &Context,
        params: &GetTypeFromTypeNodeParams,
    ) -> Result<Option<TypeResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let node = setup
            .sd
            .resolve_node_handle(&setup.program, &params.location)?;

        let t = setup
            .checker
            .borrow_mut()
            .get_type_from_type_node_exported(node);
        Ok(setup
            .sd
            .new_type_response(&setup.project_id, &setup.checker, t))
    }

    // Go: api/session.go:3287 handleGetWidenedType
    // handleGetWidenedType returns the widened type.
    pub fn handle_get_widened_type(
        &self,
        ctx: &Context,
        params: &GetWidenedTypeParams,
    ) -> Result<Option<TypeResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, t) = setup.resolve_type_handle(params.type_)?;
        let t = checker_type(&setup.checker, &owner, t);

        let result = setup.checker.borrow_mut().get_widened_type_exported(t);
        Ok(setup
            .sd
            .new_type_response(&setup.project_id, &setup.checker, result))
    }

    // Go: api/session.go:3303 handleGetParameterType
    // handleGetParameterType returns the type of a parameter at a given index in a signature.
    pub fn handle_get_parameter_type(
        &self,
        ctx: &Context,
        params: &GetParameterTypeParams,
    ) -> Result<Option<TypeResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, sig) = setup.resolve_signature_handle(params.signature)?;
        let sig = checker_signature(&setup.checker, &owner, sig);

        if params.index < 0 {
            return Err(errors::errorf(
                format!("{}: invalid parameter index", *ERR_CLIENT_ERROR),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        }

        let t = setup
            .checker
            .borrow_mut()
            .get_type_at_position_exported(sig, params.index);
        Ok(setup
            .sd
            .new_type_response(&setup.project_id, &setup.checker, t))
    }

    // Go: api/session.go:3322 handleGetTypeParameterAtPosition
    pub fn handle_get_type_parameter_at_position(
        &self,
        ctx: &Context,
        params: &GetParameterTypeParams,
    ) -> Result<Option<TypeResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, sig) = setup.resolve_signature_handle(params.signature)?;
        let sig = checker_signature(&setup.checker, &owner, sig);
        if params.index < 0 {
            return Err(errors::errorf(
                format!("{}: invalid parameter index", *ERR_CLIENT_ERROR),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        }
        let t = setup
            .checker
            .borrow_mut()
            .get_type_parameter_at_position(sig, params.index);
        Ok(setup
            .sd
            .new_type_response(&setup.project_id, &setup.checker, t))
    }

    // Go: api/session.go:3340 handleIsArrayLikeType
    // handleIsArrayLikeType returns whether a type is array-like.
    pub fn handle_is_array_like_type(
        &self,
        ctx: &Context,
        params: &IsArrayLikeTypeParams,
    ) -> Result<bool, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, t) = setup.resolve_type_handle(params.type_)?;
        let t = checker_type(&setup.checker, &owner, t);

        let result = setup.checker.borrow_mut().is_array_like_type_exported(t);
        Ok(result)
    }

    // Go: api/session.go:3356 handleIsTypeAssignableTo
    // handleIsTypeAssignableTo returns whether source is assignable to target.
    pub fn handle_is_type_assignable_to(
        &self,
        ctx: &Context,
        params: &IsTypeAssignableToParams,
    ) -> Result<bool, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (source_owner, source) = setup.resolve_type_handle(params.source)?;
        let (target_owner, target) = setup.resolve_type_handle(params.target)?;
        let source = checker_type(&setup.checker, &source_owner, source);
        let target = checker_type(&setup.checker, &target_owner, target);

        let result = setup
            .checker
            .borrow_mut()
            .is_type_assignable_to_exported(source, target);
        Ok(result)
    }

    // Go: api/session.go:3377 handleGetShorthandAssignmentValueSymbol
    // handleGetShorthandAssignmentValueSymbol returns the value symbol of a shorthand property assignment.
    pub fn handle_get_shorthand_assignment_value_symbol(
        &self,
        ctx: &Context,
        params: &GetTypeAtLocationParams,
    ) -> Result<Option<SymbolResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let node = setup
            .sd
            .resolve_node_handle(&setup.program, &params.location)?;
        if node.is_nil() {
            return Ok(None);
        }

        let symbol = setup
            .checker
            .borrow_mut()
            .get_shorthand_assignment_value_symbol(node);
        if symbol.is_nil() {
            return Ok(None);
        }

        Ok(setup.new_symbol_response(symbol))
    }

    // Go: api/session.go:3401 handleGetTypeOfSymbolAtLocation
    // handleGetTypeOfSymbolAtLocation returns the narrowed type of a symbol at a specific location.
    pub fn handle_get_type_of_symbol_at_location(
        &self,
        ctx: &Context,
        params: &GetTypeOfSymbolAtLocationParams,
    ) -> Result<Option<TypeResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, symbol) = setup.resolve_symbol_handle(&params.symbol)?;
        let symbol = checker_symbol(&setup.checker, &owner, symbol);

        let node = setup
            .sd
            .resolve_node_handle(&setup.program, &params.location)?;

        let t = setup
            .checker
            .borrow_mut()
            .get_type_of_symbol_at_location(symbol, node);
        Ok(setup
            .sd
            .new_type_response(&setup.project_id, &setup.checker, t))
    }

    // Go: api/session.go:3424 handleTypeToTypeNode
    // handleTypeToTypeNode converts a Type to a TypeNode AST and returns it as binary-encoded data.
    pub fn handle_type_to_type_node(
        &self,
        ctx: &Context,
        params: &TypeToTypeNodeParams,
    ) -> Result<Option<Box<dyn AnyValue>>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, t) = setup.resolve_type_handle(params.type_)?;
        let t = checker_type(&setup.checker, &owner, t);

        let mut enclosing_declaration = Node::NIL;
        if !params.location.0.is_empty() {
            enclosing_declaration = setup
                .sd
                .resolve_node_handle(&setup.program, &params.location)?;
        }

        let type_node = setup.checker.borrow_mut().type_to_type_node_exported(
            t,
            enclosing_declaration,
            NodeBuilderFlags(params.flags as u32),
            None,
        );
        if type_node.is_nil() {
            return Ok(None);
        }

        let data = match encoder::encode_node(type_node, Node::NIL) {
            Ok((data, _)) => data,
            Err(err) => {
                return Err(errors::errorf(
                    format!("failed to encode type node: {err}"),
                    vec![err],
                ));
            }
        };

        if self.use_binary_responses {
            return Ok(to_any(RawBinary(data)));
        }
        Ok(to_any(SourceFileResponse {
            data: base64_std_encoding_encode_to_string(&data),
        }))
    }

    // Go: api/session.go:3464 handleSignatureToSignatureDeclaration
    pub fn handle_signature_to_signature_declaration(
        &self,
        ctx: &Context,
        params: &SignatureToSignatureDeclarationParams,
    ) -> Result<Option<Box<dyn AnyValue>>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, sig) = setup.resolve_signature_handle(params.signature)?;
        let sig = checker_signature(&setup.checker, &owner, sig);

        let mut enclosing_declaration = Node::NIL;
        if !params.location.0.is_empty() {
            enclosing_declaration = setup
                .sd
                .resolve_node_handle(&setup.program, &params.location)?;
        }

        // PORT: Go converts any int32 to `ast.Kind` (int16). A value that is
        // no SyntaxKind has no Rust value. Go's node builder builds the parts
        // and then panics in the default case of its kind switch
        // (nodebuilderimpl.go:1972). `Unknown` takes the same path: the
        // builder compares the kind only with signature kinds.
        let kind = SyntaxKind::try_from(params.kind as u16).unwrap_or(SyntaxKind::Unknown);
        let node = setup
            .checker
            .borrow_mut()
            .signature_to_signature_declaration_exported(
                sig,
                kind,
                enclosing_declaration,
                NodeBuilderFlags(params.flags as u32),
            );
        if node.is_nil() {
            return Ok(None);
        }

        let data = match encoder::encode_node(node, Node::NIL) {
            Ok((data, _)) => data,
            Err(err) => {
                return Err(errors::errorf(
                    format!("failed to encode signature declaration: {err}"),
                    vec![err],
                ));
            }
        };

        if self.use_binary_responses {
            return Ok(to_any(RawBinary(data)));
        }
        Ok(to_any(SourceFileResponse {
            data: base64_std_encoding_encode_to_string(&data),
        }))
    }

    // Go: api/session.go:3503 handleTypeToString
    // handleTypeToString converts a Type to its string representation.
    pub fn handle_type_to_string(
        &self,
        ctx: &Context,
        params: &TypeToTypeNodeParams,
    ) -> Result<Option<Box<dyn AnyValue>>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, t) = setup.resolve_type_handle(params.type_)?;
        let t = checker_type(&setup.checker, &owner, t);

        let mut enclosing_declaration = Node::NIL;
        if !params.location.0.is_empty() {
            enclosing_declaration = setup
                .sd
                .resolve_node_handle(&setup.program, &params.location)?;
        }

        if params.flags != 0 {
            let text = setup.checker.borrow_mut().type_to_string_ex(
                t,
                enclosing_declaration,
                TypeFormatFlags(params.flags as u32),
                None,
            );
            return Ok(to_any(text));
        }
        let text = setup.checker.borrow_mut().type_to_string_ex(
            t,
            enclosing_declaration,
            TypeFormatFlags::ALLOW_UNIQUE_ES_SYMBOL_TYPE
                | TypeFormatFlags::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE,
            None,
        );
        Ok(to_any(text))
    }

    // Go: api/session.go:3530 handlePrintNode
    // handlePrintNode decodes a binary-encoded AST node and prints it to text.
    pub fn handle_print_node(
        &self,
        _ctx: &Context,
        params: &PrintNodeParams,
    ) -> Result<String, GoError> {
        // ts#64320
        let node = decode_print_node(&params.data)?;

        let mut source_file = Node::NIL;
        if is_source_file(node) {
            source_file = node;
        }
        Ok(new_printer(params).emit(node, source_file))
    }

    // Go: api/session.go:3564 handleEmit (tsgo#4699)
    // PORT: Go writes each output through the project session FS from the
    // emit goroutines. Here the write callback runs on the checker threads
    // and the FS belongs to this thread (`Rc`), so the callback keeps the
    // outputs and this thread writes them after the emit, in the order they
    // came. A failed write adds the Go "Could not write file" diagnostic
    // (named after the output that a `.map` file belongs to, as Go does)
    // to the result of the file that emitted it, sorted with that file's
    // diagnostics as in Go (`program_emit::add_write_failures`), and the
    // file leaves `emittedFiles`.
    pub fn handle_emit(
        &self,
        _ctx: &Context,
        params: &EmitParams,
    ) -> Result<EmitResponse, GoError> {
        let (program, mut options) = self.get_emit_options(params)?;
        // Current for the whole handler (session_p1.rs header).
        let _program = ls_program::enter(&program);
        let writes: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
        let pending = Arc::clone(&writes);
        let write_file: WriteFile = Arc::new(
            move |file_name: &str, text: &str, _data: &mut WriteFileData| {
                pending
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push((file_name.to_string(), text.to_string()));
                Ok(())
            },
        );
        options.write_file = Some(write_file);
        // ts#64115: a snapshot with a full request file system keeps the
        // outputs (Go `outputFiles`) and writes nothing.
        let sd = self.get_snapshot_data(params.snapshot)?;
        let keep_outputs = requestfilesystem::has_full_file_system(sd.file_system.as_deref());
        // Go `emitProgram` (see `emit_program`), with the per-file results
        // apart until the write failures are in.
        let mut results = program_emit::emit_file_results(options);
        let writes = std::mem::take(&mut *writes.lock().unwrap_or_else(PoisonError::into_inner));
        let mut output_files: Option<FxHashMap<String, String>> = None;
        if keep_outputs {
            let outputs = output_files.get_or_insert_with(FxHashMap::default);
            for (file_name, text) in writes {
                outputs.insert(file_name, text);
            }
        } else {
            let fs = self.snapshot_host.fs();
            let mut failures = Vec::new();
            for (file_name, text) in writes {
                if let Err(err) = fs.write_file(&file_name, &text) {
                    let output_file = file_name.strip_suffix(".map").unwrap_or(&file_name);
                    let diagnostic = new_compiler_diagnostic(
                        diag::Could_not_write_file_0_Colon_1,
                        args![output_file, fs_error_text(&err)],
                    );
                    failures.push((file_name, diagnostic));
                }
            }
            program_emit::add_write_failures(&mut results, failures);
        }
        let result = program_emit::combine_emit_results(results);
        // Go clones `EmittedFiles` and makes a nil one `[]string{}`; an empty
        // `Vec` marshals as `[]`.
        let emitted_files = result.emitted_files;
        let mut emitted_files_contents: Vec<String> = Vec::new();
        if let Some(output_files) = &output_files {
            emitted_files_contents = emitted_files
                .iter()
                .map(|file_name| output_files.get(file_name).cloned().unwrap_or_default())
                .collect();
        }
        Ok(EmitResponse {
            emit_skipped: result.emit_skipped,
            diagnostics: non_nil_diagnostics(&result.diagnostics),
            emitted_files,
            emitted_files_contents,
        })
    }

    // Go: api/session.go:3611 handleEmitToString (tsgo#4699)
    pub fn handle_emit_to_string(
        &self,
        ctx: &Context,
        params: &EmitParams,
    ) -> Result<EmitOutputResponse, GoError> {
        let (program, options) = self.get_emit_options(params)?;
        // Current for the whole handler (session_p1.rs header).
        let _program = ls_program::enter(&program);
        emit_to_output(ctx, &program, options)
    }

    // Go: api/session.go:3619 handleSelectedFilesEmit (tsgo#4699)
    pub fn handle_selected_files_emit(
        &self,
        ctx: &Context,
        params: &SelectedFilesEmitParams,
        emit_only: EmitOnly,
    ) -> Result<EmitOutputResponse, GoError> {
        let program = self.get_emit_program(params.snapshot, &params.project)?;
        // Current for the whole handler (session_p1.rs header).
        let _program = ls_program::enter(&program);
        let Some(files) = &params.files else {
            return Err(errors::errorf(
                format!("{}: files is required", *ERR_CLIENT_ERROR),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        };
        let mut target_source_files = Vec::with_capacity(files.len());
        for file in files {
            let source_file = self.resolve_optional_source_file(&program, Some(file))?;
            target_source_files.push(source_file);
        }
        emit_to_output(
            ctx,
            &program,
            EmitOptions {
                target_source_files: Some(target_source_files),
                emit_only,
                force_emit: true,
                write_file: None,
            },
        )
    }
}

// Go: api/session.go:3642 emitToOutput (tsgo#4699)
// PORT: the write callback runs on the checker threads, so the outputs are
// in an `Arc<Mutex>` (Go `mu`). The caller keeps `program` current.
fn emit_to_output(
    ctx: &Context,
    program: &compiler::NewProgram,
    mut options: EmitOptions,
) -> Result<EmitOutputResponse, GoError> {
    let output_files: Arc<Mutex<Vec<EmitOutputFile>>> = Arc::default();
    let outputs = Arc::clone(&output_files);
    let write_file: WriteFile = Arc::new(
        move |file_name: &str, text: &str, data: &mut WriteFileData| {
            let mut source_file_name = None;
            if data.source_file.is_some() {
                source_file_name = Some(source_file_file_name(data.source_file).to_string());
            }
            outputs
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(EmitOutputFile {
                    file_name: file_name.to_string(),
                    text: text.to_string(),
                    source_file_name,
                });
            Ok(())
        },
    );
    options.write_file = Some(write_file);

    let result = emit_program(ctx, program, options)?;
    let mut output_files =
        std::mem::take(&mut *output_files.lock().unwrap_or_else(PoisonError::into_inner));
    // Go: api/session.go:2698 slices.SortFunc(outputFiles, strings.Compare on the names)
    // PORT: `String` compares in byte order, as Go `strings.Compare`.
    crate::gostd::slices::sort_func(&mut output_files, |a, b| {
        a.file_name.cmp(&b.file_name) as i32
    });
    Ok(EmitOutputResponse {
        emit_skipped: result.emit_skipped,
        diagnostics: non_nil_diagnostics(&result.diagnostics),
        output_files,
    })
}

impl Session {
    // Go: api/session.go:3671 getEmitOptions (tsgo#4699)
    pub fn get_emit_options(
        &self,
        params: &EmitParams,
    ) -> Result<(Rc<compiler::NewProgram>, EmitOptions), GoError> {
        let program = self.get_emit_program(params.snapshot, &params.project)?;
        let emit_only = get_emit_only(params.emit_only)?;
        Ok((
            program,
            EmitOptions {
                emit_only,
                ..EmitOptions::default()
            },
        ))
    }

    // Go: api/session.go:3685 getEmitProgram (tsgo#4699)
    pub fn get_emit_program(
        &self,
        snapshot: SnapshotID,
        project_id: &project::ID,
    ) -> Result<Rc<compiler::NewProgram>, GoError> {
        let sd = self.get_snapshot_data(snapshot)?;
        sd.get_program(project_id)
    }
}

// Go: api/session.go:3693 getEmitOnly (tsgo#4699)
// PORT: Go converts the number to `compiler.EmitOnly` (EmitAll 0,
// EmitOnlyJs 1, EmitOnlyDts 2); the port matches it to the enum.
fn get_emit_only(value: Option<u32>) -> Result<EmitOnly, GoError> {
    let Some(value) = value else {
        return Ok(EmitOnly::All);
    };
    match value {
        0 => Ok(EmitOnly::All),
        1 => Ok(EmitOnly::Js),
        2 => Ok(EmitOnly::Dts),
        _ => Err(errors::errorf(
            format!("{}: invalid emitOnly value: {}", *ERR_CLIENT_ERROR, value),
            vec![ERR_CLIENT_ERROR.clone()],
        )),
    }
}

// Go: api/session.go:3704 emitProgram (tsgo#4699)
// PORT: Go `program.Emit` of the project program. The port runs the compile
// emit (`program_emit::emit`) with the program current (`ls_program::enter`).
// The first emit of a program version makes its compile checker pool;
// `ls_program` frees it with the program.
// `program_emit::emit` takes no context and always returns a result, so
// Go's nil result branches (a canceled `ctx`) do not happen here.
fn emit_program(
    _ctx: &Context,
    program: &compiler::NewProgram,
    options: EmitOptions,
) -> Result<EmitResult, GoError> {
    let _program = ls_program::enter(program);
    Ok(program_emit::emit(options))
}

// Go: api/session.go:3715 nonNilDiagnostics (tsgo#4699)
// PORT: a Rust `Vec` has no nil. An empty list marshals as `[]`, the same as
// Go's non-nil empty slice.
fn non_nil_diagnostics(diags: &[Diagnostic]) -> Vec<DiagnosticResponse> {
    new_diagnostic_responses(diags)
}

impl Session {
    // Go: api/session.go:3791 handleGetWellKnownSymbols
    // handleGetWellKnownSymbols returns the handle ids of the per-checker singleton
    // symbols (unknown, undefined, arguments) so the client can identify them by id.
    pub fn handle_get_well_known_symbols(
        &self,
        ctx: &Context,
        params: &GetIntrinsicTypeParams,
    ) -> Result<Option<WellKnownSymbolsResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (unknown, undefined, arguments) = {
            let c = setup.checker.borrow();
            (
                c.get_unknown_symbol(),
                c.get_undefined_symbol(),
                c.get_arguments_symbol(),
            )
        };
        // ts#64518
        {
            let c = setup.checker.borrow();
            for symbol in [unknown, undefined, arguments] {
                go_assert!(c.sym(symbol).flags.intersects(SymbolFlags::TRANSIENT));
            }
        }
        let (unknown, _) = setup
            .sd
            .register_symbol(&setup.checker, unknown, &setup.project_id);
        let (undefined, _) = setup
            .sd
            .register_symbol(&setup.checker, undefined, &setup.project_id);
        let (arguments, _) = setup
            .sd
            .register_symbol(&setup.checker, arguments, &setup.project_id);
        Ok(Some(WellKnownSymbolsResponse {
            unknown,
            undefined,
            arguments,
        }))
    }

    // Go: api/session.go:3810 handleGetWellKnownSignatures
    // handleGetWellKnownSignatures returns the handle id of the per-checker unknown
    // signature so the client can identify it by id.
    pub fn handle_get_well_known_signatures(
        &self,
        ctx: &Context,
        params: &GetIntrinsicTypeParams,
    ) -> Result<Option<WellKnownSignaturesResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let unknown = setup.checker.borrow().get_unknown_signature();
        Ok(Some(WellKnownSignaturesResponse {
            unknown: setup
                .sd
                .register_signature(&setup.project_id, &setup.checker, unknown),
        }))
    }

    // Go: api/session.go:3725 handleFormatNodeForInsertion
    // handleFormatNodeForInsertion formats a synthesized node with the correct indentation
    // for insertion at a specific position in an existing file.
    pub fn handle_format_node_for_insertion(
        &self,
        ctx: &Context,
        params: &FormatNodeForInsertionParams,
    ) -> Result<String, GoError> {
        let sd = self.get_snapshot_data(params.snapshot)?;

        let program = &sd.get_program(&params.project)?;
        // The formatter reads the target file (file header, "Current program").
        let _program = ls_program::enter(program);

        let target_source_file = self.resolve_optional_source_file(program, Some(&params.file))?;

        let data = match base64_std_encoding_decode_string(&params.data) {
            Ok(data) => data,
            Err(err) => {
                return Err(errors::errorf(
                    format!("{}: invalid base64 data: {}", *ERR_CLIENT_ERROR, err),
                    vec![ERR_CLIENT_ERROR.clone(), err],
                ));
            }
        };

        let node = match encoder::decode_nodes(&data) {
            Ok(node) => node,
            Err(err) => {
                return Err(errors::errorf(
                    format!("{}: failed to decode AST: {}", *ERR_CLIENT_ERROR, err),
                    vec![ERR_CLIENT_ERROR.clone(), err],
                ));
            }
        };

        let pos =
            source_file_get_position_map(target_source_file).utf16_to_utf8(params.position as i32);
        let format_options = sd.snapshot.user_preferences().format_code_settings;
        let new_line = format_options.editor_settings.new_line_character.clone();

        let factory = NodeFactory::new();
        let (text, node_with_pos) = print_and_position_node(
            &factory,
            node,
            Node::NIL,
            &new_line,
            format_options.editor_settings.indent_size,
            None,
        );
        // PORT: Go passes `targetSourceFile.ParseOptions()`; the port keeps
        // its file name and path (see `create_synthetic_source_file`).
        let synthetic_file = create_synthetic_source_file(
            &factory,
            node_with_pos,
            &text,
            source_file_file_name(target_source_file),
            &source_file_info(target_source_file).path,
        );

        let is_at_line_start =
            crate::format::get_line_start_position_for_position(pos, target_source_file) == pos;
        let initial_indentation = crate::format::get_indentation(
            pos,
            target_source_file,
            &format_options,
            is_at_line_start,
        );

        let mut delta = 0;
        if format_options.editor_settings.indent_size != 0
            && crate::format::should_indent_child_node(
                &format_options,
                node,
                Node::NIL,
                Node::NIL,
                &[],
            )
        {
            delta = format_options.editor_settings.indent_size;
        }

        let ctx = crate::format::with_format_code_settings(ctx, &format_options, &new_line);
        let changes = crate::format::format_node_given_indentation(
            &ctx,
            node_with_pos,
            synthetic_file,
            source_file_language_variant(target_source_file),
            initial_indentation,
            delta,
        );

        Ok(crate::frontend::core_textchange::apply_bulk_edits(
            &text, &changes,
        ))
    }

    // Go: api/session.go:3774 handleGetIntrinsicType
    // handleGetIntrinsicType returns an intrinsic type (any, string, number, etc.).
    pub fn handle_get_intrinsic_type(
        &self,
        ctx: &Context,
        params: &GetIntrinsicTypeParams,
        getter: fn(&Checker) -> TypeId,
    ) -> Result<Option<TypeResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let t = getter(&setup.checker.borrow());
        if t.is_nil() {
            return Ok(None);
        }

        Ok(setup
            .sd
            .new_type_response(&setup.project_id, &setup.checker, t))
    }

    // Go: api/session.go:3823 handleIsContextSensitive
    // handleIsContextSensitive returns whether a node is context-sensitive.
    pub fn handle_is_context_sensitive(
        &self,
        ctx: &Context,
        params: &GetContextualTypeParams,
    ) -> Result<bool, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let node = setup
            .sd
            .resolve_node_handle(&setup.program, &params.location)?;
        if node.is_nil() {
            return Ok(false);
        }

        let result = setup
            .checker
            .borrow_mut()
            .is_context_sensitive_exported(node);
        Ok(result)
    }

    // Go: api/session.go:3842 handleGetReturnTypeOfSignature
    // handleGetReturnTypeOfSignature returns the return type of a signature.
    pub fn handle_get_return_type_of_signature(
        &self,
        ctx: &Context,
        params: &GetSignaturePropertyParams,
    ) -> Result<Option<TypeResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, sig) = setup.resolve_signature_handle(params.signature)?;
        let sig = checker_signature(&setup.checker, &owner, sig);

        let t = setup
            .checker
            .borrow_mut()
            .get_return_type_of_signature_exported(sig);
        Ok(setup
            .sd
            .new_type_response(&setup.project_id, &setup.checker, t))
    }

    // Go: api/session.go:3858 handleGetRestTypeOfSignature
    // handleGetRestTypeOfSignature returns the rest type of a signature.
    pub fn handle_get_rest_type_of_signature(
        &self,
        ctx: &Context,
        params: &CheckerSignatureParams,
    ) -> Result<Option<TypeResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, sig) = setup.resolve_signature_handle(params.signature)?;
        let sig = checker_signature(&setup.checker, &owner, sig);

        let t = setup
            .checker
            .borrow_mut()
            .get_rest_type_of_signature_exported(sig);
        Ok(setup
            .sd
            .new_type_response(&setup.project_id, &setup.checker, t))
    }

    // Go: api/session.go:3875 handleGetTypePredicateOfSignature
    // handleGetTypePredicateOfSignature returns the type predicate of a signature.
    pub fn handle_get_type_predicate_of_signature(
        &self,
        ctx: &Context,
        params: &CheckerSignatureParams,
    ) -> Result<Option<TypePredicateResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, sig) = setup.resolve_signature_handle(params.signature)?;
        let sig = checker_signature(&setup.checker, &owner, sig);

        let pred = setup
            .checker
            .borrow_mut()
            .get_type_predicate_of_signature_exported(sig);
        if pred.is_nil() {
            return Ok(None);
        }

        let (kind, parameter_index, parameter_name, pred_type) = {
            let c = setup.checker.borrow();
            let p = c.pred(pred);
            (
                p.kind().0,
                p.parameter_index(),
                p.parameter_name().to_string(),
                p.type_(),
            )
        };
        let mut resp = TypePredicateResponse {
            kind,
            parameter_index,
            parameter_name,
            ..Default::default()
        };
        if pred_type.is_some() {
            resp.type_ = setup
                .sd
                .new_type_response(&setup.project_id, &setup.checker, pred_type);
        }

        Ok(Some(resp))
    }

    // Go: api/session.go:3905 handleIsArrayType
    // handleIsArrayType returns whether a type is Array<T> or ReadonlyArray<T>.
    pub fn handle_is_array_type(
        &self,
        ctx: &Context,
        params: &CheckerTypeParams,
    ) -> Result<bool, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, t) = setup.resolve_type_handle(params.type_)?;
        let t = checker_type(&setup.checker, &owner, t);

        let result = setup.checker.borrow().is_array_type_exported(t);
        Ok(result)
    }

    // Go: api/session.go resolveIndexInfoRequest (ts#64264)
    // PORT: Go calls `setup.done()` on each error path; the setup's guard
    // releases the checker when it drops there.
    pub fn resolve_index_info_request(
        &self,
        ctx: &Context,
        params: &GetIndexInfoOfTypeParams,
    ) -> Result<(CheckerSetup, TypeId, TypeId), GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, t) = setup.resolve_type_handle(params.type_)?;
        let t = checker_type(&setup.checker, &owner, t);

        let key_type = match IndexKind(params.kind) {
            IndexKind::STRING => setup.checker.borrow().get_string_type(),
            IndexKind::NUMBER => setup.checker.borrow().get_number_type(),
            _ => {
                return Err(errors::errorf(
                    format!("{}: invalid index kind {}", *ERR_CLIENT_ERROR, params.kind),
                    vec![ERR_CLIENT_ERROR.clone()],
                ));
            }
        };
        Ok((setup, t, key_type))
    }

    // Go: api/session.go handleGetIndexInfoOfType (ts#64264)
    pub fn handle_get_index_info_of_type(
        &self,
        ctx: &Context,
        params: &GetIndexInfoOfTypeParams,
    ) -> Result<Option<IndexInfoResponse>, GoError> {
        let (setup, t, key_type) = self.resolve_index_info_request(ctx, params)?;
        let info = setup
            .checker
            .borrow_mut()
            .get_index_info_of_type_exported(t, key_type);
        Ok(setup.new_index_info_response(info))
    }

    // Go: api/session.go handleGetExportSymbolOfSymbolForChecker (ts#64264)
    pub fn handle_get_export_symbol_of_symbol_for_checker(
        &self,
        ctx: &Context,
        params: &CheckerSymbolParams,
    ) -> Result<Option<SymbolResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, symbol) = setup.resolve_symbol_handle(&params.symbol)?;
        let symbol = checker_symbol(&setup.checker, &owner, symbol);
        let export_symbol = setup.checker.borrow().get_export_symbol_of_symbol(symbol);
        Ok(setup.new_symbol_response(export_symbol))
    }

    // Go: api/session.go handleIsReadonlySymbol (ts#63943)
    // handleIsReadonlySymbol returns whether a symbol is a readonly symbol.
    pub fn handle_is_readonly_symbol(
        &self,
        ctx: &Context,
        params: &CheckerSymbolParams,
    ) -> Result<bool, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, symbol) = setup.resolve_symbol_handle(&params.symbol)?;
        let symbol = checker_symbol(&setup.checker, &owner, symbol);

        let result = setup
            .checker
            .borrow_mut()
            .is_readonly_symbol_exported(symbol);
        Ok(result)
    }

    // Go: api/session.go:3938 handleGetBaseTypes
    // handleGetBaseTypes returns the base types of an interface/class type.
    pub fn handle_get_base_types(
        &self,
        ctx: &Context,
        params: &CheckerTypeParams,
    ) -> Result<Vec<Option<TypeResponse>>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, t) = setup.resolve_type_handle(params.type_)?;
        let t = checker_type(&setup.checker, &owner, t);

        let base_types = setup.checker.borrow_mut().get_base_types_exported(t);
        if base_types.is_empty() {
            return Ok(Vec::new());
        }

        let mut results = Vec::with_capacity(base_types.len());
        for bt in base_types {
            results.push(
                setup
                    .sd
                    .new_type_response(&setup.project_id, &setup.checker, bt),
            );
        }

        Ok(results)
    }

    // Go: api/session.go:3965 handleGetPropertiesOfType
    // handleGetPropertiesOfType returns the properties of a type.
    pub fn handle_get_properties_of_type(
        &self,
        ctx: &Context,
        params: &CheckerTypeParams,
    ) -> Result<Vec<Option<SymbolResponse>>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, t) = setup.resolve_type_handle(params.type_)?;
        let t = checker_type(&setup.checker, &owner, t);

        let props = setup
            .checker
            .borrow_mut()
            .get_properties_of_type_exported(t);
        if props.is_empty() {
            return Ok(Vec::new());
        }

        let mut results = Vec::with_capacity(props.len());
        for prop in props {
            results.push(setup.new_symbol_response(prop));
        }

        Ok(results)
    }

    // Go: api/session.go:3992 handleGetApparentPropertiesOfType
    // handleGetApparentPropertiesOfType returns the apparent properties of a type,
    // including CallableFunction or NewableFunction members where applicable.
    pub fn handle_get_apparent_properties_of_type(
        &self,
        ctx: &Context,
        params: &GetTypePropertyParams,
    ) -> Result<Vec<Option<SymbolResponse>>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, t) = setup.resolve_type_handle(params.type_)?;
        let t = checker_type(&setup.checker, &owner, t);

        let props = setup.checker.borrow_mut().get_apparent_properties(t);
        let mut results = Vec::with_capacity(props.len());
        for prop in props {
            results.push(setup.new_symbol_response(prop));
        }
        Ok(results)
    }

    // Go: api/session.go:4013 handleGetApparentType
    // handleGetApparentType returns the apparent type of a type.
    pub fn handle_get_apparent_type(
        &self,
        ctx: &Context,
        params: &GetTypePropertyParams,
    ) -> Result<Option<TypeResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, t) = setup.resolve_type_handle(params.type_)?;
        let t = checker_type(&setup.checker, &owner, t);

        let apparent = setup.checker.borrow_mut().get_apparent_type_exported(t);
        Ok(setup
            .sd
            .new_type_response(&setup.project_id, &setup.checker, apparent))
    }

    // Go: api/session.go:4029 handleGetReducedType (ts#63899)
    // handleGetReducedType returns the reduced type of a type.
    pub fn handle_get_reduced_type(
        &self,
        ctx: &Context,
        params: &GetTypePropertyParams,
    ) -> Result<Option<TypeResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, t) = setup.resolve_type_handle(params.type_)?;
        let t = checker_type(&setup.checker, &owner, t);

        let reduced = setup.checker.borrow_mut().get_reduced_type_exported(t);
        Ok(setup
            .sd
            .new_type_response(&setup.project_id, &setup.checker, reduced))
    }

    // Go: api/session.go:4046 handleGetIndexInfosOfType
    // handleGetIndexInfosOfType returns the index infos of a type.
    pub fn handle_get_index_infos_of_type(
        &self,
        ctx: &Context,
        params: &CheckerTypeParams,
    ) -> Result<Vec<IndexInfoResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, t) = setup.resolve_type_handle(params.type_)?;
        let t = checker_type(&setup.checker, &owner, t);

        let infos = setup
            .checker
            .borrow_mut()
            .get_index_infos_of_type_exported(t);
        if infos.is_empty() {
            return Ok(Vec::new());
        }

        let mut results = Vec::with_capacity(infos.len());
        for info in infos {
            // ts#64264: checkerSetup.newIndexInfoResponse
            results.push(
                setup
                    .new_index_info_response(info)
                    .expect("an index info of a type is non-nil"),
            );
        }

        Ok(results)
    }

    // Go: api/session.go:4108 handleGetConstraintOfTypeParameter
    // handleGetConstraintOfTypeParameter returns the constraint of a type parameter.
    pub fn handle_get_constraint_of_type_parameter(
        &self,
        ctx: &Context,
        params: &GetTypePropertyParams,
    ) -> Result<Option<TypeResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, t) = setup.resolve_type_handle(params.type_)?;
        let t = checker_type(&setup.checker, &owner, t);

        let constraint = setup
            .checker
            .borrow_mut()
            .get_constraint_of_type_parameter_exported(t);
        if constraint.is_nil() {
            return Ok(None);
        }

        Ok(setup
            .sd
            .new_type_response(&setup.project_id, &setup.checker, constraint))
    }

    // Go: api/session.go:4130 handleGetDefaultFromTypeParameter
    // handleGetDefaultFromTypeParameter returns the default type of a type parameter.
    pub fn handle_get_default_from_type_parameter(
        &self,
        ctx: &Context,
        params: &GetTypePropertyParams,
    ) -> Result<Option<TypeResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, t) = setup.resolve_type_handle(params.type_)?;
        let t = checker_type(&setup.checker, &owner, t);

        let result = setup
            .checker
            .borrow_mut()
            .get_default_from_type_parameter_exported(t);
        Ok(setup
            .sd
            .new_type_response(&setup.project_id, &setup.checker, result))
    }

    // Go: api/session.go:4147 handleGetBaseConstraintOfType
    // handleGetBaseConstraintOfType returns the base constraint of an instantiable type.
    pub fn handle_get_base_constraint_of_type(
        &self,
        ctx: &Context,
        params: &CheckerTypeParams,
    ) -> Result<Option<TypeResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, t) = setup.resolve_type_handle(params.type_)?;
        let t = checker_type(&setup.checker, &owner, t);

        let constraint = setup
            .checker
            .borrow_mut()
            .get_base_constraint_of_type_exported(t);
        if constraint.is_nil() {
            return Ok(None);
        }

        Ok(setup
            .sd
            .new_type_response(&setup.project_id, &setup.checker, constraint))
    }

    // Go: api/session.go:4169 handleGetPropertyOfType
    // handleGetPropertyOfType returns a named property symbol of a type.
    pub fn handle_get_property_of_type(
        &self,
        ctx: &Context,
        params: &GetPropertyOfTypeParams,
    ) -> Result<Option<SymbolResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, t) = setup.resolve_type_handle(params.type_)?;
        let t = checker_type(&setup.checker, &owner, t);

        let prop = setup
            .checker
            .borrow_mut()
            .get_property_of_type_exported(t, &params.name);
        if prop.is_nil() {
            return Ok(None);
        }

        Ok(setup.new_symbol_response(prop))
    }

    // Go: api/session.go handleGetTypeOfPropertyOfType (ts#64264)
    pub fn handle_get_type_of_property_of_type(
        &self,
        ctx: &Context,
        params: &GetPropertyOfTypeParams,
    ) -> Result<Option<TypeResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, t) = setup.resolve_type_handle(params.type_)?;
        let t = checker_type(&setup.checker, &owner, t);

        let prop_type = setup
            .checker
            .borrow_mut()
            .get_type_of_property_of_type_exported(t, &params.name);
        Ok(setup
            .sd
            .new_type_response(&setup.project_id, &setup.checker, prop_type))
    }

    // Go: api/session.go:4207 handleGetConstantValue
    // handleGetConstantValue returns the constant value of an enum member or const enum access.
    pub fn handle_get_constant_value(
        &self,
        ctx: &Context,
        params: &CheckerNodeParams,
    ) -> Result<Option<ConstantValueResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let node = setup
            .sd
            .resolve_node_handle(&setup.program, &params.location)?;
        if node.is_nil() {
            return Ok(None);
        }

        // ts#64241
        let value = setup.checker.borrow_mut().get_constant_value(node);
        let result = ConstantValueResponse {
            is_number: matches!(value, Some(LiteralValue::Number(_))),
            value: literal_value_to_json(value.as_ref()),
        };
        Ok(Some(result))
    }

    // Go: api/session.go:4230 handleGetSignatureFromDeclaration
    // handleGetSignatureFromDeclaration returns the signature of a function-like declaration.
    pub fn handle_get_signature_from_declaration(
        &self,
        ctx: &Context,
        params: &CheckerNodeParams,
    ) -> Result<Option<SignatureResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let node = setup
            .sd
            .resolve_node_handle(&setup.program, &params.location)?;

        let sig = setup
            .checker
            .borrow_mut()
            .get_signature_from_declaration_exported(node);
        Ok(setup.new_signature_response(sig))
    }

    // Go: api/session.go:4247 handleGetExportSpecifierLocalTargetSymbol
    // handleGetExportSpecifierLocalTargetSymbol returns the local target symbol of an export specifier.
    pub fn handle_get_export_specifier_local_target_symbol(
        &self,
        ctx: &Context,
        params: &CheckerNodeParams,
    ) -> Result<Option<SymbolResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let node = setup
            .sd
            .resolve_node_handle(&setup.program, &params.location)?;
        if node.is_nil() {
            return Ok(None);
        }

        let symbol = setup
            .checker
            .borrow_mut()
            .get_export_specifier_local_target_symbol(node);
        if symbol.is_nil() {
            return Ok(None);
        }

        Ok(setup.new_symbol_response(symbol))
    }

    // Go: api/session.go:4692 handleGetMergedSymbol (ts#64598)
    pub fn handle_get_merged_symbol(
        &self,
        ctx: &Context,
        params: &CheckerSymbolParams,
    ) -> Result<Option<SymbolResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, symbol) = setup.resolve_symbol_handle(&params.symbol)?;
        let symbol = checker_symbol(&setup.checker, &owner, symbol);

        let merged = setup.checker.borrow().get_merged_symbol(symbol);
        Ok(setup.new_symbol_response(merged))
    }

    // Go: api/session.go:4708 handleGetSymbolOfNode (ts#64598)
    // @gen-proto-nullable
    pub fn handle_get_symbol_of_node(
        &self,
        ctx: &Context,
        params: &CheckerNodeParams,
    ) -> Result<Option<SymbolResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let node = setup
            .sd
            .resolve_node_handle(&setup.program, &params.location)?;

        let symbol = setup.checker.borrow_mut().get_symbol_of_node(node);
        Ok(setup.new_symbol_response(symbol))
    }

    // Go: api/session.go:4724 handleGetSymbolOfDeclarationForChecker (ts#64598)
    // @gen-proto-nullable
    pub fn handle_get_symbol_of_declaration_for_checker(
        &self,
        ctx: &Context,
        params: &CheckerNodeParams,
    ) -> Result<Option<SymbolResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let node = setup
            .sd
            .resolve_node_handle(&setup.program, &params.location)?;
        let symbol = setup.checker.borrow_mut().get_symbol_of_declaration(node);
        Ok(setup.new_symbol_response(symbol))
    }

    // Go: api/session.go:4739 handleGetParentOfSymbolForChecker (ts#64598)
    // @gen-proto-nullable
    pub fn handle_get_parent_of_symbol_for_checker(
        &self,
        ctx: &Context,
        params: &CheckerSymbolParams,
    ) -> Result<Option<SymbolResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, symbol) = setup.resolve_symbol_handle(&params.symbol)?;
        let symbol = checker_symbol(&setup.checker, &owner, symbol);

        let parent = setup.checker.borrow_mut().get_parent_of_symbol(symbol);
        Ok(setup.new_symbol_response(parent))
    }

    // Go: api/session.go:4271 handleGetAliasedSymbol
    // handleGetAliasedSymbol resolves an alias symbol to its target.
    pub fn handle_get_aliased_symbol(
        &self,
        ctx: &Context,
        params: &CheckerSymbolParams,
    ) -> Result<Option<SymbolResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, symbol) = setup.resolve_symbol_handle(&params.symbol)?;
        let symbol = checker_symbol(&setup.checker, &owner, symbol);

        let aliased = setup.checker.borrow_mut().get_aliased_symbol(symbol);
        Ok(setup.new_symbol_response(aliased))
    }

    // Go: api/session.go:4288 handleGetFullyQualifiedName
    // handleGetFullyQualifiedName returns the fully qualified name of a symbol
    // (e.g. `"/path/to/module".Namespace.Name`).
    pub fn handle_get_fully_qualified_name(
        &self,
        ctx: &Context,
        params: &CheckerSymbolParams,
    ) -> Result<String, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, symbol) = setup.resolve_symbol_handle(&params.symbol)?;
        if symbol.is_nil() {
            return Ok(String::new());
        }
        let symbol = checker_symbol(&setup.checker, &owner, symbol);

        let result = setup
            .checker
            .borrow_mut()
            .get_fully_qualified_name_exported(symbol);
        Ok(result)
    }

    // Go: api/session.go:4365 handleGetExportsOfModule
    // handleGetExportsOfModule returns the resolved exports of a module symbol,
    // including those introduced by `export *` and re-exports.
    pub fn handle_get_exports_of_module(
        &self,
        ctx: &Context,
        params: &CheckerSymbolParams,
    ) -> Result<Vec<Option<SymbolResponse>>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, symbol) = setup.resolve_symbol_handle(&params.symbol)?;
        if symbol.is_nil() {
            return Ok(Vec::new());
        }
        let symbol = checker_symbol(&setup.checker, &owner, symbol);

        let mut exports = setup
            .checker
            .borrow_mut()
            .get_exports_of_module_exported(symbol);
        if exports.is_empty() {
            return Ok(Vec::new());
        }
        // Go: api/session.go:3326 slices.SortFunc(exports, setup.checker.CompareSymbols)
        {
            let mut c = setup.checker.borrow_mut();
            crate::gostd::slices::sort_func(&mut exports, |&a, &b| {
                c.compare_symbols_exported(a, b)
            });
        }

        let mut results = Vec::with_capacity(exports.len());
        for exp in exports {
            results.push(setup.new_symbol_response(exp));
        }

        Ok(results)
    }

    // Go: api/session.go:4421 handleGetJSDocTags
    // handleGetJSDocTags returns the JSDoc tags of a symbol as structured name/text pairs.
    pub fn handle_get_js_doc_tags(
        &self,
        ctx: &Context,
        params: &CheckerSymbolParams,
    ) -> Result<Vec<JSDocTagInfo>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, symbol) = setup.resolve_symbol_handle(&params.symbol)?;
        if symbol.is_nil() {
            return Ok(Vec::new());
        }

        // PORT: Go reads the symbol with no checker; the port reads it in
        // the arena of the checker that owns the handle.
        let tags = ls::get_symbol_js_doc_tags(&owner.borrow(), symbol);
        if tags.is_empty() {
            return Ok(Vec::new());
        }
        let mut results = Vec::with_capacity(tags.len());
        for tag in tags {
            results.push(JSDocTagInfo {
                name: tag.name,
                text: tag.text,
            });
        }
        Ok(results)
    }

    // Go: api/session.go:4448 handleGetDocumentationComment
    // handleGetDocumentationComment returns the rendered documentation comment of a symbol as plain text.
    pub fn handle_get_documentation_comment(
        &self,
        ctx: &Context,
        params: &CheckerSymbolParams,
    ) -> Result<String, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, symbol) = setup.resolve_symbol_handle(&params.symbol)?;
        if symbol.is_nil() {
            return Ok(String::new());
        }
        let symbol = checker_symbol(&setup.checker, &owner, symbol);

        let result = ls::get_symbol_documentation_comment(&mut setup.checker.borrow_mut(), symbol);
        Ok(result)
    }

    // Go: api/session.go:4468 handleGetTypeArguments
    // handleGetTypeArguments returns the type arguments of a type reference.
    pub fn handle_get_type_arguments(
        &self,
        ctx: &Context,
        params: &CheckerTypeParams,
    ) -> Result<Vec<Option<TypeResponse>>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, t) = setup.resolve_type_handle(params.type_)?;
        let t = checker_type(&setup.checker, &owner, t);

        let type_args = setup.checker.borrow_mut().get_type_arguments_exported(t);
        if type_args.is_empty() {
            return Ok(Vec::new());
        }

        let mut results = Vec::with_capacity(type_args.len());
        for ta in type_args {
            results.push(
                setup
                    .sd
                    .new_type_response(&setup.project_id, &setup.checker, ta),
            );
        }

        Ok(results)
    }

    // Go: api/session.go:4308 handleGetImmediateAliasedSymbol
    // handleGetImmediateAliasedSymbol resolves one level of alias indirection.
    pub fn handle_get_immediate_aliased_symbol(
        &self,
        ctx: &Context,
        params: &CheckerSymbolParams,
    ) -> Result<Option<SymbolResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, symbol) = setup.resolve_symbol_handle(&params.symbol)?;
        if symbol.is_nil() {
            return Ok(None);
        }
        let symbol = checker_symbol(&setup.checker, &owner, symbol);

        // PORT: Go's getImmediateAliasedSymbol asserts that the symbol is an
        // alias (checker.go:2197). The checker's assert is a debug_assert!,
        // so that no Go assert panic is new on a CLI path; this request can
        // name any symbol, so it asserts here.
        go_assert!(
            setup
                .checker
                .borrow()
                .sym(symbol)
                .flags
                .intersects(SymbolFlags::ALIAS),
            "Should only get Alias here."
        );
        let aliased = setup
            .checker
            .borrow_mut()
            .get_immediate_aliased_symbol_exported(symbol);
        if aliased.is_nil() {
            return Ok(None);
        }

        Ok(setup.new_symbol_response(aliased))
    }

    // Go: api/session.go handleMethodGetTargetSymbol (ts#63945)
    // handleGetTargetSymbol returns the target symbol if the symbol is instantiated,
    // otherwise returns the provided symbol.
    pub fn handle_method_get_target_symbol(
        &self,
        ctx: &Context,
        params: &CheckerSymbolParams,
    ) -> Result<Option<SymbolResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, symbol) = setup.resolve_symbol_handle(&params.symbol)?;
        let symbol = checker_symbol(&setup.checker, &owner, symbol);

        let target = setup
            .checker
            .borrow_mut()
            .get_target_symbol_exported(symbol);
        Ok(setup.new_symbol_response(target))
    }

    // Go: api/session.go:4396 handleGetMemberInModuleExports
    // handleGetMemberInModuleExports returns an export by name from a module symbol.
    pub fn handle_get_member_in_module_exports(
        &self,
        ctx: &Context,
        params: &GetMemberInModuleExportsParams,
    ) -> Result<Option<SymbolResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, symbol) = setup.resolve_symbol_handle(&params.symbol)?;
        if symbol.is_nil() {
            return Ok(None);
        }
        let symbol = checker_symbol(&setup.checker, &owner, symbol);

        let member = setup
            .checker
            .borrow_mut()
            .try_get_member_in_module_exports(&params.name, symbol);
        if member.is_nil() {
            return Ok(None);
        }

        Ok(setup.new_symbol_response(member))
    }
}

impl Session {
    // Go: api/session.go:4493 handleGetTrueTypeOfConditionalType
    pub fn handle_get_true_type_of_conditional_type(
        &self,
        ctx: &Context,
        params: &GetTypePropertyParams,
    ) -> Result<Option<TypeResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, t) = setup
            .sd
            .resolve_type_handle(&params.project, params.type_)?;
        let t = checker_type(&setup.checker, &owner, t);

        let result = setup
            .checker
            .borrow_mut()
            .get_true_type_of_conditional_type(t);
        Ok(setup
            .sd
            .new_type_response(&params.project, &setup.checker, result))
    }

    // Go: api/session.go:4508 handleGetFalseTypeOfConditionalType
    pub fn handle_get_false_type_of_conditional_type(
        &self,
        ctx: &Context,
        params: &GetTypePropertyParams,
    ) -> Result<Option<TypeResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, t) = setup
            .sd
            .resolve_type_handle(&params.project, params.type_)?;
        let t = checker_type(&setup.checker, &owner, t);

        let result = setup
            .checker
            .borrow_mut()
            .get_false_type_of_conditional_type(t);
        Ok(setup
            .sd
            .new_type_response(&params.project, &setup.checker, result))
    }
}

impl SnapshotData {
    // Go: api/session.go:4523 resolveNodeHandle
    pub fn resolve_node_handle(
        &self,
        program: &compiler::NewProgram,
        handle: &NodeHandle,
    ) -> Result<Node, GoError> {
        let s = handle.0.as_str();
        // Format: "index.kind.path" — we need index and path, kind is informational only.
        let Some(first_dot) = s.bytes().position(|b| b == b'.') else {
            return Err(errors::errorf(
                format!(
                    "{}: invalid node handle {}",
                    *ERR_CLIENT_ERROR,
                    gostd::strconv::quote(s)
                ),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        };
        let Some(second_dot) = s[first_dot + 1..].bytes().position(|b| b == b'.') else {
            return Err(errors::errorf(
                format!(
                    "{}: invalid node handle {}",
                    *ERR_CLIENT_ERROR,
                    gostd::strconv::quote(s)
                ),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        };
        let second_dot = second_dot + first_dot + 1; // adjust to absolute index

        let idx = match strconv_parse_uint(&s[..first_dot], 10, 32) {
            Ok(idx) => idx,
            Err(err) => {
                return Err(errors::errorf(
                    format!(
                        "{}: invalid node handle {}: {}",
                        *ERR_CLIENT_ERROR,
                        gostd::strconv::quote(s),
                        err
                    ),
                    vec![ERR_CLIENT_ERROR.clone(), err],
                ));
            }
        };
        // ts#64159
        if !try_path_key_from_canonical(&s[second_dot + 1..]) {
            return Err(errors::errorf(
                format!(
                    "{}: invalid node handle {}",
                    *ERR_CLIENT_ERROR,
                    gostd::strconv::quote(s)
                ),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        }
        let path = tspath::Path(s[second_dot + 1..].to_string());

        let source_file = program
            .get_source_file_by_path(&path)
            .map_or(Node::NIL, |f| f.root);
        if source_file.is_nil() {
            return Err(errors::errorf(
                format!(
                    "{}: node handle {} could not be resolved (file may not be loaded or handle may be stale)",
                    *ERR_CLIENT_ERROR,
                    gostd::strconv::quote(s)
                ),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        }
        let table = encoder::get_node_index_table(source_file);

        // PORT: Go also checks `table != nil`; the Rust table is never nil.
        if idx < table.nodes.len() as u64 {
            let node = table.nodes[idx as usize];
            if node.is_some() {
                return Ok(node);
            }
        }
        Err(errors::errorf(
            format!(
                "{}: node handle {} could not be resolved (file may not be loaded or handle may be stale)",
                *ERR_CLIENT_ERROR,
                gostd::strconv::quote(s)
            ),
            vec![ERR_CLIENT_ERROR.clone()],
        ))
    }
}

// Go: api/session.go:4560 computeSnapshotChanges
// computeSnapshotChanges computes the per-project source file differences between
// two snapshots. It uses DiffOrderedMaps on projects to find changed/removed projects,
// then DiffMaps on FilesByPath for each changed project to collect file-level changes.
pub fn compute_snapshot_changes(
    prev: &project::Snapshot,
    next: &project::Snapshot,
) -> SnapshotChanges {
    // ts#64319: keyed by project ID.
    let prev_projects = prev.project_collection.projects_by_id();
    let next_projects = next.project_collection.projects_by_id();

    let mut changes = SnapshotChanges::default();

    diff_ordered_maps(
        &prev_projects,
        &next_projects,
        // onAdded: new project — nothing to retain from previous snapshot.
        |_, _| {},
        // onRemoved: project removed entirely.
        |_, old_proj| {
            changes.removed_projects.push(old_proj.borrow().id());
        },
        // onModified: project changed, diff its files.
        |_, old_proj, new_proj| {
            let old_program = old_proj.borrow().get_program();
            let new_program = new_proj.borrow().get_program();
            let same_program = match (&old_program, &new_program) {
                (Some(a), Some(b)) => Rc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            };
            if same_program {
                return;
            }
            // PORT: a nil Go map is an empty map here.
            let empty: FxHashMap<tspath::Path, Rc<ParsedSourceFile>> = FxHashMap::default();
            let old_files = match &old_program {
                Some(p) => p.files_by_path(),
                None => &empty,
            };
            let new_files = match &new_program {
                Some(p) => p.files_by_path(),
                None => &empty,
            };
            let mut project_changes = ProjectFileChanges::default();
            {
                // PORT: Go map order is random; the port uses FxHashMap order.
                let mut on_removed = |path: &tspath::Path, _: &Rc<ParsedSourceFile>| {
                    project_changes.deleted_files.push(path.clone());
                };
                let mut on_changed =
                    |path: &tspath::Path, _: &Rc<ParsedSourceFile>, _: &Rc<ParsedSourceFile>| {
                        project_changes.changed_files.push(path.clone());
                    };
                diff_maps::<tspath::Path, Rc<ParsedSourceFile>>(
                    old_files,
                    new_files,
                    None, // onAdded: new file in project, not a change.
                    Some(&mut on_removed),
                    Some(&mut on_changed),
                );
            }
            if !project_changes.changed_files.is_empty()
                || !project_changes.deleted_files.is_empty()
            {
                // PORT: Go makes the nil map here; an empty map is the same
                // value to the `omitempty` field.
                changes
                    .changed_projects
                    .insert(new_proj.borrow().id(), project_changes);
            }
        },
    );

    changes
}

impl Session {
    // Go: api/session.go createSnapshotResponse (ts#64204)
    pub fn create_snapshot_response(
        &self,
        snapshot: &Rc<project::Snapshot>,
        base: Option<&Rc<project::Snapshot>>,
        request: Option<&SnapshotRequestChangesParams>,
    ) -> CreateSnapshotResponse {
        let operation = self.create_snapshot_operation_response(snapshot, request);
        let Some(base) = base else {
            let projects = snapshot.project_collection.projects();
            let mut project_responses = Vec::with_capacity(projects.len());
            for proj in &projects {
                if proj.borrow().command_line.is_some() {
                    project_responses.push(new_project_response(&proj.borrow()));
                }
            }
            return CreateSnapshotResponse {
                snapshot: snapshot_handle(snapshot),
                projects: project_responses,
                changes: None,
                operation: Some(operation),
            };
        };

        // PORT: two callbacks append, so the list is in a `RefCell`.
        let project_responses = RefCell::new(Vec::new());
        diff_ordered_maps(
            &base.project_collection.projects_by_id(),
            &snapshot.project_collection.projects_by_id(),
            |_, proj| {
                if proj.borrow().command_line.is_some() {
                    project_responses
                        .borrow_mut()
                        .push(new_project_response(&proj.borrow()));
                }
            },
            |_, _| {},
            |_, old_proj, new_proj| {
                if !Rc::ptr_eq(old_proj, new_proj) && new_proj.borrow().command_line.is_some() {
                    project_responses
                        .borrow_mut()
                        .push(new_project_response(&new_proj.borrow()));
                }
            },
        );
        CreateSnapshotResponse {
            snapshot: snapshot_handle(snapshot),
            projects: project_responses.into_inner(),
            changes: Some(compute_snapshot_changes(base, snapshot)),
            operation: Some(operation),
        }
    }

    // Go: api/session.go createSnapshotOperationResponse (ts#64204, ts#64374)
    pub fn create_snapshot_operation_response(
        &self,
        snapshot: &project::Snapshot,
        request: Option<&SnapshotRequestChangesParams>,
    ) -> SnapshotOperationResponse {
        let mut operation = SnapshotOperationResponse::default();
        let Some(request) = request else {
            return operation;
        };

        if let Some(request_create_programs) = &request.create_programs {
            let created_programs = snapshot.created_programs();
            if created_programs.len() != request_create_programs.len() {
                panic!("created program result count does not match request");
            }
            let mut results = Vec::with_capacity(created_programs.len());
            for created_program in &created_programs {
                let (program_id, ok) = created_program.borrow().id().synthetic();
                if !ok {
                    panic!("created program has non-synthetic project ID");
                }
                results.push(program_id);
            }
            operation.created_programs = Some(results);
        }

        if let Some(open_files) = &request.open_files {
            let mut results = Vec::with_capacity(open_files.len());
            for file in open_files {
                let project =
                    snapshot.get_default_project(&file.to_uri(&self.get_current_directory()));
                let Some(project) = project else {
                    panic!(
                        "no project found for opened file {}",
                        file.to_file_name(&self.get_current_directory())
                    );
                };
                results.push(OpenedFileOperationResult {
                    project: project.borrow().id(),
                });
            }
            operation.opened_files = Some(results);
        }
        operation
    }
}

impl Session {
    // Go: api/session.go:4683 Close
    // Close closes the session and releases all active snapshots,
    // regardless of their ref counts.
    // PORT: Go `closeOnce.Do`: `close_once` records the first call.
    pub fn close(&self) {
        if self.close_once.replace(true) {
            return;
        }
        self.release_language_server_refs();
        // ts#64434
        self.release_source_file_leases();

        let snapshots: Vec<Rc<project::Snapshot>> = self
            .snapshots
            .borrow_mut()
            .drain()
            .map(|(_, sd)| sd.snapshot.clone())
            .collect();
        for snapshot in snapshots {
            project::Snapshot::deref(&snapshot);
        }

        if self.owns_snapshot_host {
            self.snapshot_host.close();
        }
        // ts#64061
        self.batch_response_pages.borrow_mut().clear();
    }

    // Go: api/session.go releaseLanguageServerRefs (ts#64204)
    // PORT: the Go `languageServerUpdateMu` lock is not ported (one thread).
    fn release_language_server_refs(&self) {
        let Some(project_session) = &self.project_session else {
            return;
        };

        if self.open_projects.borrow().is_empty()
            && self.open_files.borrow().is_empty()
            && self.created_programs.borrow().is_empty()
        {
            return;
        }

        let mut api_request = project::APISnapshotRequest::default();
        if !self.open_projects.borrow().is_empty() {
            api_request.close_projects = Some(self.open_projects.borrow().clone());
        }
        if !self.open_files.borrow().is_empty() {
            api_request.close_files = Some(self.open_files.borrow().clone());
        }
        if !self.created_programs.borrow().is_empty() {
            api_request.remove_programs = Some(self.created_programs.borrow().clone());
        }
        let snapshot = match project_session.api_update(
            &(self.with_locale)(&gostd::context::background()),
            project::FileChangeSummary::default(),
            Some(&api_request),
        ) {
            Ok(snapshot) => snapshot,
            Err(_) => return,
        };
        project::Snapshot::deref(&snapshot);
        self.open_projects.borrow_mut().clear();
        self.open_files.borrow_mut().clear();
        self.created_programs.borrow_mut().clear();
    }
}

// Go: api/session.go:4740 formatSessionID
pub fn format_session_id(id: u64) -> String {
    format!("api-session-{id}")
}

impl Session {
    // Go: api/session.go:4745 toPath (at 673a5f17d713; ts#64159 renames it
    // pathKey, api/session.go:5231, which gives this key for rooted names)
    // toPath converts a file name to a normalized path.
    pub fn to_path(&self, file_name: &str) -> tspath::Path {
        tspath::to_path(
            file_name,
            &self.get_current_directory(),
            self.use_case_sensitive_file_names(),
        )
    }

    // Go: api/session.go:4750 toFileChangeSummary
    // toFileChangeSummary converts API file changes to a project.FileChangeSummary.
    pub fn to_file_change_summary(
        &self,
        changes: Option<&FileNotifications>,
    ) -> project::FileChangeSummary {
        let Some(changes) = changes else {
            return project::FileChangeSummary::default();
        };
        let mut summary = project::FileChangeSummary::default();
        if changes.invalidate_all {
            summary.invalidate_all = true;
            summary.includes_watch_change_outside_node_modules = true;
            return summary;
        }
        let cwd = self.get_current_directory();
        for doc in &changes.changed {
            let uri = doc.to_uri(&cwd);
            summary.changed.insert(uri);
        }
        for doc in &changes.created {
            let uri = doc.to_uri(&cwd);
            summary.created.insert(uri);
        }
        for doc in &changes.deleted {
            let uri = doc.to_uri(&cwd);
            summary.deleted.insert(uri);
        }
        if summary.changed.len() + summary.created.len() + summary.deleted.len() > 0 {
            summary.includes_watch_change_outside_node_modules = true;
        }
        summary
    }

    // Go: api/session.go:4779 getDiagnostics
    // PORT: Go `params.Files != nil` is `Some`: an empty list gives no
    // diagnostics, and no list gives the diagnostics of all files.
    pub fn get_diagnostics(
        &self,
        ctx: &Context,
        params: &GetDiagnosticsParams,
        getter: fn(&compiler::NewProgram, &Context, Node) -> Vec<Diagnostic>,
    ) -> Result<Vec<DiagnosticResponse>, GoError> {
        let sd = self.get_snapshot_data(params.snapshot)?;

        let program = &sd.get_program(&params.project)?;
        // Current for the whole handler (session_p1.rs header).
        let _program = ls_program::enter(program);

        if let Some(files) = &params.files {
            let mut all_diags: Vec<Diagnostic> = Vec::new();
            for file in files {
                let source_file = self.resolve_optional_source_file(program, Some(file))?;
                all_diags.extend(getter(program, ctx, source_file));
            }
            return Ok(new_diagnostic_responses(&all_diags));
        }

        Ok(new_diagnostic_responses(&getter(program, ctx, Node::NIL)))
    }

    // Go: api/session.go:4806 handleGetSyntacticDiagnostics
    pub fn handle_get_syntactic_diagnostics(
        &self,
        ctx: &Context,
        params: &GetDiagnosticsParams,
    ) -> Result<Vec<DiagnosticResponse>, GoError> {
        let ctx = &core_context::with_checker_lifetime(ctx, CheckerLifetime::DIAGNOSTICS);
        self.get_diagnostics(ctx, params, ls_program::get_syntactic_diagnostics)
    }

    // Go: api/session.go:4812 handleGetBindDiagnostics
    pub fn handle_get_bind_diagnostics(
        &self,
        ctx: &Context,
        params: &GetDiagnosticsParams,
    ) -> Result<Vec<DiagnosticResponse>, GoError> {
        let ctx = &core_context::with_checker_lifetime(ctx, CheckerLifetime::DIAGNOSTICS);
        self.get_diagnostics(ctx, params, ls_program::get_bind_diagnostics)
    }

    // Go: api/session.go:4818 handleGetSemanticDiagnostics
    pub fn handle_get_semantic_diagnostics(
        &self,
        ctx: &Context,
        params: &GetDiagnosticsParams,
    ) -> Result<Vec<DiagnosticResponse>, GoError> {
        let ctx = &core_context::with_checker_lifetime(ctx, CheckerLifetime::DIAGNOSTICS);
        self.get_diagnostics(ctx, params, ls_program::get_semantic_diagnostics)
    }

    // Go: api/session.go:4824 handleGetSuggestionDiagnostics
    pub fn handle_get_suggestion_diagnostics(
        &self,
        ctx: &Context,
        params: &GetDiagnosticsParams,
    ) -> Result<Vec<DiagnosticResponse>, GoError> {
        let ctx = &core_context::with_checker_lifetime(ctx, CheckerLifetime::DIAGNOSTICS);
        self.get_diagnostics(ctx, params, ls_program::get_suggestion_diagnostics)
    }

    // Go: api/session.go:4830 handleGetDeclarationDiagnostics
    pub fn handle_get_declaration_diagnostics(
        &self,
        ctx: &Context,
        params: &GetDiagnosticsParams,
    ) -> Result<Vec<DiagnosticResponse>, GoError> {
        let ctx = &core_context::with_checker_lifetime(ctx, CheckerLifetime::DIAGNOSTICS);
        self.get_diagnostics(ctx, params, ls_program::get_declaration_diagnostics)
    }

    // Go: api/session.go:4837 handleGetConfigFileParsingDiagnostics
    // handleGetConfigFileParsingDiagnostics returns config file parsing diagnostics.
    pub fn handle_get_config_file_parsing_diagnostics(
        &self,
        _ctx: &Context,
        params: &GetProjectDiagnosticsParams,
    ) -> Result<Vec<DiagnosticResponse>, GoError> {
        let sd = self.get_snapshot_data(params.snapshot)?;

        let program = &sd.get_program(&params.project)?;
        // Current for the whole handler (session_p1.rs header).
        let _program = ls_program::enter(program);

        let diags = program.get_config_file_parsing_diagnostics();
        Ok(new_diagnostic_responses(&diags))
    }

    // Go: api/session.go:4854 handleGetProgramDiagnostics
    // handleGetProgramDiagnostics returns program-wide diagnostics, including options diagnostics.
    pub fn handle_get_program_diagnostics(
        &self,
        _ctx: &Context,
        params: &GetProjectDiagnosticsParams,
    ) -> Result<Vec<DiagnosticResponse>, GoError> {
        let sd = self.get_snapshot_data(params.snapshot)?;

        let program = &sd.get_program(&params.project)?;
        // Current for the whole handler (session_p1.rs header).
        let _program = ls_program::enter(program);

        let diags = ls_program::get_program_diagnostics(program);
        Ok(new_diagnostic_responses(&diags))
    }

    // Go: api/session.go:4871 handleGetGlobalDiagnostics
    // handleGetGlobalDiagnostics returns global (non-file-specific) semantic diagnostics.
    pub fn handle_get_global_diagnostics(
        &self,
        ctx: &Context,
        params: &GetProjectDiagnosticsParams,
    ) -> Result<Vec<DiagnosticResponse>, GoError> {
        let ctx = &core_context::with_checker_lifetime(ctx, CheckerLifetime::DIAGNOSTICS);
        let sd = self.get_snapshot_data(params.snapshot)?;

        let proj = sd.get_project(&params.project)?;

        let program = proj.borrow().get_program();
        let Some(program) = program else {
            return Err(errors::errorf(
                format!("{}: project has no program", *ERR_CLIENT_ERROR),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        };
        // Current for the whole handler (session_p1.rs header).
        let _program = ls_program::enter(&program);

        // Global diagnostics are accumulated lazily by the project's checker pool as
        // files are checked. Force a full semantic pass so any global (non-file-specific)
        // diagnostics are produced; otherwise this would return an empty result for
        // projects using an external checker pool (the typical API case), since
        // compiler.Program.GetGlobalDiagnostics only reports for the internal pool.
        let _ = ls_program::get_semantic_diagnostics(&program, ctx, Node::NIL);

        let diags: Vec<Diagnostic> = proj
            .borrow()
            .get_project_diagnostics(ctx)
            .into_iter()
            .filter(|d| d.file.is_nil())
            .collect();
        Ok(new_diagnostic_responses(&diags))
    }

    // Go: api/session.go:4903 resolveOptionalSourceFile
    // resolveOptionalSourceFile resolves an optional DocumentIdentifier to a source file.
    // Returns nil if the identifier is nil (meaning all files).
    pub fn resolve_optional_source_file(
        &self,
        program: &compiler::NewProgram,
        file: Option<&DocumentIdentifier>,
    ) -> Result<Node, GoError> {
        let Some(file) = file else {
            return Ok(Node::NIL);
        };
        let source_file = program
            .get_source_file(&file.to_file_name(&program_base_directory(&program)))
            .map_or(Node::NIL, |f| f.root);
        if source_file.is_nil() {
            return Err(errors::errorf(
                format!(
                    "{}: source file not found: {}",
                    *ERR_CLIENT_ERROR,
                    file.string()
                ),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        }
        Ok(source_file)
    }

    // Go: api/session.go:4915 handleGetReferencesToSymbolInFile
    // handleGetReferencesToSymbolInFile returns node handles for all identifiers in a file that reference the given symbol.
    pub fn handle_get_references_to_symbol_in_file(
        &self,
        ctx: &Context,
        params: &GetReferencesToSymbolInFileParams,
    ) -> Result<Vec<NodeHandle>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, symbol) = setup.resolve_symbol_handle(&params.symbol)?;
        if symbol.is_nil() {
            return Ok(Vec::new());
        }
        let symbol = checker_symbol(&setup.checker, &owner, symbol);

        let source_file = setup
            .program
            .get_source_file(
                &params
                    .file
                    .to_file_name(&program_base_directory(&setup.program)),
            )
            .map_or(Node::NIL, |f| f.root);
        if source_file.is_nil() {
            return Err(errors::errorf(
                format!(
                    "{}: source file not found: {}",
                    *ERR_CLIENT_ERROR,
                    params.file.string()
                ),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        }

        let nodes = setup
            .checker
            .borrow_mut()
            .get_references_to_symbol_in_file(source_file, symbol);
        let mut result = Vec::with_capacity(nodes.len());
        for node in nodes {
            result.push(setup.sd.node_handle_from(node));
        }
        Ok(result)
    }

    // Go: api/session.go:4944 handleGetSignatureUsages
    pub fn handle_get_signature_usages(
        &self,
        ctx: &Context,
        params: &GetSignatureUsagesParams,
    ) -> Result<Vec<SignatureUsageResponse>, GoError> {
        let sd = self.get_snapshot_data(params.snapshot)?;
        let program = &sd.get_program(&params.project)?;
        // Current for the whole handler (session_p1.rs header).
        let _program = ls_program::enter(program);

        let signature_decl = sd.resolve_node_handle(program, &params.signature_decl)?;
        if signature_decl.is_nil() {
            return Ok(Vec::new());
        }

        let lang_svc =
            self.setup_language_service(&sd.snapshot, Rc::clone(program), &params.project, "")?;

        let usages = lang_svc.get_signature_usages(ctx, signature_decl);
        // PORT: Go `usages == nil`. Go returns a nil slice exactly when there
        // is no usage; both results marshal as `[]`.
        if usages.is_empty() {
            return Ok(Vec::new());
        }

        let mut result = Vec::with_capacity(usages.len());
        for u in &usages {
            let mut entry = SignatureUsageResponse {
                name: sd.node_handle_from(u.name),
                ..Default::default()
            };
            if u.call.is_some() {
                entry.call = sd.node_handle_from(u.call);
            }
            result.push(entry);
        }
        Ok(result)
    }

    // Go: api/session.go:4987 handleGetCompletionsAtPosition
    // handleGetCompletionsAtPosition returns completions at a position in a document.
    pub fn handle_get_completions_at_position(
        &self,
        ctx: &Context,
        params: &GetCompletionsAtPositionParams,
    ) -> Result<Option<CompletionInfoResponse>, GoError> {
        let api_ctx;
        let ctx = if params.include_symbol {
            api_ctx = core_context::with_checker_lifetime(ctx, CheckerLifetime::API);
            &api_ctx
        } else {
            ctx
        };
        let sd = self.get_snapshot_data(params.snapshot)?;
        // ts#64133
        // PORT: `run` also returns the source file; the symbol reads below take
        // the checker for it.
        let run = |snapshot: &Rc<project::Snapshot>,
                   program: &Rc<compiler::NewProgram>|
         -> Result<(Option<ls::CompletionList>, Node), GoError> {
            let source_file = program
                .get_source_file(&params.file.to_file_name(&program_base_directory(&program)))
                .map_or(Node::NIL, |f| f.root);
            if source_file.is_nil() {
                return Ok((None, source_file));
            }
            // ts#64554: the source file is the active file.
            let lang_svc = self.setup_language_service(
                snapshot,
                Rc::clone(program),
                &params.project,
                source_file_file_name(source_file),
            )?;
            // PORT: Go converts the uint32 position to a 64-bit int, and
            // UTF16ToUTF8 adds the delta of the last entry to a position past
            // all entries. A port position past i32::MAX is past any text.
            // Then Go's position is the port position less the port bytes of
            // the text that Go does not have (`go_len`), and the port runs
            // with i32::MAX, which takes the same branches as Go. Go's first
            // read of the position is the JSDoc snippet slice
            // `text[lineStart:position]` (ls/jsdoc_snippet.go:77), which
            // panics, so the port panics with Go's numbers there (see
            // `past_text_reads_jsdoc_snippet`). Past that read, the next
            // read is the slice `text[:position]` of `getWordLengthAndStart`
            // (completions.go:2885), and the port's panic there names the
            // port's bound (`past_text`): the port gives Go's numbers.
            let position_map = source_file_get_position_map(source_file);
            let port_position = i64::from(params.position)
                + i64::from(position_map.entries.last().map_or(0, |e| e.delta));
            let mut past_text = None;
            let internal_pos = match i32::try_from(port_position) {
                Ok(_) => position_map.utf16_to_utf8(params.position as i32),
                Err(_) => {
                    let text = source_file_text(source_file);
                    let len = crate::scanner_util::go_len(&text);
                    let extra = (text.len() - len) as i64;
                    let go_position = port_position - extra;
                    let go_text = format!(
                        "runtime error: slice bounds out of range [:{go_position}] with length {len}"
                    );
                    if past_text_reads_jsdoc_snippet(
                        source_file,
                        params.trigger_character.as_deref(),
                        !lang_svc
                            .user_preferences()
                            .enable_js_doc_completions
                            .is_false(),
                    ) {
                        crate::core::go_panic(go_text);
                    }
                    let port_text = format!(
                        "runtime error: slice bounds out of range [:{}] with length {len}",
                        i64::from(i32::MAX) - extra
                    );
                    past_text = Some((port_text, go_text));
                    i32::MAX
                }
            };
            drop(position_map);
            let complete = || {
                lang_svc.get_completions_at_position_exported(
                    ctx,
                    source_file,
                    internal_pos,
                    params.trigger_character.clone(),
                    params.include_symbol,
                )
            };
            let result = match past_text {
                None => complete()?,
                Some((port_text, go_text)) => {
                    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(complete)) {
                        Ok(result) => result?,
                        Err(mut payload) => {
                            if let Some(panic) = payload.downcast_mut::<crate::core::GoPanic>()
                                && panic.message == port_text
                            {
                                panic.message = go_text;
                            }
                            std::panic::resume_unwind(payload)
                        }
                    }
                }
            };
            Ok((result, source_file))
        };

        let mut program = sd.get_program(&params.project)?;
        // Current for the whole handler (session_p1.rs header).
        let mut program_guard = ls_program::enter(&program);
        let mut result = run(&sd.snapshot, &program);
        // PORT: Go `defer preparedSnapshot.Deref(...)`: the guard derefs it
        // when the handler returns.
        let mut _prepared_snapshot = ls_program::Release::noop();
        if let Err(err) = &result
            && errors::is(err, &ls::ERR_NEEDS_AUTO_IMPORTS)
        {
            // ts#64554 (Go N' api/session.go:5508): symbols must come from the
            // requested snapshot, so it must be prepared already.
            if params.include_symbol {
                return Err(errors::errorf(
                    format!(
                        "{}: snapshot is not prepared for auto-imports for {}",
                        *ERR_CLIENT_ERROR, params.file
                    ),
                    vec![ERR_CLIENT_ERROR.clone()],
                ));
            }
            // ts#64544 (Go N' api/session.go:5511): the URI of the program's
            // file name.
            let source_file = program
                .get_source_file(&params.file.to_file_name(&program_base_directory(&program)))
                .map_or(Node::NIL, |f| f.root);
            if source_file.is_nil() {
                return Ok(None);
            }
            // ts#64163
            let prepared_snapshot = self.snapshot_host.clone_snapshot_with_auto_imports(
                ctx,
                &sd.snapshot,
                &lsconv::file_name_to_document_uri(source_file_file_name(source_file)),
                None,
            );
            if let Some(project_session) = &self.project_session {
                project_session.try_adopt_snapshot_in_background(&sd.snapshot, &prepared_snapshot);
            }
            _prepared_snapshot = {
                let snapshot = prepared_snapshot.clone();
                ls_program::Release::new(move || project::Snapshot::deref(&snapshot))
            };
            if let Some(err) = ctx.err() {
                return Err(err);
            }
            // ts#64319: looked up by project ID.
            let project_id = &params.project;
            let proj = prepared_snapshot.project_collection.get_project(project_id);
            let Some(proj) = proj else {
                return Err(errors::errorf(
                    format!("{}: project {} not found", *ERR_CLIENT_ERROR, project_id.0),
                    vec![ERR_CLIENT_ERROR.clone()],
                ));
            };
            let Some(prepared_program) = proj.borrow().get_program() else {
                return Err(errors::errorf(
                    format!("{}: project has no program", *ERR_CLIENT_ERROR),
                    vec![ERR_CLIENT_ERROR.clone()],
                ));
            };
            program = prepared_program;
            drop(program_guard);
            program_guard = ls_program::enter(&program);
            result = run(&prepared_snapshot, &program);
        }
        let _program_guard = program_guard;
        let program = &program;
        let (result, source_file) = match result {
            Err(err) => return Err(err),
            Ok((None, _)) => return Ok(None),
            Ok((Some(result), source_file)) => (result, source_file),
        };
        // PORT: Go reads `item.Symbol` without a checker. The symbols live in
        // the arena of the checker the completion request used, so the port
        // takes that checker again (same request context and file; the pool
        // returns the same checker) to read them.
        let mut symbol_checker: Option<(Rc<RefCell<Checker>>, ls_program::Release)> = None;
        let mut entries = Vec::with_capacity(result.items.len());
        for item in &result.items {
            let mut entry = CompletionEntryResponse {
                name: item.label.clone(),
                sort_text: item.sort_text.clone(),
                insert_text: item.insert_text.clone(),
                filter_text: item.filter_text.clone(),
                detail: item.detail.clone(),
                ..Default::default()
            };
            if let Some(kind) = item.kind {
                entry.kind = kind.0;
            }
            if let Some(label_details) = &item.label_details {
                entry.label_details = Some(CompletionEntryLabelDetailsResponse {
                    detail: label_details.detail.clone(),
                    description: label_details.description.clone(),
                });
            }
            if item.symbol.is_some() {
                let (checker, _) = symbol_checker.get_or_insert_with(|| {
                    ls_program::get_type_checker_for_file(program, ctx, source_file)
                });
                entry.symbol = sd.new_symbol_response(checker, item.symbol, &params.project);
            }
            entries.push(entry);
        }
        Ok(Some(CompletionInfoResponse {
            is_incomplete: result.is_incomplete,
            entries,
        }))
    }

    // Go: api/session.go:5067 handleGetReferencedSymbolsForNode
    // handleGetReferencedSymbolsForNode returns node handles for all references found at a node.
    pub fn handle_get_referenced_symbols_for_node(
        &self,
        ctx: &Context,
        params: &GetReferencedSymbolsForNodeParams,
    ) -> Result<Vec<ReferencedSymbolEntry>, GoError> {
        let sd = self.get_snapshot_data(params.snapshot)?;
        let program = &sd.get_program(&params.project)?;
        // Current for the whole handler (session_p1.rs header).
        let _program = ls_program::enter(program);

        let node = sd.resolve_node_handle(program, &params.node)?;
        if node.is_nil() {
            return Ok(Vec::new());
        }

        let lang_svc =
            self.setup_language_service(&sd.snapshot, Rc::clone(program), &params.project, "")?;

        let source_files: Vec<Node> = program.get_source_files().iter().map(|f| f.root).collect();
        // PORT: Go passes `params.Position` on unchanged. Its only use is a
        // Go byte offset in `node` when `node` is a source file
        // (`getReferenceAtPosition`). Port offsets differ from Go offsets
        // after a marker unit (see `scanner_util::GO_STRING_MARKER`), so it
        // is mapped there (`port_byte_offset`).
        let position = if node.kind() == SyntaxKind::SourceFile {
            crate::scanner_util::port_byte_offset(&source_file_text(node), params.position)
        } else {
            params.position
        };
        let entries =
            lang_svc.get_referenced_symbols_for_node_exported(ctx, position, node, &source_files);
        // PORT: Go `entries == nil`; an empty result marshals as `[]` either way.
        if entries.is_empty() {
            return Ok(Vec::new());
        }

        // PORT: Go reads `symbol.Declarations` (DefinitionNode) and the
        // definition symbols without a checker. They live in the arena of the
        // request checker, which the language service took with
        // `GetTypeChecker(ctx)`; the port takes it again to read them
        // (ls/findallreferences_p1.rs header).
        let (checker, _done) = ls_program::get_type_checker(program, ctx);

        let mut result: Vec<ReferencedSymbolEntry> = Vec::new();
        for entry in &entries {
            let entry = entry.borrow();
            let def_node = entry.definition_node(&checker.borrow().symbols);
            if def_node.is_nil() {
                continue;
            }
            let mut refs: Vec<NodeHandle> = Vec::new();
            for ref_ in entry.references() {
                let ref_ = ref_.borrow();
                if ref_.is_node_entry() {
                    refs.push(sd.node_handle_from(ref_.node()));
                }
            }
            let mut re = ReferencedSymbolEntry {
                definition: sd.node_handle_from(def_node),
                references: refs,
                ..Default::default()
            };
            let sym = entry.definition_symbol();
            if sym.is_some() {
                re.symbol = sd.new_symbol_response(&checker, sym, &params.project);
            }
            result.push(re);
        }
        Ok(result)
    }
}

// Go: encoding/base64/base64.go:139 (*Encoding).EncodeToString (StdEncoding)
// PORT: the crate has no base64 dependency. Go `base64.StdEncoding`: the
// standard alphabet with `=` padding.
// PERF: (apiperf1) Go `Encode` (base64.go:145) writes each 3 bytes as 4
// bytes of a buffer of the final size; so does this. The text is ASCII, so
// the UTF-8 check at the end is one fast pass. A `String::push` for each
// byte was 5% of an API createSourceFile loop of a 260 KB file.
pub fn base64_std_encoding_encode_to_string(src: &[u8]) -> String {
    const ENCODE_STD: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let encode = |val: u32, shift: u32| ENCODE_STD[((val >> shift) & 0x3F) as usize];
    let mut dst = vec![0u8; src.len().div_ceil(3) * 4];
    let (whole, rest) = src.as_chunks::<3>();
    let (quads, _) = dst.as_chunks_mut::<4>();
    for (quad, &[b0, b1, b2]) in quads.iter_mut().zip(whole) {
        let val = (u32::from(b0) << 16) | (u32::from(b1) << 8) | u32::from(b2);
        *quad = [
            encode(val, 18),
            encode(val, 12),
            encode(val, 6),
            encode(val, 0),
        ];
    }
    // Go: the remaining small block of `Encode`, with `=` padding.
    if let Some(quad) = quads.get_mut(whole.len()) {
        let val = (u32::from(rest[0]) << 16) | rest.get(1).map_or(0, |&b| u32::from(b) << 8);
        *quad = [
            encode(val, 18),
            encode(val, 12),
            if rest.len() == 2 {
                encode(val, 6)
            } else {
                b'='
            },
            b'=',
        ];
    }
    String::from_utf8(dst).expect("base64 text is ASCII")
}

#[cfg(test)]
mod base64_tests {
    use super::*;

    // Go: encoding/base64/base64_test.go pairs (RFC 3548 examples and the
    // padding cases), StdEncoding.
    #[test]
    fn encode_to_string_matches_go() {
        for (raw, encoded) in [
            (&b""[..], ""),
            (b"f", "Zg=="),
            (b"fo", "Zm8="),
            (b"foo", "Zm9v"),
            (b"foob", "Zm9vYg=="),
            (b"fooba", "Zm9vYmE="),
            (b"foobar", "Zm9vYmFy"),
            (b"\x14\xfb\x9c\x03\xd9\x7e", "FPucA9l+"),
            (b"\x14\xfb\x9c\x03\xd9", "FPucA9k="),
            (b"\x14\xfb\x9c\x03", "FPucAw=="),
        ] {
            assert_eq!(base64_std_encoding_encode_to_string(raw), encoded);
            assert_eq!(
                base64_std_encoding_decode_string(encoded).expect("valid base64"),
                raw
            );
        }
    }
}

// Go: encoding/base64/base64.go:429 (*Encoding).DecodeString (StdEncoding)
// PORT: the crate has no base64 dependency. This is Go `Decode` as a loop of
// `decodeQuantum` (base64.go:312): the standard alphabet, `=` padding
// required, not strict, `\r` and `\n` skipped. Go's 8- and 4-byte fast paths
// decode valid input the same way and fall back to `decodeQuantum` at the
// same offset, so results and error offsets are equal. Go returns the
// partial data with the error; the only caller uses the error alone. Go
// `CorruptInputError` is a `GoError` with the same text.
pub fn base64_std_encoding_decode_string(s: &str) -> Result<Vec<u8>, GoError> {
    fn decode_map(c: u8) -> u8 {
        match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => 0xFF,
        }
    }
    // Go: base64.go:303 CorruptInputError.Error
    fn corrupt_input_error(offset: usize) -> GoError {
        errors::new(format!("illegal base64 data at input byte {offset}"))
    }

    let src = s.as_bytes();
    let mut dst: Vec<u8> = Vec::with_capacity(src.len() / 4 * 3);
    if src.is_empty() {
        return Ok(dst);
    }
    let mut si = 0usize;
    while si < src.len() {
        // Go: base64.go:312 decodeQuantum
        let mut dbuf = [0u8; 4];
        let mut dlen = 4usize;
        let mut err: Option<GoError> = None;

        let mut j = 0usize;
        while j < dbuf.len() {
            if src.len() == si {
                if j == 0 {
                    return Ok(dst);
                }
                // j == 1, or padding is required (StdEncoding)
                return Err(corrupt_input_error(si - j));
            }
            let input = src[si];
            si += 1;

            let out = decode_map(input);
            if out != 0xFF {
                dbuf[j] = out;
                j += 1;
                continue;
            }

            if input == b'\n' || input == b'\r' {
                continue;
            }

            if input != b'=' {
                return Err(corrupt_input_error(si - 1));
            }

            // We've reached the end and there's padding
            match j {
                0 | 1 => {
                    // incorrect padding
                    return Err(corrupt_input_error(si - 1));
                }
                2 => {
                    // "==" is expected, the first "=" is already consumed.
                    // skip over newlines
                    while si < src.len() && (src[si] == b'\n' || src[si] == b'\r') {
                        si += 1;
                    }
                    if si == src.len() {
                        // not enough padding
                        return Err(corrupt_input_error(src.len()));
                    }
                    if src[si] != b'=' {
                        // incorrect padding
                        return Err(corrupt_input_error(si - 1));
                    }

                    si += 1;
                }
                _ => {}
            }

            // skip over newlines
            while si < src.len() && (src[si] == b'\n' || src[si] == b'\r') {
                si += 1;
            }
            if si < src.len() {
                // trailing garbage
                err = Some(corrupt_input_error(si));
            }
            dlen = j;
            break;
        }

        // Convert 4x 6bit source bytes into 3 bytes
        let val = u32::from(dbuf[0]) << 18
            | u32::from(dbuf[1]) << 12
            | u32::from(dbuf[2]) << 6
            | u32::from(dbuf[3]);
        let bytes = [(val >> 16) as u8, (val >> 8) as u8, val as u8];
        match dlen {
            4 => dst.extend_from_slice(&bytes[..3]),
            3 => dst.extend_from_slice(&bytes[..2]),
            2 => dst.extend_from_slice(&bytes[..1]),
            _ => {}
        }

        if let Some(err) = err {
            return Err(err);
        }
    }
    Ok(dst)
}

// Go: strconv/number.go:104 ParseUint and internal/strconv/atoi.go:47 ParseUint
// PORT: Go stdlib. Only `2 <= base <= 36` is ported (the caller passes 10);
// Go `*strconv.NumError` is a `GoError` with the same text.
fn strconv_parse_uint(s: &str, base: u32, bit_size: u32) -> Result<u64, GoError> {
    // Go: strconv/number.go:258 (*NumError).Error
    let num_error = |reason: &str| {
        errors::new(format!(
            "strconv.ParseUint: parsing {}: {}",
            gostd::strconv::quote(s),
            reason
        ))
    };

    if s.is_empty() {
        return Err(num_error("invalid syntax"));
    }

    debug_assert!((2..=36).contains(&base) && (1..=64).contains(&bit_size));

    // Cutoff is the smallest number such that cutoff*base > maxUint64.
    let cutoff = u64::MAX / u64::from(base) + 1;

    let max_val: u64 = if bit_size == 64 {
        u64::MAX
    } else {
        (1u64 << bit_size) - 1
    };

    let mut n: u64 = 0;
    for &c in s.as_bytes() {
        let lower = c | (b'x' - b'X');
        let d: u8 = if c.is_ascii_digit() {
            c - b'0'
        } else if lower.is_ascii_lowercase() {
            lower - b'a' + 10
        } else {
            return Err(num_error("invalid syntax"));
        };

        if u32::from(d) >= base {
            return Err(num_error("invalid syntax"));
        }

        if n >= cutoff {
            // n*base overflows
            return Err(num_error("value out of range"));
        }
        n *= u64::from(base);

        let n1 = n.wrapping_add(u64::from(d));
        if n1 < n || n1 > max_val {
            // n+d overflows
            return Err(num_error("value out of range"));
        }
        n = n1;
    }

    Ok(n)
}

// Go: api/session.go:3543 decodePrintNode (ts#64320)
pub fn decode_print_node(encoded: &str) -> Result<Node, GoError> {
    let data = match base64_std_encoding_decode_string(encoded) {
        Ok(data) => data,
        Err(err) => {
            return Err(errors::errorf(
                format!("{}: invalid base64 data: {}", *ERR_CLIENT_ERROR, err),
                vec![ERR_CLIENT_ERROR.clone(), err],
            ));
        }
    };

    let node = match encoder::decode_nodes(&data) {
        Ok(node) => node,
        Err(err) => {
            return Err(errors::errorf(
                format!("{}: failed to decode AST: {}", *ERR_CLIENT_ERROR, err),
                vec![ERR_CLIENT_ERROR.clone(), err],
            ));
        }
    };
    Ok(node)
}

// Go: api/session.go:3556 newPrinter (ts#64320)
// PORT: private, so it does not collide with `printer::new_printer` in the
// api prelude; it calls that one by path.
fn new_printer(params: &PrintNodeParams) -> crate::printer::Printer {
    crate::printer::new_printer(
        PrinterOptions {
            preserve_source_newlines: params.preserve_source_newlines,
            never_ascii_escape: params.never_ascii_escape,
            terminate_unterminated_literals: params.terminate_unterminated_literals,
            ..Default::default()
        },
        PrintHandlers::default(),
        None,
    )
}

/// Whether Go's `getCompletionsAtPosition` (ls/completions.go:402) reads
/// a position past the text of `file` first in the JSDoc snippet slice
/// (ls/jsdoc_snippet.go:77), with the trigger character `trigger` and
/// JSDoc completions `jsdoc_on`.
// PORT: the checks before that slice read no position past the text, so
// the port runs them with i32::MAX, as `get_completions_at_position` does:
// - `IsInString` (completions.go:410) is false past every token, so the
//   trigger check is `isValidTrigger`.
// - `isValidTrigger` reads the slice for "*" (completions.go:3312), and
//   panics for an unknown trigger character as the port's does.
// - A valid " " returns before the snippet (completions.go:414).
// - `getJSDocSnippetCompletion` reads the slice when JSDoc completions are
//   on (jsdoc_snippet.go:30).
fn past_text_reads_jsdoc_snippet(file: Node, trigger: Option<&str>, jsdoc_on: bool) -> bool {
    let Some(trigger) = trigger else {
        return jsdoc_on;
    };
    match trigger {
        "*" => true,
        " " => false,
        _ => {
            let (_, previous_token) = ls::completions_p2::get_relevant_tokens(i32::MAX, file);
            ls::completions_p2::is_valid_trigger(file, trigger, previous_token, i32::MAX)
                && jsdoc_on
        }
    }
}

/// What Go `resolveSymbolReference` returns (ts#64518): the symbol, the
/// snapshot data and canonical project of a snapshot-owned symbol, and the
/// release function of a file-owned one.
pub enum ResolvedSymbolReference {
    /// A file-owned symbol. `symbols` (a binder lineage copy) holds it; the
    /// lease holds its file until this value drops.
    File {
        symbols: SymbolArena,
        symbol: SymbolId,
        _lease: LeaseGuard,
    },
    /// A snapshot-owned symbol and the checker whose arena holds it.
    Snapshot {
        sd: Rc<SnapshotData>,
        project: project::ID,
        checker: Rc<RefCell<Checker>>,
        symbol: SymbolId,
    },
}

/// Go `defer lease.Release()`: releases the lease when it drops.
pub struct LeaseGuard(Option<Rc<project::SourceFileLease>>);

impl LeaseGuard {
    /// Go `newFileSymbolResponse(symbol)` for a symbol of the leased file
    /// (`new_leased_file_symbol_response`).
    fn file_symbol_response(&self, symbols: &SymbolArena, symbol: SymbolId) -> SymbolResponse {
        match &self.0 {
            Some(lease) => {
                new_leased_file_symbol_response(symbols, symbol, lease.parsed_source_file())
            }
            None => new_file_symbol_response(symbols, symbol),
        }
    }
}

impl Drop for LeaseGuard {
    fn drop(&mut self) {
        if let Some(lease) = self.0.take() {
            lease.release();
            project::drop_released_lease(lease);
        }
    }
}

// Go: tspath/pathkey.go:31 TryPathKeyFromCanonical (ts#64159)
// Whether `path` is a canonical path key: empty, or a rooted normalized
// path.
// PORT: the port keeps string paths (bump D plan section 3, behavior only),
// and the typed path helpers are not in the Rust tspath, which the
// program lane owns. The API checks the paths that it reads from a client
// with these copies.
pub fn try_path_key_from_canonical(path: &str) -> bool {
    path.is_empty() || try_rooted_path_from_normalized(path)
}

// Go: tspath/rooted_path.go:86 TryRootedPathFromNormalized (ts#64159)
// Whether `path` is rooted and normalized: no URL query or fragment, no
// backslash, no relative or empty segment, and a trailing separator only
// on a bare root.
pub fn try_rooted_path_from_normalized(path: &str) -> bool {
    if has_rooted_url_suffix(path) {
        return false;
    }
    let mut root_length = tspath::get_encoded_root_length(path);
    if root_length < 0 {
        root_length = !root_length;
    }
    let root_length = root_length as usize;
    let bytes = path.as_bytes();
    !(path.is_empty()
        || root_length == 0
        || path.contains('\\')
        || root_length < bytes.len() && bytes[root_length] == b'/'
        || has_relative_path_segment(&path[root_length..])
        || bytes.len() == root_length && !tspath::has_trailing_directory_separator(path)
        || bytes.len() > root_length && tspath::has_trailing_directory_separator(path))
}

// Go: tspath/rooted_path.go:29 ToRootedPath (ts#64159)
// ToRootedPath resolves path against currentDirectory and normalizes it.
// Go `ToRootedFilePath` (:120) and `ToRootedDirectoryPath` (:154) are this
// function with a typed result. An empty `path`, or a URL `path` with a
// query or fragment, is a Go panic; a request handler answers it as
// `panic: <message>`.
// PORT: the API copy (see `try_path_key_from_canonical`). Go normalizes
// with `getNormalizedAbsolutePathFromDirectory` (path.go:409), which gives
// the text of `tspath::get_normalized_absolute_path` for a rooted current
// directory.
pub fn to_rooted_path(path: &str, current_directory: &str) -> String {
    if path.is_empty() {
        crate::core::go_panic("path must not be empty".to_string());
    }
    if has_rooted_url_suffix(path) {
        crate::core::go_panic("path must not contain a URL query or fragment".to_string());
    }
    if tspath::get_encoded_root_length(path) == 0
        && has_url_root(current_directory)
        && path.contains(['?', '#'])
    {
        crate::core::go_panic("relative URL path must not contain a query or fragment".to_string());
    }
    let mut normalized = tspath::get_normalized_absolute_path(path, current_directory);
    if tspath::get_encoded_root_length(&normalized) == 0 || has_rooted_url_suffix(&normalized) {
        crate::core::go_panic("path must be rooted".to_string());
    }
    // Go: tspath/rooted_path.go:65 ensureRootedPathRootSeparator
    if tspath::get_root_length(&normalized) == normalized.len()
        && !tspath::has_trailing_directory_separator(&normalized)
    {
        normalized.push('/');
    }
    normalized
}

// Go: tspath/rooted_path.go:106 hasRootedURLSuffix (ts#64159)
fn has_rooted_url_suffix(path: &str) -> bool {
    if !has_url_root(path) {
        return false;
    }
    let after_scheme = path.split_once("://").map_or("", |(_, rest)| rest);
    after_scheme.contains(['?', '#'])
}

// Go: tspath/rooted_path.go:114 hasURLRoot (ts#64159)
fn has_url_root(path: &str) -> bool {
    tspath::get_encoded_root_length(path) < 0 && path.contains("://")
}

// Go: tspath/path.go:554 hasRelativePathSegment
// Whether a segment of `p` is "." or "..", or empty between two slashes.
// PORT: a copy of the private Rust `tspath::has_relative_path_segment`.
fn has_relative_path_segment(p: &str) -> bool {
    let segments: Vec<&str> = p.split('/').collect();
    let last = segments.len() - 1;
    segments.iter().enumerate().any(|(i, segment)| {
        *segment == "." || *segment == ".." || segment.is_empty() && i != 0 && i != last
    })
}

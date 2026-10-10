//! Port of Go `checker/nodebuilderscopes.go`: signature scopes for the node
//! builder.

use crate::prelude::*;

// Go: checker/nodebuilderscopes.go:10 cloneNodeBuilderContext
// PORT: the restore closure keeps its own handle to the context and puts the
// saved copy-on-write state back.
pub fn clone_node_builder_context(context: &Rc<RefCell<NodeBuilderContext>>) -> Box<dyn FnOnce()> {
    // Make type parameters created within this context not consume the name outside this context
    // The symbol serializer ends up creating many sibling scopes that all need "separate" contexts when
    // it comes to naming things - within a normal `typeToTypeNode` call, the node builder only ever descends
    // through the type tree, so the only cases where we could have used distinct sibling scopes was when there
    // were multiple generic overloads with similar generated type parameter names
    // The effect:
    // When we write out
    // export const x: <T>(x: T) => T
    // export const y: <T>(x: T) => T
    // we write it out like that, rather than as
    // export const x: <T>(x: T) => T
    // export const y: <T_1>(x: T_1) => T_1
    let (
        restore_names,
        restore_names_by_text,
        restore_names_by_text_next_name_count,
        restore_symbol_list,
    ) = {
        let c = context.borrow();
        (
            c.type_parameter_names.enter_scope(),
            c.type_parameter_names_by_text.enter_scope(),
            c.type_parameter_names_by_text_next_name_count.enter_scope(),
            c.type_parameter_symbol_list.enter_scope(),
        )
    };
    let context = context.clone();
    Box::new(move || {
        let mut c = context.borrow_mut();
        c.type_parameter_names.restore_scope(restore_names);
        c.type_parameter_names_by_text
            .restore_scope(restore_names_by_text);
        c.type_parameter_names_by_text_next_name_count
            .restore_scope(restore_names_by_text_next_name_count);
        c.type_parameter_symbol_list
            .restore_scope(restore_symbol_list);
    })
}

// Go: checker/nodebuilderscopes.go:35 localsRecord
struct LocalsRecord {
    name: String,
    old_symbol: SymbolId,
}

// PORT: private accessor, see `p1_ctx` in nodebuilder_impl_p1.rs.
fn scopes_ctx(b: &Rc<RefCell<NodeBuilderImpl>>) -> Rc<RefCell<NodeBuilderContext>> {
    b.borrow().ctx.clone()
}

/// Reports whether `node` has node builder links that mark it as the fake
/// scope of kind `kind`.
fn is_fake_scope_of_kind(b: &Rc<RefCell<NodeBuilderImpl>>, node: Node, kind: &str) -> bool {
    let bi = b.borrow();
    if !bi.links.has(node) {
        return false;
    }
    bi.links
        .try_get(node)
        .is_some_and(|links| links.fake_scope_for_signature_declaration.as_deref() == Some(kind))
}

impl Checker {
    // Go: checker/nodebuilderscopes.go:40 addSymbolTypeToContext
    pub fn add_symbol_type_to_context(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        symbol: SymbolId,
        t: TypeId,
    ) -> Box<dyn FnOnce()> {
        let ctx = scopes_ctx(b);
        // PORT: the map is keyed by `SymbolId`, which stands for Go
        // `ast.GetSymbolId(symbol)`. That call gives the symbol its id, so it
        // is made too (`ValueSymbolLinkStore`).
        get_symbol_id(&self.symbols, symbol);
        let id = symbol;
        let old_type = ctx.borrow_mut().enclosing_symbol_types.insert(id, t);
        Box::new(move || {
            let mut c = ctx.borrow_mut();
            if let Some(old_type) = old_type {
                c.enclosing_symbol_types.insert(id, old_type);
            } else {
                c.enclosing_symbol_types.remove(&id);
            }
        })
    }

    // Go: checker/nodebuilderscopes.go:53 enterSignatureScope
    pub fn enter_signature_scope(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        signature: SignatureId,
    ) -> (Vec<SymbolId>, Box<dyn FnOnce(&mut Checker)>) {
        let expanded_params = self
            .get_expanded_parameters(signature, true /*skipUnionExpanding*/)
            .swap_remove(0);
        let (declaration, type_parameters, parameters, mapper) = {
            let sig = self.sig(signature);
            (
                sig.declaration,
                sig.type_parameters.clone(),
                sig.parameters.clone(),
                sig.mapper,
            )
        };
        let cleanup = self.enter_new_scope(
            b,
            declaration,
            &expanded_params,
            &type_parameters,
            &parameters,
            mapper,
        );
        (expanded_params, cleanup)
    }

    // Go: checker/nodebuilderscopes.go:59 enterNewScope
    // PORT: the cleanup closure takes the checker, because undoing a fake
    // scope writes to the symbol tables that the checker owns.
    pub fn enter_new_scope(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        declaration: Node,
        expanded_params: &[SymbolId],
        type_parameters: &[TypeId],
        original_parameters: &[SymbolId],
        mapper: MapperId,
    ) -> Box<dyn FnOnce(&mut Checker)> {
        let ctx = scopes_ctx(b);
        let cleanup_context = clone_node_builder_context(&ctx);
        // For regular function/method declarations, the enclosing declaration will already be signature.declaration,
        // so this is a no-op, but for arrow functions and function expressions, the enclosing declaration will be
        // the declaration that the arrow function / function expression is assigned to.
        //
        // If the parameters or return type include "typeof globalThis.paramName", using the wrong scope will lead
        // us to believe that we can emit "typeof paramName" instead, even though that would refer to the parameter,
        // not the global. Make sure we are in the right scope by changing the enclosingDeclaration to the function.
        //
        // We can't use the declaration directly; it may be in another file and so we may lose access to symbols
        // accessible to the current enclosing declaration, or gain access to symbols not accessible to the current
        // enclosing declaration. To keep this chain accurate, insert a fake scope into the chain which makes the
        // function's parameters visible.
        let mut cleanup_params: Option<Box<dyn FnOnce(&mut Checker)>> = None;
        let mut cleanup_type_params: Option<Box<dyn FnOnce(&mut Checker)>> = None;
        let (old_enclosing_decl, old_mapper) = {
            let c = ctx.borrow();
            (c.enclosing_declaration, c.mapper)
        };
        if mapper.is_some() {
            ctx.borrow_mut().mapper = mapper;
        }
        if old_enclosing_decl.is_some() && declaration.is_some() {
            // As a performance optimization, reuse the same fake scope within this chain.
            // (See the Go source for the full explanation.)

            // PORT: Go `addAll(add)` calls `add` for each name in order. Here
            // the caller collects the `(name, symbol)` pairs in the same order
            // first, right before the push, so `pushFakeScope` takes the list.
            if expanded_params.is_empty() || !expanded_params.iter().any(|p| p.is_some()) {
                cleanup_params = None;
            } else {
                let mut adds: Vec<(String, SymbolId)> = Vec::new();
                for (p_index, &param) in expanded_params.iter().enumerate() {
                    let original_param = original_parameters
                        .get(p_index)
                        .copied()
                        .unwrap_or(SymbolId::NIL);
                    // PORT: Go `originalParameters != nil`. A nil and an empty slice
                    // are the same here.
                    if !original_parameters.is_empty() && original_param != param {
                        // Can't reference the expanded parameter name, just the original, unless we've expanded the param list for some reason
                        if original_param.is_some() {
                            adds.push((self.sym(original_param).name.to_string(), original_param));
                        }
                    } else {
                        let declarations = self.sym(param).declarations.clone();
                        let mut some = false;
                        for d in declarations {
                            if is_parameter_declaration(d)
                                && d.name().is_some()
                                && is_binding_pattern(d.name())
                            {
                                self.enter_new_scope_bind_pattern(d.name(), &mut adds);
                                some = true;
                                break;
                            }
                        }
                        if !some {
                            adds.push((self.sym(param).name.to_string(), param));
                        }
                    }
                }
                cleanup_params = self.push_fake_scope(b, "params", adds);
            }

            if ctx
                .borrow()
                .flags
                .intersects(NodeBuilderFlags::GENERATE_NAMES_FOR_SHADOWED_TYPE_PARAMS)
                && type_parameters.iter().any(|p| p.is_some())
            {
                let mut adds: Vec<(String, SymbolId)> = Vec::new();
                for &type_param in type_parameters {
                    if type_param.is_nil() {
                        continue;
                    }
                    let type_param_name = self
                        .type_parameter_to_name(b, type_param)
                        .text()
                        .to_string();
                    adds.push((type_param_name, self.ty(type_param).symbol));
                }
                cleanup_type_params = self.push_fake_scope(b, "typeParams", adds);
            }
        }

        Box::new(move |c: &mut Checker| {
            if let Some(cleanup_params) = cleanup_params {
                cleanup_params(c);
            }
            if let Some(cleanup_type_params) = cleanup_type_params {
                cleanup_type_params(c);
            }
            cleanup_context();
            let mut cx = ctx.borrow_mut();
            cx.enclosing_declaration = old_enclosing_decl;
            cx.mapper = old_mapper;
        })
    }

    // Go: checker/nodebuilderscopes.go:184 bindPatternWorker (closure in enterNewScope)
    // PORT: the Go closure returns after the first element it handles, and so
    // does this method.
    fn enter_new_scope_bind_pattern(&mut self, p: Node, adds: &mut Vec<(String, SymbolId)>) {
        if let Some(e) = p.elements().iter().next() {
            match e.kind() {
                SyntaxKind::OmittedExpression => {}
                SyntaxKind::BindingElement => self.enter_new_scope_bind_element(e, adds),
                _ => panic!("Unhandled binding element kind"),
            }
        }
    }

    // Go: checker/nodebuilderscopes.go:198 bindElementWorker (closure in enterNewScope)
    fn enter_new_scope_bind_element(&mut self, e: Node, adds: &mut Vec<(String, SymbolId)>) {
        if e.name().is_some() && is_binding_pattern(e.name()) {
            self.enter_new_scope_bind_pattern(e.name(), adds);
            return;
        }
        let symbol = self.get_symbol_of_declaration(e);
        if symbol.is_some() {
            // omitted expressions are now parsed as nameless binding patterns and also have no symbol
            adds.push((self.sym(symbol).name.to_string(), symbol));
        }
    }

    // Go: checker/nodebuilderscopes.go:97 pushFakeScope (closure in enterNewScope)
    fn push_fake_scope(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        kind: &str,
        adds: Vec<(String, SymbolId)>,
    ) -> Option<Box<dyn FnOnce(&mut Checker)>> {
        let ctx = scopes_ctx(b);
        let enclosing_declaration = ctx.borrow().enclosing_declaration;
        // We only ever need to look two declarations upward.
        go_assert!(enclosing_declaration.is_some());
        let mut existing_fake_scope = Node::NIL;
        if is_fake_scope_of_kind(b, enclosing_declaration, kind) {
            existing_fake_scope = enclosing_declaration;
        }
        if existing_fake_scope.is_nil()
            && enclosing_declaration.parent().is_some()
            && is_fake_scope_of_kind(b, enclosing_declaration.parent(), kind)
        {
            existing_fake_scope = enclosing_declaration.parent();
        }
        go_assert!(existing_fake_scope.is_nil() || is_block(existing_fake_scope));

        let mut locals = SymbolTable::NIL;
        if existing_fake_scope.is_some() {
            locals = existing_fake_scope.locals();
        }
        if locals.is_nil() {
            locals = self.symbols.new_table();
        }
        let mut new_locals: Vec<String> = Vec::new();
        let mut old_locals: Vec<LocalsRecord> = Vec::new();
        for (name, symbol) in adds {
            // Add cleanup information only if we don't own the fake scope
            if existing_fake_scope.is_some() {
                let old_symbol = self.symbols.get(locals, &name);
                if old_symbol.is_nil() {
                    new_locals.push(name.clone());
                } else {
                    old_locals.push(LocalsRecord {
                        name: name.clone(),
                        old_symbol,
                    });
                }
            }
            self.symbols.set(locals, name, symbol);
        }

        if existing_fake_scope.is_nil() {
            // Use a Block for this; the type of the node doesn't matter so long as it
            // has locals, and this is cheaper/easier than using a function-ish Node.
            let fake_scope = {
                let bi = b.borrow();
                let f = bi.f();
                f.new_block(f.new_node_list(&[]), false)
            };
            b.borrow_mut()
                .links
                .get(fake_scope)
                .fake_scope_for_signature_declaration = Some(kind.to_string());
            set_node_locals(fake_scope, locals);
            set_node_parent(fake_scope, enclosing_declaration);
            ctx.borrow_mut().enclosing_declaration = fake_scope;
            None
        } else {
            // We did not create the current scope, so we have to clean it up
            let undo = move |c: &mut Checker| {
                for s in &new_locals {
                    c.symbols.delete(locals, s);
                }
                for s in old_locals {
                    c.symbols.set(locals, s.name, s.old_symbol);
                }
            };
            Some(Box::new(undo))
        }
    }
}

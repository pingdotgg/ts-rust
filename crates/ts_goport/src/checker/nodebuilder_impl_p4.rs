//! Port of checker/nodebuilderimpl.go lines 3056 to 3552: visitAndTransformType,
//! typeToTypeNode and the small node helpers that follow it.
//!
//! Relater pattern (printer plan contract item 8): NodeBuilderImpl methods are
//! `impl Checker` methods that take `b: &Rc<RefCell<NodeBuilderImpl>>`. No
//! `RefCell` borrow is held across a Checker call.

use crate::prelude::*;
use crate::printer::{EmitContext, EmitFlags};

// PORT: private accessors for NodeBuilderImpl fields. They keep the field
// shapes (owned by U11) in one place. Clone the handle out, then drop the
// borrow before any Checker call.
fn nb_ctx(b: &Rc<RefCell<NodeBuilderImpl>>) -> Rc<RefCell<NodeBuilderContext>> {
    b.borrow().ctx.clone()
}

fn nb_e(b: &Rc<RefCell<NodeBuilderImpl>>) -> Rc<EmitContext> {
    b.borrow().e.clone()
}

fn nb_tracker(b: &Rc<RefCell<NodeBuilderImpl>>) -> Rc<dyn SymbolTracker> {
    let ctx = nb_ctx(b);
    let tracker = ctx.borrow().tracker.clone();
    tracker
}

fn nb_flags(b: &Rc<RefCell<NodeBuilderImpl>>) -> NodeBuilderFlags {
    let ctx = nb_ctx(b);
    let flags = ctx.borrow().flags;
    flags
}

fn nb_add_length(b: &Rc<RefCell<NodeBuilderImpl>>, n: i32) {
    nb_ctx(b).borrow_mut().approximate_length += n;
}

/// Go `transform func(b *NodeBuilderImpl, t *Type) *ast.TypeNode`.
pub type NodeBuilderTypeTransform = fn(&mut Checker, &Rc<RefCell<NodeBuilderImpl>>, TypeId) -> Node;

impl Checker {
    // Go: checker/nodebuilderimpl.go:3212 visitAndTransformType
    pub fn visit_and_transform_type(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        t: TypeId,
        transform: NodeBuilderTypeTransform,
    ) -> Node {
        // ts#63969 (Go N' nodebuilderimpl.go:3213): past the truncation length,
        // elide before the declared type cache can clone a cached node.
        if self.check_truncation_length(b) {
            return self.create_elided_information_placeholder(b);
        }

        let ctx = nb_ctx(b);
        let mut type_id = t;
        // ts#64556 (Go N' nodebuilderimpl.go:3240)
        let is_array_or_tuple = self.is_array_or_tuple_type(t);
        if is_array_or_tuple {
            // Deferred and regular references share a cycle identity.
            let target = self.ty(t).target();
            let type_arguments = self.get_type_arguments(t).to_vec();
            type_id = self.create_type_reference(target, &type_arguments);
        }
        if ctx.borrow().visited_types.contains(&type_id) {
            return self.create_cyclic_structure_placeholder(b);
        }

        let (t_flags, t_object_flags, t_symbol) = {
            let ty = self.ty(t);
            (ty.flags, ty.object_flags, ty.symbol)
        };
        let is_constructor_object = t_object_flags.intersects(ObjectFlags::ANONYMOUS)
            && t_symbol.is_some()
            && self.sym(t_symbol).flags.intersects(SymbolFlags::CLASS);
        let id: Option<CompositeSymbolIdentity> = if is_array_or_tuple {
            // Do not bound finite container nesting by the shared Array symbol or tuple origin.
            None
        } else if t_object_flags.intersects(ObjectFlags::REFERENCE)
            && self.ty(t).as_type_reference().node.is_some()
        {
            Some(CompositeSymbolIdentity {
                is_constructor_node: false,
                symbol_id: 0,
                node_id: get_node_id(self.ty(t).as_type_reference().node),
            })
        } else if t_flags.intersects(TypeFlags::CONDITIONAL) {
            let root_node = self.ty(t).as_conditional_type().root.borrow().node;
            Some(CompositeSymbolIdentity {
                is_constructor_node: false,
                symbol_id: 0,
                node_id: get_node_id(root_node),
            })
        } else if t_symbol.is_some() {
            Some(CompositeSymbolIdentity {
                is_constructor_node: is_constructor_object,
                symbol_id: get_symbol_id(&self.symbols, t_symbol),
                node_id: 0,
            })
        } else {
            None
        };
        // Since instantiations of the same anonymous type have the same symbol, tracking symbols instead
        // of types allows us to catch circular references to instantiations of the same anonymous type

        let (key, can_use_cache, enclosing_declaration) = {
            let c = ctx.borrow();
            (
                CompositeTypeCacheIdentity {
                    type_id,
                    flags: c.flags,
                    internal_flags: c.internal_flags,
                    // ts#64556 (Go N' nodebuilderimpl.go:3273)
                    infer_type_parameters: if c.infer_type_parameters.is_empty() {
                        CacheHashKey::default()
                    } else {
                        get_type_list_key(&c.infer_type_parameters)
                    },
                },
                // Don't rely on type cache if we're expanding a type, because we need to compute `canIncreaseExpansionDepth`.
                c.max_expansion_depth < 0,
                c.enclosing_declaration,
            )
        };
        if can_use_cache
            && enclosing_declaration.is_some()
            && b.borrow().links.has(enclosing_declaration)
        {
            let cached_result = b
                .borrow_mut()
                .links
                .get(enclosing_declaration)
                .serialized_types
                .get(&key)
                .cloned();
            if let Some(cached_result) = cached_result {
                // TODO:: check if we instead store late painted statements associated with this?
                let tracker = nb_tracker(b);
                for arg in &cached_result.tracked_symbols {
                    tracker.track_symbol(self, arg.symbol, arg.enclosing_declaration, arg.meaning);
                }
                {
                    let mut c = ctx.borrow_mut();
                    if cached_result.truncating {
                        c.truncating = true;
                    }
                    c.approximate_length += cached_result.added_length;
                }
                let e = nb_e(b);
                return e.factory().deep_clone_node(cached_result.node);
            }
        }

        // ts#64558 (Go N' nodebuilderimpl.go:3293)
        // PORT: Go restores the origin depth in a `defer`; the port restores
        // it on each return below (`restore_origin`).
        let mut origin_depth: Option<(CompositeSymbolIdentity, i32)> = None;
        if t_object_flags.intersects(ObjectFlags::REVERSE_MAPPED) {
            // Growing type arguments can prevent a reverse mapped type from repeating.
            // Bound expansion by its mapped declaration as well as its type identity.
            let mapped_type = self.ty(t).as_reverse_mapped_type().mapped_type;
            let declaration = self.ty(mapped_type).as_mapped_type().declaration;
            let origin = CompositeSymbolIdentity {
                is_constructor_node: false,
                symbol_id: 0,
                node_id: get_node_id(declaration),
            };
            let depth = ctx.borrow().symbol_depth.get(&origin).copied().unwrap_or(0);
            if depth >= 100 {
                ctx.borrow_mut().truncating = true;
                return self.create_elided_information_placeholder(b);
            }
            ctx.borrow_mut().symbol_depth.insert(origin, depth + 1);
            origin_depth = Some((origin, depth));
        }
        let restore_origin = |ctx: &Rc<RefCell<NodeBuilderContext>>| {
            if let Some((origin, depth)) = origin_depth {
                ctx.borrow_mut().symbol_depth.insert(origin, depth);
            }
        };

        let mut depth = 0;
        if let Some(id) = id {
            depth = ctx.borrow().symbol_depth.get(&id).copied().unwrap_or(0);
            if depth > 10 {
                // ts#64461 (Go N' nodebuilderimpl.go:3310): the depth limit truncates.
                ctx.borrow_mut().truncating = true;
                let result = self.create_elided_information_placeholder(b);
                restore_origin(&ctx);
                return result;
            }
            ctx.borrow_mut().symbol_depth.insert(id, depth + 1);
        }
        let (prev_tracked_symbols, start_length) = {
            let mut c = ctx.borrow_mut();
            c.visited_types.insert(type_id);
            let prev = std::mem::take(&mut c.tracked_symbols);
            (prev, c.approximate_length)
        };
        let result = transform(self, b, t);
        let (
            added_length,
            reported_diagnostic,
            encountered_error,
            truncating,
            tracked_symbols,
            enclosing_declaration,
        ) = {
            let c = ctx.borrow();
            (
                c.approximate_length - start_length,
                c.reported_diagnostic,
                c.encountered_error,
                c.truncating,
                c.tracked_symbols.clone(),
                c.enclosing_declaration,
            )
        };
        if can_use_cache && !reported_diagnostic && !encountered_error {
            // PORT: Go creates the map when nil; the Rust map always exists.
            b.borrow_mut()
                .links
                .get(enclosing_declaration)
                .serialized_types
                .insert(
                    key,
                    SerializedTypeEntry {
                        node: result,
                        truncating,
                        added_length,
                        tracked_symbols,
                    },
                );
        }
        {
            let mut c = ctx.borrow_mut();
            c.visited_types.remove(&type_id);
            if let Some(id) = id {
                c.symbol_depth.insert(id, depth);
            }
            c.tracked_symbols = prev_tracked_symbols;
        }
        restore_origin(&ctx);
        result

        // !!! TODO: Attempt node reuse or parse nodes to minimize copying once text range setting is set up
        // (Go keeps a commented-out deepCloneOrReuseNode sketch here.)
    }

    // Go: checker/nodebuilderimpl.go:3299 typeToTypeNode
    pub fn type_to_type_node(&mut self, b: &Rc<RefCell<NodeBuilderImpl>>, t: TypeId) -> Node {
        let ctx = nb_ctx(b);
        // PORT: Go uses two defers (typeStack pop after the push in the body,
        // and depth-- in the alias branch). The body reports whether it pushed
        // and whether it took the depth++ path, and the deferred work runs
        // here in Go's LIFO order.
        let mut pushed = false;
        let mut depth_incremented = false;
        let result = self.type_to_type_node_body(b, t, &mut pushed, &mut depth_incremented);
        if depth_incremented {
            ctx.borrow_mut().depth -= 1;
        }
        if pushed {
            ctx.borrow_mut().type_stack.pop();
        }
        result
    }

    // PORT: body of Go typeToTypeNode (nodebuilderimpl.go:3143), split out so
    // the Go defers can run in `type_to_type_node`.
    fn type_to_type_node_body(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        mut t: TypeId,
        pushed: &mut bool,
        depth_incremented: &mut bool,
    ) -> Node {
        let ctx = nb_ctx(b);
        let e = nb_e(b);
        let f: &NodeFactory = e.factory();

        let in_type_alias = {
            let mut c = ctx.borrow_mut();
            let in_type_alias = c.flags & NodeBuilderFlags::IN_TYPE_ALIAS;
            c.flags = c.flags.without(NodeBuilderFlags::IN_TYPE_ALIAS);
            in_type_alias
        };

        if t.is_nil() {
            let mut c = ctx.borrow_mut();
            if !c
                .flags
                .intersects(NodeBuilderFlags::ALLOW_EMPTY_UNION_OR_INTERSECTION)
            {
                c.encountered_error = true;
                return Node::NIL;
                // TODO: GH#18217
            }
            c.approximate_length += 3;
            return f.new_keyword_type_node(SyntaxKind::AnyKeyword);
        }

        t = self.get_non_distributed_type_parameter(t);

        // Push type onto typeStack for expansion depth tracking
        {
            let mut c = ctx.borrow_mut();
            if c.max_expansion_depth >= 0 {
                c.type_stack.push(t);
                *pushed = true;
            }
        }

        if !nb_flags(b).intersects(NodeBuilderFlags::NO_TYPE_REDUCTION) {
            t = self.get_reduced_type(t);
        }

        let t_flags = self.ty(t).flags;
        let t_alias = self.ty(t).alias.clone();

        if t_flags.intersects(TypeFlags::ANY) {
            if let Some(alias) = &t_alias {
                return self.type_alias_to_type_reference_node(b, alias);
            }
            if t == self.unresolved_type {
                return e.add_synthetic_leading_comment(
                    f.new_keyword_type_node(SyntaxKind::AnyKeyword),
                    SyntaxKind::MultiLineCommentTrivia,
                    "unresolved",
                    false, /*hasTrailingNewLine*/
                );
            }
            nb_add_length(b, 3);
            return f.new_keyword_type_node(if t == self.intrinsic_marker_type {
                SyntaxKind::IntrinsicKeyword
            } else {
                SyntaxKind::AnyKeyword
            });
        }
        if t_flags.intersects(TypeFlags::UNKNOWN) {
            return f.new_keyword_type_node(SyntaxKind::UnknownKeyword);
        }
        if t_flags.intersects(TypeFlags::STRING) {
            nb_add_length(b, 6);
            return f.new_keyword_type_node(SyntaxKind::StringKeyword);
        }
        if t_flags.intersects(TypeFlags::NUMBER) {
            nb_add_length(b, 6);
            return f.new_keyword_type_node(SyntaxKind::NumberKeyword);
        }
        if t_flags.intersects(TypeFlags::BIG_INT) {
            nb_add_length(b, 6);
            return f.new_keyword_type_node(SyntaxKind::BigIntKeyword);
        }
        if t_flags.intersects(TypeFlags::BOOLEAN) && t_alias.is_none() {
            nb_add_length(b, 7);
            return f.new_keyword_type_node(SyntaxKind::BooleanKeyword);
        }
        let mut expanding_enum = false;
        if t_flags.intersects(TypeFlags::ENUM_LIKE) {
            let t_symbol = self.ty(t).symbol;
            if self
                .sym(t_symbol)
                .flags
                .intersects(SymbolFlags::ENUM_MEMBER)
            {
                let parent_symbol = self.get_parent_of_symbol(t_symbol);
                let parent_name =
                    self.symbol_to_type_node(b, parent_symbol, SymbolFlags::TYPE, NodeList::NIL);
                if self.get_declared_type_of_symbol(parent_symbol) == t {
                    return parent_name;
                }
                let member_name = symbol_name(&self.symbols, t_symbol);
                if is_identifier_text(&member_name, LanguageVariant::STANDARD) {
                    let reference = f.new_type_reference_node(
                        f.new_identifier(member_name),
                        NodeList::NIL, /*typeArguments*/
                    );
                    return self.append_reference_to_type(
                        b,
                        parent_name, /* as TypeReference | ImportTypeNode */
                        reference,
                    );
                }
                if is_import_type_node(parent_name) {
                    // PORT: Go sets `IsTypeOf = true` in place ("node is freshly
                    // manufactured anyhow"). Synthetic nodes are immutable here, so
                    // rebuild the import type node with isTypeOf set, and keep its
                    // parent, loc and flags.
                    let rebuilt = f.new_import_type_node(
                        true,
                        parent_name.argument(),
                        parent_name.attributes(),
                        parent_name.qualifier(),
                        parent_name.type_argument_list(),
                    );
                    set_node_loc(rebuilt, parent_name.loc());
                    set_node_flags(rebuilt, parent_name.flags());
                    set_node_parent(rebuilt, parent_name.parent());
                    // mutably update, node is freshly manufactured anyhow
                    let lit = self.nb_new_string_literal(b, &member_name);
                    return f.new_indexed_access_type_node(rebuilt, f.new_literal_type_node(lit));
                } else if is_type_reference_node(parent_name) {
                    let lit = self.nb_new_string_literal(b, &member_name);
                    return f.new_indexed_access_type_node(
                        f.new_type_query_node(parent_name.type_name(), NodeList::NIL),
                        f.new_literal_type_node(lit),
                    );
                } else {
                    panic!("Unhandled type node kind returned from `symbolToTypeNode`.");
                }
            }
            if !t_flags.intersects(TypeFlags::UNION)
                || !self.should_expand_type(b, t, false /*isAlias*/)
            {
                let t_symbol = self.ty(t).symbol;
                return self.symbol_to_type_node(b, t_symbol, SymbolFlags::TYPE, NodeList::NIL);
            }
            expanding_enum = true;
        }
        if t_flags.intersects(TypeFlags::STRING_LITERAL) {
            let value = match &self.ty(t).as_literal_type().value {
                Some(LiteralValue::String(s)) => s.clone(),
                _ => panic!("interface conversion: literal value is not string"),
            };
            // PORT: Go `len` counts Go bytes (see `scanner_util::go_len`).
            nb_add_length(b, go_len(&value) as i32 + 2);
            let lit = self.nb_new_string_literal(b, &value);
            e.add_emit_flags(lit, EmitFlags::NO_ASCII_ESCAPING);
            return f.new_literal_type_node(lit);
        }
        if t_flags.intersects(TypeFlags::NUMBER_LITERAL) {
            let value = match &self.ty(t).as_literal_type().value {
                Some(LiteralValue::Number(n)) => *n,
                _ => panic!("interface conversion: literal value is not jsnum.Number"),
            };
            let text = value.to_string();
            nb_add_length(b, text.len() as i32);
            if value.0 < 0.0 {
                return f.new_literal_type_node(f.new_prefix_unary_expression(
                    SyntaxKind::MinusToken,
                    f.new_numeric_literal(text[1..].to_string(), TokenFlags::NONE),
                ));
            } else {
                return f.new_literal_type_node(f.new_numeric_literal(text, TokenFlags::NONE));
            }
        }
        if t_flags.intersects(TypeFlags::BIG_INT_LITERAL) {
            let text = pseudo_big_int_to_string(&self.get_big_int_literal_value(t));
            nb_add_length(b, text.len() as i32 + 1);
            return f.new_literal_type_node(f.new_big_int_literal(text + "n", TokenFlags::NONE));
        }
        if t_flags.intersects(TypeFlags::BOOLEAN_LITERAL) {
            let value = match &self.ty(t).as_literal_type().value {
                Some(LiteralValue::Bool(v)) => *v,
                _ => panic!("interface conversion: literal value is not bool"),
            };
            if value {
                nb_add_length(b, 4);
                return f.new_literal_type_node(f.new_keyword_expression(SyntaxKind::TrueKeyword));
            } else {
                nb_add_length(b, 5);
                return f.new_literal_type_node(f.new_keyword_expression(SyntaxKind::FalseKeyword));
            }
        }
        if t_flags.intersects(TypeFlags::UNIQUE_ES_SYMBOL) {
            if !nb_flags(b).intersects(NodeBuilderFlags::ALLOW_UNIQUE_ES_SYMBOL_TYPE) {
                let t_symbol = self.ty(t).symbol;
                let enclosing = ctx.borrow().enclosing_declaration;
                if self.is_value_symbol_accessible(t_symbol, enclosing) {
                    nb_add_length(b, 6);
                    return self.symbol_to_type_node(
                        b,
                        t_symbol,
                        SymbolFlags::VALUE,
                        NodeList::NIL,
                    );
                }
                nb_tracker(b).report_inaccessible_unique_symbol_error(self);
            }
            nb_add_length(b, 13);
            return f.new_type_operator_node(
                SyntaxKind::UniqueKeyword,
                f.new_keyword_type_node(SyntaxKind::SymbolKeyword),
            );
        }
        if t_flags.intersects(TypeFlags::VOID) {
            nb_add_length(b, 4);
            return f.new_keyword_type_node(SyntaxKind::VoidKeyword);
        }
        if t_flags.intersects(TypeFlags::UNDEFINED) {
            nb_add_length(b, 9);
            return f.new_keyword_type_node(SyntaxKind::UndefinedKeyword);
        }
        if t_flags.intersects(TypeFlags::NULL) {
            nb_add_length(b, 4);
            return f.new_literal_type_node(f.new_keyword_expression(SyntaxKind::NullKeyword));
        }
        if t_flags.intersects(TypeFlags::NEVER) {
            nb_add_length(b, 5);
            return f.new_keyword_type_node(SyntaxKind::NeverKeyword);
        }
        if t_flags.intersects(TypeFlags::ES_SYMBOL) {
            nb_add_length(b, 6);
            return f.new_keyword_type_node(SyntaxKind::SymbolKeyword);
        }
        if t_flags.intersects(TypeFlags::NON_PRIMITIVE) {
            nb_add_length(b, 6);
            return f.new_keyword_type_node(SyntaxKind::ObjectKeyword);
        }
        if self.is_this_type_parameter(t) {
            if nb_flags(b).intersects(NodeBuilderFlags::IN_OBJECT_TYPE_LITERAL) {
                {
                    let mut c = ctx.borrow_mut();
                    if !c.encountered_error
                        && !c
                            .flags
                            .intersects(NodeBuilderFlags::ALLOW_THIS_IN_OBJECT_LITERAL)
                    {
                        c.encountered_error = true;
                    }
                }
                nb_tracker(b).report_inaccessible_this_error(self);
            }
            nb_add_length(b, 4);
            return f.new_this_type_node();
        }

        if in_type_alias.is_empty() {
            if let Some(alias) = &t_alias {
                let use_outside = nb_flags(b)
                    .intersects(NodeBuilderFlags::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE);
                let accessible = use_outside || {
                    let enclosing = ctx.borrow().enclosing_declaration;
                    self.is_type_symbol_accessible(alias.symbol(), enclosing)
                };
                if accessible {
                    // If we should expand this type alias, skip the alias and fall through to expand the underlying type
                    if !self.should_expand_type(b, t, true /*isAlias*/) {
                        let sym = alias.symbol();
                        let type_argument_nodes = self.map_to_type_nodes(
                            b,
                            alias.type_arguments(),
                            false, /*isBareList*/
                        );
                        if is_reserved_member_name(&self.sym(sym).name)
                            && !self.sym(sym).flags.intersects(SymbolFlags::CLASS)
                        {
                            return f.new_type_reference_node(
                                f.new_identifier(""),
                                type_argument_nodes,
                            );
                        }
                        let global_array_symbol = self.ty(self.global_array_type).symbol;
                        if type_argument_nodes.is_some()
                            && type_argument_nodes.nodes().len() == 1
                            && sym == global_array_symbol
                        {
                            return f.new_array_type_node(type_argument_nodes.nodes().get(0));
                        }
                        return self.symbol_to_type_node(
                            b,
                            sym,
                            SymbolFlags::TYPE,
                            type_argument_nodes,
                        );
                    }
                    // Expanding: increment depth and process the underlying type
                    ctx.borrow_mut().depth += 1;
                    *depth_incremented = true;
                }
            }
        }

        let object_flags = self.ty(t).object_flags;

        if object_flags.intersects(ObjectFlags::REFERENCE) {
            debug_assert!(self.ty(t).flags.intersects(TypeFlags::OBJECT));
            // When expanding, expand type references to their structural form
            if self.should_expand_type(b, t, false /*isAlias*/) {
                ctx.borrow_mut().depth += 1;
                let result = self.create_anonymous_type_node_ex(
                    b, t, true, /*forceClassExpansion*/
                    true, /*forceExpansion*/
                );
                ctx.borrow_mut().depth -= 1;
                return result;
            }
            // ts#64556 (Go N' nodebuilderimpl.go:3545)
            if self.is_array_or_tuple_type(t) {
                return self.visit_and_transform_type(b, t, Checker::array_or_tuple_type_to_node);
            } else if self.ty(t).as_type_reference().node.is_some() {
                return self.visit_and_transform_type(b, t, Checker::type_reference_to_type_node);
            } else {
                return self.type_reference_to_type_node(b, t);
            }
        }
        if t_flags.intersects(TypeFlags::TYPE_PARAMETER)
            || object_flags.intersects(ObjectFlags::CLASS_OR_INTERFACE)
        {
            // When expanding class or interface types, show their structural form
            if object_flags.intersects(ObjectFlags::CLASS_OR_INTERFACE)
                && self.should_expand_type(b, t, false /*isAlias*/)
            {
                ctx.borrow_mut().depth += 1;
                let result = self.create_anonymous_type_node_ex(
                    b, t, true, /*forceClassExpansion*/
                    true, /*forceExpansion*/
                );
                ctx.borrow_mut().depth -= 1;
                return result;
            }
            let t_symbol = self.ty(t).symbol;
            if t_flags.intersects(TypeFlags::TYPE_PARAMETER)
                && ctx.borrow().infer_type_parameters.contains(&t)
            {
                nb_add_length(b, go_len(&symbol_name(&self.symbols, t_symbol)) as i32 + 6);
                let mut constraint_node = Node::NIL;
                let constraint = self.get_constraint_of_type_parameter(t);
                if constraint.is_some() {
                    // If the infer type has a constraint that is not the same as the constraint
                    // we would have normally inferred based on b, we emit the constraint
                    // using `infer T extends ?`. We omit inferred constraints from type references
                    // as they may be elided.
                    let inferred_constraint = self.get_inferred_type_parameter_constraint(
                        t, true, /*omitTypeReferences*/
                    );
                    if !(inferred_constraint.is_some()
                        && self.is_type_identical_to(constraint, inferred_constraint))
                    {
                        nb_add_length(b, 9);
                        constraint_node = self.type_to_type_node(b, constraint);
                    }
                }
                let decl =
                    self.type_parameter_to_declaration_with_constraint(b, t, constraint_node);
                return f.new_infer_type_node(decl);
            }
            if nb_flags(b).intersects(NodeBuilderFlags::GENERATE_NAMES_FOR_SHADOWED_TYPE_PARAMS)
                && t_flags.intersects(TypeFlags::TYPE_PARAMETER)
            {
                let name = self.type_parameter_to_name(b, t);
                let text = name.text();
                nb_add_length(b, go_len(text) as i32);
                let id = self.nb_new_identifier(b, text, t_symbol);
                return f.new_type_reference_node(id, NodeList::NIL /*typeArguments*/);
            }
            // Ignore constraint/default when creating a usage (as opposed to declaration) of a type parameter.
            if t_symbol.is_some() {
                return self.symbol_to_type_node(b, t_symbol, SymbolFlags::TYPE, NodeList::NIL);
            }
            let variance_symbol = if self.variance_type_parameter.is_some() {
                self.ty(self.variance_type_parameter).symbol
            } else {
                SymbolId::NIL
            };
            let name = if (t == self.marker_super_type_for_check
                || t == self.marker_sub_type_for_check)
                && self.variance_type_parameter.is_some()
                && variance_symbol.is_some()
            {
                (if t == self.marker_sub_type_for_check {
                    "sub-"
                } else {
                    "super-"
                })
                .to_string()
                    + &symbol_name(&self.symbols, variance_symbol)
            } else {
                "?".to_string()
            };
            let id = self.nb_new_identifier(b, &name, SymbolId::NIL /*symbol*/);
            return f.new_type_reference_node(id, NodeList::NIL /*typeArguments*/);
        }
        if t_flags.intersects(TypeFlags::UNION) && self.ty(t).as_union_type().origin.is_some() {
            t = self.ty(t).as_union_type().origin;
        }
        let t_flags = self.ty(t).flags;
        if t_flags.intersects(TypeFlags::UNION | TypeFlags::INTERSECTION) {
            let types: Vec<TypeId> = if t_flags.intersects(TypeFlags::UNION) {
                let members = self.ty(t).types_list();
                self.format_union_types(&members, expanding_enum)
            } else {
                self.ty(t).types_list().to_vec()
            };
            if types.len() == 1 {
                return self.type_to_type_node(b, types[0]);
            }
            let type_nodes = self.map_to_type_nodes(b, &types, true /*isBareList*/);
            if type_nodes.is_some() && !type_nodes.nodes().is_empty() {
                if t_flags.intersects(TypeFlags::UNION) {
                    return f.new_union_type_node(type_nodes);
                } else {
                    return f.new_intersection_type_node(type_nodes);
                }
            } else {
                let mut c = ctx.borrow_mut();
                if !c.encountered_error
                    && !c
                        .flags
                        .intersects(NodeBuilderFlags::ALLOW_EMPTY_UNION_OR_INTERSECTION)
                {
                    c.encountered_error = true;
                }
                return Node::NIL;
                // TODO: GH#18217
            }
        }
        if object_flags.intersects(ObjectFlags::ANONYMOUS | ObjectFlags::MAPPED) {
            debug_assert!(self.ty(t).flags.intersects(TypeFlags::OBJECT));
            // The type is an object literal type.
            return self.create_anonymous_type_node(b, t);
        }
        if t_flags.intersects(TypeFlags::INDEX) {
            let indexed_type = self.ty(t).target();
            nb_add_length(b, 6);
            let index_type_node = self.type_to_type_node(b, indexed_type);
            return f.new_type_operator_node(SyntaxKind::KeyOfKeyword, index_type_node);
        }
        if t_flags.intersects(TypeFlags::TEMPLATE_LITERAL) {
            let (texts, types) = {
                let tl = self.ty(t).as_template_literal_type();
                (tl.texts.clone(), tl.types.clone())
            };
            let template_head = f.new_template_head(texts[0].clone(), "", TokenFlags::NONE);
            e.add_emit_flags(template_head, EmitFlags::NO_ASCII_ESCAPING);
            let mut spans = Vec::with_capacity(types.len());
            for (i, &ty) in types.iter().enumerate() {
                let res = if i < types.len() - 1 {
                    f.new_template_middle(texts[i + 1].clone(), "", TokenFlags::NONE)
                } else {
                    f.new_template_tail(texts[i + 1].clone(), "", TokenFlags::NONE)
                };
                e.add_emit_flags(res, EmitFlags::NO_ASCII_ESCAPING);
                let type_node = self.type_to_type_node(b, ty);
                spans.push(f.new_template_literal_type_span(type_node, res));
            }
            let template_spans = f.new_node_list(&spans);
            nb_add_length(b, 2);
            return f.new_template_literal_type_node(template_head, template_spans);
        }
        if t_flags.intersects(TypeFlags::STRING_MAPPING) {
            let target = self.ty(t).target();
            let type_node = self.type_to_type_node(b, target);
            // PORT: Go `t.AsStringMappingType().symbol` is the type's symbol.
            let mapping_symbol = self.ty(t).symbol;
            return self.symbol_to_type_node(
                b,
                mapping_symbol,
                SymbolFlags::TYPE,
                f.new_node_list(&[type_node]),
            );
        }
        if t_flags.intersects(TypeFlags::INDEXED_ACCESS) {
            let (object_type, index_type) = {
                let ia = self.ty(t).as_indexed_access_type();
                (ia.object_type, ia.index_type)
            };
            let object_type_node = self.type_to_type_node(b, object_type);
            let index_type_node = self.type_to_type_node(b, index_type);
            nb_add_length(b, 2);
            return f.new_indexed_access_type_node(object_type_node, index_type_node);
        }
        if t_flags.intersects(TypeFlags::CONDITIONAL) {
            return self.visit_and_transform_type(b, t, Checker::conditional_type_to_type_node);
        }
        if t_flags.intersects(TypeFlags::SUBSTITUTION) {
            let base_type = self.ty(t).as_substitution_type().base_type;
            let type_node = self.type_to_type_node(b, base_type);
            if !self.is_no_infer_type(t) {
                return type_node;
            }
            let no_infer_symbol = self.get_global_type_alias_symbol("NoInfer", 1, false);
            if no_infer_symbol.is_some() {
                return self.symbol_to_type_node(
                    b,
                    no_infer_symbol,
                    SymbolFlags::TYPE,
                    f.new_node_list(&[type_node]),
                );
            } else {
                return type_node;
            }
        }

        panic!("Should be unreachable.");
    }

    // Go: checker/nodebuilderimpl.go:3620 newStringLiteral
    // PORT: named `nb_new_string_literal` so it does not read like a factory call.
    pub fn nb_new_string_literal(&mut self, b: &Rc<RefCell<NodeBuilderImpl>>, text: &str) -> Node {
        self.nb_new_string_literal_ex(b, text, false /*isSingleQuote*/)
    }

    // Go: checker/nodebuilderimpl.go:3624 newStringLiteralEx
    pub fn nb_new_string_literal_ex(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        text: &str,
        is_single_quote: bool,
    ) -> Node {
        let mut flags = TokenFlags::NONE;
        if is_single_quote
            || nb_flags(b).intersects(NodeBuilderFlags::USE_SINGLE_QUOTES_FOR_STRING_LITERAL_TYPE)
        {
            flags |= TokenFlags::SINGLE_QUOTE;
        }
        let e = nb_e(b);
        e.factory().new_string_literal(text, flags)
    }

    // Direct serialization core functions for types, type aliases, and symbols

    // Go: checker/nodebuilderimpl.go:3635 TypeAlias.ToTypeReferenceNode
    // PORT: a method on TypeAlias in Go; here a Checker method that takes the alias.
    pub fn type_alias_to_type_reference_node(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        alias: &TypeAlias,
    ) -> Node {
        let name = self.symbol_to_entity_name_node(b, alias.symbol());
        let type_arguments =
            self.map_to_type_nodes(b, alias.type_arguments(), false /*isBareList*/);
        let e = nb_e(b);
        e.factory().new_type_reference_node(name, type_arguments)
    }

    // Go: checker/nodebuilderimpl.go:3639 newIdentifier
    // PORT: named `nb_new_identifier` so it does not read like a factory call.
    pub fn nb_new_identifier(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        text: &str,
        symbol: SymbolId,
    ) -> Node {
        let e = nb_e(b);
        let id = e.factory().new_identifier(text);
        if symbol.is_some() {
            b.borrow_mut().id_to_symbol.insert(id, symbol);
        }
        id
    }

    // Go: checker/nodebuilderimpl.go:3647 createAccessExpression
    pub fn create_access_expression(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        node: Node,
    ) -> Node {
        let e = nb_e(b);
        let f: &NodeFactory = e.factory();
        if is_qualified_name(node) {
            let left = self.create_access_expression(b, node.left());
            f.new_property_access_expression(
                left,
                Node::NIL, /*questionDotToken*/
                f.deep_clone_node(node.right()),
                NodeFlags::NONE,
            )
        } else if is_identifier(node)
            || is_property_access_expression(node)
            || is_expression_with_type_arguments(node)
        {
            f.deep_clone_node(node)
        } else {
            panic!("unexpected access node kind: {:?}", node.kind())
        }
    }

    // Go: checker/nodebuilderimpl.go:3658 createExpressionWithTypeArguments
    pub fn create_expression_with_type_arguments(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        expr: Node,
        type_arguments: NodeList,
    ) -> Node {
        if type_arguments.is_nil() || type_arguments.nodes().is_empty() {
            return expr;
        }
        let e = nb_e(b);
        e.factory()
            .new_expression_with_type_arguments(expr, type_arguments)
    }

    // Go: checker/nodebuilderimpl.go:3665 lookupInstantiatedTypeArgumentNodes
    pub fn lookup_instantiated_type_argument_nodes(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        chain: &[SymbolId],
        index: usize,
    ) -> NodeList {
        if self.should_write_type_parameters_in_qualified_name(b, chain, index) {
            let symbol = chain[index];
            let next_symbol = chain[index + 1];
            if !self
                .sym(next_symbol)
                .check_flags
                .intersects(CheckFlags::INSTANTIATED)
            {
                return NodeList::NIL;
            }

            let mut target_symbol = symbol;
            if self.sym(symbol).flags.intersects(SymbolFlags::ALIAS)
                && !self.can_get_type_parameters_of_class_or_interface(symbol)
            {
                target_symbol = self.resolve_alias(symbol);
            }

            if !self.can_get_type_parameters_of_class_or_interface(target_symbol) {
                return NodeList::NIL;
            }

            let mut params = self.get_type_parameters_of_class_or_interface(b, target_symbol);
            let target_mapper = self
                .value_symbol_links
                .get_by_id(&self.symbols, next_symbol)
                .mapper;
            if target_mapper.is_some() {
                params = params
                    .into_iter()
                    .map(|p| self.mapper_map(target_mapper, p))
                    .collect();
            }
            return self.map_to_type_nodes(b, &params, false /*isBareList*/);
        }
        NodeList::NIL
    }

    // Go: checker/nodebuilderimpl.go:3692 lookupExpressionChainTypeArgumentNodes
    pub fn lookup_expression_chain_type_argument_nodes(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        chain: &[SymbolId],
        index: usize,
    ) -> NodeList {
        if self.should_write_type_parameters_in_qualified_name(b, chain, index) {
            let symbol = chain[index];
            // PORT: `typeParameterSymbolList` is keyed by `SymbolId` for Go
            // `ast.GetSymbolId(symbol)`. The two are one to one. The Go call
            // gives the symbol its id, so it is made too (`ValueSymbolLinkStore`).
            get_symbol_id(&self.symbols, symbol);
            let ctx = nb_ctx(b);
            if ctx.borrow().type_parameter_symbol_list.has(&symbol) {
                return NodeList::NIL;
            }

            ctx.borrow_mut().type_parameter_symbol_list.add(symbol);
            let type_argument_nodes = self.lookup_instantiated_type_argument_nodes(b, chain, index);
            if type_argument_nodes.is_some() {
                return type_argument_nodes;
            }
            let type_parameter_nodes =
                self.type_parameters_to_type_parameter_declarations(b, symbol);
            if !type_parameter_nodes.is_empty() {
                let e = nb_e(b);
                return e.factory().new_node_list(&type_parameter_nodes);
            }
        }
        NodeList::NIL
    }

    // Go: checker/nodebuilderimpl.go:3712 shouldWriteTypeParametersInQualifiedName
    pub fn should_write_type_parameters_in_qualified_name(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        chain: &[SymbolId],
        index: usize,
    ) -> bool {
        nb_flags(b).intersects(NodeBuilderFlags::WRITE_TYPE_PARAMETERS_IN_QUALIFIED_NAME)
            && index + 1 < chain.len()
    }
}

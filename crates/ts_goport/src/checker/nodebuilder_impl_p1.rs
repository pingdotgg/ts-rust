//! Port of Go `checker/nodebuilderimpl.go` lines 1 to 1018: the node builder
//! types, the truncation and expansion helpers, and symbol to name/node
//! conversion.
//!
//! Relater pattern (printer plan contract item 8): NodeBuilderImpl methods are
//! `impl Checker` methods that take `b: &Rc<RefCell<NodeBuilderImpl>>`. No
//! `RefCell` borrow is held across a Checker call or a tracker call.

use crate::prelude::*;
use crate::printer::{EmitContext, EmitFlags, SymbolAccessibility};
use crate::pseudochecker::{PseudoChecker, new_pseudo_checker};
use std::cell::Cell;
use std::rc::Weak;

// Go: checker/nodebuilderimpl.go:24 CompositeSymbolIdentity
// PORT: Go `ast.SymbolId` and `ast.NodeId` are the u64 values that
// `get_symbol_id` and `get_node_id` return.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CompositeSymbolIdentity {
    pub is_constructor_node: bool,
    pub symbol_id: u64,
    pub node_id: u64,
}

// Go: checker/nodebuilderimpl.go:30 TrackedSymbolArgs
// PORT: Go stores `*TrackedSymbolArgs`. The struct is small and never
// mutated after creation, so it is stored by value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrackedSymbolArgs {
    pub symbol: SymbolId,
    pub enclosing_declaration: Node,
    pub meaning: SymbolFlags,
}

// Go: checker/nodebuilderimpl.go:36 SerializedTypeEntry
#[derive(Clone, Debug)]
pub struct SerializedTypeEntry {
    pub node: Node,
    pub truncating: bool,
    pub added_length: i32,
    pub tracked_symbols: Vec<TrackedSymbolArgs>,
}

// Go: checker/nodebuilderimpl.go:43 CompositeTypeCacheIdentity
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CompositeTypeCacheIdentity {
    pub type_id: TypeId,
    pub flags: NodeBuilderFlags,
    pub internal_flags: InternalNodeBuilderFlags,
    // ts#64556 (Go N' nodebuilderimpl.go:47): the zero key when the context
    // has no infer type parameters.
    pub infer_type_parameters: CacheHashKey,
}

// Go: checker/nodebuilderimpl.go:49 NodeBuilderLinks
// PORT: the Go map is created on first use. Here it always exists and starts
// empty, which reads the same.
#[derive(Clone, Debug, Default)]
pub struct NodeBuilderLinks {
    pub serialized_types: FxHashMap<CompositeTypeCacheIdentity, SerializedTypeEntry>, // Collection of types serialized at this location
    pub fake_scope_for_signature_declaration: Option<String>, // If present, this is a fake scope injected into an enclosing declaration chain.
}

// Go: checker/nodebuilderimpl.go:54 NodeBuilderSymbolLinks
// PORT: Go `module.ModeAwareCache[moduleSpecifierResult]` is a map keyed by
// `(path, resolution mode)`. `None` is the nil cache.
#[derive(Clone, Debug, Default)]
pub struct NodeBuilderSymbolLinks {
    pub specifier_cache: Option<FxHashMap<(String, ResolutionMode), ModuleSpecifierResult>>,
}

// Go: checker/nodebuilderimpl.go:58 moduleSpecifierResult
// PORT: a nil `importAttributesType` is `TypeId::NIL`.
#[derive(Clone, Debug, Default)]
pub struct ModuleSpecifierResult {
    pub specifier: String,
    pub import_attributes_type: TypeId,
}

// Go: checker/nodebuilderimpl.go:63 NodeBuilderContext
// PORT: symbol keyed maps use `SymbolId` for Go `ast.SymbolId`. A symbol id
// is the same for the same symbol, so the maps behave the same. Go
// `*ast.SourceFile` is the `SourceFile` node. Go `Host` is the program.
pub struct NodeBuilderContext {
    pub host: &'static GoProgram,
    pub tracker: Rc<dyn SymbolTracker>,
    pub approximate_length: i32,
    pub max_truncation_length: i32,
    pub encountered_error: bool,
    pub truncating: bool,
    pub reported_diagnostic: bool,
    pub flags: NodeBuilderFlags,
    pub internal_flags: InternalNodeBuilderFlags,
    pub depth: i32,
    pub max_expansion_depth: i32, // -1 means no expansion, 0+ = verbosity levels
    pub type_stack: Vec<TypeId>,
    pub can_increase_expansion_depth: bool,
    pub expansion_truncated: bool,
    pub enclosing_declaration: Node,
    pub enclosing_file: Node,
    pub infer_type_parameters: Vec<TypeId>,
    pub visited_types: FxHashSet<TypeId>,
    pub symbol_depth: FxHashMap<CompositeSymbolIdentity, i32>,
    pub tracked_symbols: Vec<TrackedSymbolArgs>,
    pub mapper: MapperId,
    pub reverse_mapped_stack: Vec<SymbolId>,
    pub enclosing_symbol_types: FxHashMap<SymbolId, TypeId>,
    pub suppress_report_inference_fallback: bool,
    pub remapped_symbol_references: FxHashMap<SymbolId, SymbolId>,

    // per signature scope state
    pub type_parameter_names: CopyOnWriteMap<TypeId, Node>,
    pub type_parameter_names_by_text: CopyOnWriteSet<String>,
    pub type_parameter_names_by_text_next_name_count: CopyOnWriteMap<String, i32>,
    pub type_parameter_symbol_list: CopyOnWriteSet<SymbolId>,
}

impl NodeBuilderContext {
    /// Go `&NodeBuilderContext{host: host}` with every other field at its zero
    /// value (empty maps and slices, nil nodes).
    // PORT: Go `tracker` is an interface that may be nil. Here it is always
    // set. Callers replace this placeholder `SymbolTrackerImpl` (which has no
    // context and no inner tracker) before any tracker call.
    #[must_use]
    pub fn new(host: &'static GoProgram) -> Self {
        Self {
            host,
            tracker: placeholder_tracker(),
            approximate_length: 0,
            max_truncation_length: 0,
            encountered_error: false,
            truncating: false,
            reported_diagnostic: false,
            flags: NodeBuilderFlags::NONE,
            internal_flags: InternalNodeBuilderFlags::NONE,
            depth: 0,
            max_expansion_depth: 0,
            type_stack: Vec::new(),
            can_increase_expansion_depth: false,
            expansion_truncated: false,
            enclosing_declaration: Node::NIL,
            enclosing_file: Node::NIL,
            infer_type_parameters: Vec::new(),
            visited_types: FxHashSet::default(),
            symbol_depth: FxHashMap::default(),
            tracked_symbols: Vec::new(),
            mapper: MapperId::NIL,
            reverse_mapped_stack: Vec::new(),
            enclosing_symbol_types: FxHashMap::default(),
            suppress_report_inference_fallback: false,
            remapped_symbol_references: FxHashMap::default(),
            type_parameter_names: CopyOnWriteMap::default(),
            type_parameter_names_by_text: CopyOnWriteSet::default(),
            type_parameter_names_by_text_next_name_count: CopyOnWriteMap::default(),
            type_parameter_symbol_list: CopyOnWriteSet::default(),
        }
    }
}

/// A `SymbolTrackerImpl` with no context and no inner tracker. It fills the
/// tracker slot until the real tracker exists.
fn placeholder_tracker() -> Rc<dyn SymbolTracker> {
    Rc::new(SymbolTrackerImpl {
        context: Weak::new(),
        inner: None,
        disable_track_symbol: Cell::new(false),
    })
}

/// Go `b.ctx = nil`: the context a `NodeBuilderImpl` holds when no
/// `NodeBuilder` entry point is active.
// PORT: Go sets `ctx` to nil. Here `ctx` is never optional, so an empty
// context stands in for nil. No node builder code reads it before an entry
// point pushes a real context.
#[must_use]
pub fn nil_context(host: &'static GoProgram) -> Rc<RefCell<NodeBuilderContext>> {
    Rc::new(RefCell::new(NodeBuilderContext::new(host)))
}

// Go: checker/nodebuilderimpl.go:97 NodeBuilderImpl
// PORT: Go `ch *Checker` is dropped. Every method is a Checker method and
// gets the checker as `self`. Go `f` is the method `f()`, which returns the
// emit context factory. Go `cloneBindingNameVisitor` is dropped: nothing in
// the ported code reads it, and the visitor would need the checker.
pub struct NodeBuilderImpl {
    // host members
    pub e: Rc<EmitContext>,
    pub pc: PseudoChecker,

    // cache
    pub links: LinkStore<Node, Box<NodeBuilderLinks>>,
    pub symbol_links: LinkStore<SymbolId, Box<NodeBuilderSymbolLinks>>,

    // state
    pub ctx: Rc<RefCell<NodeBuilderContext>>,

    // symbols for synthesized identifiers, needed for e.g. inlay hints
    pub id_to_symbol: FxHashMap<Node, SymbolId>,
}

impl NodeBuilderImpl {
    /// Go `b.f`.
    pub fn f(&self) -> &crate::ast::NodeFactory {
        self.e.factory().as_node_factory()
    }
}

// Go: checker/nodebuilderimpl.go:112 defaultMaximumTruncationLength
pub const DEFAULT_MAXIMUM_TRUNCATION_LENGTH: i32 = 160;
// Go: checker/nodebuilderimpl.go:112 noTruncationMaximumTruncationLength
pub const NO_TRUNCATION_MAXIMUM_TRUNCATION_LENGTH: i32 = 1_000_000;

// Node builder utility functions

// Go: checker/nodebuilderimpl.go:125 newNodeBuilderImpl
#[must_use]
pub fn new_node_builder_impl(
    ch: &Checker,
    e: Rc<EmitContext>,
    id_to_symbol: Option<FxHashMap<Node, SymbolId>>,
) -> NodeBuilderImpl {
    let id_to_symbol = id_to_symbol.unwrap_or_default();
    NodeBuilderImpl {
        e,
        pc: new_pseudo_checker(ch.strict_null_checks, ch.exact_optional_property_types),
        links: LinkStore::default(),
        symbol_links: LinkStore::default(),
        ctx: nil_context(ch.program),
        id_to_symbol,
    }
}

// PORT: private accessors. Clone the handle out, then drop the borrow before
// any Checker or tracker call.
fn p1_ctx(b: &Rc<RefCell<NodeBuilderImpl>>) -> Rc<RefCell<NodeBuilderContext>> {
    b.borrow().ctx.clone()
}

fn p1_e(b: &Rc<RefCell<NodeBuilderImpl>>) -> Rc<EmitContext> {
    b.borrow().e.clone()
}

fn p1_flags(b: &Rc<RefCell<NodeBuilderImpl>>) -> NodeBuilderFlags {
    let ctx = p1_ctx(b);
    let flags = ctx.borrow().flags;
    flags
}

fn p1_add_length(b: &Rc<RefCell<NodeBuilderImpl>>, n: usize) {
    let ctx = p1_ctx(b);
    ctx.borrow_mut().approximate_length += i32::try_from(n).unwrap_or(i32::MAX);
}

// Go: checker/nodebuilderimpl.go:325 getAccessStack
fn get_access_stack(ref_: Node) -> Vec<Node> {
    let mut state = ref_.type_name();
    let mut ids: Vec<Node> = Vec::new();
    while !is_identifier(state) {
        ids.insert(0, state.right());
        state = state.left();
    }
    ids.insert(0, state);
    ids
}

// Go: checker/nodebuilderimpl.go:463 isIdentifierTypeReference
fn is_identifier_type_reference(node: Node) -> bool {
    is_type_reference_node(node) && is_identifier(node.type_name())
}

// Go: checker/nodebuilderimpl.go:467 arrayIsHomogeneous
fn array_is_homogeneous<T>(array: &[T], comparer: impl Fn(&T, &T) -> bool) -> bool {
    if array.len() < 2 {
        return true;
    }
    let first = &array[0];
    for target in &array[1..] {
        if !comparer(first, target) {
            return false;
        }
    }
    true
}

// Go: checker/nodebuilderimpl.go:745 getTopmostIndexedAccessType
fn get_topmost_indexed_access_type(node: Node) -> Node {
    if is_indexed_access_type_node(node.object_type()) {
        return get_topmost_indexed_access_type(node.object_type());
    }
    node
}

// Go: checker/nodebuilderimpl.go:912 canUsePropertyAccess
fn can_use_property_access(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    // TODO: in strada, this only used `isIdentifierStart` on the first character, while this checks the whole string for validity
    // - possible strada bug?
    if let Some(rest) = name.strip_prefix('#') {
        return !rest.is_empty() && is_identifier_text(rest, LanguageVariant::STANDARD);
    }
    is_identifier_text(name, LanguageVariant::STANDARD)
}

// Go: checker/nodebuilderimpl.go:924 startsWithSingleOrDoubleQuote
fn starts_with_single_or_double_quote(str: &str) -> bool {
    str.starts_with('\'') || str.starts_with('"')
}

// Go: checker/nodebuilderimpl.go:928 startsWithSquareBracket
fn starts_with_square_bracket(str: &str) -> bool {
    str.starts_with('[')
}

// Go: checker/nodebuilderimpl.go:932 isDefaultBindingContext
fn is_default_binding_context(location: Node) -> bool {
    location.kind() == SyntaxKind::SourceFile || is_ambient_module(location)
}

impl Checker {
    // Go: checker/nodebuilderimpl.go:134 saveRestoreFlags
    // PORT: the restore closure keeps its own handle to the context.
    pub fn save_restore_flags(&mut self, b: &Rc<RefCell<NodeBuilderImpl>>) -> Box<dyn FnOnce()> {
        let ctx = p1_ctx(b);
        let (flags, internal_flags, depth) = {
            let c = ctx.borrow();
            (c.flags, c.internal_flags, c.depth)
        };

        Box::new(move || {
            let mut c = ctx.borrow_mut();
            c.flags = flags;
            c.internal_flags = internal_flags;
            c.depth = depth;
        })
    }

    // Go: checker/nodebuilderimpl.go:146 checkTruncationLength
    pub fn check_truncation_length(&mut self, b: &Rc<RefCell<NodeBuilderImpl>>) -> bool {
        let ctx = p1_ctx(b);
        let mut c = ctx.borrow_mut();
        if c.truncating {
            return c.truncating;
        }
        let max_length = if c.flags.intersects(NodeBuilderFlags::NO_TRUNCATION) {
            NO_TRUNCATION_MAXIMUM_TRUNCATION_LENGTH
        } else if c.max_truncation_length > 0 {
            c.max_truncation_length
        } else {
            DEFAULT_MAXIMUM_TRUNCATION_LENGTH
        };
        c.truncating = c.approximate_length > max_length;
        c.truncating
    }

    // Go: checker/nodebuilderimpl.go:164 checkTruncationLengthIfExpanding
    // checkTruncationLengthIfExpanding returns true if maxExpansionDepth >= 0 and truncation length exceeded.
    // When expanding, we need to mark the output as truncated so we know not to offer further expansion.
    pub fn check_truncation_length_if_expanding(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
    ) -> bool {
        let ctx = p1_ctx(b);
        let max_expansion_depth = ctx.borrow().max_expansion_depth;
        if max_expansion_depth >= 0 && self.check_truncation_length(b) {
            ctx.borrow_mut().expansion_truncated = true;
            return true;
        }
        false
    }

    // Go: checker/nodebuilderimpl.go:175 isExpandableType
    // isExpandableType reports whether t has a named representation that could be inlined
    // as its structural form during hover expansion. Filters out lib types.
    // When isAlias is true, checks whether t's alias symbol is from user code (not lib).
    pub fn is_expandable_type(
        &mut self,
        _b: &Rc<RefCell<NodeBuilderImpl>>,
        t: TypeId,
        is_alias: bool,
    ) -> bool {
        if is_alias {
            return !self.is_lib_symbol_for_hover_verbosity(self.ty(t).alias.symbol());
        }
        if self.is_lib_type_for_hover_verbosity(t) {
            return false;
        }
        let ty = self.ty(t);
        let object_flags = ty.object_flags;
        if ty.flags.intersects(TypeFlags::ENUM_LIKE)
            || object_flags.intersects(ObjectFlags::REFERENCE)
            || object_flags.intersects(ObjectFlags::CLASS_OR_INTERFACE)
        {
            return true;
        }
        if object_flags.intersects(ObjectFlags::ANONYMOUS)
            && ty.symbol.is_some()
            && self.sym(ty.symbol).flags.intersects(
                SymbolFlags::CLASS
                    | SymbolFlags::ENUM
                    | SymbolFlags::VALUE_MODULE
                    | SymbolFlags::FUNCTION
                    | SymbolFlags::METHOD,
            )
        {
            return true;
        }
        false
    }

    // Go: checker/nodebuilderimpl.go:197 isTypeOnStack
    // isTypeOnStack reports whether t is already being processed in the current expansion,
    // excluding the last element (which is the type currently being serialized by typeToTypeNode).
    pub fn is_type_on_stack(&mut self, b: &Rc<RefCell<NodeBuilderImpl>>, t: TypeId) -> bool {
        let ctx = p1_ctx(b);
        let c = ctx.borrow();
        let n = c.type_stack.len().saturating_sub(1);
        c.type_stack[..n].iter().any(|&s| s == t)
    }

    // Go: checker/nodebuilderimpl.go:212 shouldExpandType
    // shouldExpandType decides whether to expand this type at the current depth.
    // Returns true when depth < maxExpansionDepth (expand now).
    // At the boundary (depth == maxExpansionDepth), sets canIncreaseExpansionDepth
    // to signal that a higher verbosity level would reveal more detail.
    // Returns false when expansion is disabled (maxExpansionDepth < 0), the type
    // is not expandable, or the type is cyclic.
    pub fn should_expand_type(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        t: TypeId,
        is_alias: bool,
    ) -> bool {
        let ctx = p1_ctx(b);
        if ctx.borrow().max_expansion_depth < 0 {
            return false;
        }
        if !self.is_expandable_type(b, t, is_alias) {
            return false;
        }
        if self.is_type_on_stack(b, t) {
            return false;
        }
        let mut c = ctx.borrow_mut();
        if c.depth < c.max_expansion_depth {
            return true;
        }
        c.can_increase_expansion_depth = true;
        false
    }

    // Go: checker/nodebuilderimpl.go:231 isActivelyExpanding
    // isActivelyExpanding reports whether the current depth is below maxExpansionDepth,
    // meaning type-node reuse should be skipped so typeToTypeNode can expand named types.
    pub fn is_actively_expanding(&mut self, b: &Rc<RefCell<NodeBuilderImpl>>) -> bool {
        let ctx = p1_ctx(b);
        let c = ctx.borrow();
        c.max_expansion_depth > 0 && c.depth < c.max_expansion_depth
    }

    // Go: checker/nodebuilderimpl.go:239 checkTypeExpandability
    // checkTypeExpandability probes whether a type (or its type arguments) could be expanded,
    // for use after type-node reuse where shouldExpandType was never called.
    // Delegates to shouldExpandType for the actual check, then recurses into type arguments
    // of reference types (e.g., Apple inside Promise<Apple>).
    pub fn check_type_expandability(&mut self, b: &Rc<RefCell<NodeBuilderImpl>>, t: TypeId) {
        let ctx = p1_ctx(b);
        {
            let c = ctx.borrow();
            if c.max_expansion_depth < 0 || t.is_nil() || c.can_increase_expansion_depth {
                return;
            }
        }
        // Push t onto the type stack so shouldExpandType's cycle detection works correctly.
        ctx.borrow_mut().type_stack.push(t);
        // PORT: Go pops t in a `defer`; every return below breaks out of this block.
        'body: {
            // If t is an ancestor in the current expansion, return early to avoid unbounded recursion.
            if self.is_type_on_stack(b, t) {
                break 'body;
            }
            if self.ty(t).alias.is_some() {
                self.should_expand_type(b, t, true);
            }
            if !ctx.borrow().can_increase_expansion_depth {
                self.should_expand_type(b, t, false);
            }
            if ctx.borrow().can_increase_expansion_depth {
                break 'body;
            }
            // Recurse into type arguments (e.g., check Apple in Promise<Apple>).
            if self.ty(t).object_flags.intersects(ObjectFlags::REFERENCE) {
                for arg in self.get_type_arguments(t) {
                    self.check_type_expandability(b, arg);
                    if ctx.borrow().can_increase_expansion_depth {
                        break 'body;
                    }
                }
            }
        }
        ctx.borrow_mut().type_stack.pop();
    }

    // Go: checker/nodebuilderimpl.go:272 appendReferenceToType
    pub fn append_reference_to_type(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        root: Node,
        ref_: Node,
    ) -> Node {
        let e = p1_e(b);
        let f = e.factory();
        if is_import_type_node(root) {
            // first shift type arguments

            // !!! In the old emitter, an Identifier could have type arguments for use with quickinfo:
            // (see the Go source for the elided code)
            // !!! Without the above, nested type args are silently elided
            // then move qualifiers
            let ids = get_access_stack(ref_);
            let mut qualifier = root.qualifier();
            for id in ids {
                if qualifier.is_some() {
                    qualifier = f.new_qualified_name(qualifier, id);
                } else {
                    qualifier = id;
                }
            }
            return f.update_import_type_node(
                root,
                root.is_type_of(),
                root.argument(),
                root.attributes(),
                qualifier,
                ref_.type_argument_list(),
            );
        } else if is_type_reference_node(root) {
            let type_arguments = root.type_argument_list();
            if p1_flags(b).intersects(NodeBuilderFlags::USE_INSTANTIATION_EXPRESSIONS)
                && type_arguments.is_some()
                && !type_arguments.nodes().is_empty()
            {
                let access = self.create_access_expression(b, root.type_name());
                let mut expr =
                    self.create_expression_with_type_arguments(b, access, type_arguments);
                for id in get_access_stack(ref_) {
                    expr = f.new_property_access_expression(expr, Node::NIL, id, NodeFlags::NONE);
                }
                return expr;
            }
            let mut type_name = root.type_name();
            for id in get_access_stack(ref_) {
                type_name = f.new_qualified_name(type_name, id);
            }
            return f.update_type_reference_node(root, type_name, ref_.type_argument_list());
        }
        let mut expr = self.create_access_expression(b, root);
        for id in get_access_stack(ref_) {
            expr = f.new_property_access_expression(expr, Node::NIL, id, NodeFlags::NONE);
        }
        expr
    }

    // Go: checker/nodebuilderimpl.go:337 isClassInstanceSide
    // PORT: a free function that takes the checker in Go.
    pub fn is_class_instance_side(&mut self, t: TypeId) -> bool {
        let (symbol, flags, object_flags) = {
            let ty = self.ty(t);
            (ty.symbol, ty.flags, ty.object_flags)
        };
        symbol.is_some()
            && self.sym(symbol).flags.intersects(SymbolFlags::CLASS)
            && (t == self.get_declared_type_of_class_or_interface(symbol)
                || (flags.intersects(TypeFlags::OBJECT)
                    && object_flags.intersects(ObjectFlags::IS_CLASS_INSTANCE_CLONE)))
    }

    // Go: checker/nodebuilderimpl.go:341 createElidedInformationPlaceholder
    pub fn create_elided_information_placeholder(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
    ) -> Node {
        p1_add_length(b, 3);
        let e = p1_e(b);
        let f = e.factory();
        if !p1_flags(b).intersects(NodeBuilderFlags::NO_TRUNCATION) {
            return f.new_type_reference_node(
                f.new_identifier("..."),
                NodeList::NIL, /*typeArguments*/
            );
        }
        e.add_synthetic_leading_comment(
            f.new_keyword_type_node(SyntaxKind::AnyKeyword),
            SyntaxKind::MultiLineCommentTrivia,
            "elided",
            false, /*hasTrailingNewLine*/
        )
    }

    // Go: checker/nodebuilderimpl.go:349 mapToTypeNodes
    pub fn map_to_type_nodes(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        list: &[TypeId],
        is_bare_list: bool,
    ) -> NodeList {
        if list.is_empty() {
            return NodeList::NIL;
        }
        let e = p1_e(b);
        let f = e.factory();

        if self.check_truncation_length(b) {
            if !is_bare_list {
                let node = if p1_flags(b).intersects(NodeBuilderFlags::NO_TRUNCATION) {
                    e.add_synthetic_leading_comment(
                        f.new_keyword_type_node(SyntaxKind::AnyKeyword),
                        SyntaxKind::MultiLineCommentTrivia,
                        "elided",
                        false, /*hasTrailingNewLine*/
                    )
                } else {
                    f.new_type_reference_node(
                        f.new_identifier("..."),
                        NodeList::NIL, /*typeArguments*/
                    )
                };
                return f.new_node_list(&[node]);
            } else if list.len() > 2 {
                let first = self.type_to_type_node(b, list[0]);
                let last = self.type_to_type_node(b, list[list.len() - 1]);
                let middle = if p1_flags(b).intersects(NodeBuilderFlags::NO_TRUNCATION) {
                    e.add_synthetic_leading_comment(
                        f.new_keyword_type_node(SyntaxKind::AnyKeyword),
                        SyntaxKind::MultiLineCommentTrivia,
                        &format!("... {} more elided ...", list.len() - 2),
                        false, /*hasTrailingNewLine*/
                    )
                } else {
                    let text = format!("... {} more ...", list.len() - 2);
                    f.new_type_reference_node(
                        f.new_identifier(text),
                        NodeList::NIL, /*typeArguments*/
                    )
                };
                return f.new_node_list(&[first, middle, last]);
            }
        }

        let may_have_name_collisions =
            !p1_flags(b).intersects(NodeBuilderFlags::USE_FULLY_QUALIFIED_TYPE);
        // PORT: Go `collections.MultiMap[string, seenName]`; a seenName is `(t, i)`.
        let mut seen_names: Option<IndexMap<String, Vec<(TypeId, usize)>>> =
            if may_have_name_collisions {
                Some(IndexMap::new())
            } else {
                None
            };

        let mut result: Vec<Node> = Vec::with_capacity(list.len());

        for (i, &t) in list.iter().enumerate() {
            let display_index = i + 1;
            if self.check_truncation_length(b) && (display_index + 2 < list.len() - 1) {
                if p1_flags(b).intersects(NodeBuilderFlags::NO_TRUNCATION) {
                    result.push(e.add_synthetic_leading_comment(
                        f.new_keyword_type_node(SyntaxKind::AnyKeyword),
                        SyntaxKind::MultiLineCommentTrivia,
                        &format!("... {} more elided ...", list.len() - display_index),
                        false, /*hasTrailingNewLine*/
                    ));
                } else {
                    let text = format!("... {} more ...", list.len() - display_index);
                    result.push(f.new_type_reference_node(
                        f.new_identifier(text),
                        NodeList::NIL, /*typeArguments*/
                    ));
                }
                let type_node = self.type_to_type_node(b, list[list.len() - 1]);
                if type_node.is_some() {
                    result.push(type_node);
                }
                break;
            }
            p1_add_length(b, 2); // Account for whitespace + separator
            let type_node = self.type_to_type_node(b, t);
            if type_node.is_some() {
                result.push(type_node);
                if let Some(seen_names) = seen_names.as_mut() {
                    if is_identifier_type_reference(type_node) {
                        seen_names
                            .entry(type_node.type_name().text().to_string())
                            .or_default()
                            .push((t, result.len() - 1));
                    }
                }
            }
        }

        if let Some(seen_names) = seen_names {
            // To avoid printing types like `[Foo, Foo]` or `Bar & Bar` where
            // occurrences of the same name actually come from different
            // namespaces, go through the single-identifier type reference nodes
            // we just generated, and see if any names were generated more than
            // once while referring to different types. If so, regenerate the
            // type node for each entry by that name with the
            // `UseFullyQualifiedType` flag enabled.
            let restore_flags = self.save_restore_flags(b);
            {
                let ctx = p1_ctx(b);
                let mut c = ctx.borrow_mut();
                c.flags = c.flags | NodeBuilderFlags::USE_FULLY_QUALIFIED_TYPE;
            }
            for types in seen_names.values() {
                if !array_is_homogeneous(types, |a, b| self.types_are_same_reference(a.0, b.0)) {
                    for &(t, i) in types {
                        result[i] = self.type_to_type_node(b, t);
                    }
                }
            }
            restore_flags();
        }

        f.new_node_list(&result)
    }

    // Go: checker/nodebuilderimpl.go:442 serializeTypeName
    pub fn serialize_type_name(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        node: Node,
        is_type_of: bool,
        type_arguments: NodeList,
    ) -> Node {
        let meaning = if is_type_of {
            SymbolFlags::VALUE
        } else {
            SymbolFlags::TYPE
        };
        let symbol = self.resolve_entity_name(node, meaning, true, false, node);
        if symbol.is_nil() {
            return Node::NIL;
        }

        let mut resolved_symbol = symbol;
        if self.sym(symbol).flags.intersects(SymbolFlags::ALIAS) {
            resolved_symbol = self.resolve_alias(symbol);
        }

        let enclosing_declaration = p1_ctx(b).borrow().enclosing_declaration;
        if self
            .is_symbol_accessible(symbol, enclosing_declaration, meaning, false)
            .accessibility
            != SymbolAccessibility::ACCESSIBLE
        {
            return Node::NIL;
        }
        self.symbol_to_type_node(b, resolved_symbol, meaning, type_arguments)
    }

    // Go: checker/nodebuilderimpl.go:481 typesAreSameReference
    // PORT: a free function in Go. It reads type data, so it is a Checker
    // method here. Go compares `*TypeAlias` pointers; `Rc::ptr_eq` does the same.
    pub fn types_are_same_reference(&self, a: TypeId, b: TypeId) -> bool {
        let (ta, tb) = (self.ty(a), self.ty(b));
        a == b
            || ta.symbol.is_some() && ta.symbol == tb.symbol
            || match (&ta.alias, &tb.alias) {
                (Some(x), Some(y)) => Rc::ptr_eq(x, y),
                _ => false,
            }
    }

    // Go: checker/nodebuilderimpl.go:485 setCommentRange
    pub fn set_comment_range(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        node: Node,
        range_: Node,
    ) {
        let enclosing_file = p1_ctx(b).borrow().enclosing_file;
        if range_.is_some()
            && enclosing_file.is_some()
            && enclosing_file == get_source_file_of_node(range_)
        {
            // Copy comments to node for declaration emit
            p1_e(b).assign_comment_range(node, range_);
        }
    }

    // Go: checker/nodebuilderimpl.go:492 typeNodeIsEquivalentToType
    pub fn type_node_is_equivalent_to_type(
        &mut self,
        _b: &Rc<RefCell<NodeBuilderImpl>>,
        annotated_declaration: Node,
        t: TypeId,
        type_from_type_node: TypeId,
    ) -> bool {
        if type_from_type_node == t {
            return true;
        }
        if annotated_declaration.is_nil() {
            return false;
        }
        // !!!
        // used to be hasEffectiveQuestionToken for JSDoc
        if is_optional_declaration(annotated_declaration) {
            return self.get_type_with_facts(t, TypeFacts::NE_UNDEFINED) == type_from_type_node;
        }
        false
    }

    // Go: checker/nodebuilderimpl.go:507 canReuseExistingJSTypeNode
    pub fn can_reuse_existing_js_type_node(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        existing: Node,
        t: TypeId,
    ) -> bool {
        self.get_intended_type_from_js_doc_type_reference(existing).is_nil()
            && self.existing_type_node_is_not_reference_or_is_reference_with_compatible_type_argument_count(b, existing, t)
    }

    // Go: checker/nodebuilderimpl.go:511 tryGetResolvedSymbolFromTypeNode
    pub fn try_get_resolved_symbol_from_type_node(
        &mut self,
        _b: &Rc<RefCell<NodeBuilderImpl>>,
        node: Node,
    ) -> SymbolId {
        if node.is_nil() || node.parent().is_nil() {
            return SymbolId::NIL;
        }
        self.get_type_from_type_node(node);
        // call to ensure symbol is resolved
        let Some(links) = self.symbol_node_links.try_get(node) else {
            return SymbolId::NIL;
        };
        links.resolved_symbol
    }

    // Go: checker/nodebuilderimpl.go:524 existingTypeNodeIsNotReferenceOrIsReferenceWithCompatibleTypeArgumentCount
    pub fn existing_type_node_is_not_reference_or_is_reference_with_compatible_type_argument_count(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        existing: Node,
        t: TypeId,
    ) -> bool {
        // In JS, you can say something like `Foo` and get a `Foo<any>` implicitly - we don't want to preserve that original `Foo` in these cases, though.
        if !self.ty(t).object_flags.intersects(ObjectFlags::REFERENCE) {
            return true;
        }
        if !is_type_reference_node(existing) {
            return true;
        }

        let symbol = self.try_get_resolved_symbol_from_type_node(b, existing);
        if symbol.is_nil() {
            return true;
        }

        // `type` is a reference type, and `existing` is a type reference node, but we still need to make sure they refer to the _same_ target type
        // before we go comparing their type argument counts.

        let existing_target = self.get_declared_type_of_symbol(symbol);
        let target = self.ty(t).as_type_reference().object.target;
        if existing_target.is_nil() || existing_target != target {
            return true;
        }
        let type_parameters = self
            .ty(target)
            .as_interface_type()
            .type_parameters()
            .to_vec();
        let min = self.get_min_type_argument_count(&type_parameters);
        existing.type_arguments().len() as i64 >= i64::from(min)
    }

    // Go: checker/nodebuilderimpl.go:548 tryReuseExistingNonParameterTypeNode
    pub fn try_reuse_existing_non_parameter_type_node(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        existing: Node,
        t: TypeId,
        mut host: Node,
        mut annotation_type: TypeId,
    ) -> Node {
        if host.is_nil() {
            host = p1_ctx(b).borrow().enclosing_declaration;
        }
        if annotation_type.is_nil() {
            annotation_type = self.nb_get_type_from_type_node(b, existing, true);
        }
        if annotation_type.is_some()
            && self.type_node_is_equivalent_to_type(b, host, t, annotation_type)
            && self.can_reuse_existing_js_type_node(b, existing, t)
        {
            let result = self.try_reuse_existing_node_helper(b, existing);
            if result.is_some() {
                return result;
            }
        }
        Node::NIL
    }

    // Go: checker/nodebuilderimpl.go:564 getResolvedTypeWithoutAbstractConstructSignatures
    // PORT: Go takes the `*StructuredType`; here it is the type id.
    pub fn get_resolved_type_without_abstract_construct_signatures(
        &mut self,
        _b: &Rc<RefCell<NodeBuilderImpl>>,
        t: TypeId,
    ) -> TypeId {
        let (symbol, members, call_signatures, construct_signatures, index_infos, cached) = {
            let ty = self.ty(t);
            let st = ty.as_structured_type();
            (
                ty.symbol,
                st.members,
                st.call_signatures().to_vec(),
                st.construct_signatures().to_vec(),
                st.index_infos_list(),
                st.object_type_without_abstract_construct_signatures(),
            )
        };
        if construct_signatures.is_empty() {
            return t;
        }
        if cached.is_some() {
            return cached;
        }
        let filtered: Vec<SignatureId> = construct_signatures
            .iter()
            .copied()
            .filter(|&signature| {
                !self
                    .sig(signature)
                    .flags
                    .intersects(SignatureFlags::ABSTRACT)
            })
            .collect();
        if filtered.len() == construct_signatures.len() {
            self.ty_mut(t)
                .as_structured_type_mut()
                .set_object_type_without_abstract_construct_signatures(t);
            return t;
        }
        let type_copy =
            self.new_anonymous_type(symbol, members, &call_signatures, &filtered, &index_infos);
        self.ty_mut(t)
            .as_structured_type_mut()
            .set_object_type_without_abstract_construct_signatures(type_copy);
        self.ty_mut(type_copy)
            .as_structured_type_mut()
            .set_object_type_without_abstract_construct_signatures(type_copy);
        type_copy
    }

    // Go: checker/nodebuilderimpl.go:584 symbolToNode
    pub fn symbol_to_node(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        symbol: SymbolId,
        meaning: SymbolFlags,
    ) -> Node {
        let ctx = p1_ctx(b);
        if ctx
            .borrow()
            .internal_flags
            .intersects(InternalNodeBuilderFlags::WRITE_COMPUTED_PROPS)
        {
            let value_declaration = self.sym(symbol).value_declaration;
            if value_declaration.is_some() {
                let name = get_name_of_declaration(value_declaration);
                if name.is_some() && is_computed_property_name(name) {
                    return name;
                }
            }
            if self.value_symbol_links.has_by_id(&self.symbols, symbol) {
                let name_type = self
                    .value_symbol_links
                    .get_by_id(&self.symbols, symbol)
                    .name_type;
                if name_type.is_some()
                    && self
                        .ty(name_type)
                        .flags
                        .intersects(TypeFlags::ENUM_LITERAL | TypeFlags::UNIQUE_ES_SYMBOL)
                {
                    let name_type_symbol = self.ty(name_type).symbol;
                    let old_enclosing = ctx.borrow().enclosing_declaration;
                    ctx.borrow_mut().enclosing_declaration =
                        self.sym(name_type_symbol).value_declaration;
                    let expression = self.symbol_to_expression_worker(b, name_type_symbol, meaning);
                    let result = p1_e(b).factory().new_computed_property_name(expression);
                    ctx.borrow_mut().enclosing_declaration = old_enclosing;
                    return result;
                }
            }
        }
        self.symbol_to_expression(b, symbol, meaning)
    }

    // Go: checker/nodebuilderimpl.go:606 symbolToName
    pub fn symbol_to_name(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        symbol: SymbolId,
        meaning: SymbolFlags,
        expects_identifier: bool,
    ) -> Node {
        let chain = self.lookup_symbol_chain(b, symbol, meaning, false);
        {
            let ctx = p1_ctx(b);
            let mut c = ctx.borrow_mut();
            if expects_identifier
                && chain.len() != 1
                && !c.encountered_error
                && c.flags
                    .intersects(NodeBuilderFlags::ALLOW_QUALIFIED_NAME_IN_PLACE_OF_IDENTIFIER)
            {
                c.encountered_error = true;
            }
        }
        // PORT: Go passes `len(chain)-1`, which is -1 for an empty chain and
        // then panics on the index. The `usize` subtraction panics the same way.
        self.create_entity_name_from_symbol_chain(b, &chain, chain.len() - 1)
    }

    // Go: checker/nodebuilderimpl.go:614 createEntityNameFromSymbolChain
    pub fn create_entity_name_from_symbol_chain(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        chain: &[SymbolId],
        index: usize,
    ) -> Node {
        // typeParameterNodes := b.lookupTypeParameterNodes(chain, index)
        let symbol = chain[index];
        let ctx = p1_ctx(b);

        if index == 0 {
            let mut c = ctx.borrow_mut();
            c.flags = c.flags | NodeBuilderFlags::IN_INITIAL_ENTITY_NAME;
        }
        let symbol_name = self.get_name_of_symbol_as_written(b, symbol);
        if index == 0 {
            let mut c = ctx.borrow_mut();
            c.flags = NodeBuilderFlags(c.flags.0 ^ NodeBuilderFlags::IN_INITIAL_ENTITY_NAME.0);
        }

        let identifier = self.nb_new_identifier(b, &symbol_name, symbol);
        p1_e(b).add_emit_flags(identifier, EmitFlags::NO_ASCII_ESCAPING);
        // !!! TODO: smuggle type arguments out
        // if (typeParameterNodes) setIdentifierTypeArguments(identifier, factory.createNodeArray<TypeNode | TypeParameterDeclaration>(typeParameterNodes));
        // identifier.symbol = symbol;
        // expression = identifier;
        if index > 0 {
            let left = self.create_entity_name_from_symbol_chain(b, chain, index - 1);
            return p1_e(b).factory().new_qualified_name(left, identifier);
        }
        identifier
    }

    // Go: checker/nodebuilderimpl.go:642 symbolToEntityNameNode
    // TODO: Audit usages of symbolToEntityNameNode - they should probably all be symbolToName
    pub fn symbol_to_entity_name_node(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        symbol: SymbolId,
    ) -> Node {
        let name = self.sym(symbol).name.clone();
        let identifier = self.nb_new_identifier(b, &name, symbol);
        let parent = self.sym(symbol).parent;
        if parent.is_some() {
            let left = self.symbol_to_entity_name_node(b, parent);
            return p1_e(b).factory().new_qualified_name(left, identifier);
        }
        identifier
    }

    // Go: checker/nodebuilderimpl.go:650 symbolToTypeNode
    pub fn symbol_to_type_node(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        symbol: SymbolId,
        mask: SymbolFlags,
        type_arguments: NodeList,
    ) -> Node {
        let ctx = p1_ctx(b);
        let e = p1_e(b);
        let f = e.factory();
        let use_alias_outside =
            p1_flags(b).intersects(NodeBuilderFlags::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE);
        let chain = self.lookup_symbol_chain(b, symbol, mask, !use_alias_outside); // If we're using aliases outside the current scope, dont bother with the module
        if chain.is_empty() {
            return Node::NIL; // TODO: shouldn't be possible, `lookupSymbolChain` should always at least return the input symbol and issue an error
        }
        let is_type_of = mask == SymbolFlags::VALUE;
        if self
            .sym(chain[0])
            .declarations
            .iter()
            .any(|&d| has_non_global_augmentation_external_module_symbol(d))
        {
            // module is root, must use `ImportTypeNode`
            let mut non_root_parts = Node::NIL;
            if chain.len() > 1 {
                non_root_parts = self.create_access_from_symbol_chain(
                    b,
                    &chain,
                    chain.len() - 1,
                    1,
                    type_arguments,
                );
            }
            let mut type_parameter_nodes = type_arguments;
            if type_parameter_nodes.is_nil() {
                type_parameter_nodes = self.lookup_type_parameter_nodes(b, &chain, 0);
            }
            let enclosing_declaration = ctx.borrow().enclosing_declaration;
            let context_file = get_source_file_of_node(e.most_original(enclosing_declaration)); // TODO: Just use b.ctx.enclosingFile ? Or is the delayed lookup important for context moves?
            let target_file = get_source_file_of_module(&self.symbols, chain[0]);
            let mut specifier_result = ModuleSpecifierResult::default();
            let mut import_mode_override = ResolutionMode::NONE;
            let resolution_kind = self.compiler_options.get_module_resolution_kind();
            if resolution_kind == ModuleResolutionKind::NODE16
                || resolution_kind == ModuleResolutionKind::NODE_NEXT
            {
                // An `import` type directed at an esm format file is only going to resolve in esm mode - set the esm mode assertion
                if target_file.is_some()
                    && context_file.is_some()
                    && get_emit_module_format_of_file(target_file) == ModuleKind::ES_NEXT
                    && get_emit_module_format_of_file(target_file)
                        != get_emit_module_format_of_file(context_file)
                {
                    specifier_result =
                        self.get_specifier_for_module_symbol(b, chain[0], ModuleKind::ES_NEXT);
                    import_mode_override = ModuleKind::ES_NEXT;
                }
            }
            if specifier_result.specifier.is_empty() {
                specifier_result =
                    self.get_specifier_for_module_symbol(b, chain[0], ResolutionMode::NONE);
            }
            if !p1_flags(b).intersects(NodeBuilderFlags::ALLOW_NODE_MODULES_RELATIVE_PATHS) /* && b.ch.compilerOptions.GetModuleResolutionKind() != core.ModuleResolutionKindClassic */
                && specifier_result.specifier.contains("/node_modules/")
            {
                let old_specifier_result = specifier_result.clone();

                if resolution_kind == ModuleResolutionKind::NODE16
                    || resolution_kind == ModuleResolutionKind::NODE_NEXT
                {
                    // We might be able to write a portable import type using a mode override; try specifier generation again, but with a different mode set
                    let mut swapped_mode = ModuleKind::ES_NEXT;
                    if get_emit_module_format_of_file(context_file) == ModuleKind::ES_NEXT {
                        swapped_mode = ModuleKind::COMMON_JS;
                    }
                    specifier_result =
                        self.get_specifier_for_module_symbol(b, chain[0], swapped_mode);

                    if specifier_result.specifier.contains("/node_modules/") {
                        // Still unreachable :(
                        specifier_result = old_specifier_result.clone();
                    } else {
                        import_mode_override = swapped_mode;
                    }
                }

                if import_mode_override == ResolutionMode::NONE {
                    // If ultimately we can only name the symbol with a reference that dives into a `node_modules` folder, we should error
                    // since declaration files with these kinds of references are liable to fail when published :(
                    ctx.borrow_mut().encountered_error = true;
                    let tracker = ctx.borrow().tracker.clone();
                    let symbol_name = self.sym(symbol).name.clone();
                    tracker.report_likely_unsafe_import_required_error(
                        self,
                        &old_specifier_result.specifier,
                        &symbol_name,
                    );
                }
            }

            let attributes = self.create_import_attributes_for_module_specifier(
                b,
                &specifier_result,
                import_mode_override,
            );
            let lit =
                f.new_literal_type_node(self.nb_new_string_literal(b, &specifier_result.specifier));
            p1_add_length(b, go_len(&specifier_result.specifier) + 10); // specifier + import("")
            if non_root_parts.is_nil() || is_entity_name(non_root_parts) {
                // !!! TODO: smuggle type arguments out
                // const lastId = isIdentifier(nonRootParts) ? nonRootParts : nonRootParts.right;
                // setIdentifierTypeArguments(lastId, /*typeArguments*/ undefined);
                return f.new_import_type_node(
                    is_type_of,
                    lit,
                    attributes,
                    non_root_parts,
                    type_parameter_nodes,
                );
            }

            let split_node = get_topmost_indexed_access_type(non_root_parts);
            let qualifier = split_node.object_type().type_name();
            return f.new_indexed_access_type_node(
                f.new_import_type_node(
                    is_type_of,
                    lit,
                    attributes,
                    qualifier,
                    type_parameter_nodes,
                ),
                split_node.index_type(),
            );
        }

        let entity_name =
            self.create_access_from_symbol_chain(b, &chain, chain.len() - 1, 0, type_arguments);
        if is_indexed_access_type_node(entity_name) {
            return entity_name; // Indexed accesses can never be `typeof`
        }
        if is_entity_name(entity_name) {
            if is_type_of {
                return f.new_type_query_node(entity_name, NodeList::NIL);
            }
            return f.new_type_reference_node(entity_name, type_arguments);
        }
        if is_type_of && is_expression_with_type_arguments(entity_name) {
            return f.new_type_query_node(
                f.deep_clone_node(entity_name.expression()),
                entity_name.type_argument_list(),
            );
        }
        entity_name
    }

    // Go: checker/nodebuilderimpl.go:752 createAccessFromSymbolChain
    pub fn create_access_from_symbol_chain(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        chain: &[SymbolId],
        index: usize,
        stopper: usize,
        override_type_arguments: NodeList,
    ) -> Node {
        let ctx = p1_ctx(b);
        let e = p1_e(b);
        let f = e.factory();
        let mut type_parameter_nodes = override_type_arguments;
        if index != chain.len() - 1 {
            type_parameter_nodes = self.lookup_type_parameter_nodes(b, chain, index as i32);
        }
        let symbol = chain[index];
        let parent = if index > 0 {
            chain[index - 1]
        } else {
            SymbolId::NIL
        };

        let mut symbol_name = String::new();
        if index == 0 {
            {
                let mut c = ctx.borrow_mut();
                c.flags = c.flags | NodeBuilderFlags::IN_INITIAL_ENTITY_NAME;
            }
            symbol_name = self.get_name_of_symbol_as_written(b, symbol);
            p1_add_length(b, go_len(&symbol_name) + 1);
            let mut c = ctx.borrow_mut();
            c.flags = NodeBuilderFlags(c.flags.0 ^ NodeBuilderFlags::IN_INITIAL_ENTITY_NAME.0);
        } else {
            // lookup a ref to symbol within parent to handle export aliases
            if parent.is_some() {
                let exports = self.get_exports_of_symbol(parent);
                if exports.is_some() {
                    // avoid exhaustive iteration in the common case
                    let name = self.sym(symbol).name.to_string();
                    let res = self.symbols.get(exports, &name);
                    if name != INTERNAL_SYMBOL_NAME_EXPORT_EQUALS
                        && !is_late_bound_name(&name)
                        && res.is_some()
                        && self.get_symbol_if_same_reference(res, symbol).is_some()
                    {
                        symbol_name = name;
                    } else {
                        // PORT: Go collects into `map[*ast.Symbol]string`. A
                        // later name for the same symbol overwrites an earlier
                        // one, as in the Go map.
                        let mut results: FxHashMap<SymbolId, String> = FxHashMap::default();
                        for (name, ex) in self.symbols.entries(exports) {
                            if self.get_symbol_if_same_reference(ex, symbol).is_some()
                                && !is_late_bound_name(&name)
                                && name != INTERNAL_SYMBOL_NAME_EXPORT_EQUALS
                            {
                                results.insert(ex, name.to_string());
                                // break // must collect all results and sort them - exports are randomly iterated
                            }
                        }
                        let mut result_symbols: Vec<SymbolId> = results.keys().copied().collect();
                        if !result_symbols.is_empty() {
                            self.sort_symbols(&mut result_symbols);
                            symbol_name = results[&result_symbols[0]].clone();
                        }
                    }
                }
            }
        }

        if symbol_name.is_empty() {
            let mut name = Node::NIL;
            for &d in &self.sym(symbol).declarations {
                name = get_name_of_declaration(d);
                if name.is_some() {
                    break;
                }
            }
            if name.is_some()
                && is_computed_property_name(name)
                && is_entity_name(name.expression())
            {
                // PORT: Go passes `index-1`, which is -1 when index is 0 and
                // then panics on the index. The `usize` subtraction panics too.
                let lhs = self.create_access_from_symbol_chain(
                    b,
                    chain,
                    index - 1,
                    stopper,
                    override_type_arguments,
                );
                if is_entity_name(lhs) {
                    return f.new_indexed_access_type_node(
                        f.new_parenthesized_type_node(f.new_type_query_node(lhs, NodeList::NIL)),
                        f.new_type_query_node(name.expression(), NodeList::NIL),
                    );
                }
                return lhs;
            }
            symbol_name = self.get_name_of_symbol_as_written(b, symbol);
        }
        p1_add_length(b, go_len(&symbol_name) + 1);

        if !p1_flags(b).intersects(NodeBuilderFlags::FORBID_INDEXED_ACCESS_SYMBOL_REFERENCES)
            && parent.is_some()
        {
            let members = self.get_members_of_symbol(parent);
            let name = self.sym(symbol).name.clone();
            let member = self.symbols.get(members, &name);
            if members.is_some()
                && member.is_some()
                && self.get_symbol_if_same_reference(member, symbol).is_some()
            {
                // Should use an indexed access
                let lhs = self.create_access_from_symbol_chain(
                    b,
                    chain,
                    index - 1,
                    stopper,
                    override_type_arguments,
                );
                if is_indexed_access_type_node(lhs) {
                    let lit = self.nb_new_string_literal(b, &symbol_name);
                    return f.new_indexed_access_type_node(lhs, f.new_literal_type_node(lit));
                }
                let lit = self.nb_new_string_literal(b, &symbol_name);
                return f.new_indexed_access_type_node(
                    f.new_type_reference_node(lhs, type_parameter_nodes),
                    f.new_literal_type_node(lit),
                );
            }
        }

        let identifier = self.nb_new_identifier(b, &symbol_name, symbol);
        e.add_emit_flags(identifier, EmitFlags::NO_ASCII_ESCAPING);

        if index > stopper {
            let lhs = self.create_access_from_symbol_chain(
                b,
                chain,
                index - 1,
                stopper,
                override_type_arguments,
            );
            if !p1_flags(b).intersects(NodeBuilderFlags::USE_INSTANTIATION_EXPRESSIONS)
                || is_entity_name(lhs)
                    && (type_parameter_nodes.is_nil() || type_parameter_nodes.nodes().is_empty())
            {
                return f.new_qualified_name(lhs, identifier);
            }
            let access = self.create_access_expression(b, lhs);
            let expr =
                f.new_property_access_expression(access, Node::NIL, identifier, NodeFlags::NONE);
            return self.create_expression_with_type_arguments(b, expr, type_parameter_nodes);
        }
        identifier
    }

    // Go: checker/nodebuilderimpl.go:848 symbolToExpression
    pub fn symbol_to_expression(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        symbol: SymbolId,
        mask: SymbolFlags,
    ) -> Node {
        let (tracker, enclosing_declaration) = {
            let ctx = p1_ctx(b);
            let ctx = ctx.borrow();
            (ctx.tracker.clone(), ctx.enclosing_declaration)
        };
        tracker.track_symbol(self, symbol, enclosing_declaration, mask);
        self.symbol_to_expression_worker(b, symbol, mask)
    }

    // Go: checker/nodebuilderimpl.go:853 symbolToExpressionWorker
    pub fn symbol_to_expression_worker(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        symbol: SymbolId,
        mask: SymbolFlags,
    ) -> Node {
        let chain = self.lookup_symbol_chain_worker(b, symbol, mask, false);
        // PORT: see `symbol_to_name` for the empty chain case.
        self.create_expression_from_symbol_chain(b, &chain, chain.len() - 1)
    }

    // Go: checker/nodebuilderimpl.go:858 createExpressionFromSymbolChain
    pub fn create_expression_from_symbol_chain(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        chain: &[SymbolId],
        index: usize,
    ) -> Node {
        let type_parameter_nodes =
            self.lookup_expression_chain_type_argument_nodes(b, chain, index);
        let symbol = chain[index];
        let ctx = p1_ctx(b);
        let e = p1_e(b);
        let f = e.factory();

        if index == 0 {
            let mut c = ctx.borrow_mut();
            c.flags = c.flags | NodeBuilderFlags::IN_INITIAL_ENTITY_NAME;
        }
        let mut symbol_name = self.get_name_of_symbol_as_written(b, symbol);
        if index == 0 {
            let mut c = ctx.borrow_mut();
            c.flags = NodeBuilderFlags(c.flags.0 ^ NodeBuilderFlags::IN_INITIAL_ENTITY_NAME.0);
        }

        if starts_with_single_or_double_quote(&symbol_name)
            && self
                .sym(symbol)
                .declarations
                .iter()
                .any(|&d| has_non_global_augmentation_external_module_symbol(d))
        {
            let specifier_result =
                self.get_specifier_for_module_symbol(b, symbol, ResolutionMode::NONE);
            p1_add_length(b, 2 + go_len(&specifier_result.specifier));
            return self.nb_new_string_literal(b, &specifier_result.specifier);
        }

        if index == 0 || can_use_property_access(&symbol_name) {
            let identifier = self.nb_new_identifier(b, &symbol_name, symbol);
            e.add_emit_flags(identifier, EmitFlags::NO_ASCII_ESCAPING);
            p1_add_length(b, 1 + go_len(&symbol_name));
            if index > 0 {
                let left = self.create_expression_from_symbol_chain(b, chain, index - 1);
                let result =
                    f.new_property_access_expression(left, Node::NIL, identifier, NodeFlags::NONE);
                e.add_emit_flags(result, EmitFlags::NO_INDENTATION);
                return self.create_expression_with_type_arguments(b, result, type_parameter_nodes);
            }
            return self.create_expression_with_type_arguments(b, identifier, type_parameter_nodes);
        }

        if starts_with_square_bracket(&symbol_name) {
            symbol_name = symbol_name[1..symbol_name.len() - 1].to_string();
        }

        let mut expression = Node::NIL;
        if starts_with_single_or_double_quote(&symbol_name)
            && !self.sym(symbol).flags.intersects(SymbolFlags::ENUM_MEMBER)
        {
            let literal_text = unquote_string(&symbol_name);
            p1_add_length(b, go_len(&literal_text) + 2);
            expression =
                self.nb_new_string_literal_ex(b, &literal_text, symbol_name.starts_with('\''));
        } else if crate::jsnum::from_string(&symbol_name).to_string() == symbol_name {
            // TODO: the follwing in strada would assert if the number is negative, but no such assertion exists here
            // Moreover, what's even guaranteeing the name *isn't* -1 here anyway? Needs double-checking.
            p1_add_length(b, go_len(&symbol_name));
            expression = f.new_numeric_literal(symbol_name.clone(), TokenFlags::NONE);
        }
        if expression.is_nil() {
            p1_add_length(b, go_len(&symbol_name));
            expression = self.nb_new_identifier(b, &symbol_name, symbol);
            e.add_emit_flags(expression, EmitFlags::NO_ASCII_ESCAPING);
        }
        p1_add_length(b, 2); // []
        let left = self.create_expression_from_symbol_chain(b, chain, index - 1);
        let access = f.new_element_access_expression(left, Node::NIL, expression, NodeFlags::NONE);
        self.create_expression_with_type_arguments(b, access, type_parameter_nodes)
    }

    // Go: checker/nodebuilderimpl.go:936 getNameOfSymbolFromNameType
    pub fn get_name_of_symbol_from_name_type(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        symbol: SymbolId,
    ) -> String {
        if self.value_symbol_links.has_by_id(&self.symbols, symbol) {
            let name_type = self
                .value_symbol_links
                .get_by_id(&self.symbols, symbol)
                .name_type;
            if name_type.is_nil() {
                return String::new();
            }
            let name_type_flags = self.ty(name_type).flags;
            if name_type_flags.intersects(TypeFlags::STRING_OR_NUMBER_LITERAL) {
                let value = self.ty(name_type).as_literal_type().value.clone();
                let name = match &value {
                    Some(LiteralValue::String(v)) => v.clone(),
                    Some(LiteralValue::Number(v)) => v.to_string(),
                    _ => String::new(),
                };
                if !is_identifier_text(&name, LanguageVariant::STANDARD)
                    && !is_numeric_literal_name(&name)
                {
                    // PORT: Go `valueToString(nil)` panics; `expect` does the same.
                    return value_to_string(value.as_ref().expect("literal type has no value"));
                }
                if is_numeric_literal_name(&name) && name.starts_with('-') {
                    return format!("[{name}]");
                }
                return name;
            }
            if name_type_flags.intersects(TypeFlags::UNIQUE_ES_SYMBOL) {
                // PORT: Go reads `AsUniqueESSymbolType().symbol`, which is the
                // type's symbol. The Rust UniqueESSymbolType data has no copy.
                let unique_symbol = self.ty(name_type).symbol;
                let text = self.get_name_of_symbol_as_written(b, unique_symbol);
                return format!("[{text}]");
            }
        }
        String::new()
    }

    // Go: checker/nodebuilderimpl.go:973 getNameOfSymbolAsWritten
    /**
     * Gets a human-readable name for a symbol.
     * Should *not* be used for the right-hand side of a `.` -- use `symbolName(symbol)` for that instead.
     *
     * Unlike `symbolName(symbol)`, this will include quotes if the name is from a string literal.
     * It will also use a representation of a number as written instead of a decimal form, e.g. `0o11` instead of `9`.
     */
    pub fn get_name_of_symbol_as_written(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        mut symbol: SymbolId,
    ) -> String {
        let ctx = p1_ctx(b);
        // Go keys the map by `ast.GetSymbolId(symbol)`, which gives the symbol
        // its id (`ValueSymbolLinkStore`).
        get_symbol_id(&self.symbols, symbol);
        if let Some(&result) = ctx.borrow().remapped_symbol_references.get(&symbol) {
            symbol = result;
        }
        let (flags, enclosing_declaration) = {
            let c = ctx.borrow();
            (c.flags, c.enclosing_declaration)
        };
        let declarations = self.sym(symbol).declarations.clone();
        if self.sym(symbol).name == INTERNAL_SYMBOL_NAME_DEFAULT
            && !flags.intersects(NodeBuilderFlags::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE)
            // If it's not the first part of an entity name, it must print as `default`
            && (!flags.intersects(NodeBuilderFlags::IN_INITIAL_ENTITY_NAME)
                // if the symbol is synthesized, it will only be referenced externally it must print as `default`
                || declarations.is_empty()
                // if not in the same binding context (source file, module declaration), it must print as `default`
                || (enclosing_declaration.is_some()
                    && find_ancestor(declarations[0], is_default_binding_context)
                        != find_ancestor(enclosing_declaration, is_default_binding_context)))
        {
            return "default".to_string();
        }
        if !declarations.is_empty() {
            let name = declarations
                .iter()
                .map(|&d| get_name_of_declaration(d))
                .find(|n| n.is_some())
                .unwrap_or(Node::NIL); // Try using a declaration with a name, first
            if name.is_some() {
                // !!! TODO: JS Object.defineProperty declarations
                // if ast.IsCallExpression(declaration) && ast.IsBindableObjectDefinePropertyCall(declaration) {
                // 	return symbol.Name
                // }
                if is_computed_property_name(name)
                    && !self.sym(symbol).check_flags.intersects(CheckFlags::LATE)
                {
                    if self.value_symbol_links.has_by_id(&self.symbols, symbol) {
                        let name_type = self
                            .value_symbol_links
                            .get_by_id(&self.symbols, symbol)
                            .name_type;
                        if name_type.is_some()
                            && self
                                .ty(name_type)
                                .flags
                                .intersects(TypeFlags::STRING_OR_NUMBER_LITERAL)
                        {
                            let result = self.get_name_of_symbol_from_name_type(b, symbol);
                            if !result.is_empty() {
                                return result;
                            }
                        }
                    }
                }
                return declaration_name_to_string(name);
            }
            let declaration = declarations[0]; // Declaration may be nameless, but we'll try anyway
            if declaration.parent().is_some()
                && declaration.parent().kind() == SyntaxKind::VariableDeclaration
            {
                return declaration_name_to_string(declaration.parent().name());
            }
            if is_class_expression(declaration)
                || is_function_expression(declaration)
                || is_arrow_function(declaration)
            {
                // PORT: Go checks `b.ctx != nil`. The Rust context always exists.
                {
                    let mut c = ctx.borrow_mut();
                    if !c.encountered_error
                        && !c
                            .flags
                            .intersects(NodeBuilderFlags::ALLOW_ANONYMOUS_IDENTIFIER)
                    {
                        c.encountered_error = true;
                    }
                }
                match declaration.kind() {
                    SyntaxKind::ClassExpression => return "(Anonymous class)".to_string(),
                    SyntaxKind::FunctionExpression | SyntaxKind::ArrowFunction => {
                        return "(Anonymous function)".to_string();
                    }
                    _ => {}
                }
            }
        }
        let name = self.get_name_of_symbol_from_name_type(b, symbol);
        if !name.is_empty() {
            return name;
        }
        escape_internal_symbol_name(self.sym(symbol).name.as_str())
    }

    // Go: checker/nodebuilderimpl.go:1029 getTypeParametersOfClassOrInterface
    // The full set of type parameters for a generic class or interface type consists of its outer type parameters plus
    // its locally declared type parameters.
    pub fn get_type_parameters_of_class_or_interface(
        &mut self,
        _b: &Rc<RefCell<NodeBuilderImpl>>,
        symbol: SymbolId,
    ) -> Vec<TypeId> {
        let mut result: Vec<TypeId> = Vec::new();
        result.extend(self.get_outer_type_parameters_of_class_or_interface(symbol));
        result.extend(self.get_local_type_parameters_of_class_or_interface_or_type_alias(symbol));
        result
    }
}

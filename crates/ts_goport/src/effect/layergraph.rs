//! Port of Effect-TS/tsgo `internal/layergraph` (`types.go`, `extract.go`,
//! `outline.go`, `format.go`, `mermaidurl.go`, `providers.go`, `magic.go`). It models
//! how Effect Layers are composed in source code. It builds directed graphs
//! where nodes represent Layer expressions and edges represent composition
//! relationships (pipe, call, array literal, symbol). The Layer hover
//! (`effect::ls::hover`) formats them as text and Mermaid diagrams.
//!
//! `magic.go` (lane E9d) orders the outline graph for the layerMagic
//! refactor (`effect::ls::refactors::layer_magic`).

use crate::effect::graph::{Direction, Graph, MermaidOptions, NodeIndex};
use crate::effect::typeparser::*;
use crate::gostd::slices::sort_slice;
use crate::prelude::*;

// ---------------------------------------------------------------------------
// types.go
// ---------------------------------------------------------------------------

/// Go `LayerGraphNodeInfo`: holds data for each node in the layer graph.
#[derive(Clone, Debug, Default)]
pub struct LayerGraphNodeInfo {
    /// The AST expression node
    pub node: Node,
    /// The node used for display (e.g. variable name instead of initializer)
    pub display_node: Node,
    /// Parsed Layer type (ROut, E, RIn) or nil
    pub layer_type: Option<Rc<Layer>>,
    /// Individual service types from ROut (unrolled intersection, Never filtered)
    pub provides: Vec<TypeId>,
    /// Provides minus pass-through (types also in Requires)
    pub actual_provides: Vec<TypeId>,
    /// Individual service types from RIn (unrolled intersection, Never filtered)
    pub requires: Vec<TypeId>,
}

/// Go `EdgeRelationship`: the type of relationship between two nodes in the
/// layer graph.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum EdgeRelationship {
    /// Go `EdgeRelationshipCall`
    #[default]
    Call,
    /// Go `EdgeRelationshipPipe`
    Pipe,
    /// Go `EdgeRelationshipArrayLiteral`
    ArrayLiteral,
    /// Go `EdgeRelationshipSymbol`
    Symbol,
}

impl EdgeRelationship {
    /// The Go string value.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            EdgeRelationship::Call => "call",
            EdgeRelationship::Pipe => "pipe",
            EdgeRelationship::ArrayLiteral => "arrayLiteral",
            EdgeRelationship::Symbol => "symbol",
        }
    }
}

/// Go `LayerGraphEdgeInfo`: describes the relationship between two nodes in
/// the layer graph. It is a discriminated union keyed on Relationship.
// PORT: Go's zero `Relationship` is "", which no Go code creates; the port
// has no empty variant.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LayerGraphEdgeInfo {
    /// One of: "call", "pipe", "arrayLiteral", "symbol"
    pub relationship: EdgeRelationship,
    /// Only meaningful for "call" edges (0-based argument position)
    pub argument_index: i32,
    /// Only meaningful for "arrayLiteral" edges (0-based element position)
    pub index: i32,
}

impl LayerGraphEdgeInfo {
    // Go: LayerGraphEdgeInfo.MarshalJSON
    /// Produces JSON matching the reference format, only including fields
    /// relevant to each relationship type:
    ///
    /// ```text
    /// {"relationship":"call","argumentIndex":0}
    /// {"relationship":"pipe"}
    /// {"relationship":"arrayLiteral","index":0}
    /// {"relationship":"symbol"}
    /// ```
    // PORT: the relationship strings need no JSON escaping, so the port
    // writes the text that Go's `json.Marshal` gives.
    #[must_use]
    pub fn marshal_json(&self) -> String {
        match self.relationship {
            EdgeRelationship::Call => format!(
                "{{\"relationship\":\"{}\",\"argumentIndex\":{}}}",
                self.relationship.as_str(),
                self.argument_index
            ),
            EdgeRelationship::ArrayLiteral => format!(
                "{{\"relationship\":\"{}\",\"index\":{}}}",
                self.relationship.as_str(),
                self.index
            ),
            _ => format!("{{\"relationship\":\"{}\"}}", self.relationship.as_str()),
        }
    }
}

/// Go `LayerOutlineGraphNodeInfo`: holds data for nodes in the simplified
/// outline graph.
#[derive(Clone, Debug, Default)]
pub struct LayerOutlineGraphNodeInfo {
    /// The AST node
    pub node: Node,
    /// The node used for display
    pub display_node: Node,
    /// Service types provided
    pub provides: Vec<TypeId>,
    /// Actual (non-pass-through) provides
    pub actual_provides: Vec<TypeId>,
    /// Service types required
    pub requires: Vec<TypeId>,
}

/// Go `LayerMagicNode`: represents a single node in the layer magic result,
/// annotated with flags that determine which Layer.* combinator to use.
#[derive(Clone, Debug, Default)]
pub struct LayerMagicNode {
    /// Whether this node should be merged (provides a target output type)
    pub merges: bool,
    /// Whether this node provides services
    pub provides: bool,
    /// The layer expression AST node
    pub node: Node,
    /// Service types provided
    pub provided_types: Vec<TypeId>,
    /// Actual (non-pass-through) provides
    pub actual_provided_types: Vec<TypeId>,
    /// Service types required
    pub required_types: Vec<TypeId>,
}

/// Go `LayerMagicResult`: holds the output of ConvertOutlineGraphToLayerMagic.
#[derive(Clone, Debug, Default)]
pub struct LayerMagicResult {
    /// Ordered list of layer nodes with merge/provide flags
    pub nodes: Vec<LayerMagicNode>,
    /// Target output types not satisfied by any node
    pub missing_output_types: Vec<TypeId>,
}

/// Go `ProviderRequirerKind`: distinguishes between providers and requirers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProviderRequirerKind {
    /// Go `ProviderRequirerKindProvided`
    Provided,
    /// Go `ProviderRequirerKindRequired`
    Required,
}

impl ProviderRequirerKind {
    /// The Go string value.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderRequirerKind::Provided => "provided",
            ProviderRequirerKind::Required => "required",
        }
    }
}

/// Go `ProviderRequirerInfo`: summarizes which leaf layers provide or
/// require a service type.
#[derive(Clone, Debug)]
pub struct ProviderRequirerInfo {
    /// "provided" or "required"
    pub kind: ProviderRequirerKind,
    /// The service type
    pub type_: TypeId,
    /// The leaf nodes that provide/require this type
    pub nodes: Vec<Node>,
    /// Display nodes for those leaves
    pub display_nodes: Vec<Node>,
}

/// Go `ExtractLayerGraphOptions`: controls the behavior of ExtractLayerGraph.
#[derive(Clone, Copy, Debug, Default)]
pub struct ExtractLayerGraphOptions {
    /// Treat array literals [l1, l2] as implicit Layer.mergeAll (default: false)
    pub array_literal_as_merge: bool,
    /// Only explode calls to Layer module APIs (default: false)
    pub explode_only_layer_calls: bool,
    /// How many levels deep to follow identifier references (default: 0)
    pub follow_symbols_depth: i32,
    /// Do not decompose pipe/call/array expressions (default: false)
    pub skip_explode: bool,
}

// ---------------------------------------------------------------------------
// extract.go
// ---------------------------------------------------------------------------

/// Go `workItem`: represents a node to visit in the DFS work queue.
#[derive(Clone, Copy, Debug)]
pub struct WorkItem {
    pub node: Node,
    pub depth: i32,
}

// Go: ExtractLayerGraph
/// Builds a directed graph of Layer composition from the given AST nodes.
/// It performs a DFS with a two-pass approach using an explicit work stack:
///   - First pass: push children for processing
///   - Second pass: link processed children into the graph
pub fn extract_layer_graph(
    tp: &mut TypeParser<'_>,
    nodes: &[Node],
    sf: Node,
    opts: ExtractLayerGraphOptions,
) -> Graph<LayerGraphNodeInfo, LayerGraphEdgeInfo> {
    let mut g: Graph<LayerGraphNodeInfo, LayerGraphEdgeInfo> = Graph::new();
    let mut node_to_graph_index: FxHashMap<Node, NodeIndex> = FxHashMap::default();
    let mut visited_nodes: FxHashMap<Node, bool> = FxHashMap::default();
    let mut node_in_pipe_context: FxHashMap<Node, bool> = FxHashMap::default();
    let mut depth_budget: FxHashMap<Node, i32> = FxHashMap::default();

    // Resolve the Layer module import name for ExplodeOnlyLayerCalls checks.
    let mut layer_module_name = String::new();
    if !opts.skip_explode {
        layer_module_name = find_layer_module_name(sf);
    }

    // Initialize the work stack with the root nodes.
    let mut stack: Vec<WorkItem> = Vec::new();

    fn append_node_to_visit(
        depth_budget: &mut FxHashMap<Node, i32>,
        stack: &mut Vec<WorkItem>,
        n: Node,
        depth: i32,
    ) {
        depth_budget.insert(n, depth);
        stack.push(WorkItem { node: n, depth });
    }
    for &node in nodes {
        append_node_to_visit(
            &mut depth_budget,
            &mut stack,
            node,
            opts.follow_symbols_depth,
        );
    }

    fn add_node(
        g: &mut Graph<LayerGraphNodeInfo, LayerGraphEdgeInfo>,
        node_to_graph_index: &mut FxHashMap<Node, NodeIndex>,
        n: Node,
        info: LayerGraphNodeInfo,
    ) -> NodeIndex {
        let idx = g.add_node(info);
        node_to_graph_index.insert(n, idx);
        idx
    }

    while let Some(item) = stack.pop() {
        // Pop from the stack (LIFO).
        let current = item.node;
        let current_depth = depth_budget.get(&current).copied().unwrap_or(0);
        let in_pipe = |m: &FxHashMap<Node, bool>| m.get(&current).copied().unwrap_or(false);
        let visited = visited_nodes.get(&current).copied().unwrap_or(false);

        // Case 1: Pipe detection
        if !opts.skip_explode {
            if let Some(pipe_result) = tp.parse_pipe_call(current) {
                if !visited {
                    // First pass: push self back, then subject and args.
                    append_node_to_visit(&mut depth_budget, &mut stack, current, current_depth);
                    append_node_to_visit(
                        &mut depth_budget,
                        &mut stack,
                        pipe_result.subject,
                        current_depth,
                    );
                    for &arg in &pipe_result.args {
                        append_node_to_visit(&mut depth_budget, &mut stack, arg, current_depth);
                        node_in_pipe_context.insert(arg, true);
                    }
                    visited_nodes.insert(current, true);
                } else {
                    // Second pass: collect child graph indices.
                    let mut all_children: Vec<Node> =
                        Vec::with_capacity(1 + pipe_result.args.len());
                    all_children.push(pipe_result.subject);
                    all_children.extend(pipe_result.args.iter().copied());

                    let child_indices =
                        collect_child_indices(&all_children, &node_to_graph_index, &g);

                    if child_indices.len() == all_children.len() {
                        // All members are graph nodes — link them sequentially.
                        let mut last_idx: NodeIndex = 0;
                        for (i, &child_idx) in child_indices.iter().enumerate() {
                            if i > 0 {
                                g.add_edge(
                                    child_idx,
                                    last_idx,
                                    LayerGraphEdgeInfo {
                                        relationship: EdgeRelationship::Pipe,
                                        ..Default::default()
                                    },
                                );
                            }
                            last_idx = child_idx;
                        }
                        // Add a node for the pipe call itself, linking to the last child.
                        let info =
                            extract_node_info(tp, current, sf, in_pipe(&node_in_pipe_context));
                        let pipe_idx = add_node(&mut g, &mut node_to_graph_index, current, info);
                        g.add_edge(
                            pipe_idx,
                            last_idx,
                            LayerGraphEdgeInfo {
                                relationship: EdgeRelationship::Pipe,
                                ..Default::default()
                            },
                        );
                    } else {
                        // Not all children are graph nodes — remove partial children and try as leaf.
                        remove_partial_children(&all_children, &mut node_to_graph_index, &mut g);
                        let info =
                            extract_node_info(tp, current, sf, in_pipe(&node_in_pipe_context));
                        if info.layer_type.is_some() {
                            add_node(&mut g, &mut node_to_graph_index, current, info);
                        }
                    }
                }
                continue;
            }
        }

        // Case 2: Call expression
        if !opts.skip_explode && current.kind() == SyntaxKind::CallExpression {
            let call_expr = current;
            let mut should_explode = !opts.explode_only_layer_calls;
            if opts.explode_only_layer_calls {
                if is_layer_module_call(call_expr, &layer_module_name) {
                    should_explode = true;
                }
            }
            if should_explode {
                let args: Vec<Node> = call_expr.arguments().to_vec();
                if !visited {
                    // First pass: push self back, then all arguments.
                    append_node_to_visit(&mut depth_budget, &mut stack, current, current_depth);
                    for &arg in &args {
                        append_node_to_visit(&mut depth_budget, &mut stack, arg, current_depth);
                    }
                    visited_nodes.insert(current, true);
                } else {
                    // Second pass: collect child graph indices.
                    let child_indices = collect_child_indices(&args, &node_to_graph_index, &g);

                    if child_indices.len() == args.len() {
                        // All arguments are graph nodes.
                        let info =
                            extract_node_info(tp, current, sf, in_pipe(&node_in_pipe_context));
                        let call_idx = add_node(&mut g, &mut node_to_graph_index, current, info);
                        for (i, &child_idx) in child_indices.iter().enumerate() {
                            g.add_edge(
                                call_idx,
                                child_idx,
                                LayerGraphEdgeInfo {
                                    relationship: EdgeRelationship::Call,
                                    argument_index: i as i32,
                                    ..Default::default()
                                },
                            );
                        }
                    } else {
                        // Not all arguments are graph nodes.
                        remove_partial_children(&args, &mut node_to_graph_index, &mut g);
                        let info =
                            extract_node_info(tp, current, sf, in_pipe(&node_in_pipe_context));
                        if info.layer_type.is_some() {
                            add_node(&mut g, &mut node_to_graph_index, current, info);
                        }
                    }
                }
                continue;
            }
        }

        // Case 3: Array literal (when ArrayLiteralAsMerge is enabled)
        if !opts.skip_explode
            && opts.array_literal_as_merge
            && current.kind() == SyntaxKind::ArrayLiteralExpression
        {
            let array_expr = current;
            let elements: Vec<Node> = array_expr.elements().to_vec();
            if !visited {
                // First pass: push self back, then all elements.
                append_node_to_visit(&mut depth_budget, &mut stack, current, current_depth);
                for &elem in &elements {
                    append_node_to_visit(&mut depth_budget, &mut stack, elem, current_depth);
                }
                visited_nodes.insert(current, true);
            } else {
                // Second pass: collect child graph indices (doesn't require ALL).
                let child_indices = collect_child_indices(&elements, &node_to_graph_index, &g);
                if !child_indices.is_empty() {
                    let info = extract_node_info(tp, current, sf, in_pipe(&node_in_pipe_context));
                    let array_idx = add_node(&mut g, &mut node_to_graph_index, current, info);
                    for (i, &child_idx) in child_indices.iter().enumerate() {
                        g.add_edge(
                            array_idx,
                            child_idx,
                            LayerGraphEdgeInfo {
                                relationship: EdgeRelationship::ArrayLiteral,
                                index: i as i32,
                                ..Default::default()
                            },
                        );
                    }
                }
            }
            continue;
        }

        // Case 4: Symbol following
        if current_depth > 0 && is_simple_identifier(current) {
            let mut sym = tp.get_symbol_at_location(current);
            if sym.is_some() {
                if tp.checker.sym(sym).flags.intersects(SymbolFlags::ALIAS) {
                    let resolved = tp.checker.skip_alias(sym);
                    if resolved.is_some() {
                        sym = resolved;
                    }
                }
                if tp.checker.sym(sym).declarations.len() == 1 {
                    let decl = tp.checker.sym(sym).declarations[0];
                    let decl_node = get_adjusted_node(decl);
                    if decl_node.is_some() {
                        if !visited {
                            // First pass: push self back, push declaration with decremented depth.
                            append_node_to_visit(
                                &mut depth_budget,
                                &mut stack,
                                current,
                                current_depth,
                            );
                            append_node_to_visit(
                                &mut depth_budget,
                                &mut stack,
                                decl_node,
                                current_depth - 1,
                            );
                            visited_nodes.insert(current, true);
                            continue;
                        }
                        // Second pass: link to declaration if it's in the graph.
                        if let Some(&child_idx) = node_to_graph_index.get(&decl_node) {
                            let info =
                                extract_node_info(tp, current, sf, in_pipe(&node_in_pipe_context));
                            let ident_idx =
                                add_node(&mut g, &mut node_to_graph_index, current, info);
                            g.add_edge(
                                ident_idx,
                                child_idx,
                                LayerGraphEdgeInfo {
                                    relationship: EdgeRelationship::Symbol,
                                    ..Default::default()
                                },
                            );
                            continue;
                        }
                    }
                }
            }
        }

        // Case 5: Leaf node (base case)
        if is_expression(current) {
            let info = extract_node_info(tp, current, sf, in_pipe(&node_in_pipe_context));
            if info.layer_type.is_some() {
                add_node(&mut g, &mut node_to_graph_index, current, info);
            }
        }
    }

    g
}

// Go: extractNodeInfo
/// Computes the LayerGraphNodeInfo for a given AST node.
fn extract_node_info(
    tp: &mut TypeParser<'_>,
    node: Node,
    _sf: Node,
    in_pipe_context: bool,
) -> LayerGraphNodeInfo {
    let mut info = LayerGraphNodeInfo {
        node,
        display_node: get_display_node(node),
        ..Default::default()
    };

    // Get the type of the node.
    // When in a pipe context, resolve the contextual type's return type
    // (the pipe argument is a function whose return type is the Layer).
    let mut t = TypeId::NIL;
    if in_pipe_context && is_expression(node) {
        let contextual_type = tp
            .checker
            .get_contextual_type_exported(node, ContextFlags::NONE);
        if contextual_type.is_some() {
            let call_signatures = tp
                .checker
                .get_signatures_of_type_exported(contextual_type, SignatureKind::CALL);
            if call_signatures.len() == 1 {
                t = tp
                    .checker
                    .get_return_type_of_signature_exported(call_signatures[0]);
            }
        }
    } else {
        t = tp.get_type_at_location(node);
    }
    if t.is_nil() {
        return info;
    }

    // Parse as Layer type.
    let Some(layer) = tp.layer_type(t) else {
        return info;
    };
    info.layer_type = Some(layer.clone());

    // Unroll provides and requires, filtering out Never types.
    for p in tp.unroll_union_members(layer.r_out) {
        if !tp.checker.ty(p).flags.intersects(TypeFlags::NEVER) {
            info.provides.push(p);
        }
    }
    for r in tp.unroll_union_members(layer.r_in) {
        if !tp.checker.ty(r).flags.intersects(TypeFlags::NEVER) {
            info.requires.push(r);
        }
    }

    // ActualProvides: provides that are NOT assignable to the RIn type.
    for &p in &info.provides {
        if !tp.checker.is_type_assignable_to(p, layer.r_in) {
            info.actual_provides.push(p);
        }
    }

    // Sort all type lists deterministically by their string representation.
    // NOTE: Intentional divergence from .repos reference. The TypeScript implementation
    // preserves union member order in output/nested formats (using deterministicTypeOrder
    // only for providers/requirers extraction and quickinfo). We sort alphabetically
    // everywhere for consistent determinism independent of compiler-internal union ordering.
    let c = &mut *tp.checker;
    let mut sort_types = |types: &mut Vec<TypeId>| {
        sort_slice(types, |a, b| c.type_to_string(*a) < c.type_to_string(*b));
    };
    sort_types(&mut info.provides);
    sort_types(&mut info.requires);
    sort_types(&mut info.actual_provides);

    info
}

// Go: getDisplayNode
/// Returns the node used for display purposes.
/// If the node's parent is a variable declaration and the node is the initializer,
/// use the variable's name node instead.
fn get_display_node(node: Node) -> Node {
    if node.parent().is_some() && node.parent().kind() == SyntaxKind::VariableDeclaration {
        let var_decl = node.parent();
        if var_decl.initializer().is_some() && var_decl.initializer() == node {
            return var_decl.name();
        }
    }
    if node.parent().is_some() && node.parent().kind() == SyntaxKind::PropertyDeclaration {
        let prop_decl = node.parent();
        if prop_decl.initializer().is_some() && prop_decl.initializer() == node {
            return prop_decl.name();
        }
    }
    node
}

// Go: getAdjustedNode
/// Extracts the initializer expression from a declaration node to follow
/// during symbol resolution.
fn get_adjusted_node(node: Node) -> Node {
    match node.kind() {
        SyntaxKind::VariableDeclaration => node.initializer(),
        SyntaxKind::PropertyDeclaration => node.initializer(),
        _ => {
            if is_expression(node) {
                return node;
            }
            Node::NIL
        }
    }
}

// Go: isSimpleIdentifier
/// Checks if a node is a simple identifier or a chain of property accesses
/// with simple identifiers (e.g., `a`, `a.b`, `a.b.c`).
fn is_simple_identifier(node: Node) -> bool {
    if node.kind() == SyntaxKind::Identifier {
        return true;
    }
    if node.kind() == SyntaxKind::PropertyAccessExpression {
        let prop = node;
        return prop.name().is_some()
            && prop.name().kind() == SyntaxKind::Identifier
            && is_simple_identifier(prop.expression());
    }
    false
}

// Go: isLayerModuleCall
/// Checks if a call expression's callee is a Layer module API call
/// (e.g., Layer.provide, Layer.merge, etc.).
fn is_layer_module_call(call_expr: Node, layer_module_name: &str) -> bool {
    let expr = call_expr.expression();
    if expr.kind() != SyntaxKind::PropertyAccessExpression {
        return false;
    }
    let prop_access = expr;
    if prop_access.expression().kind() != SyntaxKind::Identifier {
        return false;
    }
    get_text_of_node(prop_access.expression()) == layer_module_name
}

// Go: findLayerModuleName
/// Resolves the imported identifier name for the "Layer" export from the
/// "effect" package. Falls back to "Layer" if not found.
fn find_layer_module_name(sf: Node) -> String {
    if sf.is_nil() {
        return "Layer".to_string();
    }
    for stmt in sf.statements() {
        if stmt.kind() != SyntaxKind::ImportDeclaration {
            continue;
        }
        let import_decl = stmt;
        if import_decl.module_specifier().is_nil() {
            continue;
        }
        let mut module_name = get_text_of_node(import_decl.module_specifier());
        // Strip quotes from module specifier.
        let bytes = module_name.as_bytes();
        if bytes.len() >= 2 && (bytes[0] == b'"' || bytes[0] == b'\'') {
            // PORT: Go slices bytes; the result is only compared with "effect".
            module_name = String::from_utf8_lossy(&bytes[1..bytes.len() - 1]).into_owned();
        }
        if module_name != "effect" {
            continue;
        }
        // Look for named imports.
        if import_decl.import_clause().is_nil() {
            continue;
        }
        let clause = import_decl.import_clause();
        if clause.named_bindings().is_nil()
            || clause.named_bindings().kind() != SyntaxKind::NamedImports
        {
            continue;
        }
        let named_imports = clause.named_bindings();
        for elem in named_imports.elements() {
            let spec = elem;
            // The "real" name is PropertyName if set (import { Layer as X }),
            // otherwise Name.
            let imported_name = if spec.property_name().is_some() {
                get_text_of_node(spec.property_name())
            } else {
                get_text_of_node(spec.name())
            };
            if imported_name == "Layer" {
                return get_text_of_node(spec.name());
            }
        }
    }
    "Layer".to_string()
}

// Go: collectChildIndices
/// Gathers graph node indices for the given AST nodes, filtering out nodes
/// that aren't in the graph.
fn collect_child_indices(
    nodes: &[Node],
    node_to_graph_index: &FxHashMap<Node, NodeIndex>,
    g: &Graph<LayerGraphNodeInfo, LayerGraphEdgeInfo>,
) -> Vec<NodeIndex> {
    let mut indices = Vec::new();
    for n in nodes {
        if let Some(&idx) = node_to_graph_index.get(n) {
            if g.has_node(idx) {
                indices.push(idx);
            }
        }
    }
    indices
}

// Go: removePartialChildren
/// Removes graph nodes for the given AST nodes.
fn remove_partial_children(
    nodes: &[Node],
    node_to_graph_index: &mut FxHashMap<Node, NodeIndex>,
    g: &mut Graph<LayerGraphNodeInfo, LayerGraphEdgeInfo>,
) {
    for n in nodes {
        if let Some(&idx) = node_to_graph_index.get(n) {
            g.remove_node(idx);
            node_to_graph_index.remove(n);
        }
    }
}

// ---------------------------------------------------------------------------
// outline.go
// ---------------------------------------------------------------------------

// Go: ExtractOutlineGraph
/// Builds a simplified dependency graph from the full layer graph.
/// It keeps only leaf nodes (deduplicated by symbol) and connects them based on
/// type compatibility: if node A requires a service that node B provides, add an edge.
// PORT: Go returns a `*graph.Graph` that is never nil; the port returns the
// graph by value.
pub fn extract_outline_graph(
    tp: &mut TypeParser<'_>,
    layer_graph: &Graph<LayerGraphNodeInfo, LayerGraphEdgeInfo>,
) -> Graph<LayerOutlineGraphNodeInfo, ()> {
    let mut g: Graph<LayerOutlineGraphNodeInfo, ()> = Graph::new();

    // Track providers: providedType → list of outline node indices that provide it.
    // PORT: Go walks this map in random order before the sort below; the
    // port keeps insertion order, which is one of Go's orders.
    let mut providers: IndexMap<TypeId, Vec<NodeIndex>> = IndexMap::new();
    // Track seen symbols for deduplication.
    let mut known_symbols: FxHashSet<SymbolId> = FxHashSet::default();

    // Step 1: Get leaf nodes (nodes with no outgoing edges) and deduplicate by symbol.
    let leaf_nodes: Vec<LayerGraphNodeInfo> = layer_graph
        .externals(Direction::Outgoing)
        .map(|(_, n)| n.clone())
        .collect();
    let mut deduped_leaf_nodes: Vec<LayerGraphNodeInfo> = Vec::new();
    for leaf_node in leaf_nodes {
        let sym = tp.get_symbol_at_location(leaf_node.node);
        if sym.is_nil() {
            deduped_leaf_nodes.push(leaf_node);
        } else if !known_symbols.contains(&sym) {
            deduped_leaf_nodes.push(leaf_node);
            known_symbols.insert(sym);
        }
    }

    // Step 2: Create outline nodes and build the provider map.
    for leaf_node in &deduped_leaf_nodes {
        let node_index = g.add_node(LayerOutlineGraphNodeInfo {
            node: leaf_node.node,
            display_node: leaf_node.display_node,
            provides: leaf_node.provides.clone(),
            actual_provides: leaf_node.actual_provides.clone(),
            requires: leaf_node.requires.clone(),
        });
        for &provided_type in &leaf_node.actual_provides {
            providers.entry(provided_type).or_default().push(node_index);
        }
    }

    // Sort provider types alphabetically for deterministic edge ordering. This
    // intentionally diverges from the .repos reference implementation which uses
    // non-deterministic checker iteration order, to avoid flaky output from Go's
    // map iteration.
    let mut sorted_provider_types: Vec<TypeId> = providers.keys().copied().collect();
    {
        let c = &mut *tp.checker;
        sort_slice(&mut sorted_provider_types, |a, b| {
            c.type_to_string(*a) < c.type_to_string(*b)
        });
    }

    // Step 3: Connect requires to providers based on type assignability.
    let outline_nodes: Vec<(NodeIndex, Vec<TypeId>)> = g
        .nodes()
        .map(|(idx, info)| (idx, info.requires.clone()))
        .collect();
    for (node_index, requires) in outline_nodes {
        for &required_type in &requires {
            for &provided_type in &sorted_provider_types {
                let provider_node_indices =
                    providers.get(&provided_type).cloned().unwrap_or_default();
                if required_type == provided_type
                    || tp
                        .checker
                        .is_type_assignable_to(required_type, provided_type)
                {
                    for provider_node_index in provider_node_indices {
                        if !g.has_edge(node_index, provider_node_index) {
                            g.add_edge(node_index, provider_node_index, ());
                        }
                    }
                }
            }
        }
    }

    g
}

// ---------------------------------------------------------------------------
// format.go
// ---------------------------------------------------------------------------

// Go: format.go:16 mermaidEntityEncode
/// Replaces Mermaid-special characters with Mermaid HTML entity codes.
/// Uses single-pass character-by-character replacement to avoid double-encoding.
// PORT: Go ranges over the runes of `s`, so it writes U+FFFD for each byte
// that is not valid UTF-8 (`go_runes`).
fn mermaid_entity_encode(s: &str) -> String {
    let mut b = String::with_capacity(s.len());
    for r in go_runes(s) {
        match r {
            '#' => b.push_str("#35;"),
            '"' => b.push_str("#quot;"),
            '<' => b.push_str("#lt;"),
            '>' => b.push_str("#gt;"),
            '(' => b.push_str("#40;"),
            ')' => b.push_str("#41;"),
            '{' => b.push_str("#123;"),
            '}' => b.push_str("#125;"),
            _ => b.push(r),
        }
    }
    b
}

// Go: format.go:46 mermaidSafe
/// Encodes a string for use in Mermaid subgraph headers (nested format).
/// It normalizes whitespace, truncates, and encodes only quotes and angle brackets.
// PORT: Go `strings.Fields` and `strings.TrimSpace` split and trim at Unicode
// White_Space, as `split_whitespace` and `trim` do. The cut at 50 is a Go
// byte cut (`go_len`, `go_slice`).
fn mermaid_safe(value: &str) -> String {
    let value = value.replace('\n', " ");
    // Collapse whitespace to single spaces.
    let mut value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if go_len(&value) > 50 {
        value = go_slice(&value, 0, 50).into_owned();
    }
    let value = value
        .replace('"', "#quot;")
        .replace('<', "#lt;")
        .replace('>', "#gt;");
    value.trim().to_string()
}

// Go: format.go:62 getNodeText
/// Returns the trimmed source text of a node.
fn get_node_text(node: Node, sf: Node) -> String {
    if node.is_some() {
        let parent = node.parent();
        if parent.is_some()
            && parent.kind() == SyntaxKind::PropertyDeclaration
            && parent.name() == node
            && parent.parent().is_some()
            && parent.parent().kind() == SyntaxKind::ClassDeclaration
        {
            let class_name = parent.parent().name();
            if class_name.is_some() {
                let class_text = get_source_text_of_node_from_source_file(sf, class_name, false);
                let prop_text = get_source_text_of_node_from_source_file(sf, node, false);
                let (class_text, prop_text) = (class_text.trim(), prop_text.trim());
                if !class_text.is_empty() && !prop_text.is_empty() {
                    return format!("{class_text}.{prop_text}");
                }
            }
        }
    }
    get_source_text_of_node_from_source_file(sf, node, false)
        .trim()
        .to_string()
}

// Go: format.go:76 getNodeSourceFile
/// Walks up the parent chain to find the SourceFile ancestor.
fn get_node_source_file(node: Node) -> Node {
    let mut n = node;
    while n.is_some() {
        if n.kind() == SyntaxKind::SourceFile {
            return n;
        }
        n = n.parent();
    }
    Node::NIL
}

// Go: format.go:90 formatLocation
/// Returns a human-readable location string for a node.
/// If the node is in the same source file as fromSourceFile, returns "ln {line} col {character}".
/// If different, returns "in {filename} at ln {line} col {character}".
fn format_location(node: Node, from_source_file: Node) -> String {
    let node_source_file = get_node_source_file(node);
    if node_source_file.is_nil() {
        return String::new();
    }
    let pos = get_token_pos_of_node(node, node_source_file, false);
    let (line, character) = get_ecma_line_and_utf16_character_of_position(node_source_file, pos);
    if from_source_file.is_nil() || node_source_file == from_source_file {
        return format!("ln {} col {character}", line + 1);
    }
    let mut file_name = source_file_file_name(node_source_file);
    if let Some(idx) = file_name.rfind('/') {
        file_name = &file_name[idx + 1..];
    }
    format!("in {file_name} at ln {} col {character}", line + 1)
}

// Go: format.go:108 getTypeDisplayName
/// Returns the string representation of a type with no truncation.
fn get_type_display_name(c: &mut Checker, t: TypeId) -> String {
    c.type_to_string_ex(t, Node::NIL, TypeFormatFlags::NO_TRUNCATION, None)
}

// Go: format.go:114 mermaidEscapeOutputLabel
/// Applies standard Mermaid escaping for backslash, brackets, and pipe
/// on top of entity-encoded text. This is used for the output format.
fn mermaid_escape_output_label(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace('|', "\\|")
}

// Go: format.go:128 FormatLayerGraph
/// Renders the full layer graph as a Mermaid flowchart TD with entity-encoded labels.
/// Intentional divergence from .repos reference: this output format includes cross-file location
/// annotations (e.g., "_in simple.ts at ln 6 col 19_") on nodes originating from a different source
/// file. The .repos reference omits location annotations from the output (flat graph) format, only
/// including them in nested, outline, and quickinfo formats. The Go version adds them here as an
/// enhancement to help users locate cross-file nodes in the flat graph.
// PORT: no port caller yet. In Go only the layers baselines of the Effect
// test runner use it.
pub fn format_layer_graph(
    c: &mut Checker,
    layer_graph: &Graph<LayerGraphNodeInfo, LayerGraphEdgeInfo>,
    sf: Node,
) -> String {
    let mut lines: Vec<String> = vec!["flowchart TD".to_string()];

    // Nodes in index order.
    for (idx, node_info) in layer_graph.nodes() {
        let node_source_file = get_node_source_file(node_info.node);
        let text = get_node_text(node_info.node, node_source_file);

        let provides: Vec<String> = node_info
            .provides
            .iter()
            .map(|&t| get_type_display_name(c, t))
            .collect();
        let requires: Vec<String> = node_info
            .requires
            .iter()
            .map(|&t| get_type_display_name(c, t))
            .collect();

        let mut label = format!(
            "{text}\nprovides: {}\nrequires: {}",
            provides.join(", "),
            requires.join(", ")
        );
        if node_source_file.is_some() && node_source_file != sf {
            label += &format!("\n_{}_", format_location(node_info.node, sf));
        }
        let label = mermaid_entity_encode(&label).replace('\n', "<br/>");
        let label = mermaid_escape_output_label(&label);
        lines.push(format!("  {idx}[\"{label}\"]"));
    }

    // Edges in index order.
    // PORT: Go `json.Marshal(edge.Data)` calls `MarshalJSON`, whose text
    // needs no further escaping; the error branch is not reachable.
    for (_, edge) in layer_graph.edges() {
        let edge_label = mermaid_entity_encode(&edge.data.marshal_json());
        let edge_label = mermaid_escape_output_label(&edge_label);
        lines.push(format!(
            "  {} -->|\"{edge_label}\"| {}",
            edge.source, edge.target
        ));
    }

    lines.join("\n")
}

/// The two type lists of the nested format, in Go order.
const NESTED_KINDS: [&str; 2] = ["requires", "provides"];

/// The `kind` type list of a node (`NESTED_KINDS`).
fn nested_kind_types<'a>(node_info: &'a LayerGraphNodeInfo, kind: &str) -> &'a [TypeId] {
    if kind == "requires" {
        &node_info.requires
    } else {
        &node_info.provides
    }
}

// Go: format.go:175 FormatNestedLayerGraph
/// Renders the layer graph as a Mermaid flowchart TB with nested subgraphs.
pub fn format_nested_layer_graph(
    c: &mut Checker,
    layer_graph: &Graph<LayerGraphNodeInfo, LayerGraphEdgeInfo>,
    sf: Node,
) -> String {
    let mut result: Vec<String> = Vec::new();

    // Create subgraph boxes for each node.
    for (idx, node_info) in layer_graph.nodes() {
        let mut subgraph_defs: Vec<String> = Vec::new();

        for kind in NESTED_KINDS {
            let types = nested_kind_types(node_info, kind);

            let type_nodes: Vec<String> = types
                .iter()
                .enumerate()
                .map(|(i, &t)| {
                    let type_name = get_type_display_name(c, t);
                    format!("{idx}_{kind}_{i}[\"{}\"]", mermaid_safe(&type_name))
                })
                .collect();

            if !type_nodes.is_empty() {
                let kind_label = if kind == "provides" {
                    "Provides"
                } else {
                    "Requires"
                };
                subgraph_defs.push(format!("subgraph {idx}_{kind} [{kind_label}]"));
                for tn in &type_nodes {
                    subgraph_defs.push(format!("  {tn}"));
                }
                subgraph_defs.push("end".to_string());
                subgraph_defs.push(format!("style {idx}_{kind} stroke:none"));
            }
        }

        // Wrap in inner wrap subgraph.
        let mut wrapped: Vec<String> = vec![format!("subgraph {idx}_wrap[\" \"]")];
        for line in &subgraph_defs {
            wrapped.push(format!("  {line}"));
        }
        wrapped.push("end".to_string());
        wrapped.push(format!("style {idx}_wrap fill:transparent"));
        wrapped.push(format!("style {idx}_wrap stroke:none"));

        // Get display node text and location.
        let display_node = node_info.display_node;
        let display_source_file = get_node_source_file(display_node);
        let node_text = get_node_text(display_node, display_source_file);
        let location = format_location(display_node, sf);

        // Outer subgraph.
        result.push(format!(
            "subgraph {idx} [\"`{}<br/><small>_{}_</small>`\"]",
            mermaid_safe(&node_text),
            mermaid_safe(&location)
        ));
        for line in &wrapped {
            result.push(format!("  {line}"));
        }
        result.push("end".to_string());
        result.push(format!("style {idx} fill:transparent"));
    }

    // Create edges between matching type nodes.
    // PORT: Go `GetNode` gives the zero value for a missing node; an edge's
    // nodes always exist.
    let empty = LayerGraphNodeInfo::default();
    for (_, edge) in layer_graph.edges() {
        let source_data = layer_graph.get_node(edge.source).unwrap_or(&empty);
        let target_data = layer_graph.get_node(edge.target).unwrap_or(&empty);

        let mut connected = false;
        for kind in NESTED_KINDS {
            let source_types = nested_kind_types(source_data, kind);
            let target_types = nested_kind_types(target_data, kind);

            for (i, source_type) in source_types.iter().enumerate() {
                if let Some(j) = target_types.iter().position(|t| t == source_type) {
                    result.push(format!(
                        "{}_{kind}_{i} -.-> {}_{kind}_{j}",
                        edge.source, edge.target
                    ));
                    connected = true;
                }
            }
        }

        if !connected {
            result.push(format!("{} -.-x {}", edge.source, edge.target));
        }
    }

    if result.is_empty() {
        return String::new();
    }

    let mut lines: Vec<String> = vec!["flowchart TB".to_string()];
    for line in &result {
        lines.push(format!("  {line}"));
    }
    lines.join("\n")
}

// Go: format.go:284 FormatOutlineGraph
/// Renders the outline graph as a Mermaid flowchart TD.
// PORT: Go takes the checker and does not use it; the port does not take
// it.
pub fn format_outline_graph(
    outline_graph: &Graph<LayerOutlineGraphNodeInfo, ()>,
    sf: Node,
) -> String {
    outline_graph.to_mermaid(MermaidOptions {
        direction: "TD".to_string(),
        node_label: Some(Box::new(move |node_info: &LayerOutlineGraphNodeInfo| {
            let display_node = node_info.display_node;
            let display_source_file = get_node_source_file(display_node);
            let node_text =
                mermaid_entity_encode(&get_node_text(display_node, display_source_file));
            if display_source_file == sf {
                return node_text;
            }
            format!(
                "{node_text}\n_{}_",
                mermaid_entity_encode(&format_location(display_node, sf))
            )
        })),
        edge_label: Some(Box::new(|_: &()| String::new())),
        ..Default::default()
    })
}

// Go: format.go:307 FormatQuickInfo
/// Renders provider/requirer info as a JSDoc comment.
pub fn format_quick_info(
    c: &mut Checker,
    providers_and_requirers: &[ProviderRequirerInfo],
    sf: Node,
) -> String {
    if providers_and_requirers.is_empty() {
        return String::new();
    }

    let (provided_items, required_items): (Vec<&ProviderRequirerInfo>, Vec<&ProviderRequirerInfo>) =
        providers_and_requirers
            .iter()
            .partition(|item| item.kind == ProviderRequirerKind::Provided);

    let mut textual_explanation: Vec<String> = Vec::new();

    let mut append_info = |item: &ProviderRequirerInfo, textual_explanation: &mut Vec<String>| {
        let type_name = get_type_display_name(c, item.type_);

        let mut positions: Vec<String> = Vec::new();
        for (i, &display_node) in item.display_nodes.iter().enumerate() {
            let display_source_file = get_node_source_file(display_node);
            let mut node_text = get_node_text(display_node, display_source_file).replace('\n', " ");
            // PORT: a Go byte cut (`go_len`, `go_slice`).
            if go_len(&node_text) > 50 {
                node_text = go_slice(&node_text, 0, 50).into_owned();
            }
            let mut loc = format_location(display_node, sf);
            // Intentional fix over .repos reference: the .repos version unconditionally
            // prepends "at " for the first display node, producing a double preposition
            // "provided at in file.ts at ln X col Y" for cross-file references. This
            // check skips the "at " prefix when the location already starts with "in ",
            // yielding the correct "provided in file.ts at ln X col Y".
            if !loc.starts_with("in ") && i == 0 {
                loc = format!("at {loc}");
            }
            positions.push(format!("{loc} by `{node_text}`"));
        }

        textual_explanation.push(format!(
            "- {type_name} {} {}",
            item.kind.as_str(),
            positions.join(", ")
        ));
    };

    for item in &provided_items {
        append_info(item, &mut textual_explanation);
    }
    if !provided_items.is_empty() && !required_items.is_empty() {
        textual_explanation.push(String::new());
    }
    for item in &required_items {
        append_info(item, &mut textual_explanation);
    }

    let lines: Vec<String> = textual_explanation
        .iter()
        .map(|l| format!(" * {l}"))
        .collect();
    format!("/**\n{}\n */", lines.join("\n"))
}

// ---------------------------------------------------------------------------
// mermaidurl.go
// ---------------------------------------------------------------------------

// Go: mermaidurl.go:13 EncodeMermaidURL
/// Encodes a Mermaid diagram string into a pako-compatible URL fragment.
/// It JSON-serializes {"code": diagram}, compresses with zlib at max compression,
/// base64url-encodes (no padding), and returns baseURL + "pako:" + encoded.
// PORT: root decision D3: flate2 at level 9 stands for Go `compress/zlib`
// `BestCompression`. The two give different bytes for the same input; the
// inflated payload is the same, and the compares inflate it. Go returns
// `baseURL` when `json.Marshal` or the writer fails; neither can fail here.
#[must_use]
pub fn encode_mermaid_url(base_url: &str, diagram: &str) -> String {
    use std::io::Write as _;

    let json = format!("{{\"code\":{}}}", encoding_json_marshal_string(diagram));

    let mut w = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    w.write_all(json.as_bytes())
        .expect("zlib write to a Vec does not fail");
    let compressed = w.finish().expect("zlib finish to a Vec does not fail");

    let encoded = base64_raw_url_encoding_encode_to_string(&compressed);
    format!("{base_url}pako:{encoded}")
}

/// Go `encoding/json` `Marshal` of a string, as E0's go1.27.1 builds it (the
/// v2 encoder with the v1 options): `"` and `\`, the short escapes
/// `\b \f \n \r \t`, other control chars as `\u00XX`, `<` `>` `&` as
/// `\u003c` `\u003e` `\u0026` (EscapeForHTML), U+2028 and U+2029 as
/// `\u2028` `\u2029` (EscapeForJS), and a raw U+FFFD for each byte that is
/// not valid UTF-8 (AllowInvalidUTF8; `go_runes`).
// PORT: no Go function; it inlines jsonwire `AppendQuote` with the v1 flags.
fn encoding_json_marshal_string(s: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in go_runes(s) {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c if (c as u32) < 0x20 || matches!(c, '<' | '>' | '&') => {
                let b = c as u32 as usize;
                out.push_str("\\u00");
                out.push(char::from(HEX[b >> 4]));
                out.push(char::from(HEX[b & 0xF]));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// Go: encoding/base64/base64.go (*Encoding).EncodeToString (RawURLEncoding)
/// Go `base64.RawURLEncoding.EncodeToString`: the URL-safe alphabet with no
/// padding.
fn base64_raw_url_encoding_encode_to_string(src: &[u8]) -> String {
    const ENCODE_URL: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(src.len().div_ceil(3) * 4);
    for chunk in src.chunks(3) {
        let val = chunk
            .iter()
            .enumerate()
            .fold(0u32, |v, (i, &b)| v | (u32::from(b) << (16 - 8 * i)));
        // A chunk of n bytes gives n + 1 chars.
        for k in 0..=chunk.len() {
            out.push(char::from(
                ENCODE_URL[((val >> (18 - 6 * k)) & 0x3F) as usize],
            ));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// providers.go
// ---------------------------------------------------------------------------

// Go: providers.go:17 walkLeavesMatching
/// Performs a BFS from the given start node indices, walking
/// outgoing edges. For each node, it checks if any outgoing neighbors match the
/// predicate. If matching neighbors exist, it continues walking to those neighbors.
/// If no matching neighbors exist and the current node matches the predicate,
/// the current node is emitted as a "leaf match". This finds the deepest nodes
/// in the graph that match the predicate.
fn walk_leaves_matching(
    g: &Graph<LayerGraphNodeInfo, LayerGraphEdgeInfo>,
    start_indices: &[NodeIndex],
    predicate: &mut dyn FnMut(&LayerGraphNodeInfo) -> bool,
) -> Vec<LayerGraphNodeInfo> {
    let mut result: Vec<LayerGraphNodeInfo> = Vec::new();
    let mut queue: std::collections::VecDeque<NodeIndex> = start_indices.iter().copied().collect();
    let mut discovered: FxHashSet<NodeIndex> = FxHashSet::default();

    while let Some(current) = queue.pop_front() {
        if !discovered.insert(current) {
            continue;
        }

        let neighbors = g.neighbors_directed(current, Direction::Outgoing);
        let mut matching_neighbors: Vec<NodeIndex> = Vec::new();
        for neighbor in neighbors {
            if let Some(neighbor_data) = g.get_node(neighbor)
                && predicate(neighbor_data)
            {
                matching_neighbors.push(neighbor);
            }
        }

        if !matching_neighbors.is_empty() {
            queue.extend(matching_neighbors);
        } else if let Some(node_data) = g.get_node(current)
            && predicate(node_data)
        {
            result.push(node_data.clone());
        }
    }

    result
}

// Go: providers.go:67 ExtractProvidersAndRequirers
/// Extracts summary information about which layers
/// provide or require which services. It uses a two-step algorithm:
///
/// Step 1: Get root nodes (no incoming edges) to determine WHICH types to report.
/// The root node's Layer type reflects the net result of the composition, so
/// internally-satisfied services are already excluded.
///
/// Step 2: For each type from step 1, use walkLeavesMatching BFS to find the
/// deepest leaf nodes that match, giving source location attribution.
pub fn extract_providers_and_requirers(
    c: &mut Checker,
    layer_graph: &Graph<LayerGraphNodeInfo, LayerGraphEdgeInfo>,
) -> Vec<ProviderRequirerInfo> {
    // Step 1: Get root nodes (externals with no incoming edges).
    let mut root_nodes: Vec<&LayerGraphNodeInfo> = Vec::new();
    let mut root_node_indices: Vec<NodeIndex> = Vec::new();
    for (idx, node) in layer_graph.externals(Direction::Incoming) {
        root_nodes.push(node);
        root_node_indices.push(idx);
    }

    // Collect unique Provides and Requires types from root nodes.
    // PORT: Go keeps the sets in maps, which it walks in random order before
    // the sort below; the port keeps first-insertion order, which is one of
    // Go's orders.
    let mut provides_set: IndexSet<TypeId> = IndexSet::new();
    let mut requires_set: IndexSet<TypeId> = IndexSet::new();
    for root in &root_nodes {
        provides_set.extend(root.provides.iter().copied());
        requires_set.extend(root.requires.iter().copied());
    }

    let mut result: Vec<ProviderRequirerInfo> = Vec::new();

    // Step 2: For each type, walk leaves to find attribution.
    let mut walk_types = |type_set: &IndexSet<TypeId>, kind: ProviderRequirerKind| {
        // Sort types alphabetically for deterministic output. This intentionally
        // diverges from the .repos reference implementation which uses non-deterministic
        // checker iteration order, to avoid flaky output from Go's map iteration.
        let mut types: Vec<TypeId> = type_set.iter().copied().collect();
        sort_slice(&mut types, |a, b| {
            c.type_to_string_exported(*a) < c.type_to_string_exported(*b)
        });

        for layer_type in types {
            let mut predicate = |node: &LayerGraphNodeInfo| -> bool {
                let type_list = if kind == ProviderRequirerKind::Provided {
                    &node.provides
                } else {
                    &node.requires
                };
                type_list
                    .iter()
                    .any(|&t| t == layer_type || c.is_type_assignable_to(t, layer_type))
            };

            let leaf_nodes = walk_leaves_matching(layer_graph, &root_node_indices, &mut predicate);

            result.push(ProviderRequirerInfo {
                kind,
                type_: layer_type,
                nodes: leaf_nodes.iter().map(|node| node.node).collect(),
                display_nodes: leaf_nodes.iter().map(|node| node.display_node).collect(),
            });
        }
    };

    walk_types(&provides_set, ProviderRequirerKind::Provided);
    walk_types(&requires_set, ProviderRequirerKind::Required);

    result
}

// ---------------------------------------------------------------------------
// magic.go
// ---------------------------------------------------------------------------

// Go: magic.go:15 ConvertOutlineGraphToLayerMagic
/// Takes an outline graph and a set of target output types,
/// then computes an ordered list of layer nodes annotated with merges/provides flags
/// that determine which Layer.* combinator to use (provide, provideMerge, or merge).
// PORT: Go returns a `*LayerMagicResult` that is never nil; the port returns
// it by value. Steps 2 to 6 are `layer_magic_from_output_types`, so the Go
// unit test that passes a nil type parser and no target types runs without a
// checker.
pub fn convert_outline_graph_to_layer_magic(
    tp: &mut TypeParser<'_>,
    outline_graph: &Graph<LayerOutlineGraphNodeInfo, ()>,
    target_output_types: &[TypeId],
) -> LayerMagicResult {
    // Step 1: Unroll target output types into individual types
    let mut output_types: Vec<TypeId> = Vec::new();
    for &t in target_output_types {
        output_types.extend(tp.unroll_union_members(t));
    }

    layer_magic_from_output_types(outline_graph, &output_types)
}

// Go: magic.go:26-94 (ConvertOutlineGraphToLayerMagic after step 1)
fn layer_magic_from_output_types(
    outline_graph: &Graph<LayerOutlineGraphNodeInfo, ()>,
    output_types: &[TypeId],
) -> LayerMagicResult {
    // Build a set of missing output types for fast lookup
    let mut missing_output_types: FxHashSet<TypeId> = output_types.iter().copied().collect();

    // Step 2: Clone and reverse the outline graph
    let mut reversed_graph = outline_graph.clone();
    reversed_graph.reverse();

    // Step 3: Find root indices (nodes with no incoming edges in the reversed graph)
    let mut root_indices: Vec<NodeIndex> = reversed_graph
        .externals(Direction::Incoming)
        .map(|(idx, _)| idx)
        .collect();

    // Sort roots by layer shape, then provides/requires count.
    // PORT: Go `slices.SortFunc`; the comparison ends with the index, so the
    // order is total and the sort kind does not show.
    root_indices.sort_by(|&a, &b| compare_nodes_by_order(&reversed_graph, a, b).cmp(&0));

    // Step 4: Run custom DFS post-order with ordering on the reversed graph
    let all_nodes = dfs_post_order_with_order(&reversed_graph, &root_indices);

    // Step 5: Process each visited node
    let mut result: Vec<LayerMagicNode> = Vec::new();
    for node_info in all_nodes {
        if !is_expression(node_info.node) {
            continue;
        }

        // Determine if this node should merge (provides a type still in missingOutputTypes)
        let should_merge = node_info
            .actual_provides
            .iter()
            .any(|t| missing_output_types.contains(t));

        // If merging, remove actual provides from missing output types
        if should_merge {
            for t in &node_info.actual_provides {
                missing_output_types.remove(t);
            }
        }

        result.push(LayerMagicNode {
            merges: should_merge,
            provides: true,
            node: node_info.node,
            provided_types: node_info.provides,
            actual_provided_types: node_info.actual_provides,
            required_types: node_info.requires,
        });
    }

    // Step 6: Collect remaining missing output types
    let remaining: Vec<TypeId> = output_types
        .iter()
        .copied()
        .filter(|t| missing_output_types.contains(t))
        .collect();

    LayerMagicResult {
        nodes: result,
        missing_output_types: remaining,
    }
}

// Go: magic.go:99 dfsPostOrderWithOrder
/// Performs an iterative post-order DFS with custom node ordering.
/// Neighbors are sorted by layer shape, then provides/requires count.
fn dfs_post_order_with_order(
    g: &Graph<LayerOutlineGraphNodeInfo, ()>,
    start: &[NodeIndex],
) -> Vec<LayerOutlineGraphNodeInfo> {
    // Go: the local `entry` type.
    struct Entry {
        node: NodeIndex,
        expanded: bool,
    }

    let mut discovered: FxHashSet<NodeIndex> = FxHashSet::default();
    let mut result: Vec<LayerOutlineGraphNodeInfo> = Vec::new();

    // Initialize stack with start nodes in reverse order (first start node on top)
    let mut stack: Vec<Entry> = Vec::with_capacity(start.len());
    for &node in start.iter().rev() {
        stack.push(Entry {
            node,
            expanded: false,
        });
    }

    while let Some(top) = stack.pop() {
        if discovered.contains(&top.node) {
            if top.expanded {
                // Second visit — emit post-order
                if let Some(node_data) = g.get_node(top.node) {
                    result.push(node_data.clone());
                }
            }
            continue;
        }

        // First visit — mark discovered, push back with expanded=true, then push children
        discovered.insert(top.node);
        stack.push(Entry {
            node: top.node,
            expanded: true,
        });

        // Get neighbors in outgoing direction and sort by custom order
        let neighbors = neighbors_outgoing_sorted(g, top.node);
        for &neighbor in neighbors.iter().rev() {
            if !discovered.contains(&neighbor) {
                stack.push(Entry {
                    node: neighbor,
                    expanded: false,
                });
            }
        }
    }

    result
}

// Go: magic.go:149 neighborsOutgoingSorted
/// Returns outgoing neighbor indices sorted by the layer ordering:
/// layer shape, then provides/requires count.
fn neighbors_outgoing_sorted(
    g: &Graph<LayerOutlineGraphNodeInfo, ()>,
    node_index: NodeIndex,
) -> Vec<NodeIndex> {
    let mut neighbors = g.neighbors_directed(node_index, Direction::Outgoing);
    neighbors.sort_by(|&a, &b| compare_nodes_by_order(g, a, b).cmp(&0));
    neighbors
}

// Go: magic.go:162 compareNodesByOrder
/// Compares two nodes by composition priority:
/// both provides/requires, only provides, neither, only requires.
// PORT: Go `g.GetNode` gives the zero node info for a missing index; the
// port uses the default node info the same way. The `int` lengths are
// `i64` here, so the differences cannot overflow.
fn compare_nodes_by_order(
    g: &Graph<LayerOutlineGraphNodeInfo, ()>,
    a: NodeIndex,
    b: NodeIndex,
) -> i64 {
    let zero = LayerOutlineGraphNodeInfo::default();
    let node_a = g.get_node(a).unwrap_or(&zero);
    let node_b = g.get_node(b).unwrap_or(&zero);

    let rank_diff = layer_order_rank(node_a) - layer_order_rank(node_b);
    if rank_diff != 0 {
        return rank_diff;
    }
    // Sort by provides count descending (reverse order)
    let len_diff = node_b.provides.len() as i64 - node_a.provides.len() as i64;
    if len_diff != 0 {
        return len_diff;
    }
    // Tiebreak by requires count descending (reverse order)
    let len_diff = node_b.requires.len() as i64 - node_a.requires.len() as i64;
    if len_diff != 0 {
        return len_diff;
    }
    i64::from(a) - i64::from(b)
}

// Go: magic.go:183 layerOrderRank
fn layer_order_rank(node: &LayerOutlineGraphNodeInfo) -> i64 {
    let has_provides = !node.provides.is_empty();
    let has_requires = !node.requires.is_empty();
    if has_provides && has_requires {
        0
    } else if has_provides {
        1
    } else if !has_requires {
        2
    } else {
        3
    }
}

#[cfg(test)]
mod tests {
    //! Ports of `internal/layergraph` `format_test.go`, `mermaidurl_test.go`,
    //! `providers_test.go`, `types_test.go` and `magic_test.go`.
    //! One `#[test]` per Go test function; the table rows keep their Go
    //! names in the messages.

    use super::*;
    use std::io::Read as _;

    // Go: format_test.go TestMermaidEntityEncode
    #[test]
    fn mermaid_entity_encode_cases() {
        let tests = [
            ("empty string", "", ""),
            ("no special chars", "hello world", "hello world"),
            ("quotes", r#""hello""#, "#quot;hello#quot;"),
            ("angle brackets", "<div>", "#lt;div#gt;"),
            ("parentheses", "fn(x)", "fn#40;x#41;"),
            (
                "curly braces",
                r#"{"key":"value"}"#,
                "#123;#quot;key#quot;:#quot;value#quot;#125;",
            ),
            ("hash character", "color#red", "color#35;red"),
            ("no double encoding", "#", "#35;"),
            (
                "complex type with special chars",
                r#"IsGeneric<"With<Special>Chars#!">"#,
                "IsGeneric#lt;#quot;With#lt;Special#gt;Chars#35;!#quot;#gt;",
            ),
            (
                "JSON edge label",
                r#"{"relationship":"pipe"}"#,
                "#123;#quot;relationship#quot;:#quot;pipe#quot;#125;",
            ),
            (
                "JSON edge label with argumentIndex",
                r#"{"relationship":"call","argumentIndex":0}"#,
                "#123;#quot;relationship#quot;:#quot;call#quot;,#quot;argumentIndex#quot;:0#125;",
            ),
        ];
        for (name, input, expected) in tests {
            assert_eq!(mermaid_entity_encode(input), expected, "{name}");
        }
    }

    // Go: format_test.go TestMermaidSafe
    #[test]
    fn mermaid_safe_cases() {
        let tests = [
            ("simple text", "hello", "hello"),
            (
                "newlines replaced with space",
                "hello\nworld",
                "hello world",
            ),
            ("whitespace collapsed", "hello   world", "hello world"),
            (
                "truncated to 50 chars",
                "a234567890b234567890c234567890d234567890e234567890EXTRA",
                "a234567890b234567890c234567890d234567890e234567890",
            ),
            ("quotes encoded", r#"say "hello""#, "say #quot;hello#quot;"),
            (
                "angle brackets encoded",
                "List<number>",
                "List#lt;number#gt;",
            ),
            ("hash NOT encoded", "color#red", "color#red"),
            (
                "special chars type",
                r#"IsGeneric<"With<Special>Chars#!">"#,
                "IsGeneric#lt;#quot;With#lt;Special#gt;Chars#!#quot;#gt;",
            ),
            ("trimmed", "  hello  ", "hello"),
        ];
        for (name, input, expected) in tests {
            assert_eq!(mermaid_safe(input), expected, "{name}");
        }
    }

    /// Go `base64.RawURLEncoding.DecodeString` for the tests.
    fn base64_raw_url_decode(s: &str) -> Vec<u8> {
        const ENCODE_URL: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut out = Vec::new();
        let (mut acc, mut bits) = (0u32, 0u32);
        for ch in s.bytes() {
            let v = ENCODE_URL
                .iter()
                .position(|&e| e == ch)
                .unwrap_or_else(|| panic!("not base64url: {ch}"));
            acc = (acc << 6) | v as u32;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((acc >> bits) as u8);
                acc &= (1 << bits) - 1;
            }
        }
        out
    }

    /// The inflated payload of a Mermaid URL: the text after "pako:",
    /// base64url decoded and zlib inflated.
    fn inflate_mermaid_url(url: &str, base_url: &str) -> String {
        let encoded = url
            .strip_prefix(base_url)
            .and_then(|rest| rest.strip_prefix("pako:"))
            .unwrap_or_else(|| panic!("expected prefix {base_url}pako:, got {url}"));
        let compressed = base64_raw_url_decode(encoded);
        let mut payload = String::new();
        flate2::read::ZlibDecoder::new(compressed.as_slice())
            .read_to_string(&mut payload)
            .expect("zlib decompress");
        payload
    }

    // Go: mermaidurl_test.go TestEncodeMermaidURL
    // PORT: Go unmarshals the payload and compares the code. The port
    // compares the payload text, so the Go JSON escaping is checked too.
    #[test]
    fn encode_mermaid_url_roundtrip() {
        let diagram = "flowchart TB\n  A --> B";
        let base_url = "https://mermaid.live/edit#";
        let result = encode_mermaid_url(base_url, diagram);
        assert_eq!(
            inflate_mermaid_url(&result, base_url),
            r#"{"code":"flowchart TB\n  A --\u003e B"}"#
        );
    }

    // Go: mermaidurl_test.go TestEncodeMermaidURL_CustomBase
    #[test]
    fn encode_mermaid_url_custom_base() {
        let diagram = "flowchart LR\n  X --> Y";
        let base_url = "https://www.mermaidchart.com/play#";
        let result = encode_mermaid_url(base_url, diagram);
        assert_eq!(
            inflate_mermaid_url(&result, base_url),
            r#"{"code":"flowchart LR\n  X --\u003e Y"}"#
        );
    }

    // Go: mermaidurl_test.go TestEncodeMermaidURL_Deterministic
    #[test]
    fn encode_mermaid_url_deterministic() {
        let diagram = "flowchart TB\n  A --> B --> C";
        let base_url = "https://mermaid.live/edit#";
        assert_eq!(
            encode_mermaid_url(base_url, diagram),
            encode_mermaid_url(base_url, diagram)
        );
    }

    /// The payload of the Go JSON encoder (go1.27.1, as E0 builds it) for
    /// every escape class, and the outline payload of the Go hover test
    /// `TestEffectHoverLayerQuickInfo` (its URL inflated).
    #[test]
    fn encode_mermaid_url_payload_matches_go() {
        let base_url = "https://mermaid.live/edit#";
        let diagram = "flowchart TD\n  0[\"Database.Default\"]\n  1[\"Cache.Default\"]\n  1 --> 0";
        assert_eq!(
            inflate_mermaid_url(&encode_mermaid_url(base_url, diagram), base_url),
            r#"{"code":"flowchart TD\n  0[\"Database.Default\"]\n  1[\"Cache.Default\"]\n  1 --\u003e 0"}"#
        );
        assert_eq!(
            encoding_json_marshal_string("a\\b\u{8}\u{c}\r\t\u{1}<&>\u{2028}\u{2029}é"),
            r#""a\\b\b\f\r\t\u0001\u003c\u0026\u003e\u2028\u2029é""#
        );
    }

    #[test]
    fn base64_raw_url_encoding_matches_go() {
        // Go: base64.RawURLEncoding.EncodeToString of each input.
        let tests: [(&[u8], &str); 5] = [
            (b"", ""),
            (b"f", "Zg"),
            (b"fo", "Zm8"),
            (b"foo", "Zm9v"),
            (&[0xfb, 0xff, 0xbf], "-_-_"),
        ];
        for (input, expected) in tests {
            assert_eq!(base64_raw_url_encoding_encode_to_string(input), expected);
        }
    }

    // Go: providers_test.go makeNodeInfo
    /// A minimal LayerGraphNodeInfo for testing walkLeavesMatching.
    /// When hasTag is true, Provides gets a non-empty slice so the predicate matches.
    fn make_node_info(has_tag: bool) -> LayerGraphNodeInfo {
        let mut info = LayerGraphNodeInfo::default();
        if has_tag {
            info.provides = vec![TypeId::NIL];
        }
        info
    }

    // Go: providers_test.go tagPredicate
    /// Returns true for nodes where Provides is non-empty.
    fn tag_predicate(node: &LayerGraphNodeInfo) -> bool {
        !node.provides.is_empty()
    }

    /// A graph with one node per tag and the given edges, and the
    /// walkLeavesMatching result count from node 0.
    fn walk_count(tags: &[bool], edges: &[(usize, usize)]) -> usize {
        let mut g: Graph<LayerGraphNodeInfo, LayerGraphEdgeInfo> = Graph::new();
        let nodes: Vec<NodeIndex> = tags
            .iter()
            .map(|&tag| g.add_node(make_node_info(tag)))
            .collect();
        for &(source, target) in edges {
            g.add_edge(nodes[source], nodes[target], LayerGraphEdgeInfo::default());
        }
        walk_leaves_matching(&g, &[nodes[0]], &mut tag_predicate).len()
    }

    // Go: providers_test.go TestWalkLeavesMatching
    #[test]
    fn walk_leaves_matching_cases() {
        // single node that matches
        assert_eq!(walk_count(&[true], &[]), 1);
        // single node that does not match
        assert_eq!(walk_count(&[false], &[]), 0);
        // linear chain, all match, emits deepest: root -> middle -> leaf
        assert_eq!(walk_count(&[true, true, true], &[(0, 1), (1, 2)]), 1);
        // linear chain, only root matches
        assert_eq!(walk_count(&[true, false, false], &[(0, 1), (1, 2)]), 1);
        // branching graph, multiple leaves match: root -> left, root -> right
        assert_eq!(walk_count(&[true, true, true], &[(0, 1), (0, 2)]), 2);
        // no nodes match predicate
        assert_eq!(walk_count(&[false, false], &[(0, 1)]), 0);
        // diamond graph, all match, emits only sink: A -> B, A -> C, B -> D, C -> D
        assert_eq!(
            walk_count(&[true, true, true, true], &[(0, 1), (0, 2), (1, 3), (2, 3)]),
            1
        );
    }

    // Go: types_test.go TestLayerGraphEdgeInfoMarshalJSON
    #[test]
    fn layer_graph_edge_info_marshal_json_cases() {
        let edge = |relationship, argument_index, index| LayerGraphEdgeInfo {
            relationship,
            argument_index,
            index,
        };
        let tests = [
            (
                "call edge",
                edge(EdgeRelationship::Call, 0, 0),
                r#"{"relationship":"call","argumentIndex":0}"#,
            ),
            (
                "call edge with index 2",
                edge(EdgeRelationship::Call, 2, 0),
                r#"{"relationship":"call","argumentIndex":2}"#,
            ),
            (
                "pipe edge",
                edge(EdgeRelationship::Pipe, 0, 0),
                r#"{"relationship":"pipe"}"#,
            ),
            (
                "arrayLiteral edge",
                edge(EdgeRelationship::ArrayLiteral, 0, 0),
                r#"{"relationship":"arrayLiteral","index":0}"#,
            ),
            (
                "arrayLiteral edge with index 3",
                edge(EdgeRelationship::ArrayLiteral, 0, 3),
                r#"{"relationship":"arrayLiteral","index":3}"#,
            ),
            (
                "symbol edge",
                edge(EdgeRelationship::Symbol, 0, 0),
                r#"{"relationship":"symbol"}"#,
            ),
        ];
        for (name, edge, expected) in tests {
            assert_eq!(edge.marshal_json(), expected, "{name}");
        }
    }

    // Go: magic_test.go makeOutlineNode
    // makeOutlineNode creates a LayerOutlineGraphNodeInfo for testing with the given
    // number of provides and requires (using nil types, sufficient for ordering tests).
    // PORT: Go makes a bare CallExpression node; the port makes an identifier
    // (also an expression), one new node per call as in Go.
    fn make_outline_node(
        f: &NodeFactory,
        num_provides: usize,
        num_requires: usize,
    ) -> LayerOutlineGraphNodeInfo {
        let node = f.new_identifier("n");
        LayerOutlineGraphNodeInfo {
            node,
            display_node: node,
            provides: vec![TypeId::NIL; num_provides],
            actual_provides: vec![TypeId::NIL; num_provides],
            requires: vec![TypeId::NIL; num_requires],
        }
    }

    // Go: magic_test.go TestCompareNodesByOrder
    #[test]
    fn compare_nodes_by_order_counts() {
        let mut g: Graph<LayerOutlineGraphNodeInfo, ()> = Graph::new();

        // Node A: 3 provides, 1 requires
        let idx_a = g.add_node(LayerOutlineGraphNodeInfo {
            provides: vec![TypeId::NIL; 3],
            requires: vec![TypeId::NIL; 1],
            ..Default::default()
        });
        // Node B: 1 provides, 2 requires
        let idx_b = g.add_node(LayerOutlineGraphNodeInfo {
            provides: vec![TypeId::NIL; 1],
            requires: vec![TypeId::NIL; 2],
            ..Default::default()
        });
        // Node C: 3 provides, 3 requires
        let idx_c = g.add_node(LayerOutlineGraphNodeInfo {
            provides: vec![TypeId::NIL; 3],
            requires: vec![TypeId::NIL; 3],
            ..Default::default()
        });

        // A (3 provides) should come before B (1 provides) — compare returns negative
        let cmp = compare_nodes_by_order(&g, idx_a, idx_b);
        assert!(cmp < 0, "expected A < B (more provides first), got {cmp}");

        // B (1 provides) should come after A (3 provides) — compare returns positive
        let cmp = compare_nodes_by_order(&g, idx_b, idx_a);
        assert!(cmp > 0, "expected B > A, got {cmp}");

        // A (3 provides, 1 requires) vs C (3 provides, 3 requires) — tiebreak by requires
        let cmp = compare_nodes_by_order(&g, idx_a, idx_c);
        assert!(
            cmp > 0,
            "expected A > C (C has more requires, so C first), got {cmp}"
        );
    }

    // Go: magic_test.go TestCompareNodesByOrderLayerShapes
    #[test]
    fn compare_nodes_by_order_layer_shapes() {
        let f = NodeFactory::new();
        let mut g: Graph<LayerOutlineGraphNodeInfo, ()> = Graph::new();

        let both_provides_and_requires = g.add_node(make_outline_node(&f, 1, 1));
        let only_provides = g.add_node(make_outline_node(&f, 1, 0));
        let no_provides_or_requires = g.add_node(make_outline_node(&f, 0, 0));
        let only_requires = g.add_node(make_outline_node(&f, 0, 1));

        let ordered = [
            both_provides_and_requires,
            only_provides,
            no_provides_or_requires,
            only_requires,
        ];
        for i in 0..ordered.len() - 1 {
            let cmp = compare_nodes_by_order(&g, ordered[i], ordered[i + 1]);
            assert!(
                cmp < 0,
                "expected node at order {i} to sort before order {}, got {cmp}",
                i + 1
            );
        }
    }

    // Go: magic_test.go TestDFSPostOrderWithOrder
    #[test]
    fn dfs_post_order_with_order_children_first() {
        // Build a small graph:
        //   A (3 provides) -> B (1 provides)
        //   A -> C (2 provides)
        //   B -> D (0 provides)
        let f = NodeFactory::new();
        let mut g: Graph<LayerOutlineGraphNodeInfo, ()> = Graph::new();

        let idx_a = g.add_node(make_outline_node(&f, 3, 0));
        let idx_b = g.add_node(make_outline_node(&f, 1, 0));
        let idx_c = g.add_node(make_outline_node(&f, 2, 0));
        let idx_d = g.add_node(make_outline_node(&f, 0, 0));

        g.add_edge(idx_a, idx_b, ());
        g.add_edge(idx_a, idx_c, ());
        g.add_edge(idx_b, idx_d, ());

        let result = dfs_post_order_with_order(&g, &[idx_a]);

        // Post-order: children before parents
        // A's neighbors sorted by provides desc: C (2) before B (1)
        // Visiting C first (post-order): C has no children -> emit C
        // Then B: B has child D -> emit D, then B
        // Finally A
        // Expected order: C, D, B, A
        assert_eq!(result.len(), 4, "expected 4 nodes");

        let expected_provides = [2, 0, 1, 3];
        for (i, expected) in expected_provides.into_iter().enumerate() {
            assert_eq!(
                result[i].provides.len(),
                expected,
                "result[{i}] expected {expected} provides"
            );
        }
    }

    // Go: magic_test.go TestDFSPostOrderWithOrderMultipleRoots
    #[test]
    fn dfs_post_order_with_order_multiple_roots() {
        let f = NodeFactory::new();
        let mut g: Graph<LayerOutlineGraphNodeInfo, ()> = Graph::new();

        let idx_a = g.add_node(make_outline_node(&f, 3, 0));
        let idx_b = g.add_node(make_outline_node(&f, 1, 0));

        // Two disconnected roots, sorted by provides desc: A (3) before B (1)
        let result = dfs_post_order_with_order(&g, &[idx_a, idx_b]);

        assert_eq!(result.len(), 2, "expected 2 nodes");
        // Start order is reversed on stack, so B pushed first, A on top
        // A popped first -> emitted first, then B
        assert_eq!(
            result[0].provides.len(),
            3,
            "expected first node to have 3 provides"
        );
        assert_eq!(
            result[1].provides.len(),
            1,
            "expected second node to have 1 provides"
        );
    }

    // Go: magic_test.go TestConvertOutlineGraphToLayerMagicOrdersUnrelatedLayers
    // PORT: Go calls `ConvertOutlineGraphToLayerMagic(nil, g, nil)`; with no
    // target types the type parser is not used, so the port calls the part
    // after the unroll step with no output types.
    #[test]
    fn convert_outline_graph_to_layer_magic_orders_unrelated_layers() {
        let f = NodeFactory::new();
        let mut g: Graph<LayerOutlineGraphNodeInfo, ()> = Graph::new();

        let both_provides_and_requires = make_outline_node(&f, 1, 1);
        let only_provides = make_outline_node(&f, 1, 0);
        let no_provides_or_requires = make_outline_node(&f, 0, 0);
        let only_requires = make_outline_node(&f, 0, 1);

        // Add nodes in an order that differs from the expected composition order to
        // verify unrelated roots are sorted by their layer shape, not source order.
        g.add_node(no_provides_or_requires.clone());
        g.add_node(only_requires.clone());
        g.add_node(only_provides.clone());
        g.add_node(both_provides_and_requires.clone());

        let result = layer_magic_from_output_types(&g, &[]);
        assert_eq!(result.nodes.len(), 4, "expected 4 nodes");

        let expected = [
            both_provides_and_requires,
            only_provides,
            no_provides_or_requires,
            only_requires,
        ];
        for (i, expected_node) in expected.iter().enumerate() {
            assert!(
                result.nodes[i].node == expected_node.node,
                "result[{i}] expected provides={} requires={}, got provides={} requires={}",
                expected_node.provides.len(),
                expected_node.requires.len(),
                result.nodes[i].provided_types.len(),
                result.nodes[i].required_types.len(),
            );
        }
    }

    // Go: magic_test.go TestNeighborsOutgoingSorted
    #[test]
    fn neighbors_outgoing_sorted_by_provides() {
        let f = NodeFactory::new();
        let mut g: Graph<LayerOutlineGraphNodeInfo, ()> = Graph::new();

        let idx_a = g.add_node(make_outline_node(&f, 0, 0));
        let idx_b = g.add_node(make_outline_node(&f, 1, 0));
        let idx_c = g.add_node(make_outline_node(&f, 3, 0));
        let idx_d = g.add_node(make_outline_node(&f, 2, 0));

        g.add_edge(idx_a, idx_b, ());
        g.add_edge(idx_a, idx_c, ());
        g.add_edge(idx_a, idx_d, ());

        let neighbors = neighbors_outgoing_sorted(&g, idx_a);

        // Should be sorted by provides desc: C (3), D (2), B (1)
        assert_eq!(neighbors.len(), 3, "expected 3 neighbors");
        assert_eq!(
            neighbors[0], idx_c,
            "expected first neighbor to be C (3 provides)"
        );
        assert_eq!(
            neighbors[1], idx_d,
            "expected second neighbor to be D (2 provides)"
        );
        assert_eq!(
            neighbors[2], idx_b,
            "expected third neighbor to be B (1 provides)"
        );
    }
}

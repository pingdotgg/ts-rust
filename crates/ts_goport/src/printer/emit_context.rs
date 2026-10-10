//! Port of Go `printer/emitcontext.go`.
//!
//! `EmitContext` stores side tables that transforms write and the printer
//! reads: emit flags, comment and source map ranges, original node links,
//! auto-generated name info and emit helpers.

use crate::flags_macros::go_enum;
use crate::prelude::*;

use super::factory::NodeFactory;
use super::helpers::EmitHelper;
use super::types::{EmitFlags, GeneratedIdentifierFlags};
use super::utilities::{find_span_end, find_span_end_with_emit_context};
use std::cell::OnceCell;
use std::sync::atomic::{AtomicU32, Ordering};

/// Go `*EmitHelper`. Go compares helpers by pointer.
// PORT: Go helpers are package-level `var`s, so a helper is a
// `&'static EmitHelper` and identity is `std::ptr::eq`.
pub type EmitHelperRef = &'static EmitHelper;

// Stores side-table information used during transformation that can be read by the printer to customize emit
//
// NOTE: EmitContext is not guaranteed to be thread-safe.
// PORT: Go mutates the context through a pointer that many objects share.
// Here the context is shared as `Rc<EmitContext>`, so each field is a
// `RefCell`. Methods never hold a borrow across a factory or visitor call.
#[derive(Default)]
pub struct EmitContext {
    /// Go `Factory`. Required. The NodeFactory to use to create new nodes.
    // PORT: the printer factory holds a `Weak<EmitContext>` and the context
    // holds the factory, as in Go. The factory is set once after the `Rc`
    // exists (see `new_emit_context`). The `Weak` breaks the cycle, so a
    // context is freed with its last `Rc`.
    pub(crate) factory: EmitContextFactory,
    pub(crate) auto_generate: RefCell<FxHashMap<Node, AutoGenerateInfo>>,
    pub(crate) text_source: RefCell<FxHashMap<Node, Node>>,
    pub(crate) original: RefCell<Originals>,
    pub(crate) emit_nodes: RefCell<EmitNodes>,
    pub(crate) assigned_name: RefCell<FxHashMap<Node, Node>>,
    pub(crate) class_this: RefCell<FxHashMap<Node, Node>>,
    pub(crate) var_scope_stack: RefCell<Vec<Rc<RefCell<VarScope>>>>,
    pub(crate) let_scope_stack: RefCell<Vec<Rc<RefCell<VarScope>>>>,
    /// Go `collections.OrderedSet[*EmitHelper]`.
    // PORT: an insertion-ordered `Vec` with pointer-identity checks.
    pub(crate) emit_helpers: RefCell<Vec<EmitHelperRef>>,
    /// PORT: not in Go. True when an emit node may hold a node
    /// (`set_type_node`, `set_external_helpers_module_name`), for
    /// `PrintTables::for_each_emit_node_value`.
    emit_node_refs: std::cell::Cell<bool>,
}

/// Go `environmentFlags`.
pub(crate) type EnvironmentFlags = i32;

pub(crate) const ENVIRONMENT_FLAGS_NONE: EnvironmentFlags = 0;
pub(crate) const ENVIRONMENT_FLAGS_IN_PARAMETERS: EnvironmentFlags = 1 << 0; // currently visiting a parameter list
pub(crate) const ENVIRONMENT_FLAGS_VARIABLES_HOISTED_IN_PARAMETERS: EnvironmentFlags = 1 << 1; // a temp variable was hoisted while visiting a parameter list

/// Go `varScope`.
#[derive(Default, Debug)]
pub(crate) struct VarScope {
    pub(crate) variables: Vec<Node>,
    pub(crate) functions: Vec<Node>,
    pub(crate) flags: EnvironmentFlags,
    pub(crate) initialization_statements: Vec<Node>,
}

/// Go `EmitContext.Factory`. Set once by `new_emit_context`; it derefs to the
/// printer `NodeFactory`, so `c.factory.new_x(..)` reads like Go.
#[derive(Default)]
pub struct EmitContextFactory(OnceCell<NodeFactory>);

impl std::ops::Deref for EmitContextFactory {
    type Target = NodeFactory;

    fn deref(&self) -> &NodeFactory {
        self.0.get().expect("EmitContext has no Factory")
    }
}

// Go: printer/emitcontext.go:44 NewEmitContext
#[must_use]
pub fn new_emit_context() -> Rc<EmitContext> {
    let c = Rc::new(EmitContext::default());
    let factory = NodeFactory::new(&c);
    if c.factory.0.set(factory).is_err() {
        unreachable!("EmitContext factory set twice");
    }
    c
}

/// The side tables of an `EmitContext` that the printer reads, out of the
/// context, so they can move to another thread. A d.ts twin prints there
/// (`program::send_dts_twin_job`). The transform environment (the
/// variable and lexical scopes) is not in it: it is empty after the
/// transforms.
// PORT: not in Go (perf).
pub struct PrintTables {
    auto_generate: FxHashMap<Node, AutoGenerateInfo>,
    text_source: FxHashMap<Node, Node>,
    original: Originals,
    emit_nodes: EmitNodes,
    assigned_name: FxHashMap<Node, Node>,
    class_this: FxHashMap<Node, Node>,
    emit_helpers: Vec<EmitHelperRef>,
    emit_node_refs: bool,
}

impl PrintTables {
    /// Calls `f` with every node that a table holds as a value: the nodes
    /// that a print can reach from the nodes it prints.
    pub fn for_each_value_node(&self, mut f: impl FnMut(Node)) {
        for info in self.auto_generate.values() {
            f(info.node);
        }
        for map in [&self.text_source, &self.assigned_name, &self.class_this] {
            map.values().copied().for_each(&mut f);
        }
        self.original.values().for_each(&mut f);
    }

    /// Calls `f` with the nodes that the emit node of `node` holds.
    // PERF: most contexts have none (they are JS transform data), and a
    // lookup per printed node cost about a tenth of the d.ts export walk.
    pub fn for_each_emit_node_value(&self, node: Node, mut f: impl FnMut(Node)) {
        if !self.emit_node_refs {
            return;
        }
        if let Some(emit_node) = self.emit_nodes.try_get(node) {
            f(emit_node.type_node);
            f(emit_node.external_helpers_module_name);
        }
    }
}

impl EmitContext {
    /// Moves the side tables out of this context (`PrintTables`). The
    /// context then has empty side tables.
    #[must_use]
    pub fn take_print_tables(&self) -> PrintTables {
        PrintTables {
            auto_generate: std::mem::take(&mut *self.auto_generate.borrow_mut()),
            text_source: std::mem::take(&mut *self.text_source.borrow_mut()),
            original: std::mem::take(&mut *self.original.borrow_mut()),
            emit_nodes: std::mem::take(&mut *self.emit_nodes.borrow_mut()),
            assigned_name: std::mem::take(&mut *self.assigned_name.borrow_mut()),
            class_this: std::mem::take(&mut *self.class_this.borrow_mut()),
            emit_helpers: std::mem::take(&mut *self.emit_helpers.borrow_mut()),
            emit_node_refs: self.emit_node_refs.replace(false),
        }
    }

    /// A copy of the side tables of this context (`PrintTables`).
    #[must_use]
    pub fn clone_print_tables(&self) -> PrintTables {
        PrintTables {
            auto_generate: self.auto_generate.borrow().clone(),
            text_source: self.text_source.borrow().clone(),
            original: self.original.borrow().clone(),
            emit_nodes: self.emit_nodes.borrow().clone(),
            assigned_name: self.assigned_name.borrow().clone(),
            class_this: self.class_this.borrow().clone(),
            emit_helpers: self.emit_helpers.borrow().clone(),
            emit_node_refs: self.emit_node_refs.get(),
        }
    }

    /// A new context with the side tables `tables`.
    #[must_use]
    pub fn from_print_tables(tables: PrintTables) -> Rc<EmitContext> {
        let c = new_emit_context();
        *c.auto_generate.borrow_mut() = tables.auto_generate;
        *c.text_source.borrow_mut() = tables.text_source;
        *c.original.borrow_mut() = tables.original;
        *c.emit_nodes.borrow_mut() = tables.emit_nodes;
        *c.assigned_name.borrow_mut() = tables.assigned_name;
        *c.class_this.borrow_mut() = tables.class_this;
        *c.emit_helpers.borrow_mut() = tables.emit_helpers;
        c.emit_node_refs.set(tables.emit_node_refs);
        c
    }
}

/// PORT: not in Go. ts#64649: the emit nodes of parse-tree nodes in the
/// emit context of a file's JS part after its script transforms
/// (`EmitContext::export_parse_emit_nodes`). Go runs the JS and d.ts parts
/// of a file in one context (compiler/emitter.go:50-53), so the d.ts
/// transforms and printer read what the JS transforms wrote on parse-tree
/// nodes: `EFNoTrailingSourceMap` on the names of changed parameters
/// (legacydecorators.go:145, esdecorator.go:1821), `EFNoLeadingComments` on
/// member names (esdecorator.go:1439), the type nodes of typed variable
/// names (typeeraser.go:192). Where the port runs the two parts in two
/// emitters (`program_emit`: the emit pool and the d.ts twins), the d.ts
/// part imports all of these entries into its new context before its
/// transforms (`EmitContext::import_parse_emit_nodes`). It is `Send`, so it
/// can go from the emit pool to the checker thread.
///
/// What the d.ts part does not get, none of which a d.ts part reads:
/// - entries whose key the JS part made: a d.ts tree holds parse-tree nodes
///   and nodes that the d.ts part made, never a node that the JS part made;
/// - the other tables (`original`, `auto_generate`, ...): the JS transforms
///   key them only with nodes that they made;
/// - the external helpers module name (an identifier that the JS part made,
///   see `export_parse_emit_nodes`);
/// - what the JS print writes after the export (Go printer.go:1601,
///   `EFNoSourceMap` on a function body): a d.ts never prints a body.
pub struct ParseEmitNodes(Vec<(Node, EmitNode)>);

impl EmitContext {
    /// The emit node of every parse-tree key of this context, each a full
    /// copy (`ParseEmitNodes`). The one field that is not copied is the
    /// external helpers module name: the JS part makes that identifier on
    /// its own thread, and the d.ts printer has `NoEmitHelpers` and no
    /// helper name identifier (Go printer.go:1157, :4638), so it never reads
    /// it.
    ///
    /// # Panics
    ///
    /// When a type node (Go typeeraser.go:192 `SetTypeNode(name, n.Type)`)
    /// is not a parse-tree node: another thread could not read it. Go sets
    /// only the parsed or reparsed (JSDoc `@type`) type of a parse-tree
    /// variable declaration, so this is a port error, and it stops the emit
    /// instead of a d.ts that differs from Go.
    #[must_use]
    pub fn export_parse_emit_nodes(&self) -> ParseEmitNodes {
        let emit_nodes = self.emit_nodes.borrow();
        let entries = emit_nodes
            .parsed_keys
            .iter()
            .filter(|&&node| is_parse_tree_node(node))
            .filter_map(|&node| {
                let mut emit_node = (**emit_nodes.other.try_get(node)?).clone();
                emit_node.external_helpers_module_name = Node::NIL;
                assert!(
                    emit_node.type_node.is_nil() || is_parse_tree_node(emit_node.type_node),
                    "the JS transforms set a type node that is not a parse-tree node"
                );
                Some((node, emit_node))
            })
            .collect();
        ParseEmitNodes(entries)
    }

    /// Adds the emit nodes `nodes` (`export_parse_emit_nodes` of a JS part)
    /// to this new context of the file's d.ts part, before its declaration
    /// transforms: an update or clone in those transforms copies them
    /// (Go `SetOriginalEx`, emitcontext.go:433), so they must be here first.
    pub fn import_parse_emit_nodes(&self, nodes: ParseEmitNodes) {
        let mut emit_nodes = self.emit_nodes.borrow_mut();
        for (node, emit_node) in nodes.0 {
            if emit_node.type_node.is_some() {
                self.emit_node_refs.set(true);
            }
            **emit_nodes.get(node) = emit_node;
        }
    }
}

impl EmitContext {
    /// Go `c.Factory`.
    #[must_use]
    pub fn factory(&self) -> &NodeFactory {
        &self.factory
    }

    // Go: printer/emitcontext.go:50 onCreate
    pub(crate) fn on_create(&self, node: Node) {
        crate::ast::synthetic::add_node_flags(node, NodeFlags::SYNTHESIZED);
    }

    // Go: printer/emitcontext.go:54 onUpdate
    pub(crate) fn on_update(&self, updated: Node, original: Node) {
        self.set_original(updated, original);
    }

    // Go: printer/emitcontext.go:58 onClone
    pub(crate) fn on_clone(&self, updated: Node, original: Node) {
        self.set_original(updated, original);
        if is_identifier(updated) || is_private_identifier(updated) {
            let auto_generate = self.auto_generate.borrow().get(&original).cloned();
            if let Some(auto_generate_copy) = auto_generate {
                self.auto_generate
                    .borrow_mut()
                    .insert(updated, auto_generate_copy);
            }
        }
    }

    // Go: printer/emitcontext.go:69 NewNodeVisitor
    // Creates a new NodeVisitor attached to this EmitContext
    // PORT: `ctx` is the callback state (see `ast/visitor.rs`). Go sets five
    // hooks that close over `c`. Here the visitor borrows this context for
    // `'a` in `emit_context` and calls it when those hooks are unset, so no
    // hook closure is allocated. The visitor factory is
    // `c.Factory.AsNodeFactory()`, the `ast` factory inside the printer one.
    pub fn new_node_visitor<'a, C>(
        &'a self,
        visit: impl Fn(Node, &mut NodeVisitor<'a, C>) -> Node + 'a,
        ctx: C,
    ) -> NodeVisitor<'a, C> {
        let mut visitor = new_node_visitor(
            visit,
            Some(&self.factory().ast),
            NodeVisitorHooks::default(),
            ctx,
        );
        visitor.emit_context = Some(self);
        visitor
    }

    //
    // Environment tracking
    //

    // Go: printer/emitcontext.go:88 StartVariableEnvironment
    // Starts a new VariableEnvironment used to track hoisted `var` statements and function declarations.
    //
    // NOTE: This is the equivalent of `transformContext.startLexicalEnvironment` in Strada.
    pub fn start_variable_environment(&self) {
        self.var_scope_stack
            .borrow_mut()
            .push(Rc::new(RefCell::new(VarScope::default())));
        self.start_lexical_environment();
    }

    // Go: printer/emitcontext.go:96 EndVariableEnvironment
    // Ends the current VariableEnvironment, returning a list of statements that should be emitted at the start of the current scope.
    //
    // NOTE: This is the equivalent of `transformContext.endLexicalEnvironment` in Strada.
    pub fn end_variable_environment(&self) -> Vec<Node> {
        let scope = self
            .var_scope_stack
            .borrow_mut()
            .pop()
            .expect("stack is empty");
        let (functions, variables, initialization_statements) = {
            let s = scope.borrow();
            (
                s.functions.clone(),
                s.variables.clone(),
                s.initialization_statements.clone(),
            )
        };
        let mut statements: Vec<Node> = Vec::new();
        if !functions.is_empty() {
            statements = functions;
        }
        if !variables.is_empty() {
            let f = self.factory();
            let var_decl_list =
                f.new_variable_declaration_list(f.new_node_list(&variables), NodeFlags::NONE);
            let var_statement =
                f.new_variable_statement(ModifierList::NIL /*modifiers*/, var_decl_list);
            self.set_emit_flags(var_statement, EmitFlags::CUSTOM_PROLOGUE);
            statements.push(var_statement);
        }
        if !initialization_statements.is_empty() {
            statements.extend(initialization_statements);
        }
        statements.extend(self.end_lexical_environment());
        statements
    }

    // Go: printer/emitcontext.go:115 EndAndMergeVariableEnvironmentList
    // Invokes c.EndVariableEnvironment() and merges the results into `statements`
    pub fn end_and_merge_variable_environment_list(&self, statements: NodeList) -> NodeList {
        let nodes: Vec<Node> = if statements.is_some() {
            statements.nodes().to_vec()
        } else {
            Vec::new()
        };

        let (result, changed) = self.end_and_merge_variable_environment_(&nodes);
        if changed {
            // PORT: Go `list := NewNodeList(result); list.Loc = statements.Loc`.
            return new_synthetic_node_list(&result, statements.loc());
        }

        statements
    }

    // Go: printer/emitcontext.go:131 EndAndMergeVariableEnvironment
    // Invokes c.EndVariableEnvironment() and merges the results into `statements`
    pub fn end_and_merge_variable_environment(&self, statements: &[Node]) -> Vec<Node> {
        let (result, _) = self.end_and_merge_variable_environment_(statements);
        result
    }

    // Go: printer/emitcontext.go:136 endAndMergeVariableEnvironment
    // PORT: trailing `_` separates the unexported Go method from the exported one.
    fn end_and_merge_variable_environment_(&self, statements: &[Node]) -> (Vec<Node>, bool) {
        let declarations = self.end_variable_environment();
        self.merge_environment_(statements, &declarations)
    }

    // Go: printer/emitcontext.go:143 AddVariableDeclaration
    // Adds a `var` declaration to the current VariableEnvironment
    //
    // NOTE: This is the equivalent of `transformContext.hoistVariableDeclaration` in Strada.
    pub fn add_variable_declaration(&self, name: Node) {
        let var_decl = self.factory().new_variable_declaration(
            name,
            Node::NIL, /*exclamationToken*/
            Node::NIL, /*typeNode*/
            Node::NIL, /*initializer*/
        );
        self.set_emit_flags(var_decl, EmitFlags::NO_NESTED_SOURCE_MAPS);
        let scope = self
            .var_scope_stack
            .borrow()
            .last()
            .cloned()
            .expect("stack is empty");
        let mut scope = scope.borrow_mut();
        scope.variables.push(var_decl);
        if scope.flags & ENVIRONMENT_FLAGS_IN_PARAMETERS != 0 {
            scope.flags |= ENVIRONMENT_FLAGS_VARIABLES_HOISTED_IN_PARAMETERS;
        }
    }

    // Go: printer/emitcontext.go:156 AddHoistedFunctionDeclaration
    // Adds a hoisted function declaration to the current VariableEnvironment
    //
    // NOTE: This is the equivalent of `transformContext.hoistFunctionDeclaration` in Strada.
    pub fn add_hoisted_function_declaration(&self, node: Node) {
        self.set_emit_flags(node, EmitFlags::CUSTOM_PROLOGUE);
        let scope = self
            .var_scope_stack
            .borrow()
            .last()
            .cloned()
            .expect("stack is empty");
        scope.borrow_mut().functions.push(node);
    }

    // Go: printer/emitcontext.go:168 StartLexicalEnvironment
    // Starts a new LexicalEnvironment used to track block-scoped `let`, `const`, and `using` declarations.
    //
    // NOTE: This is the equivalent of `transformContext.startBlockScope` in Strada.
    // NOTE: This is *not* the same as `startLexicalEnvironment` in Strada as that method is incorrectly named.
    pub fn start_lexical_environment(&self) {
        self.let_scope_stack
            .borrow_mut()
            .push(Rc::new(RefCell::new(VarScope::default())));
    }

    // Go: printer/emitcontext.go:176 EndLexicalEnvironment
    // Ends the current EndLexicalEnvironment, returning a list of statements that should be emitted at the start of the current scope.
    //
    // NOTE: This is the equivalent of `transformContext.endLexicalEnvironment` in Strada.
    // NOTE: This is *not* the same as `endLexicalEnvironment` in Strada as that method is incorrectly named.
    pub fn end_lexical_environment(&self) -> Vec<Node> {
        let scope = self
            .let_scope_stack
            .borrow_mut()
            .pop()
            .expect("stack is empty");
        let variables = scope.borrow().variables.clone();
        let mut statements: Vec<Node> = Vec::new();
        if !variables.is_empty() {
            let f = self.factory();
            let var_decl_list =
                f.new_variable_declaration_list(f.new_node_list(&variables), NodeFlags::LET);
            let var_statement =
                f.new_variable_statement(ModifierList::NIL /*modifiers*/, var_decl_list);
            self.set_emit_flags(var_statement, EmitFlags::CUSTOM_PROLOGUE);
            statements.push(var_statement);
        }
        statements
    }

    // Go: printer/emitcontext.go:189 EndAndMergeLexicalEnvironmentList
    // Invokes c.EndLexicalEnvironment() and merges the results into `statements`
    pub fn end_and_merge_lexical_environment_list(&self, statements: NodeList) -> NodeList {
        let nodes: Vec<Node> = if statements.is_some() {
            statements.nodes().to_vec()
        } else {
            Vec::new()
        };

        let (result, changed) = self.end_and_merge_lexical_environment_(&nodes);
        if changed {
            // PORT: Go `list := NewNodeList(result); list.Loc = statements.Loc`.
            return new_synthetic_node_list(&result, statements.loc());
        }

        statements
    }

    // Go: printer/emitcontext.go:205 EndAndMergeLexicalEnvironment
    // Invokes c.EndLexicalEnvironment() and merges the results into `statements`
    pub fn end_and_merge_lexical_environment(&self, statements: &[Node]) -> Vec<Node> {
        let (result, _) = self.end_and_merge_lexical_environment_(statements);
        result
    }

    // Go: printer/emitcontext.go:211 endAndMergeLexicalEnvironment
    // Invokes c.EndLexicalEnvironment() and merges the results into `statements`
    fn end_and_merge_lexical_environment_(&self, statements: &[Node]) -> (Vec<Node>, bool) {
        let declarations = self.end_lexical_environment();
        self.merge_environment_(statements, &declarations)
    }

    // Go: printer/emitcontext.go:216 AddLexicalDeclaration
    // Adds a `let` declaration to the current LexicalEnvironment.
    pub fn add_lexical_declaration(&self, name: Node) {
        let var_decl = self.factory().new_variable_declaration(
            name,
            Node::NIL, /*exclamationToken*/
            Node::NIL, /*typeNode*/
            Node::NIL, /*initializer*/
        );
        self.set_emit_flags(var_decl, EmitFlags::NO_NESTED_SOURCE_MAPS);
        let scope = self
            .let_scope_stack
            .borrow()
            .last()
            .cloned()
            .expect("stack is empty");
        scope.borrow_mut().variables.push(var_decl);
    }

    // Go: printer/emitcontext.go:224 MergeEnvironmentList
    // Merges declarations produced by c.EndVariableEnvironment() or c.EndLexicalEnvironment() into a statement list
    pub fn merge_environment_list(&self, statements: NodeList, declarations: &[Node]) -> NodeList {
        let (result, changed) = self.merge_environment_(&statements.nodes().to_vec(), declarations);
        if changed {
            // PORT: Go `list := NewNodeList(result); list.Loc = statements.Loc`.
            return new_synthetic_node_list(&result, statements.loc());
        }
        statements
    }

    // Go: printer/emitcontext.go:234 MergeEnvironment
    // Merges declarations produced by c.EndVariableEnvironment() or c.EndLexicalEnvironment() into a slice of statements
    pub fn merge_environment(&self, statements: &[Node], declarations: &[Node]) -> Vec<Node> {
        let (result, _) = self.merge_environment_(statements, declarations);
        result
    }

    // Go: printer/emitcontext.go:239 mergeEnvironment
    fn merge_environment_(&self, statements: &[Node], declarations: &[Node]) -> (Vec<Node>, bool) {
        if declarations.is_empty() {
            return (statements.to_vec(), false);
        }

        // When we merge new lexical statements into an existing statement list, we merge them in the following manner:
        //
        // Given:
        //
        // | Left                               | Right                               |
        // |------------------------------------|-------------------------------------|
        // | [standard prologues (left)]        | [standard prologues (right)]        |
        // | [hoisted functions (left)]         | [hoisted functions (right)]         |
        // | [hoisted variables (left)]         | [hoisted variables (right)]         |
        // | [lexical init statements (left)]   | [lexical init statements (right)]   |
        // | [other statements (left)]          |                                     |
        //
        // The resulting statement list will be:
        //
        // | Result                              |
        // |-------------------------------------|
        // | [standard prologues (right)]        |
        // | [standard prologues (left)]         |
        // | [hoisted functions (right)]         |
        // | [hoisted functions (left)]          |
        // | [hoisted variables (right)]         |
        // | [hoisted variables (left)]          |
        // | [lexical init statements (right)]   |
        // | [lexical init statements (left)]    |
        // | [other statements (left)]           |
        //
        // NOTE: It is expected that new lexical init statements must be evaluated before existing lexical init statements,
        // as the prior transformation may depend on the evaluation of the lexical init statements to be in the correct state.

        let mut changed = false;

        // find standard prologues on left in the following order: standard directives, hoisted functions, hoisted variables, other custom
        let left_standard_prologue_end = find_span_end(statements, is_prologue_directive, 0);
        let left_hoisted_functions_end = find_span_end_with_emit_context(
            self,
            statements,
            EmitContext::is_hoisted_function,
            left_standard_prologue_end,
        );
        let left_hoisted_variables_end = find_span_end_with_emit_context(
            self,
            statements,
            EmitContext::is_hoisted_variable_statement,
            left_hoisted_functions_end,
        );

        // find standard prologues on right in the following order: standard directives, hoisted functions, hoisted variables, other custom
        let right_standard_prologue_end = find_span_end(declarations, is_prologue_directive, 0);
        let right_hoisted_functions_end = find_span_end_with_emit_context(
            self,
            declarations,
            EmitContext::is_hoisted_function,
            right_standard_prologue_end,
        );
        let right_hoisted_variables_end = find_span_end_with_emit_context(
            self,
            declarations,
            EmitContext::is_hoisted_variable_statement,
            right_hoisted_functions_end,
        );
        let right_custom_prologue_end = find_span_end_with_emit_context(
            self,
            declarations,
            EmitContext::is_custom_prologue,
            right_hoisted_variables_end,
        );
        if right_custom_prologue_end as usize != declarations.len() {
            panic!("Expected declarations to be valid standard or custom prologues");
        }

        let mut left: Vec<Node> = statements.to_vec();

        // PORT: Go `core.Splice(left, at, 0, items...)` inserts `items` at `at`.
        let splice = |left: &mut Vec<Node>, at: i32, items: &[Node]| {
            let at = (at.max(0) as usize).min(left.len());
            left.splice(at..at, items.iter().copied());
        };

        // splice other custom prologues from right into left
        if right_custom_prologue_end > right_hoisted_variables_end {
            splice(
                &mut left,
                left_hoisted_variables_end,
                &declarations
                    [right_hoisted_variables_end as usize..right_custom_prologue_end as usize],
            );
            changed = true;
        }

        // splice hoisted variables from right into left
        if right_hoisted_variables_end > right_hoisted_functions_end {
            splice(
                &mut left,
                left_hoisted_functions_end,
                &declarations
                    [right_hoisted_functions_end as usize..right_hoisted_variables_end as usize],
            );
            changed = true;
        }

        // splice hoisted functions from right into left
        if right_hoisted_functions_end > right_standard_prologue_end {
            splice(
                &mut left,
                left_standard_prologue_end,
                &declarations
                    [right_standard_prologue_end as usize..right_hoisted_functions_end as usize],
            );
            changed = true;
        }

        // splice standard prologues from right into left (that are not already in left)
        if right_standard_prologue_end > 0 {
            if left_standard_prologue_end == 0 {
                splice(
                    &mut left,
                    0,
                    &declarations[..right_standard_prologue_end as usize],
                );
                changed = true;
            } else {
                let mut left_prologues: FxHashSet<String> = FxHashSet::default();
                for i in 0..left_standard_prologue_end as usize {
                    let left_prologue = statements[i];
                    left_prologues.insert(left_prologue.expression().text().to_string());
                }
                for i in (0..right_standard_prologue_end as usize).rev() {
                    let right_prologue = declarations[i];
                    if !left_prologues.contains(right_prologue.expression().text()) {
                        left.insert(0, right_prologue);
                        changed = true;
                    }
                }
            }
        }

        (left, changed)
    }

    // Go: printer/emitcontext.go:333 isCustomPrologue
    fn is_custom_prologue(&self, node: Node) -> bool {
        self.emit_flags(node).intersects(EmitFlags::CUSTOM_PROLOGUE)
    }

    // Go: printer/emitcontext.go:337 isHoistedFunction
    fn is_hoisted_function(&self, node: Node) -> bool {
        self.is_custom_prologue(node) && is_function_declaration(node)
    }

    // Go: printer/emitcontext.go:345 isHoistedVariableStatement
    fn is_hoisted_variable_statement(&self, node: Node) -> bool {
        self.is_custom_prologue(node)
            && is_variable_statement(node)
            && node
                .declaration_list()
                .declarations()
                .nodes()
                .iter()
                .all(is_hoisted_variable)
    }

    //
    // Name Generation
    //

    // Go: printer/emitcontext.go:356 HasAutoGenerateInfo
    // Gets whether a given name has an associated AutoGenerateInfo entry.
    #[must_use]
    pub fn has_auto_generate_info(&self, node: Node) -> bool {
        if node.is_some() {
            return self.auto_generate.borrow().contains_key(&node);
        }
        false
    }

    // Go: printer/emitcontext.go:365 GetAutoGenerateInfo
    // Gets the associated AutoGenerateInfo entry for a given name.
    // PORT: Go returns the shared `*AutoGenerateInfo`; this returns a copy.
    #[must_use]
    pub fn get_auto_generate_info(&self, name: Node) -> Option<AutoGenerateInfo> {
        if name.is_nil() {
            return None;
        }
        self.auto_generate.borrow().get(&name).cloned()
    }

    // Go: printer/emitcontext.go:373 GetNodeForGeneratedName
    // Walks the associated AutoGenerateInfo entries of a name to find the root Nopde from which the name should be generated.
    #[must_use]
    pub fn get_node_for_generated_name(&self, name: Node) -> Node {
        let auto_generate = self.auto_generate.borrow().get(&name).cloned();
        if let Some(auto_generate) = auto_generate {
            if auto_generate.flags.is_node() {
                return self
                    .get_node_for_generated_name_worker(auto_generate.node, auto_generate.id);
            }
        }
        name
    }

    // Go: printer/emitcontext.go:380 getNodeForGeneratedNameWorker
    pub(crate) fn get_node_for_generated_name_worker(
        &self,
        mut node: Node,
        auto_generate_id: AutoGenerateId,
    ) -> Node {
        let mut original = self.original(node);
        while original.is_some() {
            node = original;
            if is_member_name(node) {
                // if "node" is a different generated name (having a different "autoGenerateId"), use it and stop traversing.
                let auto_generate = self.auto_generate.borrow().get(&node).cloned();
                let Some(auto_generate) = auto_generate else {
                    break;
                };
                if auto_generate.flags.is_node() && auto_generate.id != auto_generate_id {
                    break;
                }
                if auto_generate.flags.is_node() {
                    original = auto_generate.node;
                    continue;
                }
            }
            original = self.original(node);
        }
        node
    }

    //
    // Original Node Tracking
    //

    // Go: printer/emitcontext.go:425 SetOriginal
    // Sets the original node for a given node.
    //
    // NOTE: This is the equivalent to `setOriginalNode` in Strada.
    pub fn set_original(&self, node: Node, original: Node) {
        self.set_original_ex(node, original, false);
    }

    // Go: printer/emitcontext.go:429 UnsetOriginal
    pub fn unset_original(&self, node: Node) {
        self.original.borrow_mut().remove(node);
    }

    // Go: printer/emitcontext.go:433 SetOriginalEx
    pub fn set_original_ex(&self, node: Node, original: Node, allow_overwrite: bool) {
        if original.is_nil() {
            panic!("Original cannot be nil.");
        }

        // PERF: emitast1. One lookup of `node`, not a get and an insert.
        if !self
            .original
            .borrow_mut()
            .set(node, original, allow_overwrite)
        {
            return;
        }
        let mut emit_nodes = self.emit_nodes.borrow_mut();
        // PERF: emitast2. Copy only the fields that `copy_from` reads, not
        // the whole boxed emit node with its comment lists.
        if let Some(source) = emit_nodes.try_get(original).map(|e| e.copied_fields()) {
            emit_nodes.get(node).copy_from(source);
        }
    }

    // Go: printer/emitcontext.go:458 Original
    // Gets the original node for a given node.
    //
    // NOTE: This is the equivalent to reading `node.original` in Strada.
    #[must_use]
    pub fn original(&self, node: Node) -> Node {
        self.original.borrow().get(node)
    }

    // Go: printer/emitcontext.go:466 MostOriginal
    // Gets the most original node associated with this node by walking Original pointers.
    //
    // NOTE: This method is analogous to `getOriginalNode` in the old compiler, but the name has changed to avoid accidental
    // conflation with `SetOriginal`/`Original`
    #[must_use]
    pub fn most_original(&self, mut node: Node) -> Node {
        if node.is_some() {
            let mut original = self.original(node);
            while original.is_some() {
                node = original;
                original = self.original(node);
            }
        }
        node
    }

    // Go: printer/emitcontext.go:480 ParseNode
    // Gets the original parse tree node for a given node.
    //
    // NOTE: This is the equivalent to `getParseTreeNode` in Strada.
    #[must_use]
    pub fn parse_node(&self, node: Node) -> Node {
        let node = self.most_original(node);
        if node.is_some() && is_parse_tree_node(node) {
            return node;
        }
        Node::NIL
    }

    // Go: printer/emitcontext.go:488 IsFileLevelUniqueName
    // PORT: `source_file_has_identifier` is Go `(*SourceFile).HasIdentifier`
    // (tsgo#4731, the syntax lane's part).
    pub fn is_file_level_unique_name(
        &self,
        source_file: Node,
        name: &str,
        has_global_name: Option<&dyn Fn(&str) -> bool>,
    ) -> bool {
        if let Some(has_global_name) = has_global_name
            && has_global_name(name)
        {
            return false;
        }
        let source_file = self.most_original(source_file);
        !source_file_has_identifier(source_file, name)
    }
}

// Go: printer/emitcontext.go:341 isHoistedVariable
fn is_hoisted_variable(node: Node) -> bool {
    is_identifier(node.name()) && node.initializer().is_nil()
}

/// Go `AutoGenerateOptions`.
#[derive(Clone, Debug, Default)]
pub struct AutoGenerateOptions {
    pub flags: GeneratedIdentifierFlags,
    pub prefix: String,
    pub suffix: String,
}

/// Go `nextAutoGenerateId atomic.Uint32`. Use `next_auto_generate_id()` for
/// Go `AutoGenerateId(nextAutoGenerateId.Add(1))`.
pub static NEXT_AUTO_GENERATE_ID: AtomicU32 = AtomicU32::new(0);

/// Go `AutoGenerateId(nextAutoGenerateId.Add(1))`.
#[must_use]
pub fn next_auto_generate_id() -> AutoGenerateId {
    AutoGenerateId(NEXT_AUTO_GENERATE_ID.fetch_add(1, Ordering::Relaxed) + 1)
}

/// Go `AutoGenerateId`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AutoGenerateId(pub u32);

/// Go `AutoGenerateInfo`.
#[derive(Clone, Debug, Default)]
pub struct AutoGenerateInfo {
    pub flags: GeneratedIdentifierFlags, // Specifies whether to auto-generate the text for an identifier.
    pub id: AutoGenerateId, // Ensures unique generated identifiers get unique names, but clones get the same name.
    pub prefix: String,     // Optional prefix to apply to the start of the generated name
    pub suffix: String,     // Optional suffix to apply to the end of the generated name
    pub node: Node, // For a GeneratedIdentifierFlagsNode, the node from which to generate an identifier
}

//
// Emit-related Data
//

/// Go `emitNodeFlags`.
pub(crate) type EmitNodeFlags = u32;

pub(crate) const HAS_COMMENT_RANGE: EmitNodeFlags = 1 << 0;
pub(crate) const HAS_SOURCE_MAP_RANGE: EmitNodeFlags = 1 << 1;

// Go: printer/emitcontext.go:507 SnippetKind
go_enum!(SnippetKind, i32 {
    TAB_STOP = 0; // SnippetKindTabStop
});

// Go: printer/emitcontext.go:513 SnippetElement
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SnippetElement {
    pub kind: SnippetKind,
    pub order: i32,
}

/// Go `SynthesizedComment`.
#[derive(Clone, Debug)]
pub struct SynthesizedComment {
    pub kind: SyntaxKind,
    pub loc: TextRange,
    pub has_leading_new_line: bool,
    pub has_trailing_new_line: bool,
    pub text: String,
}

/// Go `emitNode`.
#[derive(Clone, Default)]
pub(crate) struct EmitNode {
    pub(crate) flags: EmitNodeFlags,
    pub(crate) emit_flags: EmitFlags,
    pub(crate) comment_range: TextRange,
    pub(crate) source_map_range: TextRange,
    /// Go nil map is `None`.
    pub(crate) token_source_map_ranges: Option<FxHashMap<SyntaxKind, TextRange>>,
    pub(crate) helpers: Vec<EmitHelperRef>,
    pub(crate) external_helpers_module_name: Node,
    pub(crate) leading_comments: Vec<SynthesizedComment>,
    pub(crate) trailing_comments: Vec<SynthesizedComment>,
    pub(crate) type_node: Node,
    /// Go nil `*SnippetElement` is `None`.
    pub(crate) snippet_element: Option<SnippetElement>,
}

impl EmitNode {
    /// An emit node with the fields of this one that `copy_from` reads, so
    /// a copy needs no box and no comment list clone.
    fn copied_fields(&self) -> EmitNode {
        EmitNode {
            flags: self.flags,
            emit_flags: self.emit_flags,
            comment_range: self.comment_range,
            source_map_range: self.source_map_range,
            token_source_map_ranges: self.token_source_map_ranges.clone(),
            helpers: self.helpers.clone(),
            external_helpers_module_name: self.external_helpers_module_name,
            snippet_element: self.snippet_element,
            ..EmitNode::default()
        }
    }

    // Go: printer/emitcontext.go:541 copyFrom
    // NOTE: This method is not guaranteed to be thread-safe
    // PORT: `source` is a copy (`copied_fields`), so its map and helper
    // list move here; Go clones them (`maps.Clone`, `slices.Clone`).
    pub(crate) fn copy_from(&mut self, source: EmitNode) {
        self.flags = source.flags;
        self.emit_flags = source.emit_flags;
        self.comment_range = source.comment_range;
        self.source_map_range = source.source_map_range;
        self.token_source_map_ranges = source.token_source_map_ranges;
        self.helpers = source.helpers;
        self.external_helpers_module_name = source.external_helpers_module_name;
        if let Some(snippet_element) = source.snippet_element {
            self.snippet_element = Some(snippet_element);
        }
    }
}

/// Go `core.AppendIfUnique` for `*EmitHelper` (pointer identity).
fn append_helper_if_unique(helpers: &mut Vec<EmitHelperRef>, helper: EmitHelperRef) {
    if !helpers.iter().any(|h| std::ptr::eq(*h, helper)) {
        helpers.push(helper);
    }
}

impl EmitContext {
    // Go: printer/emitcontext.go:555 EmitFlags
    #[must_use]
    pub fn emit_flags(&self, node: Node) -> EmitFlags {
        if let Some(emit_node) = self.emit_nodes.borrow().try_get(node) {
            return emit_node.emit_flags;
        }
        EmitFlags::NONE
    }

    // Go: printer/emitcontext.go:562 SetEmitFlags
    pub fn set_emit_flags(&self, node: Node, flags: EmitFlags) {
        self.emit_nodes.borrow_mut().get(node).emit_flags = flags;
    }

    // Go: printer/emitcontext.go:566 AddEmitFlags
    pub fn add_emit_flags(&self, node: Node, flags: EmitFlags) {
        let mut emit_nodes = self.emit_nodes.borrow_mut();
        let emit_node = emit_nodes.get(node);
        emit_node.emit_flags = emit_node.emit_flags | flags;
    }

    // Go: printer/emitcontext.go:570 EmitContext.SnippetElement
    #[must_use]
    pub fn snippet_element(&self, node: Node) -> Option<SnippetElement> {
        if let Some(emit_node) = self.emit_nodes.borrow().try_get(node) {
            return emit_node.snippet_element;
        }
        None
    }

    // Go: printer/emitcontext.go:577 SetSnippetElement
    pub fn set_snippet_element(&self, node: Node, snippet_element: SnippetElement) {
        self.emit_nodes.borrow_mut().get(node).snippet_element = Some(snippet_element);
    }

    // Go: printer/emitcontext.go:582 CommentRange
    // Gets the range to use for a node when emitting comments.
    #[must_use]
    pub fn comment_range(&self, node: Node) -> TextRange {
        if let Some(emit_node) = self.emit_nodes.borrow().try_get(node) {
            if emit_node.flags & HAS_COMMENT_RANGE != 0 {
                return emit_node.comment_range;
            }
        }
        node.loc()
    }

    // Go: printer/emitcontext.go:590 SetCommentRange
    // Sets the range to use for a node when emitting comments.
    pub fn set_comment_range(&self, node: Node, loc: TextRange) {
        let mut emit_nodes = self.emit_nodes.borrow_mut();
        let emit_node = emit_nodes.get(node);
        emit_node.comment_range = loc;
        emit_node.flags |= HAS_COMMENT_RANGE;
    }

    // Go: printer/emitcontext.go:597 AssignCommentRange
    // Sets the range to use for a node when emitting comments.
    pub fn assign_comment_range(&self, to: Node, from: Node) {
        self.set_comment_range(to, self.comment_range(from));
    }

    // Go: printer/emitcontext.go:602 SourceMapRange
    // Gets the range to use for a node when emitting source maps.
    #[must_use]
    pub fn source_map_range(&self, node: Node) -> TextRange {
        if let Some(emit_node) = self.emit_nodes.borrow().try_get(node) {
            if emit_node.flags & HAS_SOURCE_MAP_RANGE != 0 {
                return emit_node.source_map_range;
            }
        }
        node.loc()
    }

    // Go: printer/emitcontext.go:610 SetSourceMapRange
    // Sets the range to use for a node when emitting source maps.
    pub fn set_source_map_range(&self, node: Node, loc: TextRange) {
        let mut emit_nodes = self.emit_nodes.borrow_mut();
        let emit_node = emit_nodes.get(node);
        emit_node.source_map_range = loc;
        emit_node.flags |= HAS_SOURCE_MAP_RANGE;
    }

    // Go: printer/emitcontext.go:617 AssignSourceMapRange
    // Sets the range to use for a node when emitting source maps.
    pub fn assign_source_map_range(&self, to: Node, from: Node) {
        self.set_source_map_range(to, self.source_map_range(from));
    }

    // Go: printer/emitcontext.go:622 AssignCommentAndSourceMapRanges
    // Sets the range to use for a node when emitting comments and source maps.
    pub fn assign_comment_and_source_map_ranges(&self, to: Node, from: Node) {
        // PORT: Go gets `emitNode` for `to` first. The Rust borrow is taken
        // after the reads; `Get` creates the same entry either way.
        let comment_range = self.comment_range(from);
        let source_map_range = self.source_map_range(from);
        let mut emit_nodes = self.emit_nodes.borrow_mut();
        let emit_node = emit_nodes.get(to);
        emit_node.comment_range = comment_range;
        emit_node.source_map_range = source_map_range;
        emit_node.flags |= HAS_COMMENT_RANGE | HAS_SOURCE_MAP_RANGE;
    }

    // Go: printer/emitcontext.go:632 TokenSourceMapRange
    // Gets the range for a token of a node when emitting source maps.
    #[must_use]
    pub fn token_source_map_range(&self, node: Node, kind: SyntaxKind) -> (TextRange, bool) {
        if let Some(emit_node) = self.emit_nodes.borrow().try_get(node) {
            if let Some(ranges) = &emit_node.token_source_map_ranges {
                if let Some(loc) = ranges.get(&kind) {
                    return (*loc, true);
                }
            }
        }
        (TextRange::new(0, 0), false)
    }

    // Go: printer/emitcontext.go:642 SetTokenSourceMapRange
    // Sets the range for a token of a node when emitting source maps.
    pub fn set_token_source_map_range(&self, node: Node, kind: SyntaxKind, loc: TextRange) {
        let mut emit_nodes = self.emit_nodes.borrow_mut();
        let emit_node = emit_nodes.get(node);
        emit_node
            .token_source_map_ranges
            .get_or_insert_with(FxHashMap::default)
            .insert(kind, loc);
    }

    // Go: printer/emitcontext.go:650 AssignedName
    #[must_use]
    pub fn assigned_name(&self, node: Node) -> Node {
        self.assigned_name
            .borrow()
            .get(&node)
            .copied()
            .unwrap_or(Node::NIL)
    }

    // Go: printer/emitcontext.go:654 TextSource
    #[must_use]
    pub fn text_source(&self, node: Node) -> Node {
        self.text_source
            .borrow()
            .get(&node)
            .copied()
            .unwrap_or(Node::NIL)
    }

    // Go: printer/emitcontext.go:658 SetAssignedName
    pub fn set_assigned_name(&self, node: Node, name: Node) {
        self.assigned_name.borrow_mut().insert(node, name);
    }

    // Go: printer/emitcontext.go:665 ClassThis
    #[must_use]
    pub fn class_this(&self, node: Node) -> Node {
        self.class_this
            .borrow()
            .get(&node)
            .copied()
            .unwrap_or(Node::NIL)
    }

    // Go: printer/emitcontext.go:669 SetClassThis
    pub fn set_class_this(&self, node: Node, class_this: Node) {
        self.class_this.borrow_mut().insert(node, class_this);
    }

    // Go: printer/emitcontext.go:676 RequestEmitHelper
    pub fn request_emit_helper(&self, helper: EmitHelperRef) {
        if helper.scoped {
            panic!("Cannot request a scoped emit helper");
        }
        for h in helper.dependencies.iter() {
            self.request_emit_helper(h);
        }
        append_helper_if_unique(&mut self.emit_helpers.borrow_mut(), helper);
    }

    // Go: printer/emitcontext.go:686 ReadEmitHelpers
    pub fn read_emit_helpers(&self) -> Vec<EmitHelperRef> {
        std::mem::take(&mut *self.emit_helpers.borrow_mut())
    }

    // Go: printer/emitcontext.go:692 AddEmitHelper
    pub fn add_emit_helper(&self, node: Node, helper: &[EmitHelperRef]) {
        let mut emit_nodes = self.emit_nodes.borrow_mut();
        let emit_node = emit_nodes.get(node);
        for h in helper {
            append_helper_if_unique(&mut emit_node.helpers, h);
        }
    }

    // Go: printer/emitcontext.go:699 MoveEmitHelpers
    pub fn move_emit_helpers(
        &self,
        source: Node,
        target: Node,
        predicate: &mut dyn FnMut(EmitHelperRef) -> bool,
    ) {
        let source_emit_helpers = match self.emit_nodes.borrow().try_get(source) {
            None => return,
            Some(source_emit_node) => source_emit_node.helpers.clone(),
        };
        if source_emit_helpers.is_empty() {
            return;
        }

        // PORT: Go compacts the source slice in place while it appends to the
        // target. The predicate runs without a borrow held.
        let mut kept: Vec<EmitHelperRef> = Vec::with_capacity(source_emit_helpers.len());
        let mut helpers_removed = 0;
        for helper in source_emit_helpers {
            if predicate(helper) {
                helpers_removed += 1;
                let mut emit_nodes = self.emit_nodes.borrow_mut();
                append_helper_if_unique(&mut emit_nodes.get(target).helpers, helper);
            } else {
                kept.push(helper);
            }
        }
        // Go gets the target entry even when no helper moves.
        self.emit_nodes.borrow_mut().get(target);

        if helpers_removed > 0 {
            self.emit_nodes.borrow_mut().get(source).helpers = kept;
        }
    }

    // Go: printer/emitcontext.go:727 GetEmitHelpers
    #[must_use]
    pub fn get_emit_helpers(&self, node: Node) -> Vec<EmitHelperRef> {
        if let Some(emit_node) = self.emit_nodes.borrow().try_get(node) {
            return emit_node.helpers.clone();
        }
        Vec::new()
    }

    // Go: printer/emitcontext.go:735 GetExternalHelpersModuleName
    #[must_use]
    pub fn get_external_helpers_module_name(&self, node: Node) -> Node {
        let parse_node = self.parse_node(node);
        if parse_node.is_some() {
            if let Some(emit_node) = self.emit_nodes.borrow().try_get(parse_node) {
                return emit_node.external_helpers_module_name;
            }
        }
        Node::NIL
    }

    // Go: printer/emitcontext.go:744 SetExternalHelpersModuleName
    pub fn set_external_helpers_module_name(&self, node: Node, name: Node) {
        let parse_node = self.parse_node(node);
        if parse_node.is_nil() {
            panic!(
                "Node must be a parse tree node or have an Original pointer to a parse tree node."
            );
        }

        self.emit_nodes
            .borrow_mut()
            .get(parse_node)
            .external_helpers_module_name = name;
        self.emit_node_refs.set(true);
    }

    // Go: printer/emitcontext.go:754 HasRecordedExternalHelpers
    #[must_use]
    pub fn has_recorded_external_helpers(&self, node: Node) -> bool {
        let parse_node = self.parse_node(node);
        if parse_node.is_some() {
            return match self.emit_nodes.borrow().try_get(parse_node) {
                None => false,
                Some(emit_node) => {
                    emit_node.external_helpers_module_name.is_some()
                        || emit_node.emit_flags.intersects(EmitFlags::EXTERNAL_HELPERS)
                }
            };
        }
        false
    }

    // Go: printer/emitcontext.go:762 IsCallToHelper
    #[must_use]
    pub fn is_call_to_helper(&self, first_segment: Node, helper_name: &str) -> bool {
        is_call_expression(first_segment)
            && is_identifier(first_segment.expression())
            && self
                .emit_flags(first_segment.expression())
                .intersects(EmitFlags::HELPER_NAME)
            && first_segment.expression().text() == helper_name
    }

    //
    // Visitor Hooks
    //

    // Go: printer/emitcontext.go:773 VisitVariableEnvironment
    pub fn visit_variable_environment<C>(
        &self,
        nodes: NodeList,
        visitor: &mut NodeVisitor<'_, C>,
    ) -> NodeList {
        self.start_variable_environment();
        let visited = visitor.visit_nodes(nodes);
        self.end_and_merge_variable_environment_list(visited)
    }

    // Go: printer/emitcontext.go:778 VisitParameters
    pub fn visit_parameters<C>(
        &self,
        nodes: NodeList,
        visitor: &mut NodeVisitor<'_, C>,
    ) -> NodeList {
        self.start_variable_environment();
        let scope = self
            .var_scope_stack
            .borrow()
            .last()
            .cloned()
            .expect("stack is empty");
        let old_flags = scope.borrow().flags;
        scope.borrow_mut().flags |= ENVIRONMENT_FLAGS_IN_PARAMETERS;
        let mut nodes = visitor.visit_nodes(nodes);

        // As of ES2015, any runtime execution of that occurs in for a parameter (such as evaluating an
        // initializer or a binding pattern), occurs in its own lexical scope. As a result, any expression
        // that we might transform that introduces a temporary variable would fail as the temporary variable
        // exists in a different lexical scope. To address this, we move any binding patterns and initializers
        // in a parameter list to the body if we detect a variable being hoisted while visiting a parameter list
        // when the emit target is greater than ES2015. (Which is now all targets.)
        let hoisted = scope.borrow().flags & ENVIRONMENT_FLAGS_VARIABLES_HOISTED_IN_PARAMETERS != 0;
        if hoisted {
            nodes = self.add_default_value_assignments_if_needed(nodes);
        }
        scope.borrow_mut().flags = old_flags;
        // !!! c.suspendVariableEnvironment()
        nodes
    }

    // Go: printer/emitcontext.go:799 addDefaultValueAssignmentsIfNeeded
    fn add_default_value_assignments_if_needed(&self, node_list: NodeList) -> NodeList {
        if node_list.is_nil() {
            return node_list;
        }
        let mut result: Option<Vec<Node>> = None;
        let nodes = node_list.nodes().to_vec();
        for (i, &parameter) in nodes.iter().enumerate() {
            let updated = self.add_default_value_assignment_if_needed(parameter);
            if updated != parameter {
                result.get_or_insert_with(|| nodes.clone())[i] = updated;
            }
        }
        if let Some(result) = result {
            // PORT: Go `res := NewNodeList(result); res.Loc = nodeList.Loc`.
            return new_synthetic_node_list(&result, node_list.loc());
        }
        node_list
    }

    // Go: printer/emitcontext.go:822 addDefaultValueAssignmentIfNeeded
    fn add_default_value_assignment_if_needed(&self, parameter: Node) -> Node {
        // A rest parameter cannot have a binding pattern or an initializer,
        // so let's just ignore it.
        if parameter.dot_dot_dot_token().is_some() {
            return parameter;
        } else if is_binding_pattern(parameter.name()) {
            return self.add_default_value_assignment_for_binding_pattern(parameter);
        } else if parameter.initializer().is_some() {
            return self.add_default_value_assignment_for_initializer(
                parameter,
                parameter.name(),
                parameter.initializer(),
            );
        }
        parameter
    }

    // Go: printer/emitcontext.go:835 addDefaultValueAssignmentForBindingPattern
    fn add_default_value_assignment_for_binding_pattern(&self, parameter: Node) -> Node {
        let f = self.factory();
        let init_node = if parameter.initializer().is_some() {
            f.new_conditional_expression(
                f.new_strict_equality_expression(
                    f.new_generated_name_for_node(parameter),
                    f.new_void_zero_expression(),
                ),
                f.new_token(SyntaxKind::QuestionToken),
                parameter.initializer(),
                f.new_token(SyntaxKind::ColonToken),
                f.new_generated_name_for_node(parameter),
            )
        } else {
            f.new_generated_name_for_node(parameter)
        };
        self.add_initialization_statement(f.new_variable_statement(
            ModifierList::NIL,
            f.new_variable_declaration_list(
                f.new_node_list(&[f.new_variable_declaration(
                    parameter.name(),
                    Node::NIL,
                    parameter.type_(),
                    init_node,
                )]),
                NodeFlags::NONE,
            ),
        ));
        f.update_parameter_declaration(
            parameter,
            parameter.modifiers(),
            parameter.dot_dot_dot_token(),
            f.new_generated_name_for_node(parameter),
            parameter.question_token(),
            parameter.type_(),
            Node::NIL,
        )
    }

    // Go: printer/emitcontext.go:871 addDefaultValueAssignmentForInitializer
    fn add_default_value_assignment_for_initializer(
        &self,
        parameter: Node,
        name: Node,
        initializer: Node,
    ) -> Node {
        let f = self.factory();
        self.add_emit_flags(
            initializer,
            EmitFlags::NO_SOURCE_MAP | EmitFlags::NO_COMMENTS,
        );
        let name_clone = f.clone_node(name);
        self.add_emit_flags(name_clone, EmitFlags::NO_SOURCE_MAP);
        let init_assignment = f.new_assignment_expression(name_clone, initializer);
        set_node_loc(init_assignment, parameter.loc());
        self.add_emit_flags(init_assignment, EmitFlags::NO_COMMENTS);
        let init_block = f.new_block(
            f.new_node_list(&[f.new_expression_statement(init_assignment)]),
            false,
        );
        set_node_loc(init_block, parameter.loc());
        self.add_emit_flags(
            init_block,
            EmitFlags::SINGLE_LINE
                | EmitFlags::NO_TRAILING_SOURCE_MAP
                | EmitFlags::NO_TOKEN_SOURCE_MAPS
                | EmitFlags::NO_COMMENTS,
        );
        self.add_initialization_statement(f.new_if_statement(
            f.new_type_check(f.clone_node(name), "undefined"),
            init_block,
            Node::NIL,
        ));
        f.update_parameter_declaration(
            parameter,
            parameter.modifiers(),
            parameter.dot_dot_dot_token(),
            parameter.name(),
            parameter.question_token(),
            parameter.type_(),
            Node::NIL,
        )
    }

    // Go: printer/emitcontext.go:900 AddInitializationStatement
    pub fn add_initialization_statement(&self, node: Node) {
        // PORT: Go `Peek` panics on an empty stack before the nil check.
        let scope = self
            .var_scope_stack
            .borrow()
            .last()
            .cloned()
            .expect("stack is empty");
        self.add_emit_flags(node, EmitFlags::CUSTOM_PROLOGUE);
        scope.borrow_mut().initialization_statements.push(node);
    }

    // Go: printer/emitcontext.go:909 ConvertToFunctionBlock
    pub fn convert_to_function_block(&self, node: Node, multi_line: bool) -> Node {
        if is_block(node) {
            return node;
        }
        let f = self.factory();
        let return_statement = f.new_return_statement(node);
        set_node_loc(return_statement, node.loc());
        // PORT: Go sets `statements.Loc` after `NewNodeList`. A list `Loc` is
        // fixed when the list is made, so it is made with the loc.
        let statements = f.new_node_list_with_loc(&[return_statement], node.loc());
        let block = f.new_block(statements, multi_line);
        set_node_loc(block, node.loc());
        block
    }

    // Go: printer/emitcontext.go:922 VisitFunctionBody
    pub fn visit_function_body<C>(&self, node: Node, visitor: &mut NodeVisitor<'_, C>) -> Node {
        // !!! c.resumeVariableEnvironment()
        let updated = visitor.visit_node(node);
        let declarations = self.end_variable_environment();
        if declarations.is_empty() {
            return updated;
        }

        let f = self.factory();
        if updated.is_nil() {
            return f.new_block(f.new_node_list(&declarations), true /*multiLine*/);
        }

        if !is_block(updated) {
            self.add_emit_flags(updated, EmitFlags::NO_COMMENTS);
            let block = self.convert_to_function_block(updated, false /*multiLine*/);
            return f.update_block(
                block,
                self.merge_environment_list(block.statement_list(), &declarations),
                block.multi_line(),
            );
        }

        f.update_block(
            updated,
            self.merge_environment_list(updated.statement_list(), &declarations),
            updated.multi_line(),
        )
    }

    // Go: printer/emitcontext.go:951 VisitIterationBody
    pub fn visit_iteration_body<C>(&self, body: Node, visitor: &mut NodeVisitor<'_, C>) -> Node {
        if body.is_nil() {
            return Node::NIL;
        }

        self.start_lexical_environment();
        let updated = self.visit_embedded_statement(body, visitor);
        if updated.is_nil() {
            panic!("Expected visitor to return a statement.");
        }

        let mut statements = self.end_lexical_environment();
        if !statements.is_empty() {
            let f = self.factory();
            if is_block(updated) {
                statements.extend(updated.statements().iter());
                let statements_list =
                    new_synthetic_node_list(&statements, updated.statement_list().loc());
                return f.update_block(updated, statements_list, updated.multi_line());
            }
            statements.push(updated);
            return f.new_block(f.new_node_list(&statements), true /*multiLine*/);
        }

        updated
    }

    // Go: printer/emitcontext.go:977 VisitEmbeddedStatement
    pub fn visit_embedded_statement<C>(
        &self,
        node: Node,
        visitor: &mut NodeVisitor<'_, C>,
    ) -> Node {
        if node.is_nil() {
            return Node::NIL;
        }
        let embedded_statement = visitor.visit_embedded_statement(node);
        if embedded_statement.is_nil() || is_not_emitted_statement(embedded_statement) {
            let empty_statement = visitor.factory().new_empty_statement();
            set_node_loc(empty_statement, node.loc());
            self.set_original(empty_statement, node);
            self.assign_comment_range(empty_statement, node);
            return empty_statement;
        }
        embedded_statement
    }

    // Go: printer/emitcontext.go:992 SetSyntheticLeadingComments
    pub fn set_synthetic_leading_comments(
        &self,
        node: Node,
        comments: Vec<SynthesizedComment>,
    ) -> Node {
        self.emit_nodes.borrow_mut().get(node).leading_comments = comments;
        node
    }

    // Go: printer/emitcontext.go:997 AddSyntheticLeadingComment
    pub fn add_synthetic_leading_comment(
        &self,
        node: Node,
        kind: SyntaxKind,
        text: &str,
        has_trailing_new_line: bool,
    ) -> Node {
        self.emit_nodes
            .borrow_mut()
            .get(node)
            .leading_comments
            .push(SynthesizedComment {
                kind,
                loc: TextRange::new(-1, -1),
                has_leading_new_line: false,
                has_trailing_new_line,
                text: text.to_string(),
            });
        node
    }

    // Go: printer/emitcontext.go:1002 GetSyntheticLeadingComments
    #[must_use]
    pub fn get_synthetic_leading_comments(&self, node: Node) -> Vec<SynthesizedComment> {
        if let Some(emit_node) = self.emit_nodes.borrow().try_get(node) {
            return emit_node.leading_comments.clone();
        }
        Vec::new()
    }

    // Go: printer/emitcontext.go:1009 SetSyntheticTrailingComments
    pub fn set_synthetic_trailing_comments(
        &self,
        node: Node,
        comments: Vec<SynthesizedComment>,
    ) -> Node {
        self.emit_nodes.borrow_mut().get(node).trailing_comments = comments;
        node
    }

    // Go: printer/emitcontext.go:1014 AddSyntheticTrailingComment
    pub fn add_synthetic_trailing_comment(
        &self,
        node: Node,
        kind: SyntaxKind,
        text: &str,
        has_trailing_new_line: bool,
    ) -> Node {
        self.emit_nodes
            .borrow_mut()
            .get(node)
            .trailing_comments
            .push(SynthesizedComment {
                kind,
                loc: TextRange::new(-1, -1),
                has_leading_new_line: false,
                has_trailing_new_line,
                text: text.to_string(),
            });
        node
    }

    // Go: printer/emitcontext.go:1019 GetSyntheticTrailingComments
    #[must_use]
    pub fn get_synthetic_trailing_comments(&self, node: Node) -> Vec<SynthesizedComment> {
        if let Some(emit_node) = self.emit_nodes.borrow().try_get(node) {
            return emit_node.trailing_comments.clone();
        }
        Vec::new()
    }

    // Go: printer/emitcontext.go:1028 SetTypeNode
    // SetTypeNode stores the original type node on a name node when the type is erased,
    // so the emitter can use the type's position for comment preservation.
    pub fn set_type_node(&self, node: Node, type_node: Node) {
        self.emit_nodes.borrow_mut().get(node).type_node = type_node;
        self.emit_node_refs.set(true);
    }

    // Go: printer/emitcontext.go:1033 GetTypeNode
    // GetTypeNode gets the type node stored on a name node by the type eraser.
    #[must_use]
    pub fn get_type_node(&self, node: Node) -> Node {
        if let Some(emit_node) = self.emit_nodes.borrow().try_get(node) {
            return emit_node.type_node;
        }
        Node::NIL
    }

    // Go: printer/emitcontext.go:1040 NewNotEmittedStatement
    pub fn new_not_emitted_statement(&self, node: Node) -> Node {
        let statement = self.factory().new_not_emitted_statement();
        set_node_loc(statement, node.loc());
        self.set_original(statement, node);
        self.assign_comment_range(statement, node);
        statement
    }
}

// ──────────────────────────────────────────────────────────────────────
// Node-keyed side tables
// ──────────────────────────────────────────────────────────────────────

/// Keys per page of a `SyntheticPages` table.
const NODE_PAGE: usize = 64;

/// The most pages in the window of a `SyntheticPages` table: 2M handles,
/// so the page list is at most 256 KiB.
const NODE_WINDOW_PAGES: usize = 1 << 15;

/// `SyntheticPages::first` of a table with no window yet. A handle page
/// minus it (wrapping) is never in the window.
const NO_WINDOW: usize = usize::MAX / 2;

/// The records of an `EmitContext` table whose keys are synthetic (factory)
/// nodes, in pages by handle number. A synthetic handle is the slot number
/// of its node in the arena of its thread, and the nodes that a context
/// sees are mostly the ones its transforms make, which have near numbers.
/// The window has `NODE_WINDOW_PAGES` pages from the page of the first key
/// written. A key outside it is not here: the owner table keeps it in its
/// other store, with the parsed keys.
// PERF: emitast2. A read is a few loads, not a hash lookup: emit flags,
// comment ranges and original links are read for each node that a
// transform makes or the printer prints.
#[derive(Clone)]
struct SyntheticPages<V> {
    /// The handle page of `pages[0]`, or `NO_WINDOW`.
    first: usize,
    pages: Vec<Option<Box<[Option<V>; NODE_PAGE]>>>,
}

impl<V> Default for SyntheticPages<V> {
    fn default() -> Self {
        Self {
            first: NO_WINDOW,
            pages: Vec::new(),
        }
    }
}

impl<V> SyntheticPages<V> {
    /// The page index and cell of synthetic key `n` in the window, or
    /// `None` for a parsed key and a key outside the window.
    #[inline(always)]
    fn position(&self, n: Node) -> Option<(usize, usize)> {
        if n.file_index() != crate::ast::synthetic::SYNTHETIC_NODE_FILE {
            return None;
        }
        let handle = (n.0 & 0xffff_ffff) as usize;
        let page = (handle / NODE_PAGE).wrapping_sub(self.first);
        (page < NODE_WINDOW_PAGES).then_some((page, handle % NODE_PAGE))
    }

    /// The value of key `n`: `Some` for a synthetic key in the window (with
    /// its value or `None`), `None` for any other key.
    #[inline(always)]
    fn read(&self, n: Node) -> Option<Option<&V>> {
        let (page, cell) = self.position(n)?;
        Some(
            self.pages
                .get(page)
                .and_then(|p| p.as_deref())
                .and_then(|p| p[cell].as_ref()),
        )
    }

    /// The cell of key `n` to write, or `None` for a key that is not here
    /// (see `read`). The first synthetic key written starts the window.
    #[inline(always)]
    fn cell_mut(&mut self, n: Node) -> Option<&mut Option<V>> {
        if self.first == NO_WINDOW && n.file_index() == crate::ast::synthetic::SYNTHETIC_NODE_FILE {
            self.first = (n.0 & 0xffff_ffff) as usize / NODE_PAGE;
        }
        let (page, cell) = self.position(n)?;
        if page >= self.pages.len() || self.pages[page].is_none() {
            self.add_page(page);
        }
        let p = self.pages[page].as_deref_mut().expect("page was added");
        Some(&mut p[cell])
    }

    #[cold]
    #[inline(never)]
    fn add_page(&mut self, page: usize) {
        if page >= self.pages.len() {
            self.pages.resize_with(page + 1, || None);
        }
        self.pages[page].get_or_insert_with(|| Box::new(std::array::from_fn(|_| None)));
    }

    fn values(&self) -> impl Iterator<Item = &V> {
        self.pages.iter().flatten().flat_map(|p| p.iter().flatten())
    }
}

/// Go `EmitContext.original`: the original node of each node that has one.
// PORT: Go `map[*ast.Node]*ast.Node`; see `SyntheticPages`.
#[derive(Clone, Default)]
pub(crate) struct Originals {
    synthetic: SyntheticPages<Node>,
    /// Parsed keys, and synthetic keys outside the window.
    other: FxHashMap<Node, Node>,
}

impl Originals {
    /// The original of `node`, or nil.
    #[inline]
    fn get(&self, node: Node) -> Node {
        match self.synthetic.read(node) {
            Some(original) => original.copied().unwrap_or(Node::NIL),
            None => self.other.get(&node).copied().unwrap_or(Node::NIL),
        }
    }

    /// The map write of Go `SetOriginalEx`: sets the original of `node` when
    /// it has none and returns true; else panics on a different original
    /// unless `allow_overwrite`, which replaces it.
    #[inline]
    fn set(&mut self, node: Node, original: Node, allow_overwrite: bool) -> bool {
        let cell = match self.synthetic.cell_mut(node) {
            Some(cell) => cell,
            None => {
                return match self.other.entry(node) {
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        entry.insert(original);
                        true
                    }
                    std::collections::hash_map::Entry::Occupied(mut entry) => {
                        Self::check_overwrite(*entry.get(), original, allow_overwrite);
                        if allow_overwrite {
                            entry.insert(original);
                        }
                        false
                    }
                };
            }
        };
        match cell {
            None => {
                *cell = Some(original);
                true
            }
            Some(old) => {
                Self::check_overwrite(*old, original, allow_overwrite);
                if allow_overwrite {
                    *old = original;
                }
                false
            }
        }
    }

    fn check_overwrite(old: Node, original: Node, allow_overwrite: bool) {
        if !allow_overwrite && old != original {
            panic!("Original node already set.");
        }
    }

    fn remove(&mut self, node: Node) {
        match self.synthetic.position(node) {
            Some((page, cell)) => {
                if let Some(Some(p)) = self.synthetic.pages.get_mut(page) {
                    p[cell] = None;
                }
            }
            None => {
                self.other.remove(&node);
            }
        }
    }

    fn clear(&mut self) {
        self.synthetic = SyntheticPages::default();
        self.other.clear();
    }

    fn values(&self) -> impl Iterator<Item = Node> {
        self.synthetic.values().chain(self.other.values()).copied()
    }
}

/// Go `EmitContext.emitNodes`: the emit node of each node that has one.
/// Parsed keys keep their records in the pages of a `LinkStore`.
// PORT: Go `core.LinkStore[*ast.Node, emitNode]`; see `SyntheticPages`.
#[derive(Clone, Default)]
pub(crate) struct EmitNodes {
    synthetic: SyntheticPages<Box<EmitNode>>,
    /// Parsed keys, and synthetic keys outside the window.
    other: LinkStore<Node, Box<EmitNode>>,
    /// PORT: not in Go. The keys of `other` that are not synthetic handles
    /// (parse-tree nodes), in the order of their first record, for
    /// `EmitContext::export_parse_emit_nodes`.
    parsed_keys: Vec<Node>,
}

impl EmitNodes {
    /// Go `emitNodes.TryGet(node)`.
    #[inline]
    fn try_get(&self, node: Node) -> Option<&Box<EmitNode>> {
        match self.synthetic.read(node) {
            Some(emit_node) => emit_node,
            None => self.other.try_get(node),
        }
    }

    /// Go `emitNodes.Get(node)`: creates the record on first use.
    #[inline]
    fn get(&mut self, node: Node) -> &mut Box<EmitNode> {
        match self.synthetic.cell_mut(node) {
            Some(cell) => cell.get_or_insert_with(Box::default),
            None => {
                if node.file_index() != crate::ast::synthetic::SYNTHETIC_NODE_FILE
                    && !self.other.has(node)
                {
                    self.parsed_keys.push(node);
                }
                self.other.get(node)
            }
        }
    }
}

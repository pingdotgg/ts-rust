//! Port of Go `transformers/estransforms/classfields.go` lines 1 to 1580:
//! the transformer state, its constructor and the visitors up to
//! `visitParenthesizedExpression`. The rest is in `class_fields_p2.rs`.
//!
//! PORT: Go keeps nine `*ast.NodeVisitor` fields. They are built on demand
//! with `TxVisitors::with_visitor` and the Go callback (for example
//! `tx.discardedValueVisitor.VisitNode(n)` is
//! `self.with_visitor(Self::visit_discarded_value, |v| v.visit_node(n))`).
//! Go shares the class lexical environment and private environment records
//! through pointers and mutates them in place; they are `Rc<RefCell<..>>`.

use super::class_this::is_class_this_assignment_block;
use super::contract::{TransformOptions, TransformReferenceResolver, TransformerBox};
use super::named_evaluation::{
    is_class_named_evaluation_helper_block, is_named_evaluation_and, transform_named_evaluation,
};
use super::utilities::{TxVisitors, create_accessor_property_backing_field, impl_es_transformer};
use crate::prelude::*;
use crate::printer::factory::PrivateIdentifierKind;
use crate::printer::{AutoGenerateOptions, EmitContext, EmitFlags, GeneratedIdentifierFlags};
use crate::transformers::modifier_visitor::extract_modifiers;
use crate::transformers::utilities::{
    get_non_assignment_operator_for_compound_assignment, is_simple_inlineable_expression,
};

// Go: transformers/estransforms/classfields.go:18 classFacts
/// classFacts tracks various facts about a class being transformed.
pub(super) type ClassFacts = u32;

pub(super) const CLASS_FACTS_NONE: ClassFacts = 0;
pub(super) const CLASS_FACTS_CLASS_WAS_DECORATED: ClassFacts = 1 << 0;
pub(super) const CLASS_FACTS_NEEDS_CLASS_CONSTRUCTOR_REFERENCE: ClassFacts = 1 << 1;
pub(super) const CLASS_FACTS_NEEDS_CLASS_SUPER_REFERENCE: ClassFacts = 1 << 2;
pub(super) const CLASS_FACTS_NEEDS_SUBSTITUTION_FOR_THIS_IN_CLASS_STATIC_FIELD: ClassFacts = 1 << 3;
pub(super) const CLASS_FACTS_WILL_HOIST_INITIALIZERS_TO_CONSTRUCTOR: ClassFacts = 1 << 4;

/// Go `ast.SubtreeContainsLexicalThisOrSuper`.
pub(super) const SUBTREE_CONTAINS_LEXICAL_THIS_OR_SUPER: SubtreeFacts =
    SubtreeFacts::SUBTREE_CONTAINS_LEXICAL_THIS.union(SubtreeFacts::SUBTREE_CONTAINS_LEXICAL_SUPER);

// Go: transformers/estransforms/classfields.go:31 privateIdentifierInfo
/// privateIdentifierInfo stores information about a private identifier during transformation.
#[derive(Clone, Debug)]
pub(super) struct PrivateIdentifierInfo {
    pub(super) kind: PrivateIdentifierKind,
    /// brandCheckIdentifier can contain:
    ///  - For instance field: The WeakMap that will be the storage for the field.
    ///  - For instance methods or accessors: The WeakSet that will be used for brand checking.
    ///  - For static members: The constructor that will be used for brand checking.
    pub(super) brand_check_identifier: Node,
    /// isStatic stores if the identifier is static or not.
    pub(super) is_static: bool,
    /// isValid stores if the identifier declaration is valid or not. Reserved names (e.g. #constructor)
    /// or duplicate identifiers are considered invalid.
    pub(super) is_valid: bool,
    /// variableName contains the variable that will serve as the storage for a static field.
    pub(super) variable_name: Node,
    /// methodName is the identifier for a variable that will contain the private method implementation.
    pub(super) method_name: Node,
    /// getterName is the identifier for a variable that will contain the private get accessor implementation, if any.
    pub(super) getter_name: Node,
    /// setterName is the identifier for a variable that will contain the private set accessor implementation, if any.
    pub(super) setter_name: Node,
}

impl PrivateIdentifierInfo {
    /// Go `&privateIdentifierInfo{kind: kind}` (all other fields zero).
    pub(super) fn new(kind: PrivateIdentifierKind) -> Self {
        Self {
            kind,
            brand_check_identifier: Node::NIL,
            is_static: false,
            is_valid: false,
            variable_name: Node::NIL,
            method_name: Node::NIL,
            getter_name: Node::NIL,
            setter_name: Node::NIL,
        }
    }
}

/// Go `*privateIdentifierInfo`.
pub(super) type PrivateIdentifierInfoRef = Rc<RefCell<PrivateIdentifierInfo>>;

// Go: transformers/estransforms/classfields.go:54 privateEnvironmentData
/// privateEnvironmentData stores class-scoped environment data for private identifiers.
#[derive(Clone, Copy, Default)]
pub(super) struct PrivateEnvironmentData {
    /// className is used for prefixing generated variable names.
    pub(super) class_name: Node,
    /// weakSetName is used for brand check on private methods.
    pub(super) weak_set_name: Node,
}

// Go: transformers/estransforms/classfields.go:65 privateEnvironment
/// privateEnvironment stores a map of private identifier names to their transform info.
/// Like Strada, it uses two separate maps: one for non-generated identifiers (keyed by text)
/// and one for generated identifiers (keyed by original AST node). This prevents collisions
/// when different auto-accessors produce generated backing field names with the same text.
#[derive(Default)]
pub(super) struct PrivateEnvironment {
    pub(super) data: PrivateEnvironmentData,
    pub(super) members: FxHashMap<String, PrivateIdentifierInfoRef>,
    pub(super) generated_identifiers: FxHashMap<Node, PrivateIdentifierInfoRef>,
}

/// Go `*privateEnvironment`.
pub(super) type PrivateEnvironmentRef = Rc<RefCell<PrivateEnvironment>>;

// Go: transformers/estransforms/classfields.go:72 classLexicalEnvironment
/// classLexicalEnvironment stores information about the lexical environment of a class.
#[derive(Clone, Copy, Default)]
pub(super) struct ClassLexicalEnvironment {
    pub(super) facts: ClassFacts,
    /// classConstructor is used for brand checks on static members, and `this` references in static initializers.
    pub(super) class_constructor: Node,
    pub(super) class_this: Node,
    /// superClassReference is used for `super` references in static initializers.
    pub(super) super_class_reference: Node,
}

/// Go `*classLexicalEnvironment`.
pub(super) type ClassLexicalEnvironmentRef = Rc<RefCell<ClassLexicalEnvironment>>;

// Go: transformers/estransforms/classfields.go:82 classLexicalEnv
/// classLexicalEnv is a linked list of class lexical environments.
#[derive(Default)]
pub(super) struct ClassLexicalEnv {
    pub(super) previous: Option<Rc<ClassLexicalEnv>>,
    pub(super) data: RefCell<Option<ClassLexicalEnvironmentRef>>,
    pub(super) private_env: RefCell<Option<PrivateEnvironmentRef>>,
}

impl ClassLexicalEnv {
    /// Go `env.data` (nil is `None`).
    pub(super) fn data(&self) -> Option<ClassLexicalEnvironmentRef> {
        self.data.borrow().clone()
    }

    /// Go `env.privateEnv` (nil is `None`).
    pub(super) fn private_env(&self) -> Option<PrivateEnvironmentRef> {
        self.private_env.borrow().clone()
    }
}

// Go: transformers/estransforms/classfields.go:88 classFieldsTransformer
pub struct ClassFieldsTransformer {
    pub(super) emit_context: Rc<EmitContext>,
    pub(super) compiler_options: &'static CompilerOptions,
    pub(super) resolver: Rc<dyn TransformReferenceResolver>,

    // Computed configuration flags
    pub(super) should_transform_initializers_using_set: bool,
    pub(super) should_transform_initializers_using_define: bool,
    pub(super) should_transform_initializers: bool,
    pub(super) should_transform_private_elements_or_class_static_blocks: bool,
    pub(super) should_transform_auto_accessors: bool,
    pub(super) should_transform_this_in_static_initializers: bool,
    pub(super) should_transform_super_in_static_initializers: bool,
    pub(super) should_transform_private_static_elements_in_file: bool,
    pub(super) legacy_decorators: bool,

    /// pendingExpressions tracks what computed name expressions originating from elided names
    /// must be inlined at the next execution site, in document order.
    pub(super) pending_expressions: Vec<Node>,
    /// pendingStatements tracks what computed name expression statements and static property
    /// initializers must be emitted at the next execution site, in document order (for decorated classes).
    pub(super) pending_statements: Vec<Node>,
    pub(super) lexical_environment: Option<Rc<ClassLexicalEnv>>,
    pub(super) current_class_container: Node,
    pub(super) current_class_element: Node,
    /// classAliases maps class declarations to alias identifiers for substituting class name
    /// references in static initializers. Replaces Strada's onSubstituteNode/trySubstituteClassAlias.
    pub(super) class_aliases: FxHashMap<Node, Node>,
    pub(super) enclosing_class_declarations: FxHashSet<Node>,
    pub(super) in_iteration_statement: bool,
    /// insideComputedPropertyName replaces Strada's onEmitNode for ComputedPropertyName, which
    /// switches to the outer lexical environment. Used by visitThisExpression() to apply
    /// the outer environment's substitution without requiring currentClassElement to be static.
    pub(super) inside_computed_property_name: bool,
    pub(super) parent_node: Node,
    pub(super) current_node: Node,
}

impl_es_transformer!(ClassFieldsTransformer);

// Go: transformers/estransforms/classfields.go:140 newClassFieldsTransformer
pub fn new_class_fields_transformer(opts: &TransformOptions) -> Option<TransformerBox> {
    let language_version = opts.compiler_options.get_emit_script_target();
    let use_define_for_class_fields = opts.compiler_options.get_use_define_for_class_fields();

    // When targeting ESNext+ with useDefineForClassFields (the default), there are no class
    // field transformations to perform and no prior transform sets EFTransformPrivateStaticElements,
    // so every node would be returned unchanged. Skip entirely.
    if language_version >= ScriptTarget::ES_NEXT && use_define_for_class_fields {
        return None;
    }

    // Always transform field initializers using Set semantics when `useDefineForClassFields: false`.
    let should_transform_initializers_using_set = !use_define_for_class_fields;

    // Transform field initializers using Define semantics when `useDefineForClassFields: true` and target < ES2022.
    let should_transform_initializers_using_define =
        use_define_for_class_fields && language_version < ScriptTarget::ES2022;

    let should_transform_initializers =
        should_transform_initializers_using_set || should_transform_initializers_using_define;

    // We need to transform private members and class static blocks when target < ES2022.
    let should_transform_private_elements_or_class_static_blocks =
        language_version < ScriptTarget::ES2022;

    // We need to transform `accessor` fields when target < ESNext.
    // We may need to transform `accessor` fields when `useDefineForClassFields: false`
    let should_transform_auto_accessors = language_version < ScriptTarget::ES_NEXT;

    // We need to transform `this` in a static initializer into a reference to the class
    // when target < ES2022 since the assignment will be moved outside of the class body.
    let should_transform_this_in_static_initializers = language_version < ScriptTarget::ES2022;

    // Since target is always >= ES2015, this is always the same as
    // shouldTransformThisInStaticInitializers.
    let should_transform_super_in_static_initializers =
        should_transform_this_in_static_initializers;

    Some(Box::new(ClassFieldsTransformer {
        emit_context: opts.context.clone(),
        compiler_options: opts.compiler_options,
        resolver: opts.resolver.clone(),
        should_transform_initializers_using_set,
        should_transform_initializers_using_define,
        should_transform_initializers,
        should_transform_private_elements_or_class_static_blocks,
        should_transform_auto_accessors,
        should_transform_this_in_static_initializers,
        should_transform_super_in_static_initializers,
        should_transform_private_static_elements_in_file: false,
        legacy_decorators: opts.compiler_options.experimental_decorators.is_true(),
        pending_expressions: Vec::new(),
        pending_statements: Vec::new(),
        lexical_environment: None,
        current_class_container: Node::NIL,
        current_class_element: Node::NIL,
        class_aliases: FxHashMap::default(),
        enclosing_class_declarations: FxHashSet::default(),
        in_iteration_statement: false,
        inside_computed_property_name: false,
        parent_node: Node::NIL,
        current_node: Node::NIL,
    }))
}

impl ClassFieldsTransformer {
    /// Go `tx.lexicalEnvironment != nil && tx.lexicalEnvironment.data != nil`, returning a copy of `data`.
    pub(super) fn lexical_environment_data(&self) -> Option<ClassLexicalEnvironment> {
        self.lexical_environment
            .as_ref()
            .and_then(|env| env.data())
            .map(|data| *data.borrow())
    }

    /// Go `isNamedEvaluationAnd(tx.EmitContext(), node, tx.isAnonymousClassNeedingAssignedName)`.
    pub(super) fn is_named_evaluation_needing_assigned_name(&self, node: Node) -> bool {
        let ec = self.ec();
        is_named_evaluation_and(
            &ec,
            node,
            Some(&mut |n: Node| self.is_anonymous_class_needing_assigned_name_worker(n)),
        )
    }

    // Go: transformers/estransforms/classfields.go:199 classFieldsTransformer.requiresBlockScopedVar
    /// requiresBlockScopedVar returns true when private field temp variables should be
    /// declared as block-scoped (let) rather than function-scoped (var). This occurs when
    /// a class expression is directly inside a loop body.
    /// Replaces Strada's resolver.hasNodeCheckFlag(node, NodeCheckFlags.BlockScopedBindingInLoop).
    pub(super) fn requires_block_scoped_var(&self) -> bool {
        self.in_iteration_statement
            && self.current_class_container.is_some()
            && is_class_expression(self.current_class_container)
    }

    // Go: transformers/estransforms/classfields.go:207 classFieldsTransformer.classExpressionNeedsBlockScopedTemp
    /// classExpressionNeedsBlockScopedTemp returns true when the class expression's temp variable
    /// must be block-scoped. This is more specific than requiresBlockScopedVar: the class temp only
    /// needs to be block-scoped when the class expression has a non-static property with a computed
    /// property name inside a loop (matching the checker's BlockScopedBindingInLoop on the class node).
    pub(super) fn class_expression_needs_block_scoped_temp(&self) -> bool {
        if !self.requires_block_scoped_var() {
            return false;
        }
        for member in self.current_class_container.members().iter() {
            if is_property_declaration(member)
                && !has_static_modifier(member)
                && member.name().is_some()
                && is_computed_property_name(member.name())
            {
                return true;
            }
        }
        false
    }

    // Go: transformers/estransforms/classfields.go:220 classFieldsTransformer.visitSourceFile
    pub(super) fn visit_source_file(&mut self, node: Node) -> Node {
        if super::utilities::source_file_is_declaration_file(node) {
            return node;
        }
        let ec = self.ec();
        self.lexical_environment = None;
        self.should_transform_private_static_elements_in_file = ec
            .emit_flags(node)
            .intersects(EmitFlags::TRANSFORM_PRIVATE_STATIC_ELEMENTS);
        self.class_aliases = FxHashMap::default();
        self.enclosing_class_declarations.clear();
        let visited = self.visit_each_child(node);
        ec.add_emit_helper(visited, &ec.read_emit_helpers());
        self.class_aliases = FxHashMap::default();
        self.enclosing_class_declarations.clear();
        visited
    }

    // Go: transformers/estransforms/classfields.go:235 classFieldsTransformer.visitModifier
    pub(super) fn visit_modifier(&mut self, node: Node) -> Node {
        if node.kind() == SyntaxKind::AccessorKeyword {
            if self.should_transform_auto_accessors_in_current_class() {
                return Node::NIL;
            }
            return node;
        }
        if is_modifier(node) {
            return node;
        }
        Node::NIL
    }

    /// Go `tx.modifierVisitor.VisitModifiers(modifiers)`.
    pub(super) fn modifier_visitor_visit_modifiers(
        &mut self,
        modifiers: ModifierList,
    ) -> ModifierList {
        self.with_visitor(Self::visit_modifier, |v| v.visit_modifiers(modifiers))
    }

    /// Go `tx.discardedValueVisitor.VisitNode(node)`.
    pub(super) fn discarded_value_visitor_visit_node(&mut self, node: Node) -> Node {
        self.with_visitor(Self::visit_discarded_value, |v| v.visit_node(node))
    }

    /// Go `tx.classElementVisitor.VisitEachChild(node)`.
    pub(super) fn class_element_visitor_visit_each_child(&mut self, node: Node) -> Node {
        self.with_visitor(Self::visit_class_element, |v| v.visit_each_child(node))
    }

    /// Go `tx.heritageClauseVisitor.VisitEachChild(node)`.
    pub(super) fn heritage_clause_visitor_visit_each_child(&mut self, node: Node) -> Node {
        self.with_visitor(Self::visit_heritage_clause, |v| v.visit_each_child(node))
    }

    // Go: transformers/estransforms/classfields.go:248 classFieldsTransformer.pushNode
    pub(super) fn push_node(&mut self, node: Node) -> Node {
        let grandparent_node = self.parent_node;
        self.parent_node = self.current_node;
        self.current_node = node;
        grandparent_node
    }

    // Go: transformers/estransforms/classfields.go:255 classFieldsTransformer.popNode
    pub(super) fn pop_node(&mut self, grandparent_node: Node) {
        self.current_node = self.parent_node;
        self.parent_node = grandparent_node;
    }

    // Go: transformers/estransforms/classfields.go:265 classFieldsTransformer.visitForSubstitution
    /// visitForSubstitution visits nodes solely for class alias substitution in subtrees
    /// that don't contain class field or lexical this/super transforms. It substitutes
    /// identifiers that reference class declarations with their aliases, while skipping
    /// the .Name() of PropertyAccessExpressions since Strada's onSubstituteNode only
    /// fires for EmitHint.Expression, which excludes property access names.
    pub(super) fn visit_for_substitution(&mut self, node: Node) -> Node {
        if node.kind() == SyntaxKind::Identifier {
            return self.visit_identifier(node);
        }
        if node.kind() == SyntaxKind::PropertyAccessExpression && is_identifier(node.name()) {
            return self.visit_property_access_expression_for_substitution(node);
        }
        self.with_visitor(Self::visit_for_substitution, |v| v.visit_each_child(node))
    }

    // Go: transformers/estransforms/classfields.go:276 classFieldsTransformer.visit
    /// visit is the main visitor.
    pub(super) fn visit(&mut self, node: Node) -> Node {
        let grandparent_node = self.push_node(node);
        let result = self.visit_worker(node);
        self.pop_node(grandparent_node);
        result
    }

    /// The body of Go `visit` between `pushNode` and the deferred `popNode`.
    pub(super) fn visit_worker(&mut self, node: Node) -> Node {
        if !node.subtree_facts().intersects(
            SubtreeFacts::SUBTREE_CONTAINS_CLASS_FIELDS | SUBTREE_CONTAINS_LEXICAL_THIS_OR_SUPER,
        ) {
            if self.current_class_container.is_some() && !self.class_aliases.is_empty() {
                // Continue visiting for alias substitution even in non-class-field subtrees.
                return self.visit_for_substitution(node);
            }
            return node;
        }

        match node.kind() {
            SyntaxKind::SourceFile => self.visit_source_file(node),
            SyntaxKind::ClassDeclaration => self.visit_class_declaration(node),
            SyntaxKind::ClassExpression => self.visit_class_expression(node),
            SyntaxKind::ClassStaticBlockDeclaration | SyntaxKind::PropertyDeclaration => {
                panic!("Use `classElementVisitor` instead.")
            }
            SyntaxKind::PropertyAssignment => self.visit_property_assignment(node),
            SyntaxKind::VariableStatement => self.visit_variable_statement(node),
            SyntaxKind::VariableDeclaration => self.visit_variable_declaration(node),
            SyntaxKind::Parameter => self.visit_parameter_declaration(node),
            SyntaxKind::BindingElement => self.visit_binding_element(node),
            SyntaxKind::ExportAssignment => self.visit_export_assignment(node),
            SyntaxKind::PrivateIdentifier => self.visit_private_identifier(node),
            SyntaxKind::PropertyAccessExpression => self.visit_property_access_expression(node),
            SyntaxKind::ElementAccessExpression => self.visit_element_access_expression(node),
            SyntaxKind::PrefixUnaryExpression | SyntaxKind::PostfixUnaryExpression => {
                self.visit_pre_or_postfix_unary_expression(node, false /*discarded*/)
            }
            SyntaxKind::BinaryExpression => {
                self.visit_binary_expression(node, false /*discarded*/)
            }
            SyntaxKind::ParenthesizedExpression => {
                self.visit_parenthesized_expression(node, false /*discarded*/)
            }
            SyntaxKind::CallExpression => self.visit_call_expression(node),
            SyntaxKind::ExpressionStatement => self.visit_expression_statement(node),
            SyntaxKind::TaggedTemplateExpression => self.visit_tagged_template_expression(node),
            SyntaxKind::ForStatement => self.visit_for_statement(node),
            SyntaxKind::ForInStatement
            | SyntaxKind::ForOfStatement
            | SyntaxKind::DoStatement
            | SyntaxKind::WhileStatement => {
                self.set_in_iteration_statement_and(true, Self::visit_each_child_of_node, node)
            }
            SyntaxKind::ThisKeyword => self.visit_this_expression(node),
            SyntaxKind::FunctionDeclaration | SyntaxKind::FunctionExpression => self
                .set_in_iteration_statement_and(
                    false,
                    Self::visit_function_expression_or_declaration,
                    node,
                ),
            SyntaxKind::Constructor
            | SyntaxKind::MethodDeclaration
            | SyntaxKind::GetAccessor
            | SyntaxKind::SetAccessor => self.set_in_iteration_statement_and(
                false,
                Self::set_class_element_and_visit_each_child,
                node,
            ),
            _ => self.visit_each_child(node),
        }
    }

    // Go: transformers/estransforms/classfields.go:343 classFieldsTransformer.visitDiscardedValue
    /// visitDiscardedValue visits a node in an expression whose result is discarded.
    pub(super) fn visit_discarded_value(&mut self, node: Node) -> Node {
        match node.kind() {
            SyntaxKind::PrefixUnaryExpression | SyntaxKind::PostfixUnaryExpression => {
                self.visit_pre_or_postfix_unary_expression(node, true /*discarded*/)
            }
            SyntaxKind::BinaryExpression => {
                self.visit_binary_expression(node, true /*discarded*/)
            }
            SyntaxKind::ParenthesizedExpression => {
                self.visit_parenthesized_expression(node, true /*discarded*/)
            }
            _ => self.visit(node),
        }
    }

    // Go: transformers/estransforms/classfields.go:357 classFieldsTransformer.visitHeritageClause
    /// visitHeritageClause visits a node in a HeritageClause.
    pub(super) fn visit_heritage_clause(&mut self, node: Node) -> Node {
        match node.kind() {
            SyntaxKind::HeritageClause => self.heritage_clause_visitor_visit_each_child(node),
            SyntaxKind::ExpressionWithTypeArguments => {
                self.visit_expression_with_type_arguments_in_heritage_clause(node)
            }
            _ => self.visit(node),
        }
    }

    // Go: transformers/estransforms/classfields.go:369 classFieldsTransformer.visitAssignmentTarget
    /// visitAssignmentTarget visits the assignment target of a destructuring assignment.
    pub(super) fn visit_assignment_target(&mut self, node: Node) -> Node {
        match node.kind() {
            SyntaxKind::ObjectLiteralExpression | SyntaxKind::ArrayLiteralExpression => {
                self.visit_assignment_pattern(node)
            }
            _ => self.visit(node),
        }
    }

    // Go: transformers/estransforms/classfields.go:378 classFieldsTransformer.visitDestructuringAssignmentTarget
    pub(super) fn visit_destructuring_assignment_target(&mut self, node: Node) -> Node {
        if is_object_literal_expression(node) || is_array_literal_expression(node) {
            return self.visit_assignment_pattern(node);
        }
        if is_property_access_expression(node) && is_private_identifier(node.name()) {
            return self.wrap_private_identifier_for_destructuring_target(node);
        }
        if self.should_transform_super_in_static_initializers
            && self.current_class_element.is_some()
            && is_super_property(node)
            && is_static_property_declaration_or_class_static_block(self.current_class_element)
        {
            if let Some(data) = self.lexical_environment_data() {
                if data.facts & CLASS_FACTS_CLASS_WAS_DECORATED != 0 {
                    return self.visit_invalid_super_property(node);
                }
                if data.class_constructor.is_some() && data.super_class_reference.is_some() {
                    let mut name = Node::NIL;
                    if is_element_access_expression(node) {
                        name = self.visit_node(node.argument_expression());
                    } else if is_property_access_expression(node) && is_identifier(node.name()) {
                        name = self
                            .ec()
                            .factory()
                            .new_string_literal_from_node(node.name());
                    }
                    if name.is_some() {
                        let ec = self.ec();
                        let f = ec.factory();
                        let temp = f.new_temp_variable();
                        let set_expr = f.new_reflect_set_call(
                            data.super_class_reference,
                            name,
                            temp,
                            data.class_constructor,
                        );
                        return f.new_assignment_target_wrapper(temp, set_expr);
                    }
                }
            }
        }
        self.visit_each_child(node)
    }

    // Go: transformers/estransforms/classfields.go:416 classFieldsTransformer.visitClassElement
    /// visitClassElement visits a member of a class.
    pub(super) fn visit_class_element(&mut self, node: Node) -> Node {
        match node.kind() {
            SyntaxKind::Constructor => {
                self.set_current_class_element_and(node, Self::visit_constructor_declaration, node)
            }
            SyntaxKind::GetAccessor | SyntaxKind::SetAccessor | SyntaxKind::MethodDeclaration => {
                self.set_current_class_element_and(
                    node,
                    Self::visit_method_or_accessor_declaration,
                    node,
                )
            }
            SyntaxKind::PropertyDeclaration => {
                self.set_current_class_element_and(node, Self::visit_property_declaration, node)
            }
            SyntaxKind::ClassStaticBlockDeclaration => self.set_current_class_element_and(
                node,
                Self::visit_class_static_block_declaration,
                node,
            ),
            SyntaxKind::ComputedPropertyName => self.visit_computed_property_name(node),
            SyntaxKind::SemicolonClassElement => node,
            _ => {
                if is_modifier_like(node) {
                    return self.visit_modifier(node);
                }
                self.visit(node)
            }
        }
    }

    // Go: transformers/estransforms/classfields.go:439 classFieldsTransformer.visitPropertyName
    /// visitPropertyName visits a property name of a class member.
    pub(super) fn visit_property_name(&mut self, name: Node) -> Node {
        if is_computed_property_name(name) {
            return self.visit_computed_property_name(name);
        }
        self.visit_node(name)
    }

    // Go: transformers/estransforms/classfields.go:447 classFieldsTransformer.visitAccessorFieldResult
    /// visitAccessorFieldResult visits the results of an auto-accessor field transformation in a second pass.
    pub(super) fn visit_accessor_field_result(&mut self, node: Node) -> Node {
        match node.kind() {
            SyntaxKind::PropertyDeclaration => self.transform_field_initializer(node),
            SyntaxKind::GetAccessor | SyntaxKind::SetAccessor => self.visit_class_element(node),
            _ => crate::gostd::debug::fail_bad_syntax_kind(
                node.kind(),
                Some(
                    "Expected node to either be a PropertyDeclaration, GetAccessorDeclaration, or SetAccessorDeclaration",
                ),
            ),
        }
    }

    // Go: transformers/estransforms/classfields.go:462 classFieldsTransformer.visitIdentifier
    /// visitIdentifier replaces Strada's onSubstituteNode/trySubstituteClassAlias. Instead of
    /// substituting at emit time using NodeCheckFlags.ConstructorReference, we resolve the
    /// identifier to its declaration and check if that declaration has a registered alias.
    pub(super) fn visit_identifier(&mut self, node: Node) -> Node {
        let ec = self.ec();
        let declaration = self
            .resolver
            .get_referenced_value_declaration(ec.most_original(node));
        if declaration.is_some() {
            if let Some(&alias) = self.class_aliases.get(&declaration) {
                if self.enclosing_class_declarations.contains(&declaration) {
                    let clone = ec.factory().clone_node(alias);
                    ec.set_source_map_range(clone, node.loc());
                    ec.set_comment_range(clone, node.loc());
                    return clone;
                }
            }
        }
        node
    }

    // Go: transformers/estransforms/classfields.go:479 classFieldsTransformer.visitPrivateIdentifier
    /// visitPrivateIdentifier handles an undeclared private name. Replace it with an empty
    /// identifier to indicate a problem with the code.
    /// Note: private identifiers in statement position (e.g., `#;`) are intercepted earlier
    /// by visitExpressionStatement, which preserves them so the runtime throws a SyntaxError.
    pub(super) fn visit_private_identifier(&mut self, node: Node) -> Node {
        if !self.should_transform_private_elements_or_class_static_blocks {
            return node;
        }
        if self.parent_node.is_some() && is_statement(self.parent_node) {
            return node;
        }
        let ec = self.ec();
        let result = ec.factory().new_identifier("");
        ec.set_original(result, node);
        result
    }

    // Go: transformers/estransforms/classfields.go:492 classFieldsTransformer.transformPrivateIdentifierInInExpression
    /// transformPrivateIdentifierInInExpression visits `#id in expr`.
    pub(super) fn transform_private_identifier_in_in_expression(&mut self, node: Node) -> Node {
        let info = self.access_private_identifier(node.left());
        if let Some(info) = info {
            let receiver = self.visit_node(node.right());
            let ec = self.ec();
            let brand_check_identifier = info.borrow().brand_check_identifier;
            let result = ec
                .factory()
                .new_class_private_field_in_helper(brand_check_identifier, receiver);
            ec.set_original(result, node);
            return result;
        }
        // Private name has not been declared. Subsequent transformers will handle this error
        self.visit_each_child(node)
    }

    // Go: transformers/estransforms/classfields.go:504 classFieldsTransformer.visitPropertyAssignment
    pub(super) fn visit_property_assignment(&mut self, mut node: Node) -> Node {
        // 13.2.5.5 RS: PropertyDefinitionEvaluation
        //   PropertyAssignment : PropertyName `:` AssignmentExpression
        //     ...
        //     5. If IsAnonymousFunctionDefinition(|AssignmentExpression|) is *true* and _isProtoSetter_ is *false*, then
        //        a. Let _popValue_ be ? NamedEvaluation of |AssignmentExpression| with argument _propKey_.
        //     ...

        if self.is_named_evaluation_needing_assigned_name(node) {
            node = transform_named_evaluation(
                &self.ec(),
                node,
                false, /*ignoreEmptyStringLiteral*/
                "",    /*assignedName*/
            );
        }
        self.visit_each_child(node)
    }

    // Go: transformers/estransforms/classfields.go:518 classFieldsTransformer.visitVariableStatement
    pub(super) fn visit_variable_statement(&mut self, node: Node) -> Node {
        let saved_pending_statements = std::mem::take(&mut self.pending_statements);

        let visited_node = self.visit_each_child(node);

        if !self.pending_statements.is_empty() {
            let mut result: Vec<Node> = Vec::with_capacity(1 + self.pending_statements.len());
            result.push(visited_node);
            result.extend_from_slice(&self.pending_statements);
            self.pending_statements = saved_pending_statements;
            return self.ec().factory().new_syntax_list(&result);
        }

        self.pending_statements = saved_pending_statements;
        visited_node
    }

    // Go: transformers/estransforms/classfields.go:536 classFieldsTransformer.visitVariableDeclaration
    pub(super) fn visit_variable_declaration(&mut self, mut node: Node) -> Node {
        // 14.3.1.2 RS: Evaluation
        //   LexicalBinding : BindingIdentifier Initializer
        //     ...
        //     3. If IsAnonymousFunctionDefinition(|Initializer|) is *true*, then
        //        a. Let _value_ be ? NamedEvaluation of |Initializer| with argument _bindingId_.
        //     ...
        //
        // 14.3.2.1 RS: Evaluation
        //   VariableDeclaration : BindingIdentifier Initializer
        //     ...
        //     3. If IsAnonymousFunctionDefinition(|Initializer|) is *true*, then
        //        a. Let _value_ be ? NamedEvaluation of |Initializer| with argument _bindingId_.
        //     ...

        if self.is_named_evaluation_needing_assigned_name(node) {
            node = transform_named_evaluation(&self.ec(), node, false, "");
        }
        self.visit_each_child(node)
    }

    // Go: transformers/estransforms/classfields.go:557 classFieldsTransformer.visitParameterDeclaration
    pub(super) fn visit_parameter_declaration(&mut self, mut node: Node) -> Node {
        // 8.6.3 RS: IteratorBindingInitialization
        //   SingleNameBinding : BindingIdentifier Initializer?
        //     ...
        //     5. If |Initializer| is present and _v_ is *undefined*, then
        //        a. If IsAnonymousFunctionDefinition(|Initializer|) is *true*, then
        //           i. Set _v_ to ? NamedEvaluation of |Initializer| with argument _bindingId_.
        //     ...
        //
        // 14.3.3.3 RS: KeyedBindingInitialization
        //   SingleNameBinding : BindingIdentifier Initializer?
        //     ...
        //     4. If |Initializer| is present and _v_ is *undefined*, then
        //        a. If IsAnonymousFunctionDefinition(|Initializer|) is *true*, then
        //           i. Set _v_ to ? NamedEvaluation of |Initializer| with argument _bindingId_.
        //     ...

        if self.is_named_evaluation_needing_assigned_name(node) {
            node = transform_named_evaluation(&self.ec(), node, false, "");
        }
        self.visit_each_child(node)
    }

    // Go: transformers/estransforms/classfields.go:580 classFieldsTransformer.visitBindingElement
    pub(super) fn visit_binding_element(&mut self, mut node: Node) -> Node {
        // 8.6.3 RS: IteratorBindingInitialization
        //   SingleNameBinding : BindingIdentifier Initializer?
        //     ...
        //     5. If |Initializer| is present and _v_ is *undefined*, then
        //        a. If IsAnonymousFunctionDefinition(|Initializer|) is *true*, then
        //           i. Set _v_ to ? NamedEvaluation of |Initializer| with argument _bindingId_.
        //     ...
        //
        // 14.3.3.3 RS: KeyedBindingInitialization
        //   SingleNameBinding : BindingIdentifier Initializer?
        //     ...
        //     4. If |Initializer| is present and _v_ is *undefined*, then
        //        a. If IsAnonymousFunctionDefinition(|Initializer|) is *true*, then
        //           i. Set _v_ to ? NamedEvaluation of |Initializer| with argument _bindingId_.
        //     ...

        if self.is_named_evaluation_needing_assigned_name(node) {
            node = transform_named_evaluation(&self.ec(), node, false, "");
        }
        self.visit_each_child(node)
    }

    // Go: transformers/estransforms/classfields.go:603 classFieldsTransformer.visitExportAssignment
    pub(super) fn visit_export_assignment(&mut self, mut node: Node) -> Node {
        // 16.2.3.7 RS: Evaluation
        //   ExportDeclaration : `export` `default` AssignmentExpression `;`
        //     1. If IsAnonymousFunctionDefinition(|AssignmentExpression|) is *true*, then
        //        a. Let _value_ be ? NamedEvaluation of |AssignmentExpression| with argument `"default"`.
        //     ...

        // NOTE: Since emit for `export =` translates to `module.exports = ...`, the assigned name of the class
        // is `""`.

        if self.is_named_evaluation_needing_assigned_name(node) {
            let assigned_name = if !node.is_export_equals() {
                "default"
            } else {
                ""
            };
            node = transform_named_evaluation(
                &self.ec(),
                node,
                true, /*ignoreEmptyStringLiteral*/
                assigned_name,
            );
        }
        self.visit_each_child(node)
    }

    // Go: transformers/estransforms/classfields.go:623 classFieldsTransformer.injectPendingExpressions
    pub(super) fn inject_pending_expressions(&mut self, mut expression: Node) -> Node {
        if !self.pending_expressions.is_empty() {
            let ec = self.ec();
            let f = ec.factory();
            if is_parenthesized_expression(expression) {
                self.pending_expressions.push(expression.expression());
                expression = f.update_parenthesized_expression(
                    expression,
                    f.inline_expressions(&self.pending_expressions),
                );
            } else {
                let mut exprs = self.pending_expressions.clone();
                exprs.push(expression);
                expression = f.inline_expressions(&exprs);
            }
            self.pending_expressions = Vec::new();
        }
        expression
    }

    // Go: transformers/estransforms/classfields.go:640 classFieldsTransformer.visitComputedPropertyName
    pub(super) fn visit_computed_property_name(&mut self, node: Node) -> Node {
        // Computed property names are evaluated in the enclosing scope, not the current class.
        // Replaces Strada's onEmitNode for ComputedPropertyName which switches to
        // lexicalEnvironment?.previous. We do this explicitly during transformation.
        let saved_lexical_environment = self.lexical_environment.clone();
        let saved_inside_computed_property_name = self.inside_computed_property_name;
        self.inside_computed_property_name = true;
        if let Some(previous) = self
            .lexical_environment
            .as_ref()
            .and_then(|env| env.previous.clone())
        {
            self.lexical_environment = Some(previous);
        }
        let expression = self.visit_node(node.expression());
        self.lexical_environment = saved_lexical_environment;
        self.inside_computed_property_name = saved_inside_computed_property_name;
        let injected = self.inject_pending_expressions(expression);
        self.ec()
            .factory()
            .update_computed_property_name(node, injected)
    }

    // Go: transformers/estransforms/classfields.go:656 classFieldsTransformer.visitConstructorDeclaration
    pub(super) fn visit_constructor_declaration(&mut self, node: Node) -> Node {
        if self.current_class_container.is_some() {
            return self.transform_constructor(node, self.current_class_container);
        }
        self.visit_each_child(node)
    }

    // Go: transformers/estransforms/classfields.go:663 classFieldsTransformer.shouldTransformClassElementToWeakMap
    pub(super) fn should_transform_class_element_to_weak_map(&self, node: Node) -> bool {
        if self.should_transform_private_elements_or_class_static_blocks {
            return true;
        }
        self.should_always_transform_private_static_elements(node)
    }

    // Go: transformers/estransforms/classfields.go:670 classFieldsTransformer.shouldAlwaysTransformPrivateStaticElements
    pub(super) fn should_always_transform_private_static_elements(&self, node: Node) -> bool {
        has_static_modifier(node)
            && self
                .emit_context
                .emit_flags(node)
                .intersects(EmitFlags::TRANSFORM_PRIVATE_STATIC_ELEMENTS)
    }

    // Go: transformers/estransforms/classfields.go:677 classFieldsTransformer.nodeHasTransformPrivateStaticElementsFlag
    /// nodeHasTransformPrivateStaticElementsFlag checks the emit flag on a class node (not a member).
    /// Unlike shouldAlwaysTransformPrivateStaticElements, this does not check HasStaticModifier,
    /// since class nodes themselves don't have a static modifier.
    pub(super) fn node_has_transform_private_static_elements_flag(&self, node: Node) -> bool {
        self.emit_context
            .emit_flags(node)
            .intersects(EmitFlags::TRANSFORM_PRIVATE_STATIC_ELEMENTS)
    }

    // Go: transformers/estransforms/classfields.go:681 classFieldsTransformer.visitMethodOrAccessorDeclaration
    pub(super) fn visit_method_or_accessor_declaration(&mut self, node: Node) -> Node {
        go_assert!(!has_decorators(node));

        if !is_private_identifier_class_element_declaration(node)
            || !self.should_transform_class_element_to_weak_map(node)
        {
            return self.class_element_visitor_visit_each_child(node);
        }

        // leave invalid code untransformed
        let env = self.get_private_identifier_environment();
        let info = self.get_private_identifier(&env, node.name());
        go_assert!(
            info.is_some(),
            "Undeclared private name for property declaration."
        );
        if let Some(info) = &info {
            let info = info.borrow();
            if info.kind == PrivateIdentifierKind::UNTRANSFORMED || !info.is_valid {
                return node;
            }
        }

        let function_name = self.get_hoisted_function_name(node);
        if function_name.is_some() {
            let ec = self.ec();
            let modifiers = self.extract_non_static_non_accessor_modifiers(node);
            ec.start_variable_environment();
            let saved = self.in_iteration_statement;
            self.in_iteration_statement = false;
            let body = self.visit_function_body(node.body());
            let params = self.visit_nodes(node.parameter_list());
            self.in_iteration_statement = saved;

            let f = ec.factory();
            let func_expr = f.new_function_expression(
                modifiers,
                node.asterisk_token(),
                function_name,
                NodeList::NIL,
                params,
                Node::NIL,
                Node::NIL,
                body,
            );
            let assignment = f.new_assignment_expression(function_name, func_expr);
            self.add_pending_expressions(&[assignment]);
        }

        // remove method declaration from class
        Node::NIL
    }

    // Go: transformers/estransforms/classfields.go:714 classFieldsTransformer.extractNonStaticNonAccessorModifiers
    pub(super) fn extract_non_static_non_accessor_modifiers(&self, node: Node) -> ModifierList {
        extract_modifiers(
            &self.emit_context,
            node.modifiers(),
            !(ModifierFlags::STATIC | ModifierFlags::ACCESSOR),
        )
    }

    // Go: transformers/estransforms/classfields.go:718 classFieldsTransformer.setCurrentClassElementAnd
    pub(super) fn set_current_class_element_and<R>(
        &mut self,
        class_element: Node,
        visitor: fn(&mut Self, Node) -> R,
        node: Node,
    ) -> R {
        if class_element != self.current_class_element {
            let saved = self.current_class_element;
            self.current_class_element = class_element;
            let result = visitor(self, node);
            self.current_class_element = saved;
            return result;
        }
        visitor(self, node)
    }

    // Go: transformers/estransforms/classfields.go:730 classFieldsTransformer.visitEachChildOfNode
    /// visitEachChildOfNode just calls Visitor.VisitEachChild, but is necessary to avoid repeated closure allocations when passing as a callback.
    pub(super) fn visit_each_child_of_node(&mut self, node: Node) -> Node {
        self.visit_each_child(node)
    }

    // Go: transformers/estransforms/classfields.go:734 classFieldsTransformer.setInIterationStatementAnd
    pub(super) fn set_in_iteration_statement_and(
        &mut self,
        in_iteration: bool,
        visitor: fn(&mut Self, Node) -> Node,
        node: Node,
    ) -> Node {
        if self.in_iteration_statement != in_iteration {
            let saved = self.in_iteration_statement;
            self.in_iteration_statement = in_iteration;
            let result = visitor(self, node);
            self.in_iteration_statement = saved;
            return result;
        }
        visitor(self, node)
    }

    // Go: transformers/estransforms/classfields.go:745 classFieldsTransformer.clearClassElementAndVisitEachChild
    #[allow(dead_code)]
    pub(super) fn clear_class_element_and_visit_each_child(&mut self, node: Node) -> Node {
        self.set_current_class_element_and(Node::NIL, Self::visit_each_child_of_node, node)
    }

    // Go: transformers/estransforms/classfields.go:761 classFieldsTransformer.visitFunctionExpressionOrDeclaration
    /// visitFunctionExpressionOrDeclaration handles lexical environment scoping for function
    /// expressions and declarations, mirroring Strada's onEmitNode behavior.
    ///
    /// In Strada, onEmitNode checks whether a FunctionExpression has been registered in
    /// lexicalEnvironmentMap (via its original node). If found, the lexical environment is
    /// restored; otherwise it is cleared (since regular functions create a new `this` scope).
    ///
    /// Since Corsa performs substitution eagerly (no emit-time hooks), we replicate this by
    /// preserving currentClassElement for function expressions whose original node is a class
    /// member of the current class. This allows visitThisExpression to correctly substitute
    /// `this` -> `_classThis` inside synthesized functions (e.g., ES decorator descriptor
    /// methods for static private auto-accessors).
    pub(super) fn visit_function_expression_or_declaration(&mut self, node: Node) -> Node {
        if self.current_class_element.is_some() {
            let ec = self.ec();
            let original = ec.most_original(node);
            if original != node && self.current_class_container.is_some() {
                for member in self.current_class_container.members().iter() {
                    if ec.most_original(member) == original && is_static(member) {
                        // The function expression originates from a static class member (e.g., a
                        // descriptor method synthesized by the ES decorator transformer for a
                        // static private auto-accessor). Preserve the current class element so
                        // that visitThisExpression can substitute `this` with `_classThis`.
                        // Non-static members must NOT preserve the class element because `this`
                        // inside their descriptor functions should remain dynamic.
                        return self.visit_each_child_of_node(node);
                    }
                }
            }
        }
        self.set_current_class_element_and(Node::NIL, Self::visit_each_child_of_node, node)
    }

    // Go: transformers/estransforms/classfields.go:781 classFieldsTransformer.setClassElementAndVisitEachChild
    pub(super) fn set_class_element_and_visit_each_child(&mut self, node: Node) -> Node {
        self.set_current_class_element_and(node, Self::visit_each_child_of_node, node)
    }

    // Go: transformers/estransforms/classfields.go:785 classFieldsTransformer.getHoistedFunctionName
    pub(super) fn get_hoisted_function_name(&self, node: Node) -> Node {
        go_assert!(node.name().is_some() && is_private_identifier(node.name()));
        let info = self.access_private_identifier(node.name());
        go_assert!(
            info.is_some(),
            "Undeclared private name for property declaration."
        );
        let Some(info) = info else {
            return Node::NIL;
        };
        let info = info.borrow();
        if info.kind == PrivateIdentifierKind::METHOD {
            return info.method_name;
        }
        if info.kind == PrivateIdentifierKind::ACCESSOR {
            if is_get_accessor_declaration(node) {
                return info.getter_name;
            }
            if is_set_accessor_declaration(node) {
                return info.setter_name;
            }
        }
        Node::NIL
    }

    // Go: transformers/estransforms/classfields.go:803 classFieldsTransformer.tryGetClassThis
    pub(super) fn try_get_class_this(&mut self) -> Node {
        let class_this = self.try_get_class_this_no_container();
        if class_this.is_some() {
            return class_this;
        }
        if self.current_class_container.is_some() {
            return self.current_class_container.name();
        }
        Node::NIL
    }

    // Go: transformers/estransforms/classfields.go:813 classFieldsTransformer.tryGetClassThisNoContainer
    pub(super) fn try_get_class_this_no_container(&mut self) -> Node {
        let lex = *self.get_class_lexical_environment().borrow();
        if lex.class_this.is_some() {
            return lex.class_this;
        }
        if lex.class_constructor.is_some() {
            return lex.class_constructor;
        }
        Node::NIL
    }

    // Go: transformers/estransforms/classfields.go:833 classFieldsTransformer.transformAutoAccessor
    /// transformAutoAccessor transforms an auto-accessor property:
    ///
    /// ```text
    /// accessor x = 1;
    /// ```
    ///
    /// into:
    ///
    /// ```text
    /// #x = 1;
    /// get x() { return this.#x; }
    /// set x(value) { this.#x = value; }
    /// ```
    pub(super) fn transform_auto_accessor(&mut self, node: Node) -> Node {
        let ec = self.ec();
        let f = ec.factory();
        let comment_range = ec.comment_range(node);
        let source_map_range = ec.source_map_range(node);

        // Since we're creating two declarations where there was previously one, cache
        // the expression for any computed property names.
        let name = node.name();
        let mut getter_name = name;
        let mut setter_name = name;
        if is_computed_property_name(name) && !is_simple_inlineable_expression(name.expression()) {
            let cache_assignment = find_computed_property_name_cache_assignment(&ec, name);
            if cache_assignment.is_some() {
                let visited = self.visit_node(name.expression());
                getter_name = f.update_computed_property_name(name, visited);
                setter_name = f.update_computed_property_name(name, cache_assignment.left());
            } else {
                let temp = f.new_temp_variable();
                ec.set_source_map_range(temp, name.expression().loc());
                ec.add_variable_declaration(temp);
                let expression = self.visit_node(name.expression());
                let assignment = f.new_assignment_expression(temp, expression);
                ec.set_source_map_range(assignment, name.expression().loc());
                getter_name = f.update_computed_property_name(name, assignment);
                setter_name = f.update_computed_property_name(name, temp);
            }
        }

        let modifiers = self.modifier_visitor_visit_modifiers(node.modifiers());
        let backing_field =
            create_accessor_property_backing_field(f, node, modifiers, node.initializer());
        ec.set_original(backing_field, node);
        ec.add_emit_flags(backing_field, EmitFlags::NO_COMMENTS);
        ec.set_source_map_range(backing_field, source_map_range);

        let receiver = if is_static(node) {
            let receiver = self.try_get_class_this();
            if receiver.is_nil() {
                f.new_this_expression()
            } else {
                receiver
            }
        } else {
            f.new_this_expression()
        };

        let getter =
            self.create_accessor_property_get_redirector(node, modifiers, getter_name, receiver);
        ec.set_original(getter, node);
        ec.set_comment_range(getter, comment_range);
        ec.set_source_map_range(getter, source_map_range);

        // create a fresh copy of the modifiers so that we don't duplicate comments
        let setter_modifiers = if modifiers.is_some() {
            let nodes =
                create_modifiers_from_modifier_flags(modifiers.modifier_flags(), &mut |k| {
                    f.new_modifier(k)
                });
            f.new_modifier_list(&nodes)
        } else {
            ModifierList::NIL
        };
        let setter = self.create_accessor_property_set_redirector(
            node,
            setter_modifiers,
            setter_name,
            receiver,
        );
        ec.set_original(setter, node);
        ec.add_emit_flags(setter, EmitFlags::NO_COMMENTS);
        ec.set_source_map_range(setter, source_map_range);

        // Visit the results in a second pass
        let (visited, _) = self.with_visitor(Self::visit_accessor_field_result, |v| {
            v.visit_slice(&[backing_field, getter, setter])
        });
        f.new_syntax_list(&visited)
    }

    // Go: transformers/estransforms/classfields.go:895 classFieldsTransformer.transformPrivateFieldInitializer
    pub(super) fn transform_private_field_initializer(&mut self, mut node: Node) -> Node {
        let ec = self.ec();
        let f = ec.factory();
        if self.should_transform_class_element_to_weak_map(node) {
            // If we are transforming private elements into WeakMap/WeakSet, we should elide the node.
            let env = self.get_private_identifier_environment();
            let info = self.get_private_identifier(&env, node.name());
            go_assert!(
                info.is_some(),
                "Undeclared private name for property declaration."
            );
            let (untransformed, is_valid, is_static) =
                info.as_ref().map_or((false, false, false), |i| {
                    let i = i.borrow();
                    (
                        i.kind == PrivateIdentifierKind::UNTRANSFORMED,
                        i.is_valid,
                        i.is_static,
                    )
                });

            // Leave invalid code untransformed
            if untransformed || !is_valid {
                return node;
            }

            // If we encounter a valid private static field and we're not transforming
            // class static blocks, convert to a static block initializer.
            if is_static && !self.should_transform_private_elements_or_class_static_blocks {
                // TODO: fix
                let this_expression = f.new_this_expression();
                let statement =
                    self.transform_property_or_class_static_block(node, this_expression);
                if statement.is_some() {
                    return f.new_class_static_block_declaration(
                        ModifierList::NIL, /*modifiers*/
                        f.new_block(f.new_node_list(&[statement]), true /*multiLine*/),
                    );
                }
            }

            return Node::NIL;
        }

        if self.should_transform_initializers_using_set
            && !has_static_modifier(node)
            && self.lexical_environment_data().is_some_and(|data| {
                data.facts & CLASS_FACTS_WILL_HOIST_INITIALIZERS_TO_CONSTRUCTOR != 0
            })
        {
            let modifiers = self.visit_modifiers(node.modifiers());
            return f.update_property_declaration(
                node,
                modifiers,
                node.name(),
                Node::NIL, /*postfixToken*/
                Node::NIL, /*typeNode*/
                Node::NIL, /*initializer*/
            );
        }

        if self.is_named_evaluation_needing_assigned_name(node) {
            node = transform_named_evaluation(&ec, node, false, "");
        }

        let modifiers = self.modifier_visitor_visit_modifiers(node.modifiers());
        let name = self.visit_property_name(node.name());
        let initializer = self.visit_node(node.initializer());
        f.update_property_declaration(
            node,
            modifiers,
            name,
            Node::NIL, /*postfixToken*/
            Node::NIL, /*typeNode*/
            initializer,
        )
    }

    // Go: transformers/estransforms/classfields.go:949 classFieldsTransformer.transformPublicFieldInitializer
    pub(super) fn transform_public_field_initializer(&mut self, node: Node) -> Node {
        let ec = self.ec();
        let f = ec.factory();
        if self.should_transform_initializers && !is_auto_accessor_property_declaration(node) {
            // Elide the property declaration; the initializer will be moved to the constructor.
            // For computed property names, we still need to emit the expression.
            let expr = self.get_property_name_expression_if_needed(
                node.name(),
                node.initializer().is_some()
                    || self.compiler_options.get_use_define_for_class_fields(),
            );
            if expr.is_some() {
                for e in flatten_comma_list(expr) {
                    self.add_pending_expressions(&[e]);
                }
            }

            // When target >= ES2022 (i.e., !shouldTransformPrivateElementsOrClassStaticBlocks) and we
            // still need to transform initializers (useDefineForClassFields: false), static property
            // initializers must be converted into `static { this.x = ...; }` blocks so that `this`
            // refers to the class constructor inside the static block.
            if is_static(node) && !self.should_transform_private_elements_or_class_static_blocks {
                let this_expression = f.new_this_expression();
                let initializer_statement =
                    self.transform_property_or_class_static_block(node, this_expression);
                if initializer_statement.is_some() {
                    let static_block = f.new_class_static_block_declaration(
                        ModifierList::NIL, /*modifiers*/
                        f.new_block(f.new_node_list(&[initializer_statement]), false),
                    );

                    ec.set_original(static_block, node);
                    ec.set_comment_range(static_block, node.loc());

                    ec.add_emit_flags(initializer_statement, EmitFlags::NO_COMMENTS);
                    return static_block;
                }
            }

            return Node::NIL;
        }

        let modifiers = self.modifier_visitor_visit_modifiers(node.modifiers());
        let name = self.visit_property_name(node.name());
        let initializer = self.visit_node(node.initializer());
        f.update_property_declaration(
            node,
            modifiers,
            name,
            Node::NIL, /*postfixToken*/
            Node::NIL, /*typeNode*/
            initializer,
        )
    }

    // Go: transformers/estransforms/classfields.go:993 classFieldsTransformer.transformFieldInitializer
    pub(super) fn transform_field_initializer(&mut self, node: Node) -> Node {
        go_assert!(
            !has_decorators(node),
            "Decorators should already have been transformed and elided."
        );
        if is_private_identifier_class_element_declaration(node) {
            return self.transform_private_field_initializer(node);
        }
        self.transform_public_field_initializer(node)
    }

    // Go: transformers/estransforms/classfields.go:1001 classFieldsTransformer.shouldTransformAutoAccessorsInCurrentClass
    pub(super) fn should_transform_auto_accessors_in_current_class(&self) -> bool {
        if self.should_transform_auto_accessors {
            return true;
        }
        // When targeting ESNext with useDefineForClassFields: false, auto-accessors are only
        // transformed if the current class will hoist initializers to the constructor.
        self.lexical_environment_data().is_some_and(|data| {
            data.facts & CLASS_FACTS_WILL_HOIST_INITIALIZERS_TO_CONSTRUCTOR != 0
        })
    }

    // Go: transformers/estransforms/classfields.go:1011 classFieldsTransformer.visitPropertyDeclaration
    pub(super) fn visit_property_declaration(&mut self, node: Node) -> Node {
        // If this is an auto-accessor, we defer to `transformAutoAccessor`. That function
        // will in turn call `transformFieldInitializer` as needed.
        if is_auto_accessor_property_declaration(node)
            && (self.should_transform_auto_accessors_in_current_class()
                || has_static_modifier(node)
                    && self.should_always_transform_private_static_elements(node))
        {
            return self.transform_auto_accessor(node);
        }
        self.transform_field_initializer(node)
    }

    // Go: transformers/estransforms/classfields.go:1022 classFieldsTransformer.createPrivateIdentifierAccess
    pub(super) fn create_private_identifier_access(
        &mut self,
        info: &PrivateIdentifierInfoRef,
        receiver: Node,
    ) -> Node {
        let receiver = self.visit_node(receiver);
        self.create_private_identifier_access_helper(info, receiver)
    }

    // Go: transformers/estransforms/classfields.go:1027 classFieldsTransformer.createPrivateIdentifierAccessHelper
    pub(super) fn create_private_identifier_access_helper(
        &self,
        info: &PrivateIdentifierInfoRef,
        receiver: Node,
    ) -> Node {
        let ec = &self.emit_context;
        let f = ec.factory();
        ec.set_comment_range(receiver, TextRange::new(-1, receiver.end()));

        let info = info.borrow();
        match info.kind {
            PrivateIdentifierKind::ACCESSOR => f.new_class_private_field_get_helper(
                receiver,
                info.brand_check_identifier,
                info.kind,
                info.getter_name,
            ),
            PrivateIdentifierKind::METHOD => f.new_class_private_field_get_helper(
                receiver,
                info.brand_check_identifier,
                info.kind,
                info.method_name,
            ),
            PrivateIdentifierKind::FIELD => {
                let fn_ = if info.is_static {
                    info.variable_name
                } else {
                    Node::NIL
                };
                f.new_class_private_field_get_helper(
                    receiver,
                    info.brand_check_identifier,
                    info.kind,
                    fn_,
                )
            }
            PrivateIdentifierKind::UNTRANSFORMED => crate::gostd::debug::fail(
                "Access helpers should not be created for untransformed private elements",
            ),
            _ => {
                debug_assert!(false, "Unknown private element type");
                Node::NIL
            }
        }
    }

    // Go: transformers/estransforms/classfields.go:1064 classFieldsTransformer.visitPropertyAccessExpression
    pub(super) fn visit_property_access_expression(&mut self, node: Node) -> Node {
        let ec = self.ec();
        if is_private_identifier(node.name()) {
            let info = self.access_private_identifier(node.name());
            if let Some(info) = info {
                let result = self.create_private_identifier_access(&info, node.expression());
                ec.set_original(result, node);
                set_node_loc(result, node.loc());
                return result;
            }
        }
        if self.should_transform_super_in_static_initializers
            && self.current_class_element.is_some()
            && is_super_property(node)
            && is_identifier(node.name())
            && is_static_property_declaration_or_class_static_block(self.current_class_element)
        {
            if let Some(data) = self.lexical_environment_data() {
                if data.facts & CLASS_FACTS_CLASS_WAS_DECORATED != 0 {
                    return self.visit_invalid_super_property(node);
                }
                if data.class_constructor.is_some() && data.super_class_reference.is_some() {
                    // converts `super.x` into `Reflect.get(_baseTemp, "x", _classTemp)`
                    let f = ec.factory();
                    let super_property = f.new_reflect_get_call(
                        data.super_class_reference,
                        f.new_string_literal_from_node(node.name()),
                        data.class_constructor,
                    );
                    ec.set_original(super_property, node.expression());
                    set_node_loc(super_property, node.expression().loc());
                    return super_property;
                }
            }
        }
        // Visit only the expression, not the name (when it's a regular identifier), to prevent
        // substitution of property names. Strada's onSubstituteNode only fires for
        // EmitHint.Expression, which excludes the .name of PropertyAccessExpression.
        // Private identifier names are still visited through VisitEachChild so they can be
        // transformed by visitPrivateIdentifier.
        if is_identifier(node.name()) {
            return self.visit_property_access_expression_for_substitution(node);
        }
        self.visit_each_child(node)
    }

    // Go: transformers/estransforms/classfields.go:1108 classFieldsTransformer.visitPropertyAccessExpressionForSubstitution
    /// visitPropertyAccessExpressionForSubstitution visits only the expression of a PropertyAccessExpression,
    /// leaving the name unchanged. This prevents the name from being treated as a standalone identifier
    /// reference and incorrectly substituted with a class alias.
    pub(super) fn visit_property_access_expression_for_substitution(&mut self, node: Node) -> Node {
        let expression = self.visit_node(node.expression());
        if expression != node.expression() {
            return self.ec().factory().update_property_access_expression(
                node,
                expression,
                node.question_dot_token(),
                node.name(),
                node.flags(),
            );
        }
        node
    }

    // Go: transformers/estransforms/classfields.go:1116 classFieldsTransformer.visitElementAccessExpression
    pub(super) fn visit_element_access_expression(&mut self, node: Node) -> Node {
        if self.should_transform_super_in_static_initializers
            && self.current_class_element.is_some()
            && is_super_property(node)
            && is_static_property_declaration_or_class_static_block(self.current_class_element)
        {
            if let Some(data) = self.lexical_environment_data() {
                if data.facts & CLASS_FACTS_CLASS_WAS_DECORATED != 0 {
                    return self.visit_invalid_super_property(node);
                }
                if data.class_constructor.is_some() && data.super_class_reference.is_some() {
                    // converts `super[x]` into `Reflect.get(_baseTemp, x, _classTemp)`
                    let argument = self.visit_node(node.argument_expression());
                    let ec = self.ec();
                    let super_property = ec.factory().new_reflect_get_call(
                        data.super_class_reference,
                        argument,
                        data.class_constructor,
                    );
                    ec.set_original(super_property, node.expression());
                    set_node_loc(super_property, node.expression().loc());
                    return super_property;
                }
            }
        }
        self.visit_each_child(node)
    }

    // Go: transformers/estransforms/classfields.go:1140 classFieldsTransformer.visitPreOrPostfixUnaryExpression
    pub(super) fn visit_pre_or_postfix_unary_expression(
        &mut self,
        node: Node,
        discarded: bool,
    ) -> Node {
        let ec = self.ec();
        let f = ec.factory();
        let operator = node.operator();
        let operand = node.operand();

        if operator == SyntaxKind::PlusPlusToken || operator == SyntaxKind::MinusMinusToken {
            let operand_skipped = skip_parentheses(operand);

            // Private identifier property access
            if is_property_access_expression(operand_skipped)
                && is_private_identifier(operand_skipped.name())
            {
                let info = self.access_private_identifier(operand_skipped.name());
                if let Some(info) = info {
                    let receiver = self.visit_node(operand_skipped.expression());
                    let (read_expression, initialize_expression) =
                        self.create_copiable_receiver_expr(receiver);

                    let mut expression =
                        self.create_private_identifier_access_helper(&info, read_expression);
                    let mut temp = Node::NIL;
                    if !is_prefix_unary_expression(node) && !discarded {
                        temp = f.new_temp_variable();
                        ec.add_variable_declaration(temp);
                    }
                    expression = expand_pre_or_postfix_increment_or_decrement_expression(
                        f, &ec, node, expression, temp,
                    );
                    let assign_receiver = if initialize_expression.is_some() {
                        initialize_expression
                    } else {
                        read_expression
                    };
                    expression = self.create_private_identifier_assignment(
                        &info,
                        assign_receiver,
                        expression,
                        SyntaxKind::EqualsToken,
                    );
                    ec.set_original(expression, node);
                    set_node_loc(expression, node.loc());
                    if temp.is_some() {
                        expression = f.new_comma_expression(expression, temp);
                        set_node_loc(expression, node.loc());
                    }
                    return expression;
                }
            } else if self.should_transform_super_in_static_initializers
                && self.current_class_element.is_some()
                && is_super_property(operand_skipped)
                && is_static_property_declaration_or_class_static_block(self.current_class_element)
                && let Some(data) = self.lexical_environment_data()
            {
                // converts `++super.a` into `(Reflect.set(_baseTemp, "a", (_a = Reflect.get(_baseTemp, "a", _classTemp), _b = ++_a), _classTemp), _b)`
                // converts `++super[f()]` into `(Reflect.set(_baseTemp, _a = f(), (_b = Reflect.get(_baseTemp, _a, _classTemp), _c = ++_b), _classTemp), _c)`
                // converts `--super.a` into `(Reflect.set(_baseTemp, "a", (_a = Reflect.get(_baseTemp, "a", _classTemp), _b = --_a), _classTemp), _b)`
                // converts `--super[f()]` into `(Reflect.set(_baseTemp, _a = f(), (_b = Reflect.get(_baseTemp, _a, _classTemp), _c = --_b), _classTemp), _c)`
                // converts `super.a++` into `(Reflect.set(_baseTemp, "a", (_a = Reflect.get(_baseTemp, "a", _classTemp), _b = _a++), _classTemp), _b)`
                // converts `super[f()]++` into `(Reflect.set(_baseTemp, _a = f(), (_b = Reflect.get(_baseTemp, _a, _classTemp), _c = _b++), _classTemp), _c)`
                // converts `super.a--` into `(Reflect.set(_baseTemp, "a", (_a = Reflect.get(_baseTemp, "a", _classTemp), _b = _a--), _classTemp), _b)`
                // converts `super[f()]--` into `(Reflect.set(_baseTemp, _a = f(), (_b = Reflect.get(_baseTemp, _a, _classTemp), _c = _b--), _classTemp), _c)`
                if data.facts & CLASS_FACTS_CLASS_WAS_DECORATED != 0 {
                    let visited_expr = self.visit_invalid_super_property(operand_skipped);
                    if is_prefix_unary_expression(node) {
                        return f.update_prefix_unary_expression(
                            node,
                            node.operator(),
                            visited_expr,
                        );
                    }
                    return f.update_postfix_unary_expression(node, visited_expr, node.operator());
                }
                if data.class_constructor.is_some() && data.super_class_reference.is_some() {
                    let mut setter_name = Node::NIL;
                    let mut getter_name = Node::NIL;
                    if is_property_access_expression(operand_skipped) {
                        if is_identifier(operand_skipped.name()) {
                            getter_name = f.new_string_literal_from_node(operand_skipped.name());
                            setter_name = getter_name;
                        }
                    } else if is_element_access_expression(operand_skipped) {
                        if is_simple_inlineable_expression(operand_skipped.argument_expression()) {
                            getter_name = operand_skipped.argument_expression();
                            setter_name = getter_name;
                        } else {
                            getter_name = f.new_temp_variable();
                            ec.add_variable_declaration(getter_name);
                            let argument = self.visit_node(operand_skipped.argument_expression());
                            setter_name = f.new_assignment_expression(getter_name, argument);
                        }
                    }
                    if setter_name.is_some() && getter_name.is_some() {
                        let mut expression = f.new_reflect_get_call(
                            data.super_class_reference,
                            getter_name,
                            data.class_constructor,
                        );
                        set_node_loc(expression, operand_skipped.loc());

                        let mut temp = Node::NIL;
                        if !discarded {
                            temp = f.new_temp_variable();
                            ec.add_variable_declaration(temp);
                        }
                        expression = expand_pre_or_postfix_increment_or_decrement_expression(
                            f, &ec, node, expression, temp,
                        );
                        expression = f.new_reflect_set_call(
                            data.super_class_reference,
                            setter_name,
                            expression,
                            data.class_constructor,
                        );
                        ec.set_original(expression, node);
                        set_node_loc(expression, node.loc());
                        if temp.is_some() {
                            expression = f.new_comma_expression(expression, temp);
                            set_node_loc(expression, node.loc());
                        }
                        return expression;
                    }
                }
            }
        }
        self.visit_each_child(node)
    }

    // Go: transformers/estransforms/classfields.go:1244 classFieldsTransformer.visitForStatement
    pub(super) fn visit_for_statement(&mut self, node: Node) -> Node {
        let initializer = self.discarded_value_visitor_visit_node(node.initializer());
        let condition = self.visit_node(node.condition());
        let incrementor = self.discarded_value_visitor_visit_node(node.incrementor());
        let saved = self.in_iteration_statement;
        self.in_iteration_statement = true;
        let body = self.visit_iteration_body(node.statement());
        self.in_iteration_statement = saved;
        self.ec()
            .factory()
            .update_for_statement(node, initializer, condition, incrementor, body)
    }

    // Go: transformers/estransforms/classfields.go:1255 classFieldsTransformer.visitExpressionStatement
    pub(super) fn visit_expression_statement(&mut self, node: Node) -> Node {
        // Preserve private identifiers that appear directly as the expression of an
        // ExpressionStatement (e.g., `#;`). This is error-recovery output from the parser
        // for invalid syntax. Keeping it ensures the runtime throws a SyntaxError rather
        // than silently succeeding with an empty statement.
        if is_private_identifier(node.expression())
            && self.should_transform_private_elements_or_class_static_blocks
        {
            return node;
        }
        let expression = self.discarded_value_visitor_visit_node(node.expression());
        self.ec()
            .factory()
            .update_expression_statement(node, expression)
    }

    // Go: transformers/estransforms/classfields.go:1269 classFieldsTransformer.createCopiableReceiverExpr
    /// Returns `(readExpression, initializeExpression)`.
    pub(super) fn create_copiable_receiver_expr(&self, receiver: Node) -> (Node, Node) {
        let ec = &self.emit_context;
        let f = ec.factory();
        let clone = if !node_is_synthesized(receiver) {
            f.clone_node(receiver)
        } else {
            receiver
        };
        if is_simple_inlineable_expression(receiver) {
            return (clone, Node::NIL);
        }
        let read_expression = f.new_temp_variable();
        ec.add_variable_declaration(read_expression);
        let initialize_expression = f.new_assignment_expression(read_expression, clone);
        (read_expression, initialize_expression)
    }

    // Go: transformers/estransforms/classfields.go:1283 classFieldsTransformer.visitCallExpression
    pub(super) fn visit_call_expression(&mut self, node: Node) -> Node {
        let ec = self.ec();
        let f = ec.factory();
        if is_property_access_expression(node.expression())
            && is_private_identifier(node.expression().name())
            && self
                .access_private_identifier(node.expression().name())
                .is_some()
        {
            // obj.#x()

            // Transform call expressions of private names to properly bind the `this` parameter.
            let (this_arg, target) = self.create_call_binding(node.expression());
            let visited_target = self.visit_node(target);
            let visited_this_arg = self.visit_node(this_arg);
            let visited_args = self.visit_nodes(node.argument_list());
            let mut all_args: Vec<Node> = Vec::with_capacity(1 + visited_args.nodes().len());
            all_args.push(visited_this_arg);
            all_args.extend(visited_args.nodes().iter());
            if node.flags().intersects(NodeFlags::OPTIONAL_CHAIN) {
                return f.update_call_expression(
                    node,
                    f.new_property_access_expression(
                        visited_target,
                        node.question_dot_token(),
                        f.new_identifier("call"),
                        NodeFlags::OPTIONAL_CHAIN,
                    ),
                    Node::NIL,     /*questionDotToken*/
                    NodeList::NIL, /*typeArguments*/
                    f.new_node_list(&all_args),
                    node.flags(),
                );
            }
            return f.update_call_expression(
                node,
                f.new_property_access_expression(
                    visited_target,
                    Node::NIL,
                    f.new_identifier("call"),
                    NodeFlags::NONE,
                ),
                Node::NIL,     /*questionDotToken*/
                NodeList::NIL, /*typeArguments*/
                f.new_node_list(&all_args),
                node.flags(),
            );
        }

        if self.should_transform_super_in_static_initializers
            && self.current_class_element.is_some()
            && is_super_property(node.expression())
            && is_static_property_declaration_or_class_static_block(self.current_class_element)
            && let Some(data) = self.lexical_environment_data()
            && data.class_constructor.is_some()
        {
            // super.x()
            // super[x]()

            // converts `super.f(...)` into `Reflect.get(_baseTemp, "f", _classTemp).call(_classTemp, ...)`
            let target = self.visit_node(node.expression());
            let arguments = self.visit_nodes(node.argument_list());
            let invocation = f.new_function_call_call(
                target,
                data.class_constructor,
                &arguments.nodes().to_vec(),
            );
            ec.set_original(invocation, node);
            set_node_loc(invocation, node.loc());
            return invocation;
        }

        self.visit_each_child(node)
    }

    // Go: transformers/estransforms/classfields.go:1338 classFieldsTransformer.visitTaggedTemplateExpression
    pub(super) fn visit_tagged_template_expression(&mut self, node: Node) -> Node {
        let ec = self.ec();
        let f = ec.factory();
        if is_property_access_expression(node.tag())
            && is_private_identifier(node.tag().name())
            && self.access_private_identifier(node.tag().name()).is_some()
        {
            // Bind the `this` correctly for tagged template literals when the tag is a private identifier property access.
            let (this_arg, target) = self.create_call_binding(node.tag());
            let visited_target = self.visit_node(target);
            let visited_this_arg = self.visit_node(this_arg);
            let bind_expr = f.new_call_expression(
                f.new_property_access_expression(
                    visited_target,
                    Node::NIL,
                    f.new_identifier("bind"),
                    NodeFlags::NONE,
                ),
                Node::NIL,     /*questionDotToken*/
                NodeList::NIL, /*typeArguments*/
                f.new_node_list(&[visited_this_arg]),
                NodeFlags::NONE,
            );
            let template = self.visit_node(node.template());
            return f.update_tagged_template_expression(
                node,
                bind_expr,
                Node::NIL,     /*questionDotToken*/
                NodeList::NIL, /*typeArguments*/
                template,
                node.flags(),
            );
        }

        if self.should_transform_super_in_static_initializers
            && self.current_class_element.is_some()
            && is_super_property(node.tag())
            && is_static_property_declaration_or_class_static_block(self.current_class_element)
            && let Some(data) = self.lexical_environment_data()
            && data.class_constructor.is_some()
        {
            // converts `` super.f`x` `` into `` Reflect.get(_baseTemp, "f", _classTemp).bind(_classTemp)`x` ``
            let tag = self.visit_node(node.tag());
            let invocation = f.new_function_bind_call(tag, data.class_constructor, &[]);
            ec.set_original(invocation, node);
            set_node_loc(invocation, node.loc());
            let template = self.visit_node(node.template());
            return f.update_tagged_template_expression(
                node,
                invocation,
                Node::NIL,     /*questionDotToken*/
                NodeList::NIL, /*typeArguments*/
                template,
                node.flags(),
            );
        }

        self.visit_each_child(node)
    }

    // Go: transformers/estransforms/classfields.go:1386 classFieldsTransformer.transformClassStaticBlockDeclaration
    pub(super) fn transform_class_static_block_declaration(&mut self, node: Node) -> Node {
        let ec = self.ec();
        let f = ec.factory();
        if self.should_transform_private_elements_or_class_static_blocks {
            if is_class_this_assignment_block(&ec, node) {
                let result = self.visit_node(node.body().statements().get(0).expression());
                // If the generated `_classThis` assignment is a noop (i.e., `_classThis = _classThis`), we can
                // eliminate the expression
                if is_assignment_expression(result, true /*excludeCompoundAssignment*/)
                    && result.left() == result.right()
                {
                    return Node::NIL;
                }
                return result;
            }

            if is_class_named_evaluation_helper_block(&ec, node) {
                return self.visit_node(node.body().statements().get(0).expression());
            }

            ec.start_variable_environment();
            let statements = self.set_current_class_element_and_visit_statements(
                node,
                &node.body().statements().to_vec(),
            );
            let statements = ec.end_and_merge_variable_environment(&statements);

            // PORT: Go builds the IIFE with `NewImmediatelyInvokedArrowFunction` and then sets
            // `arrowFunction.Body.Statements.Loc`. A synthetic list fixes its `Loc` at creation, so the
            // IIFE is built here with the same factory calls and the list gets that `Loc` up front.
            let statements_list =
                f.new_node_list_with_loc(&statements, node.body().statement_list().loc());
            let arrow_function = f.new_arrow_function(
                ModifierList::NIL,
                NodeList::NIL,
                f.new_node_list(&[]),
                Node::NIL,
                Node::NIL,
                f.new_token(SyntaxKind::EqualsGreaterThanToken),
                f.new_block(statements_list, true),
            );
            let iife = f.new_call_expression(
                f.new_parenthesized_expression(arrow_function),
                Node::NIL,
                NodeList::NIL,
                f.new_node_list(&[]),
                NodeFlags::NONE,
            );
            ec.set_original(arrow_function, node);
            ec.add_emit_flags(arrow_function, EmitFlags::NO_LEXICAL_ARGUMENTS);
            // Preserve the statement list source range so the printer can emit detached comments
            // (e.g., `// do` inside an otherwise empty static block)
            ec.set_original(iife, node);
            ec.assign_source_map_range(iife, node);
            ec.add_emit_flags(arrow_function, EmitFlags::NO_LEXICAL_THIS);
            return iife;
        }
        Node::NIL
    }

    // Go: transformers/estransforms/classfields.go:1424 classFieldsTransformer.setCurrentClassElementAndVisitStatements
    pub(super) fn set_current_class_element_and_visit_statements(
        &mut self,
        class_element: Node,
        statements: &[Node],
    ) -> Vec<Node> {
        let saved_current_class_element = self.current_class_element;
        self.current_class_element = class_element;
        let (result, _) = self.visit_slice(statements);
        self.current_class_element = saved_current_class_element;
        result
    }

    // Go: transformers/estransforms/classfields.go:1432 classFieldsTransformer.isAnonymousClassNeedingAssignedNameWorker
    pub(super) fn is_anonymous_class_needing_assigned_name_worker(&self, node: Node) -> bool {
        if is_class_expression(node) && node.name().is_nil() {
            let static_properties_or_class_static_blocks =
                self.get_static_properties_and_class_static_block(node);
            if static_properties_or_class_static_blocks
                .iter()
                .any(|&n| is_class_named_evaluation_helper_block(&self.emit_context, n))
            {
                return false;
            }
            let has_transformable_statics = (self
                .should_transform_private_elements_or_class_static_blocks
                || self.node_has_transform_private_static_elements_flag(node))
                && static_properties_or_class_static_blocks.iter().any(|&n| {
                    is_class_static_block_declaration(n)
                        || is_private_identifier_class_element_declaration(n)
                        || self.should_transform_initializers && is_initialized_property(n)
                });
            return has_transformable_statics;
        }
        false
    }

    // Go: transformers/estransforms/classfields.go:1452 classFieldsTransformer.visitBinaryExpression
    pub(super) fn visit_binary_expression(&mut self, mut node: Node, discarded: bool) -> Node {
        let ec = self.ec();
        let f = ec.factory();
        if is_destructuring_assignment(node) {
            // ({ x: obj.#x } = ...)
            // ({ x: super.x } = ...)
            // ({ x: super[x] } = ...)
            let saved_pending_expressions = std::mem::take(&mut self.pending_expressions);
            let left =
                self.with_visitor(Self::visit_assignment_target, |v| v.visit_node(node.left()));
            let right = self.visit_node(node.right());
            let updated = f.update_binary_expression(
                node,
                ModifierList::NIL,
                left,
                Node::NIL,
                node.operator_token(),
                right,
            );
            let result = if !self.pending_expressions.is_empty() {
                let mut exprs = self.pending_expressions.clone();
                exprs.push(updated);
                f.inline_expressions(&exprs)
            } else {
                updated
            };
            self.pending_expressions = saved_pending_expressions;
            return result;
        }

        if is_assignment_expression(node, false /*excludeCompound*/) {
            // 13.15.2 RS: Evaluation
            //   AssignmentExpression : LeftHandSideExpression `=` AssignmentExpression
            //     1. If |LeftHandSideExpression| is neither an |ObjectLiteral| nor an |ArrayLiteral|, then
            //        a. Let _lref_ be ? Evaluation of |LeftHandSideExpression|.
            //        b. If IsAnonymousFunctionDefinition(|AssignmentExpression|) and IsIdentifierRef of |LeftHandSideExpression| are both *true*, then
            //           i. Let _rval_ be ? NamedEvaluation of |AssignmentExpression| with argument _lref_.[[ReferencedName]].
            //     ...
            //
            //   AssignmentExpression : LeftHandSideExpression `&&=` AssignmentExpression
            //     ...
            //     5. If IsAnonymousFunctionDefinition(|AssignmentExpression|) is *true* and IsIdentifierRef of |LeftHandSideExpression| is *true*, then
            //        a. Let _rval_ be ? NamedEvaluation of |AssignmentExpression| with argument _lref_.[[ReferencedName]].
            //     ...
            //
            //   AssignmentExpression : LeftHandSideExpression `||=` AssignmentExpression
            //     ...
            //     5. If IsAnonymousFunctionDefinition(|AssignmentExpression|) is *true* and IsIdentifierRef of |LeftHandSideExpression| is *true*, then
            //        a. Let _rval_ be ? NamedEvaluation of |AssignmentExpression| with argument _lref_.[[ReferencedName]].
            //     ...
            //
            //   AssignmentExpression : LeftHandSideExpression `??=` AssignmentExpression
            //     ...
            //     4. If IsAnonymousFunctionDefinition(|AssignmentExpression|) is *true* and IsIdentifierRef of |LeftHandSideExpression| is *true*, then
            //        a. Let _rval_ be ? NamedEvaluation of |AssignmentExpression| with argument _lref_.[[ReferencedName]].
            //     ...

            if self.is_named_evaluation_needing_assigned_name(node) {
                node = transform_named_evaluation(&ec, node, false, "");
                go_assert!(node.is_some() && is_assignment_expression(node, false));
            }

            let left = skip_outer_expressions(
                node.left(),
                OuterExpressionKinds::OEK_PARTIALLY_EMITTED_EXPRESSIONS
                    | OuterExpressionKinds::OEK_PARENTHESES,
            );
            if is_property_access_expression(left) && is_private_identifier(left.name()) {
                // obj.#x = ...
                let info = self.access_private_identifier(left.name());
                if let Some(info) = info {
                    let result = self.create_private_identifier_assignment(
                        &info,
                        left.expression(),
                        node.right(),
                        node.operator_token().kind(),
                    );
                    ec.set_original(result, node);
                    set_node_loc(result, node.loc());
                    return result;
                }
            } else if self.should_transform_super_in_static_initializers
                && self.current_class_element.is_some()
                && is_super_property(node.left())
                && is_static_property_declaration_or_class_static_block(self.current_class_element)
                && let Some(data) = self.lexical_environment_data()
            {
                // super.x = ...
                // super[x] = ...
                // super.x += ...
                // super.x -= ...
                if data.facts & CLASS_FACTS_CLASS_WAS_DECORATED != 0 {
                    let left = self.visit_invalid_super_property(node.left());
                    let right = self.visit_node(node.right());
                    return f.update_binary_expression(
                        node,
                        ModifierList::NIL,
                        left,
                        Node::NIL,
                        node.operator_token(),
                        right,
                    );
                }
                if data.class_constructor.is_some() && data.super_class_reference.is_some() {
                    let mut setter_name = Node::NIL;
                    if is_element_access_expression(node.left()) {
                        setter_name = self.visit_node(node.left().argument_expression());
                    } else if is_property_access_expression(node.left())
                        && is_identifier(node.left().name())
                    {
                        setter_name = f.new_string_literal_from_node(node.left().name());
                    }
                    if setter_name.is_some() {
                        // converts `super.x = 1` into `(Reflect.set(_baseTemp, "x", _a = 1, _classTemp), _a)`
                        // converts `super[f()] = 1` into `(Reflect.set(_baseTemp, f(), _a = 1, _classTemp), _a)`
                        // converts `super.x += 1` into `(Reflect.set(_baseTemp, "x", _a = Reflect.get(_baseTemp, "x", _classtemp) + 1, _classTemp), _a)`
                        // converts `super[f()] += 1` into `(Reflect.set(_baseTemp, _a = f(), _b = Reflect.get(_baseTemp, _a, _classtemp) + 1, _classTemp), _b)`

                        let mut expression = self.visit_node(node.right());
                        if is_compound_assignment(node.operator_token().kind()) {
                            let mut getter_name = setter_name;
                            if !is_simple_inlineable_expression(setter_name) {
                                getter_name = f.new_temp_variable();
                                ec.add_variable_declaration(getter_name);
                                setter_name = f.new_assignment_expression(getter_name, setter_name);
                            }
                            let super_property_get = f.new_reflect_get_call(
                                data.super_class_reference,
                                getter_name,
                                data.class_constructor,
                            );
                            ec.set_original(super_property_get, node.left());
                            set_node_loc(super_property_get, node.left().loc());
                            expression = f.new_binary_expression(
                                ModifierList::NIL,
                                super_property_get,
                                Node::NIL,
                                f.new_token(get_non_assignment_operator_for_compound_assignment(
                                    node.operator_token().kind(),
                                )),
                                expression,
                            );
                            set_node_loc(expression, node.loc());
                        }

                        let mut temp = Node::NIL;
                        if !discarded {
                            temp = f.new_temp_variable();
                            ec.add_variable_declaration(temp);
                        }
                        if temp.is_some() {
                            expression = f.new_assignment_expression(temp, expression);
                            set_node_loc(expression, node.loc());
                        }

                        expression = f.new_reflect_set_call(
                            data.super_class_reference,
                            setter_name,
                            expression,
                            data.class_constructor,
                        );
                        ec.set_original(expression, node);
                        set_node_loc(expression, node.loc());

                        if temp.is_some() {
                            expression = f.new_comma_expression(expression, temp);
                            set_node_loc(expression, node.loc());
                        }
                        return expression;
                    }
                }
            }
        }

        if node.operator_token().kind() == SyntaxKind::InKeyword
            && is_private_identifier(node.left())
        {
            // #x in obj
            return self.transform_private_identifier_in_in_expression(node);
        }

        self.visit_each_child(node)
    }

    // Go: transformers/estransforms/classfields.go:1614 classFieldsTransformer.visitParenthesizedExpression
    pub(super) fn visit_parenthesized_expression(&mut self, node: Node, discarded: bool) -> Node {
        // 8.4.5 RS: NamedEvaluation
        //   ParenthesizedExpression : `(` Expression `)`
        //     ...
        //     2. Return ? NamedEvaluation of |Expression| with argument _name_.
        if discarded {
            let expression = self.discarded_value_visitor_visit_node(node.expression());
            return self
                .ec()
                .factory()
                .update_parenthesized_expression(node, expression);
        }
        let expression = self.visit_node(node.expression());
        self.ec()
            .factory()
            .update_parenthesized_expression(node, expression)
    }
}

// Go: transformers/estransforms/classfields.go:3395 isStaticPropertyDeclarationOrClassStaticBlock
pub(super) fn is_static_property_declaration_or_class_static_block(node: Node) -> bool {
    is_class_static_block_declaration(node)
        || (is_property_declaration(node) && has_static_modifier(node))
}

// Go: transformers/estransforms/classfields.go:3547 flattenCommaList
/// flattenCommaList decomposes a comma expression tree into a sequence of expressions.
// PORT: Go returns an `iter.Seq`; every caller consumes it fully, so this
// returns the sequence as a `Vec`.
pub(super) fn flatten_comma_list(node: Node) -> Vec<Node> {
    let mut result = Vec::new();
    flatten_comma_list_worker(node, &mut |n| {
        result.push(n);
        true
    });
    result
}

// Go: transformers/estransforms/classfields.go:3553 flattenCommaListWorker
fn flatten_comma_list_worker(node: Node, yield_: &mut dyn FnMut(Node) -> bool) -> bool {
    if is_parenthesized_expression(node) && node_is_synthesized(node) {
        flatten_comma_list_worker(node.expression(), yield_)
    } else if is_comma_expression(node) {
        flatten_comma_list_worker(node.left(), yield_)
            && flatten_comma_list_worker(node.right(), yield_)
    } else {
        yield_(node)
    }
}

// Go: transformers/estransforms/classfields.go:3564 findComputedPropertyNameCacheAssignment
pub(super) fn find_computed_property_name_cache_assignment(
    _emit_context: &EmitContext,
    name: Node,
) -> Node {
    let mut node = name.expression();
    loop {
        node = skip_outer_expressions(node, OuterExpressionKinds(0));
        if is_binary_expression(node) && node.operator_token().kind() == SyntaxKind::CommaToken {
            node = node.right();
            continue;
        }
        if is_assignment_expression(node, true /*excludeCompoundAssignment*/)
            && is_identifier(node.left())
        {
            return node;
        }
        break;
    }
    Node::NIL
}

// Go: transformers/estransforms/classfields.go:3580 expandPreOrPostfixIncrementOrDecrementExpression
pub(super) fn expand_pre_or_postfix_increment_or_decrement_expression(
    factory: &crate::printer::factory::NodeFactory,
    emit_context: &EmitContext,
    node: Node,
    expression: Node,
    result_variable: Node,
) -> Node {
    let operator = node.operator();
    let operand = node.operand();

    let temp = factory.new_temp_variable();
    emit_context.add_variable_declaration(temp);
    let mut expression = factory.new_assignment_expression(temp, expression);
    set_node_loc(expression, operand.loc());

    let mut operation = if is_prefix_unary_expression(node) {
        factory.new_prefix_unary_expression(operator, temp)
    } else {
        factory.new_postfix_unary_expression(temp, operator)
    };
    set_node_loc(operation, node.loc());

    if result_variable.is_some() {
        operation = factory.new_assignment_expression(result_variable, operation);
        set_node_loc(operation, node.loc());
    }

    expression = factory.new_comma_expression(expression, operation);
    set_node_loc(expression, node.loc());

    if is_postfix_unary_expression(node) {
        expression = factory.new_comma_expression(expression, temp);
        set_node_loc(expression, node.loc());
    }

    expression
}

/// Go `printer.AutoGenerateOptions{Flags: printer.GeneratedIdentifierFlagsReservedInNestedScopes}`.
pub(super) fn reserved_in_nested_scopes() -> AutoGenerateOptions {
    AutoGenerateOptions {
        flags: GeneratedIdentifierFlags::RESERVED_IN_NESTED_SCOPES,
        ..Default::default()
    }
}

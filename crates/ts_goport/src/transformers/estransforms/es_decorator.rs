//! Port of Go `transformers/estransforms/esdecorator.go` lines 1 to 1720:
//! the transformer state, its constructor, the lexical scope stack, the root
//! visitors, `transformClassLike` and the class element visitors. The rest is
//! in `es_decorator_p2.rs`.
//!
//! PORT: Go keeps twelve `*ast.NodeVisitor` fields. They are built on demand
//! with `TxVisitors::with_visitor`. Go shares `*classInfo` between the scope
//! stack and `classInfoStack` and mutates it in place; it is
//! `Rc<RefCell<ClassInfo>>`. The scope stack is a `Box` linked list.

// Class/Decorator evaluation order, as it pertains to this transformer:
//
// 1. Class decorators are evaluated outside of the private name scope of the class.
//    - 15.8.20 RS: BindingClassDeclarationEvaluation
//    - 15.8.21 RS: Evaluation
//    - 8.3.5 RS: NamedEvaluation
// 2. ClassHeritage clause is evaluated outside of the private name scope of the class.
//    - 15.8.19 RS: ClassDefinitionEvaluation, Step 8.c.
// 3. The name of the class is assigned.
// 4. For each member:
//    a. Member Decorators are evaluated.
//       - 15.8.19 RS: ClassDefinitionEvaluation, Step 23.
//       - Probably 15.7.13 RS: ClassElementEvaluation, but it's missing from spec text.
//    b. Computed Property name is evaluated
//       - 15.8.19 RS: ClassDefinitionEvaluation, Step 23.
//       - 15.8.15 RS: ClassFieldDefinitionEvaluation, Step 1.
//       - 15.4.5 RS: MethodDefinitionEvaluation, Step 1.
// 5. Static non-field (method/getter/setter/auto-accessor) element decorators are applied
// 6. Non-static non-field (method/getter/setter/auto-accessor) element decorators are applied
// 7. Static field (excl. auto-accessor) element decorators are applied
// 8. Non-static field (excl. auto-accessor) element decorators are applied
// 9. Class decorators are applied
// 10. Class binding is initialized
// 11. Static method extra initializers are evaluated
// 12. Static fields are initialized (incl. extra initializers) and static blocks are evaluated
// 13. Class extra initializers are evaluated
//
// Class constructor evaluation order, as it pertains to this transformer:
//
// 1. Instance method extra initializers are evaluated
// 2. For each instance field/auto-accessor:
//    a. The field is initialized and defined on the instance.
//    b. Extra initializers for the field are evaluated.

use super::class_fields::find_computed_property_name_cache_assignment;
use super::class_this::is_class_this_assignment_block;
use super::contract::{TransformOptions, TransformerBox};
use super::named_evaluation::{
    class_has_declared_or_explicitly_assigned_name,
    inject_class_named_evaluation_helper_block_if_missing, is_class_named_evaluation_helper_block,
    is_named_evaluation_and, transform_named_evaluation,
};
use super::utilities::{TxVisitors, create_accessor_property_backing_field, impl_es_transformer};
use crate::prelude::*;
use crate::printer::factory::AssignedNameOptions;
use crate::printer::{AutoGenerateOptions, EmitContext, EmitFlags, GeneratedIdentifierFlags};
use crate::transformers::utilities::{
    find_super_statement_index_path, is_generated_identifier, is_simple_inlineable_expression,
    move_range_past_decorators, single_or_many,
};

// Go: transformers/estransforms/esdecorator.go:48 lexicalEntryKind
/// lexicalEntryKind discriminates the kind of lexical scope entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LexicalEntryKind {
    Class,
    ClassElement,
    Name,
    Other,
}

// Go: transformers/estransforms/esdecorator.go:59 lexicalEntry
/// lexicalEntry represents a single entry in the lexical scope stack used to track
/// nested class declarations and their state during transformation.
pub(super) struct LexicalEntry {
    pub(super) kind: LexicalEntryKind,
    pub(super) next: Option<Box<LexicalEntry>>,
    pub(super) class_info_data: Option<ClassInfoRef>,
    pub(super) saved_pending_expressions: Vec<Node>,
    pub(super) class_this_data: Node,
    pub(super) class_super_data: Node,
    pub(super) depth: i32,
}

impl LexicalEntry {
    fn new(kind: LexicalEntryKind, next: Option<Box<LexicalEntry>>) -> Self {
        Self {
            kind,
            next,
            class_info_data: None,
            saved_pending_expressions: Vec::new(),
            class_this_data: Node::NIL,
            class_super_data: Node::NIL,
            depth: 0,
        }
    }
}

// Go: transformers/estransforms/esdecorator.go:70 memberInfo
/// memberInfo stores decoration-related data for a single class element.
#[derive(Clone, Copy, Default)]
pub(super) struct MemberInfo {
    pub(super) member_decorators_name: Node, // used in class definition step 4.a
    pub(super) member_initializers_name: Node, // used in class definition step 12 and constructor evaluation step 2.a
    pub(super) member_extra_initializers_name: Node, // used in class definition step 12 and constructor evaluation step 2.b
    pub(super) member_descriptor_name: Node,
}

// Go: transformers/estransforms/esdecorator.go:78 classInfo
/// classInfo stores all transformation data for a single decorated class.
#[derive(Default)]
pub(super) struct ClassInfo {
    pub(super) class: Node,
    pub(super) class_decorators_name: Node, // used in class definition step 2
    pub(super) class_descriptor_name: Node, // used in class definition step 10
    pub(super) class_extra_initializers_name: Node, // used in class definition step 13
    pub(super) class_this: Node,            // `_classThis`, if needed.
    pub(super) class_super: Node,           // `_classSuper`, if needed.
    pub(super) metadata_reference: Node,
    pub(super) member_infos: IndexMap<Node, MemberInfo>, // used in class definition step 4.a, 12, and constructor evaluation
    pub(super) instance_method_extra_initializers_name: Node, // used in constructor evaluation step 1
    pub(super) static_method_extra_initializers_name: Node,   // used in class definition step 11
    pub(super) static_non_field_decoration_statements: Vec<Node>,
    pub(super) non_static_non_field_decoration_statements: Vec<Node>,
    pub(super) static_field_decoration_statements: Vec<Node>,
    pub(super) non_static_field_decoration_statements: Vec<Node>,
    pub(super) has_static_initializers: bool,
    pub(super) has_non_ambient_instance_fields: bool,
    pub(super) has_static_private_class_elements: bool,
    pub(super) pending_static_initializers: Vec<Node>,
    pub(super) pending_instance_initializers: Vec<Node>,
}

/// Go `*classInfo`.
pub(super) type ClassInfoRef = Rc<RefCell<ClassInfo>>;

// Go: transformers/estransforms/esdecorator.go:100 esDecoratorTransformer
pub struct EsDecoratorTransformer {
    pub(super) emit_context: Rc<EmitContext>,
    pub(super) compiler_options: &'static CompilerOptions,
    pub(super) top: Option<Box<LexicalEntry>>,
    pub(super) class_info_stack: Option<ClassInfoRef>,
    pub(super) class_this: Node,
    pub(super) class_super: Node,
    pub(super) pending_expressions: Vec<Node>,
    pub(super) outer_this: Node,
    pub(super) should_transform_private_static_elements_in_file: bool,
}

impl_es_transformer!(EsDecoratorTransformer);

// Go: transformers/estransforms/esdecorator.go:124 newESDecoratorTransformer
pub fn new_es_decorator_transformer(opts: &TransformOptions) -> Option<TransformerBox> {
    // When experimentalDecorators is set, the legacy decorator transformer handles all
    // decorators. When targeting ESNext with useDefineForClassFields, there's nothing to
    // transform. In either case every node would be returned unchanged, so skip entirely.
    if opts.compiler_options.experimental_decorators.is_true()
        || (opts.compiler_options.get_emit_script_target() >= ScriptTarget::ES_NEXT
            && opts.compiler_options.get_use_define_for_class_fields())
    {
        return None;
    }
    Some(Box::new(EsDecoratorTransformer {
        emit_context: opts.context.clone(),
        compiler_options: opts.compiler_options,
        top: None,
        class_info_stack: None,
        class_this: Node::NIL,
        class_super: Node::NIL,
        pending_expressions: Vec::new(),
        outer_this: Node::NIL,
        should_transform_private_static_elements_in_file: false,
    }))
}

/// `createDescriptorFunc`.
pub(super) type CreateDescriptorFunc = fn(&mut EsDecoratorTransformer, Node, ModifierList) -> Node;

// Go: transformers/estransforms/esdecorator.go:1232 partialResult
#[derive(Clone, Copy)]
pub(super) struct PartialResult {
    pub(super) modifiers: ModifierList,
    pub(super) referenced_name: Node,
    pub(super) name: Node,
    pub(super) initializers_name: Node,
    pub(super) extra_initializers_name: Node,
    pub(super) descriptor_name: Node,
    pub(super) this_arg: Node,
}

impl Default for PartialResult {
    fn default() -> Self {
        Self {
            modifiers: ModifierList::NIL,
            referenced_name: Node::NIL,
            name: Node::NIL,
            initializers_name: Node::NIL,
            extra_initializers_name: Node::NIL,
            descriptor_name: Node::NIL,
            this_arg: Node::NIL,
        }
    }
}

/// Go `printer.AutoGenerateOptions{Flags: GeneratedIdentifierFlagsOptimistic | GeneratedIdentifierFlagsFileLevel}`.
pub(super) fn optimistic_file_level() -> AutoGenerateOptions {
    AutoGenerateOptions {
        flags: GeneratedIdentifierFlags::OPTIMISTIC | GeneratedIdentifierFlags::FILE_LEVEL,
        ..Default::default()
    }
}

impl EsDecoratorTransformer {
    // Go: transformers/estransforms/esdecorator.go:165 esDecoratorTransformer.updateState
    pub(super) fn update_state(&mut self) {
        self.class_info_stack = None;
        self.class_this = Node::NIL;
        self.class_super = Node::NIL;
        let Some(top) = self.top.as_ref() else {
            return;
        };
        match top.kind {
            LexicalEntryKind::Class => {
                self.class_info_stack = top.class_info_data.clone();
            }
            LexicalEntryKind::ClassElement => {
                self.class_info_stack = top
                    .next
                    .as_ref()
                    .and_then(|next| next.class_info_data.clone());
                self.class_this = top.class_this_data;
                self.class_super = top.class_super_data;
            }
            LexicalEntryKind::Name => {
                let grandparent = top
                    .next
                    .as_ref()
                    .and_then(|n| n.next.as_ref())
                    .and_then(|n| n.next.as_ref());
                if let Some(grandparent) = grandparent
                    && grandparent.kind == LexicalEntryKind::ClassElement
                {
                    self.class_info_stack = grandparent
                        .next
                        .as_ref()
                        .and_then(|next| next.class_info_data.clone());
                    self.class_this = grandparent.class_this_data;
                    self.class_super = grandparent.class_super_data;
                }
            }
            LexicalEntryKind::Other => {}
        }
    }

    // Go: transformers/estransforms/esdecorator.go:189 esDecoratorTransformer.enterClass
    pub(super) fn enter_class(&mut self, ci: Option<ClassInfoRef>) {
        let mut entry = LexicalEntry::new(LexicalEntryKind::Class, self.top.take());
        entry.class_info_data = ci;
        entry.saved_pending_expressions = std::mem::take(&mut self.pending_expressions);
        self.top = Some(Box::new(entry));
        self.update_state();
    }

    // Go: transformers/estransforms/esdecorator.go:200 esDecoratorTransformer.exitClass
    pub(super) fn exit_class(&mut self) {
        // Go evaluates the message args first: a nil `tx.top` is a nil dereference.
        // Go loads `tx.top.kind` (offset 0) for the assert message: addr=0x0
        // (esdecorator.go:201).
        let top = self
            .top
            .take()
            .unwrap_or_else(|| crate::core::go_nil_dereference_at(0x0));
        go_assert!(
            top.kind == LexicalEntryKind::Class,
            "Incorrect value for top.kind. Expected top.kind to be 'class' but got '{}' instead.",
            top.kind as i32
        );
        let top = *top;
        self.pending_expressions = top.saved_pending_expressions;
        self.top = top.next;
        self.update_state();
    }

    // Go: transformers/estransforms/esdecorator.go:207 esDecoratorTransformer.enterClassElement
    pub(super) fn enter_class_element(&mut self, node: Node) {
        let top_kind = self
            .top
            .as_ref()
            // Go loads `tx.top.kind` (offset 0) for the assert message: addr=0x0
            // (esdecorator.go:208).
            .unwrap_or_else(|| crate::core::go_nil_dereference_at(0x0))
            .kind;
        go_assert!(
            top_kind == LexicalEntryKind::Class,
            "Incorrect value for top.kind. Expected top.kind to be 'class' but got '{}' instead.",
            top_kind as i32
        );
        let mut entry = LexicalEntry::new(LexicalEntryKind::ClassElement, self.top.take());
        if is_class_static_block_declaration(node)
            || is_property_declaration(node) && has_static_modifier(node)
        {
            if let Some(ci) = entry.next.as_ref().and_then(|n| n.class_info_data.clone()) {
                let ci = ci.borrow();
                entry.class_this_data = ci.class_this;
                entry.class_super_data = ci.class_super;
            }
        }
        self.top = Some(Box::new(entry));
        self.update_state();
    }

    // Go: transformers/estransforms/esdecorator.go:222 esDecoratorTransformer.exitClassElement
    pub(super) fn exit_class_element(&mut self) {
        // Go loads `tx.top.kind` (offset 0) for the assert message: addr=0x0
        // (esdecorator.go:223).
        let top = self
            .top
            .take()
            .unwrap_or_else(|| crate::core::go_nil_dereference_at(0x0));
        go_assert!(
            top.kind == LexicalEntryKind::ClassElement,
            "Incorrect value for top.kind. Expected top.kind to be 'class-element' but got '{}' instead.",
            top.kind as i32
        );
        let next_kind = top
            .next
            .as_ref()
            // Go loads `tx.top.next.kind` (offset 0) for the assert message: addr=0x0
            // (esdecorator.go:224).
            .unwrap_or_else(|| crate::core::go_nil_dereference_at(0x0))
            .kind;
        go_assert!(
            next_kind == LexicalEntryKind::Class,
            "Incorrect value for top.next.kind. Expected top.next.kind to be 'class' but got '{}' instead.",
            next_kind as i32
        );
        self.top = top.next;
        self.update_state();
    }

    // Go: transformers/estransforms/esdecorator.go:229 esDecoratorTransformer.enterName
    pub(super) fn enter_name(&mut self) {
        let top_kind = self
            .top
            .as_ref()
            // Go loads `tx.top.kind` (offset 0) for the assert message: addr=0x0
            // (esdecorator.go:230).
            .unwrap_or_else(|| crate::core::go_nil_dereference_at(0x0))
            .kind;
        go_assert!(
            top_kind == LexicalEntryKind::ClassElement,
            "Incorrect value for top.kind. Expected top.kind to be 'class-element' but got '{}' instead.",
            top_kind as i32
        );
        let entry = LexicalEntry::new(LexicalEntryKind::Name, self.top.take());
        self.top = Some(Box::new(entry));
        self.update_state();
    }

    // Go: transformers/estransforms/esdecorator.go:238 esDecoratorTransformer.exitName
    pub(super) fn exit_name(&mut self) {
        // Go loads `tx.top.kind` (offset 0) for the assert message: addr=0x0
        // (esdecorator.go:239).
        let top = self
            .top
            .take()
            .unwrap_or_else(|| crate::core::go_nil_dereference_at(0x0));
        go_assert!(
            top.kind == LexicalEntryKind::Name,
            "Incorrect value for top.kind. Expected top.kind to be 'name' but got '{}' instead.",
            top.kind as i32
        );
        self.top = top.next;
        self.update_state();
    }

    // Go: transformers/estransforms/esdecorator.go:244 esDecoratorTransformer.enterOther
    pub(super) fn enter_other(&mut self) {
        if let Some(top) = self.top.as_mut()
            && top.kind == LexicalEntryKind::Other
        {
            go_assert!(self.pending_expressions.is_empty());
            top.depth += 1;
        } else {
            let mut entry = LexicalEntry::new(LexicalEntryKind::Other, self.top.take());
            entry.saved_pending_expressions = std::mem::take(&mut self.pending_expressions);
            self.top = Some(Box::new(entry));
            self.update_state();
        }
    }

    // Go: transformers/estransforms/esdecorator.go:259 esDecoratorTransformer.exitOther
    pub(super) fn exit_other(&mut self) {
        let top_kind = self
            .top
            .as_ref()
            // Go loads `tx.top.kind` (offset 0) for the assert message: addr=0x0
            // (esdecorator.go:260).
            .unwrap_or_else(|| crate::core::go_nil_dereference_at(0x0))
            .kind;
        go_assert!(
            top_kind == LexicalEntryKind::Other,
            "Incorrect value for top.kind. Expected top.kind to be 'other' but got '{}' instead.",
            top_kind as i32
        );
        let top = self.top.as_mut().expect("top is nil");
        if top.depth > 0 {
            go_assert!(self.pending_expressions.is_empty());
            top.depth -= 1;
        } else {
            let top = *self.top.take().expect("top is nil");
            self.pending_expressions = top.saved_pending_expressions;
            self.top = top.next;
            self.update_state();
        }
    }

    // Go: transformers/estransforms/esdecorator.go:271 esDecoratorTransformer.visitSourceFile
    pub(super) fn visit_source_file(&mut self, node: Node) -> Node {
        let ec = self.ec();
        self.top = None;
        self.should_transform_private_static_elements_in_file = false;
        let visited = self.visit_each_child(node);
        ec.add_emit_helper(visited, &ec.read_emit_helpers());
        if self.should_transform_private_static_elements_in_file {
            ec.add_emit_flags(visited, EmitFlags::TRANSFORM_PRIVATE_STATIC_ELEMENTS);
            self.should_transform_private_static_elements_in_file = false;
        }
        visited
    }

    // Go: transformers/estransforms/esdecorator.go:283 esDecoratorTransformer.outerThisVisit
    pub(super) fn outer_this_visit(&mut self, n: Node) -> Node {
        if !n
            .subtree_facts()
            .intersects(SubtreeFacts::SUBTREE_CONTAINS_LEXICAL_THIS)
            && n.kind() != SyntaxKind::ThisKeyword
        {
            return n;
        }
        if n.kind() == SyntaxKind::ThisKeyword {
            if self.outer_this.is_nil() {
                self.outer_this = self.ec().factory().new_unique_name_ex(
                    "_outerThis",
                    AutoGenerateOptions {
                        flags: GeneratedIdentifierFlags::OPTIMISTIC,
                        ..Default::default()
                    },
                );
            }
            return self.outer_this;
        }
        self.with_visitor(Self::outer_this_visit, |v| v.visit_each_child(n))
    }

    // Go: transformers/estransforms/esdecorator.go:298 esDecoratorTransformer.shouldVisitNode
    pub(super) fn should_visit_node(&self, node: Node) -> bool {
        node.subtree_facts()
            .intersects(SubtreeFacts::SUBTREE_CONTAINS_DECORATORS)
            || (self.class_this.is_some()
                && node
                    .subtree_facts()
                    .intersects(SubtreeFacts::SUBTREE_CONTAINS_LEXICAL_THIS))
            || (self.class_this.is_some()
                && self.class_super.is_some()
                && node
                    .subtree_facts()
                    .intersects(SubtreeFacts::SUBTREE_CONTAINS_LEXICAL_SUPER))
    }

    // Go: transformers/estransforms/esdecorator.go:304 esDecoratorTransformer.visit
    pub(super) fn visit(&mut self, node: Node) -> Node {
        if node.kind() == SyntaxKind::SourceFile {
            return self.visit_source_file(node);
        }
        if !self.should_visit_node(node) {
            return node;
        }
        match node.kind() {
            // Decorators are elided. In Strada, a separate `modifierVisitor` drops decorators
            // before they reach `visitor` via visitEachChild. Here, `visit` serves as both
            // visitors, so decorators from modifier lists reach it directly.
            SyntaxKind::Decorator => Node::NIL,
            SyntaxKind::ClassDeclaration => self.visit_class_declaration(node),
            SyntaxKind::ClassExpression => self.visit_class_expression(node),
            SyntaxKind::Constructor
            | SyntaxKind::PropertyDeclaration
            | SyntaxKind::ClassStaticBlockDeclaration => crate::gostd::debug::fail(
                "Not supported outside of a class. Use 'classElementVisitor' instead.",
            ),
            SyntaxKind::Parameter => self.visit_parameter_declaration(node),
            // Support NamedEvaluation to ensure the correct class name for class expressions.
            SyntaxKind::BinaryExpression => {
                self.visit_binary_expression(node, false /*discarded*/)
            }
            SyntaxKind::PropertyAssignment
            | SyntaxKind::VariableDeclaration
            | SyntaxKind::BindingElement => {
                self.visit_named_evaluation_site(node, node.initializer())
            }
            SyntaxKind::ExportAssignment => self.visit_export_assignment(node),
            SyntaxKind::ThisKeyword => self.visit_this_expression(node),
            SyntaxKind::ForStatement => self.visit_for_statement(node),
            SyntaxKind::ExpressionStatement => self.visit_expression_statement(node),
            SyntaxKind::ParenthesizedExpression => {
                self.visit_parenthesized_expression(node, false /*discarded*/)
            }
            SyntaxKind::PartiallyEmittedExpression => {
                self.visit_partially_emitted_expression(node, false /*discarded*/)
            }
            SyntaxKind::CallExpression => self.visit_call_expression(node),
            SyntaxKind::TaggedTemplateExpression => self.visit_tagged_template_expression(node),
            SyntaxKind::PrefixUnaryExpression | SyntaxKind::PostfixUnaryExpression => {
                self.visit_pre_or_postfix_unary_expression(node, false /*discarded*/)
            }
            SyntaxKind::PropertyAccessExpression => self.visit_property_access_expression(node),
            SyntaxKind::ElementAccessExpression => self.visit_element_access_expression(node),
            SyntaxKind::ComputedPropertyName => self.visit_computed_property_name(node),
            SyntaxKind::MethodDeclaration
            | SyntaxKind::SetAccessor
            | SyntaxKind::GetAccessor
            | SyntaxKind::FunctionExpression
            | SyntaxKind::FunctionDeclaration => {
                self.enter_other();
                let result = self.visit_each_child(node);
                self.exit_other();
                result
            }
            _ => self.visit_each_child(node),
        }
    }

    // Go: transformers/estransforms/esdecorator.go:369 esDecoratorTransformer.modifierVisitorVisit
    pub(super) fn modifier_visitor_visit(&mut self, node: Node) -> Node {
        if node.kind() == SyntaxKind::Decorator {
            return Node::NIL;
        }
        node
    }

    /// Go `tx.modifierVisitor.VisitModifiers(modifiers)`.
    pub(super) fn modifier_visitor_visit_modifiers(
        &mut self,
        modifiers: ModifierList,
    ) -> ModifierList {
        self.with_visitor(Self::modifier_visitor_visit, |v| {
            v.visit_modifiers(modifiers)
        })
    }

    // Go: transformers/estransforms/esdecorator.go:376 esDecoratorTransformer.classElementVisitorVisit
    pub(super) fn class_element_visitor_visit(&mut self, node: Node) -> Node {
        match node.kind() {
            SyntaxKind::Constructor => self.visit_constructor_declaration(node),
            SyntaxKind::MethodDeclaration => self.visit_method_declaration(node),
            SyntaxKind::GetAccessor => self.visit_get_accessor_declaration(node),
            SyntaxKind::SetAccessor => self.visit_set_accessor_declaration(node),
            SyntaxKind::PropertyDeclaration => self.visit_property_declaration(node),
            SyntaxKind::ClassStaticBlockDeclaration => {
                self.visit_class_static_block_declaration(node)
            }
            _ => self.visit(node),
        }
    }

    // Go: transformers/estransforms/esdecorator.go:395 esDecoratorTransformer.discardedValueVisit
    pub(super) fn discarded_value_visit(&mut self, node: Node) -> Node {
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
            SyntaxKind::PartiallyEmittedExpression => {
                self.visit_partially_emitted_expression(node, true /*discarded*/)
            }
            _ => self.visit(node),
        }
    }

    /// Go `tx.discardedVisitor.VisitNode(node)`.
    pub(super) fn discarded_visitor_visit_node(&mut self, node: Node) -> Node {
        self.with_visitor(Self::discarded_value_visit, |v| v.visit_node(node))
    }

    // Go: transformers/estransforms/esdecorator.go:410 esDecoratorTransformer.nonConstructorClassElementVisit
    pub(super) fn non_constructor_class_element_visit(&mut self, node: Node) -> Node {
        if is_constructor_declaration(node) {
            return node; // skip constructors in pass 1
        }
        self.class_element_visitor_visit(node)
    }

    // Go: transformers/estransforms/esdecorator.go:417 esDecoratorTransformer.constructorClassElementVisit
    pub(super) fn constructor_class_element_visit(&mut self, node: Node) -> Node {
        if is_constructor_declaration(node) {
            return self.class_element_visitor_visit(node);
        }
        node
    }

    // Go: transformers/estransforms/esdecorator.go:424 esDecoratorTransformer.exportStrippingModifierVisit
    pub(super) fn export_stripping_modifier_visit(&mut self, node: Node) -> Node {
        if node.kind() == SyntaxKind::ExportKeyword {
            return Node::NIL;
        }
        self.modifier_visitor_visit(node)
    }

    /// Go `tx.staticOnlyModifierVisitor` callback.
    pub(super) fn static_only_modifier_visit(&mut self, node: Node) -> Node {
        if node.kind() == SyntaxKind::StaticKeyword {
            return node;
        }
        Node::NIL
    }

    /// Go `tx.asyncOnlyModifierVisitor` callback.
    pub(super) fn async_only_modifier_visit(&mut self, node: Node) -> Node {
        if node.kind() == SyntaxKind::AsyncKeyword {
            return node;
        }
        Node::NIL
    }

    /// Go `tx.accessorStrippingModifierVisitor` callback.
    pub(super) fn accessor_stripping_modifier_visit(&mut self, node: Node) -> Node {
        if node.kind() == SyntaxKind::AccessorKeyword {
            return Node::NIL;
        }
        node
    }

    // Go: transformers/estransforms/esdecorator.go:464 esDecoratorTransformer.createHelperVariable
    pub(super) fn create_helper_variable(&self, node: Node, suffix: &str) -> Node {
        let ec = &self.emit_context;
        ec.factory().new_unique_name_ex(
            &format!("{}_{}", get_helper_variable_name(ec, node), suffix),
            AutoGenerateOptions {
                flags: GeneratedIdentifierFlags::OPTIMISTIC
                    | GeneratedIdentifierFlags::RESERVED_IN_NESTED_SCOPES,
                ..Default::default()
            },
        )
    }

    // Go: transformers/estransforms/esdecorator.go:471 esDecoratorTransformer.createLet
    pub(super) fn create_let(&self, name: Node, initializer: Node) -> Node {
        let f = self.emit_context.factory();
        f.new_variable_statement(
            ModifierList::NIL,
            f.new_variable_declaration_list(
                f.new_node_list(&[f.new_variable_declaration(
                    name,
                    Node::NIL,
                    Node::NIL,
                    initializer,
                )]),
                NodeFlags::LET,
            ),
        )
    }

    // Go: transformers/estransforms/esdecorator.go:483 esDecoratorTransformer.createClassInfo
    pub(super) fn create_class_info(&self, node: Node) -> ClassInfo {
        let ec = &self.emit_context;
        let f = ec.factory();
        let mut ci = ClassInfo {
            class: node,
            metadata_reference: f.new_unique_name_ex("_metadata", optimistic_file_level()),
            ..Default::default()
        };

        // Before visiting we perform a first pass to collect information we'll need
        // as we descend.

        // If the class itself is decorated, create a _classThis binding
        if node_is_decorated(false, node, Node::NIL, Node::NIL) {
            let needs_unique_class_this = node.members().iter().any(|member| {
                (is_private_identifier_class_element_declaration(member)
                    || is_auto_accessor_property_declaration(member))
                    && has_static_modifier(member)
            });
            // We do not mark _classThis as FileLevel if it may be reused by class private fields, which requires the
            // ability access the captured `_classThis` of outer scopes.
            let mut flags =
                GeneratedIdentifierFlags::OPTIMISTIC | GeneratedIdentifierFlags::FILE_LEVEL;
            if needs_unique_class_this {
                flags = GeneratedIdentifierFlags::OPTIMISTIC
                    | GeneratedIdentifierFlags::RESERVED_IN_NESTED_SCOPES;
            }
            ci.class_this = f.new_unique_name_ex(
                "_classThis",
                AutoGenerateOptions {
                    flags,
                    ..Default::default()
                },
            );
        }

        for member in node.members().iter() {
            if is_method_or_accessor(member)
                && node_or_child_is_decorated(false, member, node, Node::NIL)
            {
                if has_static_modifier(member) {
                    if ci.static_method_extra_initializers_name.is_nil() {
                        ci.static_method_extra_initializers_name = f.new_unique_name_ex(
                            "_staticExtraInitializers",
                            optimistic_file_level(),
                        );
                        let renamed_class_this = if ci.class_this.is_some() {
                            ci.class_this
                        } else {
                            f.new_this_expression()
                        };
                        let initializer = f.new_run_initializers_helper(
                            renamed_class_this,
                            ci.static_method_extra_initializers_name,
                            Node::NIL,
                        );
                        let name_range = node.name();
                        if name_range.is_some() {
                            ec.set_source_map_range(initializer, name_range.loc());
                        } else {
                            ec.set_source_map_range(initializer, move_range_past_decorators(node));
                        }
                        ci.pending_static_initializers.push(initializer);
                    }
                } else if ci.instance_method_extra_initializers_name.is_nil() {
                    ci.instance_method_extra_initializers_name =
                        f.new_unique_name_ex("_instanceExtraInitializers", optimistic_file_level());
                    let initializer = f.new_run_initializers_helper(
                        f.new_this_expression(),
                        ci.instance_method_extra_initializers_name,
                        Node::NIL,
                    );
                    let name_range = node.name();
                    if name_range.is_some() {
                        ec.set_source_map_range(initializer, name_range.loc());
                    } else {
                        ec.set_source_map_range(initializer, move_range_past_decorators(node));
                    }
                    ci.pending_instance_initializers.push(initializer);
                }
            }

            if is_class_static_block_declaration(member) {
                if !is_class_named_evaluation_helper_block(ec, member) {
                    ci.has_static_initializers = true;
                }
            } else if is_property_declaration(member) {
                if has_static_modifier(member) {
                    ci.has_static_initializers = ci.has_static_initializers
                        || member.initializer().is_some()
                        || has_decorators(member);
                } else {
                    ci.has_non_ambient_instance_fields = ci.has_non_ambient_instance_fields
                        || !has_syntactic_modifier(member, ModifierFlags::AMBIENT);
                }
            }

            if (is_private_identifier_class_element_declaration(member)
                || is_auto_accessor_property_declaration(member))
                && has_static_modifier(member)
            {
                ci.has_static_private_class_elements = true;
            }

            // exit early if possible
            if ci.static_method_extra_initializers_name.is_some()
                && ci.instance_method_extra_initializers_name.is_some()
                && ci.has_static_initializers
                && ci.has_non_ambient_instance_fields
                && ci.has_static_private_class_elements
            {
                break;
            }
        }

        ci
    }

    // Go: transformers/estransforms/esdecorator.go:577 esDecoratorTransformer.transformClassLike
    pub(super) fn transform_class_like(&mut self, mut node: Node) -> Node {
        let ec = self.ec();
        let f = ec.factory();

        ec.start_variable_environment();

        // When a class has class decorators we end up transforming it into a statement that would otherwise give it an
        // assigned name. If the class doesn't have an assigned name, we'll give it an assigned name of `""`.
        if !class_has_declared_or_explicitly_assigned_name(&ec, node)
            && class_or_constructor_parameter_is_decorated(false, node)
        {
            node = inject_class_named_evaluation_helper_block_if_missing(
                &ec,
                node,
                f.new_string_literal("", TokenFlags::NONE),
                Node::NIL,
            );
        }

        let class_reference = f.get_local_name_ex(node, AssignedNameOptions::default());
        let ci = Rc::new(RefCell::new(self.create_class_info(node)));
        let mut class_definition_statements: Vec<Node> = Vec::new();
        let mut leading_block_statements: Vec<Node> = Vec::new();
        let mut trailing_block_statements: Vec<Node> = Vec::new();
        let mut synthetic_constructor = Node::NIL;
        let mut heritage_clauses = NodeList::NIL;
        let mut should_transform_private_static_elements_in_class = false;

        // 1. Class decorators are evaluated outside the private name scope of the class.
        //
        // - Since class decorators don't have privileged access to private names defined inside the class,
        //   they must be evaluated outside of the class body.
        // - Since a class decorator can replace the class constructor, we must define a variable to keep track
        //   of the mutated class.
        // - Since a class decorator can add extra initializers, we must define a variable to keep track of
        //   extra initializers.
        let class_decorators =
            self.transform_all_decorators_of_declaration(&node.decorators().to_vec());
        if !class_decorators.is_empty() {
            let mut c = ci.borrow_mut();
            go_assert!(c.class_this.is_some());

            c.class_decorators_name =
                f.new_unique_name_ex("_classDecorators", optimistic_file_level());
            c.class_descriptor_name =
                f.new_unique_name_ex("_classDescriptor", optimistic_file_level());
            c.class_extra_initializers_name =
                f.new_unique_name_ex("_classExtraInitializers", optimistic_file_level());

            let decorators_array =
                f.new_array_literal_expression(f.new_node_list(&class_decorators), false);
            class_definition_statements
                .push(self.create_let(c.class_decorators_name, decorators_array));
            class_definition_statements.push(self.create_let(c.class_descriptor_name, Node::NIL));
            class_definition_statements.push(self.create_let(
                c.class_extra_initializers_name,
                f.new_array_literal_expression(f.new_node_list(&[]), false),
            ));
            class_definition_statements.push(self.create_let(c.class_this, Node::NIL));

            if !class_decorators.is_empty() && c.has_static_private_class_elements {
                should_transform_private_static_elements_in_class = true;
                self.should_transform_private_static_elements_in_file = true;
            }
        }

        // 2. ClassHeritage clause is evaluated outside of the private name scope of the class.
        let extends_clause = get_heritage_clause(node, SyntaxKind::ExtendsKeyword);
        let mut extends_element = Node::NIL;
        if extends_clause.is_some() {
            let types = extends_clause.types();
            if types.is_some() && !types.nodes().is_empty() {
                extends_element = types.nodes().get(0);
            }
        }
        let mut extends_expression = Node::NIL;
        if extends_element.is_some() {
            extends_expression = self.visit_node(extends_element.expression());
        }

        if extends_expression.is_some() {
            // Rewrite `super` in static initializers so that we can use the correct `this`.
            let class_super = f.new_unique_name_ex("_classSuper", optimistic_file_level());
            ci.borrow_mut().class_super = class_super;

            // Ensure we do not give the class or function an assigned name due to the variable by prefixing it
            // with `0, `.
            let unwrapped =
                skip_outer_expressions(extends_expression, OuterExpressionKinds::OEK_ALL);
            let mut safe_extends_expression = extends_expression;
            if (is_class_expression(unwrapped) && unwrapped.name().is_nil())
                || (is_function_expression(unwrapped) && unwrapped.name().is_nil())
                || is_arrow_function(unwrapped)
            {
                safe_extends_expression = f.new_comma_expression(
                    f.new_numeric_literal("0", TokenFlags::NONE),
                    extends_expression,
                );
            }
            class_definition_statements.push(self.create_let(class_super, safe_extends_expression));

            let updated_extends_element = f.update_expression_with_type_arguments(
                extends_element,
                class_super,
                NodeList::NIL,
            );
            let updated_extends_clause = f.update_heritage_clause(
                extends_clause,
                extends_clause.token(),
                f.new_node_list(&[updated_extends_element]),
            );
            heritage_clauses = f.new_node_list(&[updated_extends_clause]);
        }

        let class_this = ci.borrow().class_this;
        let renamed_class_this = if class_this.is_some() {
            class_this
        } else {
            f.new_this_expression()
        };

        // 3. The name of the class is assigned.
        //
        // If the class did not have a name, the caller should have performed injectClassNamedEvaluationHelperBlockIfMissing
        // prior to calling this function if a name was needed.

        // 4. For each member:
        //    a. Member Decorators are evaluated
        //    b. Computed Property Name is evaluated, if present
        //
        // We visit members in two passes:
        // - The first pass visits methods, accessors, and fields to collect decorators and computed property names.
        // - The second pass visits the constructor to add instance initializers.
        //
        // NOTE: If there are no constructors, but there are instance initializers, a synthetic constructor is added.
        self.enter_class(Some(ci.clone()));

        let (metadata_reference, class_super) = {
            let c = ci.borrow();
            (c.metadata_reference, c.class_super)
        };
        leading_block_statements.push(self.create_metadata(metadata_reference, class_super));

        // Since the constructor can appear anywhere in the class body and its transform depends on other class elements,
        // we must first visit all non-constructor members, then visit the constructor, all while maintaining document order.
        let members = self.with_visitor(Self::non_constructor_class_element_visit, |v| {
            v.visit_nodes(node.member_list())
        });
        let mut members = self.with_visitor(Self::constructor_class_element_visit, |v| {
            v.visit_nodes(members)
        });

        // Handle pending expressions (computed property names and decorator evaluations)
        if !self.pending_expressions.is_empty() {
            // If a pending expression contains a lexical `this`, we'll need to capture the lexical `this` of the
            // container and transform it in the expression. This ensures we use the correct `this` in the resulting
            // class `static` block. We don't use substitution here because the size of the tree we are visiting
            // is likely to be small and doesn't justify the complexity of introducing substitution.
            self.outer_this = Node::NIL;
            for mut expr in self.pending_expressions.clone() {
                // If a pending expression contains lexical `this`, capture it
                if expr
                    .subtree_facts()
                    .intersects(SubtreeFacts::SUBTREE_CONTAINS_LEXICAL_THIS)
                {
                    expr = self.with_visitor(Self::outer_this_visit, |v| v.visit_node(expr));
                }
                let statement = f.new_expression_statement(expr);
                leading_block_statements.push(statement);
            }
            if self.outer_this.is_some() {
                class_definition_statements
                    .insert(0, self.create_let(self.outer_this, f.new_this_expression()));
            }
            self.pending_expressions = Vec::new();
        }
        self.exit_class();

        // If there are instance initializers but no constructor, synthesize one
        if !ci.borrow().pending_instance_initializers.is_empty()
            && get_first_constructor_with_body(node).is_nil()
        {
            let initializer_statements = self.prepare_constructor(&ci);
            if !initializer_statements.is_empty() {
                let is_derived_class = extends_element.is_some()
                    && skip_outer_expressions(
                        extends_element.expression(),
                        OuterExpressionKinds::OEK_ALL,
                    )
                    .kind()
                        != SyntaxKind::NullKeyword;
                let mut constructor_statements: Vec<Node> = Vec::new();
                if is_derived_class {
                    let spread_arguments = f.new_spread_element(f.new_identifier("arguments"));
                    let super_call = f.new_call_expression(
                        f.new_keyword_expression(SyntaxKind::SuperKeyword),
                        Node::NIL,
                        NodeList::NIL,
                        f.new_node_list(&[spread_arguments]),
                        NodeFlags::NONE,
                    );
                    constructor_statements.push(f.new_expression_statement(super_call));
                }
                constructor_statements.extend(initializer_statements);
                let constructor_body = f.new_block(f.new_node_list(&constructor_statements), true);
                synthetic_constructor = f.new_constructor_declaration(
                    ModifierList::NIL,
                    NodeList::NIL,
                    f.new_node_list(&[]),
                    Node::NIL,
                    Node::NIL,
                    constructor_body,
                );
            }
        }

        {
            let c = ci.borrow();
            // Used in class definition steps 5,7,11
            if c.static_method_extra_initializers_name.is_some() {
                class_definition_statements.push(self.create_let(
                    c.static_method_extra_initializers_name,
                    f.new_array_literal_expression(f.new_node_list(&[]), false),
                ));
            }

            // Used in class definition steps 6,8, and construction
            if c.instance_method_extra_initializers_name.is_some() {
                class_definition_statements.push(self.create_let(
                    c.instance_method_extra_initializers_name,
                    f.new_array_literal_expression(f.new_node_list(&[]), false),
                ));
            }

            // Used in class definition steps 7, 8, 12, and construction.
            // Emit member info variable declarations; the reference implementation emits static member vars first, then non-static.
            if !c.member_infos.is_empty() {
                class_definition_statements
                    .extend(self.emit_member_info_declarations(&c, true /*isStatic*/));
                class_definition_statements
                    .extend(self.emit_member_info_declarations(&c, false /*isStatic*/));
            }

            // 5. Static non-field element decorators are applied
            leading_block_statements.extend_from_slice(&c.static_non_field_decoration_statements);

            // 6. Non-static non-field element decorators are applied
            leading_block_statements
                .extend_from_slice(&c.non_static_non_field_decoration_statements);

            // 7. Static field element decorators are applied
            leading_block_statements.extend_from_slice(&c.static_field_decoration_statements);

            // 8. Non-static field element decorators are applied
            leading_block_statements.extend_from_slice(&c.non_static_field_decoration_statements);

            // 9. Class decorators are applied
            // 10. Class binding is initialized
            //
            // produces:
            //   __esDecorate(null, _classDescriptor = { value: this }, _classDecorators, { kind: "class", name: this.name, metadata }, null, _classExtraInitializers);
            if c.class_descriptor_name.is_some()
                && c.class_decorators_name.is_some()
                && c.class_extra_initializers_name.is_some()
                && c.class_this.is_some()
            {
                let value_property = f.new_property_assignment(
                    ModifierList::NIL,
                    f.new_identifier("value"),
                    Node::NIL,
                    Node::NIL,
                    renamed_class_this,
                );
                let class_descriptor =
                    f.new_object_literal_expression(f.new_node_list(&[value_property]), false);
                let class_descriptor_assignment =
                    f.new_assignment_expression(c.class_descriptor_name, class_descriptor);
                let class_name_reference = f.new_property_access_expression(
                    renamed_class_this,
                    Node::NIL,
                    f.new_identifier("name"),
                    NodeFlags::NONE,
                );

                let context_obj = f.new_es_decorate_class_context_object(
                    class_name_reference,
                    c.metadata_reference,
                );

                let es_decorate_helper = f.new_es_decorate_helper(
                    f.new_token(SyntaxKind::NullKeyword),
                    class_descriptor_assignment,
                    c.class_decorators_name,
                    context_obj,
                    f.new_token(SyntaxKind::NullKeyword),
                    c.class_extra_initializers_name,
                );
                let es_decorate_statement = f.new_expression_statement(es_decorate_helper);
                ec.set_source_map_range(es_decorate_statement, move_range_past_decorators(node));
                leading_block_statements.push(es_decorate_statement);

                // produces:
                //   C = _classThis = _classDescriptor.value;
                let class_descriptor_value_ref = f.new_property_access_expression(
                    c.class_descriptor_name,
                    Node::NIL,
                    f.new_identifier("value"),
                    NodeFlags::NONE,
                );
                let class_this_assignment =
                    f.new_assignment_expression(c.class_this, class_descriptor_value_ref);
                let class_reference_assignment =
                    f.new_assignment_expression(class_reference, class_this_assignment);
                leading_block_statements
                    .push(f.new_expression_statement(class_reference_assignment));
            }
        }

        // produces:
        //   if (metadata) Object.defineProperty(C, Symbol.metadata, { configurable: true, writable: true, value: metadata });
        leading_block_statements
            .push(self.create_symbol_metadata(renamed_class_this, metadata_reference));

        // 11. Static extra initializers
        // 12. Static fields are initialized
        let pending_static_initializers =
            std::mem::take(&mut ci.borrow_mut().pending_static_initializers);
        if !pending_static_initializers.is_empty() {
            for initializer in pending_static_initializers {
                let initializer_statement = f.new_expression_statement(initializer);
                ec.set_source_map_range(initializer_statement, ec.source_map_range(initializer));
                trailing_block_statements.push(initializer_statement);
            }
        }

        // 13. Class extra initializers
        let class_extra_initializers_name = ci.borrow().class_extra_initializers_name;
        if class_extra_initializers_name.is_some() {
            let run_class_initializers_helper = f.new_run_initializers_helper(
                renamed_class_this,
                class_extra_initializers_name,
                Node::NIL,
            );
            let run_class_initializers_statement =
                f.new_expression_statement(run_class_initializers_helper);
            if node.name().is_some() {
                ec.set_source_map_range(run_class_initializers_statement, node.name().loc());
            } else {
                ec.set_source_map_range(
                    run_class_initializers_statement,
                    move_range_past_decorators(node),
                );
            }
            trailing_block_statements.push(run_class_initializers_statement);
        }

        // If there are no other static initializers to run, combine the leading and trailing block statements
        if !leading_block_statements.is_empty()
            && !trailing_block_statements.is_empty()
            && !ci.borrow().has_static_initializers
        {
            leading_block_statements.append(&mut trailing_block_statements);
        }

        // prepare a leading `static {}` block, if necessary
        //
        // produces:
        //   class C {
        //       static { ... }
        //       ...
        //   }
        let mut leading_static_block = Node::NIL;
        if !leading_block_statements.is_empty() {
            leading_static_block = f.new_class_static_block_declaration(
                ModifierList::NIL,
                f.new_block(f.new_node_list(&leading_block_statements), true),
            );
        }

        if leading_static_block.is_some() && should_transform_private_static_elements_in_class {
            // We use EFTransformPrivateStaticElements as a marker on a class static block
            // to inform the classFields transform that it shouldn't rename `this` to `_classThis` in the
            // transformed class static block.
            ec.set_emit_flags(
                leading_static_block,
                EmitFlags::TRANSFORM_PRIVATE_STATIC_ELEMENTS,
            );
        }

        // prepare a trailing `static {}` block, if necessary
        //
        // produces:
        //   class C {
        //       ...
        //       static { ... }
        //   }
        let mut trailing_static_block = Node::NIL;
        if !trailing_block_statements.is_empty() {
            trailing_static_block = f.new_class_static_block_declaration(
                ModifierList::NIL,
                f.new_block(f.new_node_list(&trailing_block_statements), true),
            );
        }

        // Assemble new members list
        if leading_static_block.is_some()
            || synthetic_constructor.is_some()
            || trailing_static_block.is_some()
        {
            let member_nodes = members.nodes().to_vec();
            let mut new_members: Vec<Node> = Vec::with_capacity(member_nodes.len() + 3);

            // Find the existing NamedEvaluation helper block index
            // PORT: Go uses -1 for "not found" and slices at index + 1.
            let split = member_nodes
                .iter()
                .position(|&m| is_class_named_evaluation_helper_block(&ec, m))
                .map_or(0, |i| i + 1);

            // add the leading `static {}` block
            if leading_static_block.is_some() {
                // add the `static {}` block after any existing NamedEvaluation helper block, if one exists.
                new_members.extend_from_slice(&member_nodes[..split]);
                new_members.push(leading_static_block);
                new_members.extend_from_slice(&member_nodes[split..]);
            } else {
                new_members.extend_from_slice(&member_nodes);
            }

            // append the synthetic constructor, if necessary
            if synthetic_constructor.is_some() {
                new_members.push(synthetic_constructor);
            }

            // append a trailing `static {}` block, if necessary
            if trailing_static_block.is_some() {
                new_members.push(trailing_static_block);
            }

            members = f.new_node_list_with_loc(&new_members, members.loc());
        }

        let lexical_environment = ec.end_variable_environment();

        let class_expression;
        if !class_decorators.is_empty() {
            let mut ce = f.new_class_expression(
                ModifierList::NIL,
                Node::NIL,
                NodeList::NIL,
                heritage_clauses,
                members,
            );
            ec.set_original(ce, node);
            if class_this.is_some() {
                ce = inject_class_this_assignment_if_missing(&ec, f, ce, class_this);
            }
            class_expression = ce;

            // We use `var` instead of `let` so we can leverage NamedEvaluation to define the class name
            // and still be able to ensure it is initialized prior to any use in `static {}`.

            // produces:
            //   (() => {
            //       let _classDecorators = [...];
            //       let _classDescriptor;
            //       let _classExtraInitializers = [];
            //       let _classThis;
            //       ...
            //       var C = class {
            //           static {
            //               __esDecorate(null, _classDescriptor = { value: this }, _classDecorators, ...);
            //               C = _classThis = _classDescriptor.value;
            //           }
            //           static x = 1;
            //           static y = C.x; // `C` will already be defined here.
            //           static { ... }
            //       };
            //       return C;
            //   })();

            let class_reference_declaration =
                f.new_variable_declaration(class_reference, Node::NIL, Node::NIL, class_expression);
            let class_reference_var_decl_list = f.new_variable_declaration_list(
                f.new_node_list(&[class_reference_declaration]),
                NodeFlags::NONE,
            );
            let return_expr = if class_this.is_some() {
                f.new_assignment_expression(class_reference, class_this)
            } else {
                class_reference
            };
            class_definition_statements
                .push(f.new_variable_statement(ModifierList::NIL, class_reference_var_decl_list));
            class_definition_statements.push(f.new_return_statement(return_expr));
        } else {
            // produces:
            //   return <classExpression>;
            class_expression = f.new_class_expression(
                ModifierList::NIL,
                node.name(),
                NodeList::NIL,
                heritage_clauses,
                members,
            );
            ec.set_original(class_expression, node);
            class_definition_statements.push(f.new_return_statement(class_expression));
        }

        if should_transform_private_static_elements_in_class {
            ec.add_emit_flags(
                class_expression,
                EmitFlags::TRANSFORM_PRIVATE_STATIC_ELEMENTS,
            );
            for member in class_expression.members().iter() {
                if (is_private_identifier_class_element_declaration(member)
                    || is_auto_accessor_property_declaration(member))
                    && has_static_modifier(member)
                {
                    ec.add_emit_flags(member, EmitFlags::TRANSFORM_PRIVATE_STATIC_ELEMENTS);
                }
            }
        }

        let merged_statements =
            ec.merge_environment(&class_definition_statements, &lexical_environment);
        f.new_immediately_invoked_arrow_function(&merged_statements)
    }

    // Go: transformers/estransforms/esdecorator.go:993 esDecoratorTransformer.emitMemberInfoDeclarations
    /// Generates let declarations for member decorator info variables, filtered by static/non-static.
    pub(super) fn emit_member_info_declarations(
        &self,
        ci: &ClassInfo,
        is_static_: bool,
    ) -> Vec<Node> {
        let f = self.emit_context.factory();
        let mut stmts: Vec<Node> = Vec::new();
        for (&member, mi) in &ci.member_infos {
            if is_static(member) != is_static_ {
                continue;
            }
            stmts.push(self.create_let(mi.member_decorators_name, Node::NIL));
            if mi.member_initializers_name.is_some() {
                stmts.push(self.create_let(
                    mi.member_initializers_name,
                    f.new_array_literal_expression(f.new_node_list(&[]), false),
                ));
            }
            if mi.member_extra_initializers_name.is_some() {
                stmts.push(self.create_let(
                    mi.member_extra_initializers_name,
                    f.new_array_literal_expression(f.new_node_list(&[]), false),
                ));
            }
            if mi.member_descriptor_name.is_some() {
                stmts.push(self.create_let(mi.member_descriptor_name, Node::NIL));
            }
        }
        stmts
    }

    // Go: transformers/estransforms/esdecorator.go:1019 esDecoratorTransformer.visitClassDeclaration
    pub(super) fn visit_class_declaration(&mut self, node: Node) -> Node {
        if is_decorated_class_like(node) {
            let ec = self.ec();
            let f = ec.factory();
            let mut statements: Vec<Node> = Vec::new();

            let mut original_class = ec.most_original(node);
            if !is_class_like(original_class) {
                original_class = node;
            }
            let class_name = if original_class.name().is_some() {
                f.new_string_literal_from_node(original_class.name())
            } else {
                f.new_string_literal("default", TokenFlags::NONE)
            };

            let is_export = has_syntactic_modifier(node, ModifierFlags::EXPORT);
            let is_default = has_syntactic_modifier(node, ModifierFlags::DEFAULT);

            let mut class_node = node;
            if node.name().is_nil() {
                class_node = inject_class_named_evaluation_helper_block_if_missing(
                    &ec,
                    class_node,
                    class_name,
                    Node::NIL,
                );
            }

            if is_export && is_default {
                let iife = self.transform_class_like(class_node);
                if class_node.name().is_some() {
                    // produces:
                    //   let C = (() => { ... })();
                    //   export default C;
                    let var_decl = f.new_variable_declaration(
                        f.get_local_name(class_node),
                        Node::NIL,
                        Node::NIL,
                        iife,
                    );
                    ec.set_original(var_decl, class_node);
                    let var_decls = f.new_variable_declaration_list(
                        f.new_node_list(&[var_decl]),
                        NodeFlags::LET,
                    );
                    let var_statement = f.new_variable_statement(ModifierList::NIL, var_decls);
                    statements.push(var_statement);

                    let export_statement = f.new_export_default(f.get_declaration_name(class_node));
                    ec.set_original(export_statement, class_node);
                    ec.assign_comment_range(export_statement, class_node);
                    ec.set_source_map_range(
                        export_statement,
                        move_range_past_decorators(class_node),
                    );
                    statements.push(export_statement);
                } else {
                    // produces:
                    //   export default (() => { ... })();
                    let export_statement = f.new_export_default(iife);
                    ec.set_original(export_statement, class_node);
                    ec.assign_comment_range(export_statement, class_node);
                    ec.set_source_map_range(
                        export_statement,
                        move_range_past_decorators(class_node),
                    );
                    statements.push(export_statement);
                }
            } else {
                go_assert!(
                    class_node.name().is_some(),
                    "A class declaration that is not a default export must have a name."
                );
                // produces:
                //   let C = (() => { ... })();
                let iife = self.transform_class_like(class_node);
                let modifiers = self.with_visitor(Self::export_stripping_modifier_visit, |v| {
                    v.visit_modifiers(class_node.modifiers())
                });

                let decl_name = f.get_local_name_ex(
                    class_node,
                    AssignedNameOptions {
                        allow_source_maps: true,
                        ..Default::default()
                    },
                );
                let var_decl = f.new_variable_declaration(decl_name, Node::NIL, Node::NIL, iife);
                ec.set_original(var_decl, class_node);
                let var_decls =
                    f.new_variable_declaration_list(f.new_node_list(&[var_decl]), NodeFlags::LET);
                let var_statement = f.new_variable_statement(modifiers, var_decls);
                ec.set_original(var_statement, class_node);
                ec.assign_comment_range(var_statement, class_node);
                statements.push(var_statement);

                if is_export {
                    // produces:
                    //   export { C };
                    let export_statement = f.new_external_module_export(decl_name);
                    ec.set_original(export_statement, class_node);
                    statements.push(export_statement);
                }
            }

            return single_or_many(Some(&statements), f);
        }

        // Non-decorated class
        let modifiers = self.modifier_visitor_visit_modifiers(node.modifiers());
        let heritage_clauses = self.visit_nodes(node.heritage_clauses());
        self.enter_class(None);
        let members = self.with_visitor(Self::class_element_visitor_visit, |v| {
            v.visit_nodes(node.member_list())
        });
        self.exit_class();
        self.ec().factory().update_class_declaration(
            node,
            modifiers,
            node.name(),
            NodeList::NIL,
            heritage_clauses,
            members,
        )
    }

    // Go: transformers/estransforms/esdecorator.go:1107 esDecoratorTransformer.visitClassExpression
    pub(super) fn visit_class_expression(&mut self, node: Node) -> Node {
        if is_decorated_class_like(node) {
            let iife = self.transform_class_like(node);
            self.ec().set_original(iife, node);
            return iife;
        }

        let modifiers = self.modifier_visitor_visit_modifiers(node.modifiers());
        let heritage_clauses = self.visit_nodes(node.heritage_clauses());
        self.enter_class(None);
        let members = self.with_visitor(Self::class_element_visitor_visit, |v| {
            v.visit_nodes(node.member_list())
        });
        self.exit_class();
        self.ec().factory().update_class_expression(
            node,
            modifiers,
            node.name(),
            NodeList::NIL,
            heritage_clauses,
            members,
        )
    }

    // Go: transformers/estransforms/esdecorator.go:1122 esDecoratorTransformer.prepareConstructor
    pub(super) fn prepare_constructor(&self, ci: &ClassInfoRef) -> Vec<Node> {
        // Decorated instance members can add "extra" initializers to the instance. If a class contains any instance
        // fields, we'll inject the `__runInitializers()` call for these extra initializers into the initializer of
        // the first class member that will be initialized. However, if the class does not contain any fields that
        // we can piggyback on, we need to synthesize a `__runInitializers()` call in the constructor instead.
        let pending = std::mem::take(&mut ci.borrow_mut().pending_instance_initializers);
        if pending.is_empty() {
            return Vec::new();
        }
        let f = self.emit_context.factory();
        vec![f.new_expression_statement(f.inline_expressions(&pending))]
    }

    // Go: transformers/estransforms/esdecorator.go:1138 esDecoratorTransformer.transformConstructorBodyWorker
    pub(super) fn transform_constructor_body_worker(
        &mut self,
        mut statements_out: Vec<Node>,
        statements_in: &[Node],
        statement_offset: usize,
        super_path: &[usize],
        super_path_depth: usize,
        initializer_statements: &[Node],
    ) -> Vec<Node> {
        let ec = self.ec();
        let f = ec.factory();
        let super_statement_index = super_path[super_path_depth];
        // Visit statements before super
        if super_statement_index > statement_offset {
            for &s in &statements_in[statement_offset..super_statement_index] {
                statements_out.push(self.visit_node(s));
            }
        }

        let super_statement = statements_in[super_statement_index];
        if is_try_statement(super_statement) {
            // Recurse into try block
            let try_block_node = super_statement.try_block();
            let try_block_statements = self.transform_constructor_body_worker(
                Vec::new(),
                &try_block_node.statements().to_vec(),
                0,
                super_path,
                super_path_depth + 1,
                initializer_statements,
            );

            let new_try_block = f.new_block(f.new_node_list(&try_block_statements), true);
            // Use the original try block's range even though the statements may differ due to
            // injected initializer statements. This preserves source map fidelity for the enclosing
            // try statement.
            set_node_loc(new_try_block, try_block_node.loc());

            let mut catch_clause = Node::NIL;
            if super_statement.catch_clause().is_some() {
                catch_clause = self.visit_node(super_statement.catch_clause());
            }
            let mut finally_block = Node::NIL;
            if super_statement.finally_block().is_some() {
                finally_block = self.visit_node(super_statement.finally_block());
            }
            let updated =
                f.update_try_statement(super_statement, new_try_block, catch_clause, finally_block);
            statements_out.push(updated);
        } else {
            statements_out.push(self.visit_node(super_statement));
            statements_out.extend_from_slice(initializer_statements);
        }

        // Visit statements after super
        if super_statement_index + 1 < statements_in.len() {
            for &s in &statements_in[super_statement_index + 1..] {
                statements_out.push(self.visit_node(s));
            }
        }
        statements_out
    }

    // Go: transformers/estransforms/esdecorator.go:1184 esDecoratorTransformer.visitConstructorDeclaration
    pub(super) fn visit_constructor_declaration(&mut self, node: Node) -> Node {
        self.enter_class_element(node);
        let ec = self.ec();
        let f = ec.factory();
        let modifiers = self.modifier_visitor_visit_modifiers(node.modifiers());
        let parameters = self.visit_nodes(node.parameter_list());

        let mut body = Node::NIL;
        let ctor_body = node.body();
        if ctor_body.is_some()
            && let Some(ci) = self.class_info_stack.clone()
        {
            // If there are instance extra initializers we need to add them to the body along with any
            // field initializers
            let initializer_statements = self.prepare_constructor(&ci);
            if !initializer_statements.is_empty() {
                let mut stmts: Vec<Node> = Vec::new();
                let body_statements = ctor_body.statements().to_vec();
                let (prologue, rest) = f.split_standard_prologue(&body_statements);
                stmts.extend_from_slice(prologue);

                let super_statement_indices = find_super_statement_index_path(rest, 0);
                if !super_statement_indices.is_empty() {
                    stmts = self.transform_constructor_body_worker(
                        stmts,
                        rest,
                        0,
                        &super_statement_indices,
                        0,
                        &initializer_statements,
                    );
                } else {
                    stmts.extend(initializer_statements);
                    let (visited, _) = self.visit_slice(rest);
                    stmts.extend(visited);
                }

                body = f.new_block(f.new_node_list(&stmts), true);
                ec.set_original(body, ctor_body);
                set_node_loc(body, ctor_body.loc());
            }
        }

        if body.is_nil() {
            body = self.visit_node(ctor_body);
        }
        self.exit_class_element();
        f.update_constructor_declaration(
            node,
            modifiers,
            NodeList::NIL,
            parameters,
            Node::NIL,
            Node::NIL,
            body,
        )
    }

    // Go: transformers/estransforms/esdecorator.go:1222 esDecoratorTransformer.finishClassElement
    pub(super) fn finish_class_element(&self, updated: Node, original: Node) -> Node {
        if updated != original {
            // While we emit the source map for the node after skipping decorators and modifiers,
            // we need to emit the comments for the original range.
            self.emit_context.assign_comment_range(updated, original);
            self.emit_context
                .set_source_map_range(updated, move_range_past_decorators(original));
        }
        updated
    }

    // Go: transformers/estransforms/esdecorator.go:1244 esDecoratorTransformer.partialTransformClassElement
    pub(super) fn partial_transform_class_element(
        &mut self,
        member: Node,
        ci: Option<ClassInfoRef>,
        create_descriptor: Option<CreateDescriptorFunc>,
    ) -> PartialResult {
        let ec = self.ec();
        let f = ec.factory();

        let Some(ci) = ci else {
            let modifiers = self.modifier_visitor_visit_modifiers(member.modifiers());
            self.enter_name();
            let name = self.visit_property_name(member.name());
            self.exit_name();
            return PartialResult {
                modifiers,
                name,
                ..Default::default()
            };
        };

        // Member decorators require privileged access to private names. However, computed property
        // evaluation occurs interspersed with decorator evaluation. This means that if we encounter
        // a computed property name we must inline decorator evaluation.

        // Collect decorators for this member. Decorator expressions evaluate outside the class body,
        // so `this` should NOT be replaced with `_classThis`.
        let saved_class_this = self.class_this;
        self.class_this = Node::NIL;
        let member_decorators =
            self.transform_all_decorators_of_declaration(&member.decorators().to_vec());
        self.class_this = saved_class_this;
        let modifiers = self.modifier_visitor_visit_modifiers(member.modifiers());

        let mut result = PartialResult {
            modifiers,
            ..Default::default()
        };

        if !member_decorators.is_empty() {
            let member_decorators_name = self.create_helper_variable(member, "decorators");
            let member_decorators_array =
                f.new_array_literal_expression(f.new_node_list(&member_decorators), false);
            let member_decorators_assignment =
                f.new_assignment_expression(member_decorators_name, member_decorators_array);
            ci.borrow_mut().member_infos.insert(
                member,
                MemberInfo {
                    member_decorators_name,
                    ..Default::default()
                },
            );
            self.pending_expressions.push(member_decorators_assignment);

            // 5. Static non-field (method/getter/setter/auto-accessor) element decorators are applied
            // 6. Non-static non-field (method/getter/setter/auto-accessor) element decorators are applied
            // 7. Static field (excl. auto-accessor) element decorators are applied
            // 8. Non-static field (excl. auto-accessor) element decorators are applied

            // Determine decorator kind
            let kind = if is_get_accessor_declaration(member) {
                "getter"
            } else if is_set_accessor_declaration(member) {
                "setter"
            } else if is_method_declaration(member) {
                "method"
            } else if is_auto_accessor_property_declaration(member) {
                "accessor"
            } else if is_property_declaration(member) {
                "field"
            } else {
                crate::gostd::debug::fail("Unexpected class element kind.")
            };

            // Determine the property name for the context
            let mut property_name_computed = false;
            let mut property_name_expr = Node::NIL;
            let member_name = member.name();
            if member_name.is_some()
                && (is_identifier(member_name) || is_private_identifier(member_name))
            {
                property_name_computed = false;
                property_name_expr = member_name;
            } else if member_name.is_some() && is_property_name_literal(member_name) {
                property_name_computed = true;
                property_name_expr = f.new_string_literal_from_node(member_name);
            } else if member_name.is_some() && is_computed_property_name(member_name) {
                let cpn_expression = member_name.expression();
                if is_property_name_literal(cpn_expression) && !is_identifier(cpn_expression) {
                    property_name_computed = true;
                    property_name_expr = f.new_string_literal_from_node(cpn_expression);
                } else {
                    self.enter_name();
                    let (referenced_name, name) = self.visit_referenced_property_name(member_name);
                    result.referenced_name = referenced_name;
                    result.name = name;
                    self.exit_name();
                    property_name_computed = true;
                    property_name_expr = result.referenced_name;
                }
            }

            let metadata_reference = ci.borrow().metadata_reference;
            let context_obj = f.new_es_decorate_class_element_context_object(
                kind,
                property_name_computed,
                property_name_expr,
                is_static(member),
                member_name.is_some() && is_private_identifier(member_name),
                // 15.7.3 CreateDecoratorAccessObject (kind, name)
                // 2. If _kind_ is ~field~, ~method~, ~accessor~, or ~getter~, then ...
                is_property_declaration(member)
                    || is_get_accessor_declaration(member)
                    || is_method_declaration(member),
                // 3. If _kind_ is ~field~, ~accessor~, or ~setter~, then ...
                is_property_declaration(member) || is_set_accessor_declaration(member),
                metadata_reference,
            );

            if is_method_or_accessor(member) {
                // produces (public elements):
                //   __esDecorate(this, null, _static_member_decorators, { kind: "method", name: "...", static: true, private: false, access: { ... } }, _staticExtraInitializers);
                //   __esDecorate(this, null, _member_decorators, { kind: "method", name: "...", static: false, private: false, access: { ... } }, _instanceExtraInitializers);
                //   __esDecorate(this, null, _static_member_decorators, { kind: "getter", name: "...", static: true, private: false, access: { ... } }, _staticExtraInitializers);
                //   __esDecorate(this, null, _member_decorators, { kind: "getter", name: "...", static: false, private: false, access: { ... } }, _instanceExtraInitializers);
                //   __esDecorate(this, null, _static_member_decorators, { kind: "setter", name: "...", static: true, private: false, access: { ... } }, _staticExtraInitializers);
                //   __esDecorate(this, null, _member_decorators, { kind: "setter", name: "...", static: false, private: false, access: { ... } }, _instanceExtraInitializers);
                //
                // produces (private elements):
                //   __esDecorate(this, _static_member_descriptor = { value() { ... } }, _static_member_decorators, { kind: "method", name: "...", static: true, private: true, access: { ... } }, _staticExtraInitializers);
                //   __esDecorate(this, _member_descriptor = { value() { ... } }, _member_decorators, { kind: "method", name: "...", static: false, private: true, access: { ... } }, _instanceExtraInitializers);
                //   __esDecorate(this, _static_member_descriptor = { get() { ... } }, _static_member_decorators, { kind: "getter", name: "...", static: true, private: true, access: { ... } }, _staticExtraInitializers);
                //   __esDecorate(this, _member_descriptor = { get() { ... } }, _member_decorators, { kind: "getter", name: "...", static: false, private: true, access: { ... } }, _instanceExtraInitializers);
                //   __esDecorate(this, _static_member_descriptor = { set() { ... } }, _static_member_decorators, { kind: "setter", name: "...", static: true, private: true, access: { ... } }, _staticExtraInitializers);
                //   __esDecorate(this, _member_descriptor = { set() { ... } }, _member_decorators, { kind: "setter", name: "...", static: false, private: true, access: { ... } }, _instanceExtraInitializers);
                let method_extra_initializers_name = if is_static(member) {
                    ci.borrow().static_method_extra_initializers_name
                } else {
                    ci.borrow().instance_method_extra_initializers_name
                };
                go_assert!(
                    method_extra_initializers_name.is_some(),
                    "methodExtraInitializersName should be defined"
                );

                let descriptor_arg = if let Some(create_descriptor) = create_descriptor
                    && is_private_identifier_class_element_declaration(member)
                {
                    // For private members, extract the method/accessor body into a descriptor object.
                    // Filter modifiers to only keep async.
                    let async_mods = self.with_visitor(Self::async_only_modifier_visit, |v| {
                        v.visit_modifiers(modifiers)
                    });
                    let descriptor = create_descriptor(self, member, async_mods);
                    let member_descriptor_name = self.create_helper_variable(member, "descriptor");
                    if let Some(mi) = ci.borrow_mut().member_infos.get_mut(&member) {
                        mi.member_descriptor_name = member_descriptor_name;
                    }
                    result.descriptor_name = member_descriptor_name;
                    f.new_assignment_expression(member_descriptor_name, descriptor)
                } else {
                    f.new_token(SyntaxKind::NullKeyword)
                };

                let es_decorate_expr = f.new_es_decorate_helper(
                    f.new_this_expression(),
                    descriptor_arg,
                    member_decorators_name,
                    context_obj,
                    f.new_token(SyntaxKind::NullKeyword),
                    method_extra_initializers_name,
                );
                let es_decorate_statement = f.new_expression_statement(es_decorate_expr);
                ec.set_source_map_range(es_decorate_statement, move_range_past_decorators(member));
                self.append_decoration_statement(&ci, member, es_decorate_statement);
            } else if is_property_declaration(member) {
                let member_initializers_name = self.create_helper_variable(member, "initializers");
                let member_extra_initializers_name =
                    self.create_helper_variable(member, "extraInitializers");
                if let Some(mi) = ci.borrow_mut().member_infos.get_mut(&member) {
                    mi.member_initializers_name = member_initializers_name;
                    mi.member_extra_initializers_name = member_extra_initializers_name;
                }
                result.initializers_name = member_initializers_name;
                result.extra_initializers_name = member_extra_initializers_name;
                if is_static(member) {
                    result.this_arg = ci.borrow().class_this;
                }

                let ctor_arg = if is_auto_accessor_property_declaration(member) {
                    f.new_this_expression()
                } else {
                    f.new_token(SyntaxKind::NullKeyword)
                };

                let descriptor_arg = if let Some(create_descriptor) = create_descriptor
                    && is_private_identifier_class_element_declaration(member)
                    && has_accessor_modifier(member)
                {
                    let descriptor = create_descriptor(self, member, ModifierList::NIL);
                    let member_descriptor_name = self.create_helper_variable(member, "descriptor");
                    if let Some(mi) = ci.borrow_mut().member_infos.get_mut(&member) {
                        mi.member_descriptor_name = member_descriptor_name;
                    }
                    result.descriptor_name = member_descriptor_name;
                    f.new_assignment_expression(member_descriptor_name, descriptor)
                } else {
                    f.new_token(SyntaxKind::NullKeyword)
                };

                // produces:
                //   __esDecorate(null, null, _static_member_decorators, { kind: "field", name: "...", static: true, private: ..., access: { ... } }, _staticExtraInitializers);
                //   __esDecorate(null, null, _member_decorators, { kind: "field", name: "...", static: false, private: ..., access: { ... } }, _instanceExtraInitializers);
                let es_decorate_expr = f.new_es_decorate_helper(
                    ctor_arg,
                    descriptor_arg,
                    member_decorators_name,
                    context_obj,
                    member_initializers_name,
                    member_extra_initializers_name,
                );
                let es_decorate_statement = f.new_expression_statement(es_decorate_expr);
                ec.set_source_map_range(es_decorate_statement, move_range_past_decorators(member));
                self.append_decoration_statement(&ci, member, es_decorate_statement);
            }
        }

        if result.name.is_nil() {
            self.enter_name();
            result.name = self.visit_property_name(member.name());
            self.exit_name();
        }

        if (modifiers.is_nil() || modifiers.nodes().is_empty())
            && (is_method_declaration(member) || is_property_declaration(member))
        {
            // Don't emit leading comments on the name for methods and properties without modifiers, otherwise we
            // will end up printing duplicate comments.
            ec.set_emit_flags(result.name, EmitFlags::NO_LEADING_COMMENTS);
        }

        result
    }

    // Go: transformers/estransforms/esdecorator.go:1447 esDecoratorTransformer.appendDecorationStatement
    /// appendDecorationStatement appends an __esDecorate statement to the appropriate
    /// decoration statement list on classInfo based on the member's kind and static-ness.
    pub(super) fn append_decoration_statement(&self, ci: &ClassInfoRef, member: Node, stmt: Node) {
        let mut ci = ci.borrow_mut();
        if is_method_or_accessor(member) || is_auto_accessor_property_declaration(member) {
            if is_static(member) {
                ci.static_non_field_decoration_statements.push(stmt);
            } else {
                ci.non_static_non_field_decoration_statements.push(stmt);
            }
        } else if is_property_declaration(member) && !is_auto_accessor_property_declaration(member)
        {
            if is_static(member) {
                ci.static_field_decoration_statements.push(stmt);
            } else {
                ci.non_static_field_decoration_statements.push(stmt);
            }
        } else {
            crate::gostd::debug::fail("Unexpected class element kind.");
        }
    }

    // Go: transformers/estransforms/esdecorator.go:1465 esDecoratorTransformer.visitMethodDeclaration
    pub(super) fn visit_method_declaration(&mut self, node: Node) -> Node {
        self.enter_class_element(node);
        let result = self.partial_transform_class_element(
            node,
            self.class_info_stack.clone(),
            Some(Self::create_method_descriptor_object),
        );
        if result.descriptor_name.is_some() {
            self.exit_class_element();
            let forwarder = self.create_method_descriptor_forwarder(
                result.modifiers,
                result.name,
                result.descriptor_name,
            );
            return self.finish_class_element(forwarder, node);
        }
        let parameters = self.visit_nodes(node.parameter_list());
        let body = self.visit_node(node.body());
        self.exit_class_element();
        let updated = self.ec().factory().update_method_declaration(
            node,
            result.modifiers,
            node.asterisk_token(),
            result.name,
            Node::NIL,
            NodeList::NIL,
            parameters,
            Node::NIL,
            Node::NIL,
            body,
        );
        self.finish_class_element(updated, node)
    }

    // Go: transformers/estransforms/esdecorator.go:1482 esDecoratorTransformer.visitGetAccessorDeclaration
    pub(super) fn visit_get_accessor_declaration(&mut self, node: Node) -> Node {
        self.enter_class_element(node);
        let result = self.partial_transform_class_element(
            node,
            self.class_info_stack.clone(),
            Some(Self::create_get_accessor_descriptor_object),
        );
        if result.descriptor_name.is_some() {
            self.exit_class_element();
            let forwarder = self.create_get_accessor_descriptor_forwarder(
                result.modifiers,
                result.name,
                result.descriptor_name,
            );
            return self.finish_class_element(forwarder, node);
        }
        let parameters = self.visit_nodes(node.parameter_list());
        let body = self.visit_node(node.body());
        self.exit_class_element();
        let updated = self.ec().factory().update_get_accessor_declaration(
            node,
            result.modifiers,
            result.name,
            NodeList::NIL,
            parameters,
            Node::NIL,
            Node::NIL,
            body,
        );
        self.finish_class_element(updated, node)
    }

    // Go: transformers/estransforms/esdecorator.go:1499 esDecoratorTransformer.visitSetAccessorDeclaration
    pub(super) fn visit_set_accessor_declaration(&mut self, node: Node) -> Node {
        self.enter_class_element(node);
        let result = self.partial_transform_class_element(
            node,
            self.class_info_stack.clone(),
            Some(Self::create_set_accessor_descriptor_object),
        );
        if result.descriptor_name.is_some() {
            self.exit_class_element();
            let forwarder = self.create_set_accessor_descriptor_forwarder(
                result.modifiers,
                result.name,
                result.descriptor_name,
            );
            return self.finish_class_element(forwarder, node);
        }
        let parameters = self.visit_nodes(node.parameter_list());
        let body = self.visit_node(node.body());
        self.exit_class_element();
        let updated = self.ec().factory().update_set_accessor_declaration(
            node,
            result.modifiers,
            result.name,
            NodeList::NIL,
            parameters,
            Node::NIL,
            Node::NIL,
            body,
        );
        self.finish_class_element(updated, node)
    }

    // Go: transformers/estransforms/esdecorator.go:1516 esDecoratorTransformer.visitClassStaticBlockDeclaration
    pub(super) fn visit_class_static_block_declaration(&mut self, node: Node) -> Node {
        self.enter_class_element(node);
        let ec = self.ec();
        let f = ec.factory();

        let mut result;
        if is_class_named_evaluation_helper_block(&ec, node) {
            result = self.visit_each_child(node);
            // Transfer AssignedName metadata to the new node so isClassNamedEvaluationHelperBlock
            // can still find it after visiting (visiting may create a new node when this->_classThis)
            let assigned_name = ec.assigned_name(node);
            if assigned_name.is_some() && result != node {
                ec.set_assigned_name(result, assigned_name);
            }
        } else if is_class_this_assignment_block(&ec, node) {
            let saved_class_this = self.class_this;
            self.class_this = Node::NIL;
            result = self.visit_each_child(node);
            self.class_this = saved_class_this;
        } else {
            // Use a nested variable environment so temp vars generated during static block
            // content transformation (e.g., super access temps) stay scoped to the static block.
            ec.start_variable_environment();
            result = self.visit_each_child(node);
            let var_statements = ec.end_variable_environment();
            if !var_statements.is_empty() {
                // Inject var declarations at the start of the static block's body
                let block_body = result.body();
                let mut new_stmts: Vec<Node> =
                    Vec::with_capacity(var_statements.len() + block_body.statements().len());
                new_stmts.extend_from_slice(&var_statements);
                new_stmts.extend(block_body.statements().iter());
                result = f.new_class_static_block_declaration(
                    ModifierList::NIL,
                    f.new_block(f.new_node_list(&new_stmts), block_body.multi_line()),
                );
            }
            if let Some(ci) = self.class_info_stack.clone() {
                ci.borrow_mut().has_static_initializers = true;
                let pending = std::mem::take(&mut ci.borrow_mut().pending_static_initializers);
                if !pending.is_empty() {
                    // If we tried to inject the pending initializers into the current block, we might run into
                    // variable name collisions due to sharing this blocks scope. To avoid this, we inject a new
                    // static block that contains the pending initializers that precedes this block.
                    let mut stmts: Vec<Node> = Vec::new();
                    for init in pending {
                        let init_stmt = f.new_expression_statement(init);
                        ec.set_source_map_range(init_stmt, ec.source_map_range(init));
                        stmts.push(init_stmt);
                    }
                    let body = f.new_block(f.new_node_list(&stmts), true);
                    let static_block =
                        f.new_class_static_block_declaration(ModifierList::NIL, body);
                    // Return both the new static block and the original
                    self.exit_class_element();
                    return single_or_many(Some(&[static_block, result][..]), f);
                }
            }
        }

        self.exit_class_element();
        result
    }

    // Go: transformers/estransforms/esdecorator.go:1574 esDecoratorTransformer.visitPropertyDeclaration
    pub(super) fn visit_property_declaration(&mut self, mut node: Node) -> Node {
        let ec = self.ec();
        let f = ec.factory();
        if is_named_evaluation_and(
            &ec,
            node,
            Some(&mut |n: Node| is_anonymous_class_needing_assigned_name(n)),
        ) {
            node = transform_named_evaluation(
                &ec,
                node,
                can_ignore_empty_string_literal_in_assigned_name(node.initializer()),
                "",
            );
        }

        self.enter_class_element(node);

        // TODO(rbuckton): We support decorating `declare x` fields with legacyDecorators, but we currently don't
        //                 support them with esDecorators. We need to consider whether we will support them in the
        //                 future, and how. For now, these should be elided by the `ts` transform.
        go_assert!(
            !has_syntactic_modifier(node, ModifierFlags::AMBIENT),
            "Not yet implemented."
        );

        // 10.2.1.3 RS: EvaluateBody
        //   Initializer : `=` AssignmentExpression
        //     ...
        //     3. If IsAnonymousFunctionDefinition(|AssignmentExpression|) is *true*, then
        //        a. Let _value_ be ? NamedEvaluation of |Initializer| with argument _functionObject_.[[ClassFieldInitializerName]].
        //     ...

        let create_descriptor: Option<CreateDescriptorFunc> = if has_accessor_modifier(node) {
            Some(Self::create_accessor_property_descriptor_object)
        } else {
            None
        };
        let result = self.partial_transform_class_element(
            node,
            self.class_info_stack.clone(),
            create_descriptor,
        );

        ec.start_variable_environment();

        let mut initializer = self.visit_node(node.initializer());
        if result.initializers_name.is_some() {
            let this_arg = if result.this_arg.is_some() {
                result.this_arg
            } else {
                f.new_this_expression()
            };
            if initializer.is_nil() {
                initializer = f.new_void_zero_expression();
            }
            initializer =
                f.new_run_initializers_helper(this_arg, result.initializers_name, initializer);
        }

        if is_static(node)
            && initializer.is_some()
            && let Some(ci) = &self.class_info_stack
        {
            ci.borrow_mut().has_static_initializers = true;
        }

        let declarations = ec.end_variable_environment();
        if !declarations.is_empty() {
            let mut stmts: Vec<Node> = Vec::with_capacity(declarations.len() + 1);
            stmts.extend_from_slice(&declarations);
            stmts.push(f.new_return_statement(initializer));
            initializer = f.new_immediately_invoked_arrow_function(&stmts);
        }

        if let Some(ci) = self.class_info_stack.clone() {
            if is_static(node) {
                initializer = self.inject_pending_initializers(&ci, true, initializer);
                if result.extra_initializers_name.is_some() {
                    let class_this = ci.borrow().class_this;
                    let this_arg = if class_this.is_some() {
                        class_this
                    } else {
                        f.new_this_expression()
                    };
                    ci.borrow_mut().pending_static_initializers.push(
                        f.new_run_initializers_helper(
                            this_arg,
                            result.extra_initializers_name,
                            Node::NIL,
                        ),
                    );
                }
            } else {
                initializer = self.inject_pending_initializers(&ci, false, initializer);
                if result.extra_initializers_name.is_some() {
                    ci.borrow_mut().pending_instance_initializers.push(
                        f.new_run_initializers_helper(
                            f.new_this_expression(),
                            result.extra_initializers_name,
                            Node::NIL,
                        ),
                    );
                }
            }
        }

        self.exit_class_element();

        if has_accessor_modifier(node) && result.descriptor_name.is_some() {
            // given:
            //  accessor #x = 1;
            //
            // emits:
            //  static {
            //      _esDecorate(null, _private_x_descriptor = { get() { return this.#x_1; }, set(value) { this.#x_1 = value; } }, ...)
            //  }
            //  ...
            //  #x_1 = 1;
            //  get #x() { return _private_x_descriptor.get.call(this); }
            //  set #x(value) { _private_x_descriptor.set.call(this, value); }

            let comment_range = ec.comment_range(node);
            let source_map_range = ec.source_map_range(node);

            // Since we're creating two declarations where there was previously one, cache
            // the expression for any computed property names.
            let prop_name = node.name();
            let mut getter_name = result.name;
            let mut setter_name = result.name;
            if is_computed_property_name(prop_name)
                && !is_simple_inlineable_expression(prop_name.expression())
            {
                let cache_assignment = find_computed_property_name_cache_assignment(&ec, prop_name);
                if cache_assignment.is_some() {
                    let visited = self.visit_node(prop_name.expression());
                    getter_name = f.update_computed_property_name(prop_name, visited);
                    setter_name =
                        f.update_computed_property_name(prop_name, cache_assignment.left());
                } else {
                    let temp = f.new_temp_variable();
                    ec.set_source_map_range(temp, prop_name.expression().loc());
                    ec.add_variable_declaration(temp);
                    let expression = self.visit_node(prop_name.expression());
                    let assignment = f.new_assignment_expression(temp, expression);
                    ec.set_source_map_range(assignment, prop_name.expression().loc());
                    getter_name = f.update_computed_property_name(prop_name, assignment);
                    setter_name = f.update_computed_property_name(prop_name, temp);
                }
            }

            let modifiers_without_accessor = self
                .with_visitor(Self::accessor_stripping_modifier_visit, |v| {
                    v.visit_modifiers(result.modifiers)
                });

            let backing_field = create_accessor_property_backing_field(
                f,
                node,
                modifiers_without_accessor,
                initializer,
            );
            ec.set_original(backing_field, node);
            ec.set_emit_flags(backing_field, EmitFlags::NO_COMMENTS);
            ec.set_source_map_range(backing_field, source_map_range);
            ec.set_source_map_range(backing_field.name(), ec.source_map_range(node.name()));

            let getter = self.create_get_accessor_descriptor_forwarder(
                modifiers_without_accessor,
                getter_name,
                result.descriptor_name,
            );
            ec.set_original(getter, node);
            ec.set_comment_range(getter, comment_range);
            ec.set_source_map_range(getter, source_map_range);

            let setter = self.create_set_accessor_descriptor_forwarder(
                modifiers_without_accessor,
                setter_name,
                result.descriptor_name,
            );
            ec.set_original(setter, node);
            ec.set_emit_flags(setter, EmitFlags::NO_COMMENTS);
            ec.set_source_map_range(setter, source_map_range);

            return single_or_many(Some(&[backing_field, getter, setter][..]), f);
        }

        let updated = f.update_property_declaration(
            node,
            result.modifiers,
            result.name,
            Node::NIL,
            Node::NIL,
            initializer,
        );
        self.finish_class_element(updated, node)
    }
}

// Go: transformers/estransforms/esdecorator.go:431 getHelperVariableName
pub(super) fn get_helper_variable_name(ec: &EmitContext, node: Node) -> String {
    let name = node.name();
    let mut declaration_name =
        if name.is_some() && is_identifier(name) && !is_generated_identifier(ec, name) {
            name.text().to_string()
        } else if name.is_some() && is_private_identifier(name) && !ec.has_auto_generate_info(name)
        {
            let text = name.text();
            if text.len() > 1 {
                text[1..].to_string()
            } else {
                String::new()
            }
        } else if name.is_some()
            && is_string_literal(name)
            && is_identifier_text(name.text(), LanguageVariant::STANDARD)
        {
            name.text().to_string()
        } else if is_class_like(node) {
            "class".to_string()
        } else {
            "member".to_string()
        };

    if is_get_accessor_declaration(node) {
        declaration_name = format!("get_{declaration_name}");
    }
    if is_set_accessor_declaration(node) {
        declaration_name = format!("set_{declaration_name}");
    }
    if name.is_some() && is_private_identifier(name) {
        declaration_name = format!("private_{declaration_name}");
    }
    if is_static(node) {
        declaration_name = format!("static_{declaration_name}");
    }
    format!("_{declaration_name}")
}

// Go: transformers/estransforms/esdecorator.go:1014 isDecoratedClassLike
pub(super) fn is_decorated_class_like(node: Node) -> bool {
    class_or_constructor_parameter_is_decorated(false, node)
        || child_is_decorated(false, node, Node::NIL)
}

// Go: transformers/estransforms/esdecorator.go:1877 isAnonymousClassNeedingAssignedName
pub(super) fn is_anonymous_class_needing_assigned_name(node: Node) -> bool {
    is_class_expression(node) && node.name().is_nil() && is_decorated_class_like(node)
}

// Go: transformers/estransforms/esdecorator.go:1885 canIgnoreEmptyStringLiteralInAssignedName
/// The IIFE produced for `(@dec class {})` will result in an assigned name of the form
/// `var class_1 = class { };`, and thus the empty string cannot be ignored. However, The IIFE
/// produced for `(class { @dec x; })` will not result in an assigned name since it
/// transforms to `return class { };`, and thus the empty string *can* be ignored.
pub(super) fn can_ignore_empty_string_literal_in_assigned_name(node: Node) -> bool {
    if node.is_nil() {
        return false;
    }
    let inner_expression = skip_outer_expressions(node, OuterExpressionKinds::OEK_ALL);
    is_class_expression(inner_expression)
        && inner_expression.name().is_nil()
        && !class_or_constructor_parameter_is_decorated(false, inner_expression)
}

// Go: transformers/estransforms/esdecorator.go:2719 injectClassThisAssignmentIfMissing
pub(super) fn inject_class_this_assignment_if_missing(
    ec: &EmitContext,
    f: &crate::printer::factory::NodeFactory,
    node: Node,
    class_this: Node,
) -> Node {
    if super::class_fields_p2::class_has_class_this_assignment(ec, node) {
        return node;
    }

    // Create: static { _classThis = this; }
    let expression = f.new_assignment_expression(class_this, f.new_this_expression());
    let statement = f.new_expression_statement(expression);
    let body = f.new_block(f.new_node_list(&[statement]), false);
    let static_block = f.new_class_static_block_declaration(ModifierList::NIL, body);
    ec.set_class_this(static_block, class_this);

    if node.name().is_some() {
        ec.set_source_map_range(statement, node.name().loc());
    }

    let mut new_members: Vec<Node> = Vec::with_capacity(1 + node.members().len());
    new_members.push(static_block);
    new_members.extend(node.members().iter());
    let members_list = f.new_node_list_with_loc(&new_members, node.member_list().loc());

    let updated_node = if is_class_declaration(node) {
        f.update_class_declaration(
            node,
            node.modifiers(),
            node.name(),
            NodeList::NIL,
            node.heritage_clauses(),
            members_list,
        )
    } else {
        f.update_class_expression(
            node,
            node.modifiers(),
            node.name(),
            NodeList::NIL,
            node.heritage_clauses(),
            members_list,
        )
    };
    ec.set_class_this(updated_node, class_this);
    updated_node
}

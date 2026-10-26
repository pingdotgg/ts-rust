//! Port of Go `transformers/transformer.go` plus the shared transformer
//! contract (`TransformOptions`, `TransformerFactory`) from `chain.go`.
//!
//! PORT: Go builds `tx.visitor` once (`transformers/transformer.go:23`) and
//! returns it from `Visitor()` (`transformer.go:31`). Here each transformer
//! implements the `Transformer` trait. `TransformerVisit::with_visitor` builds
//! a `NodeVisitor` for each call with a borrowed callback, so the call does not
//! allocate. The callback reaches the transformer state through `v.ctx`.

use crate::prelude::*;

use crate::ast::visitor::NodeVisitor;
use crate::printer::EmitResolver;

// Go: binder/referenceresolver.go:9 ReferenceResolver
/// Go `binder.ReferenceResolver` as the transformers see it. The checker
/// `EmitResolver` and the binder resolver both implement it.
// PORT: the binder `ReferenceResolver` struct takes a checker on each call.
// Transformers do not hold a checker, so they use this `&self` interface.
pub trait TransformReferenceResolver {
    fn get_referenced_export_container(&self, node: Node, prefix_locals: bool) -> Node;
    fn get_referenced_import_declaration(&self, node: Node) -> Node;
    fn get_referenced_value_declaration(&self, node: Node) -> Node;
    fn get_referenced_value_declarations(&self, node: Node) -> Vec<Node>;
    fn get_element_access_expression_name(&self, expression: Node) -> String;
    fn get_referenced_member_value_declaration(&self, node: Node) -> Node;
}

/// Go `referenceResolver = emitResolver`: an `EmitResolver` used as the
/// transform reference resolver.
// PORT: Go `printer.EmitResolver` embeds `binder.ReferenceResolver`, so the
// assignment is an interface conversion. A Rust trait object cannot convert
// to an unrelated trait object, so this adapter forwards the calls.
pub struct EmitResolverReferenceResolver(pub Rc<dyn EmitResolver>);

impl TransformReferenceResolver for EmitResolverReferenceResolver {
    fn get_referenced_export_container(&self, node: Node, prefix_locals: bool) -> Node {
        self.0.get_referenced_export_container(node, prefix_locals)
    }

    fn get_referenced_import_declaration(&self, node: Node) -> Node {
        self.0.get_referenced_import_declaration(node)
    }

    fn get_referenced_value_declaration(&self, node: Node) -> Node {
        self.0.get_referenced_value_declaration(node)
    }

    fn get_referenced_value_declarations(&self, node: Node) -> Vec<Node> {
        self.0.get_referenced_value_declarations(node)
    }

    fn get_element_access_expression_name(&self, expression: Node) -> String {
        self.0.get_element_access_expression_name(expression)
    }

    fn get_referenced_member_value_declaration(&self, node: Node) -> Node {
        self.0.get_referenced_member_value_declaration(node)
    }
}

// Go: transformers/chain.go:27 TransformOptions
// PORT: Go `GetEmitModuleFormatOfFile func(file ast.HasFileName)` takes the
// source file node here. Go `CaseSensitivity` (ts#64581) is the
// `use_case_sensitive_file_names` flag; its Go zero value
// (`CaseInsensitive`) is `false`.
#[derive(Clone)]
pub struct TransformOptions {
    pub context: Rc<EmitContext>,
    pub compiler_options: &'static CompilerOptions,
    pub resolver: Rc<dyn TransformReferenceResolver>,
    pub emit_resolver: Rc<dyn EmitResolver>,
    pub get_emit_module_format_of_file: Rc<dyn Fn(Node) -> ModuleKind>,
    /// ts#64581: the output extension rewrites of the content mappers.
    pub content_mapper_extension_rewrites: Vec<crate::frontend::tspath::ExtensionRewrite>,
    pub use_case_sensitive_file_names: bool,
}

// Go: transformers/transformer.go:8 Transformer
/// Go `*transformers.Transformer`. Each Go transformer is a struct that
/// implements this trait.
pub trait Transformer {
    // Go: transformers/transformer.go:27 EmitContext
    fn emit_context(&self) -> &Rc<EmitContext>;

    // Go: transformers/transformer.go:39 TransformSourceFile
    fn transform_source_file(&mut self, file: Node) -> Node;
}

/// A constructed transformer (Go `*transformers.Transformer`).
pub type TransformerBox = Box<dyn Transformer>;

// Go: transformers/chain.go:37 TransformerFactory
/// Go `TransformerFactory`. `None` is Go nil.
pub type TransformerFactory = dyn Fn(&TransformOptions) -> Option<TransformerBox>;

/// The Go `Transformer` root visitor for a transformer struct.
///
/// Go builds `tx.visitor` once (`transformers/transformer.go:23`) and returns
/// it from `Visitor()` (`transformer.go:31`). The port builds a `NodeVisitor`
/// for each `with_visitor` call with a borrowed callback, so the call does not
/// allocate. The transformer is the visitor context (`v.ctx`).
/// Go `tx.Visitor().VisitNode(n)`
/// is `self.with_visitor(|v| v.visit_node(n))`, and Go
/// `tx.Visitor().VisitEachChild(n)` is `self.with_visitor(|v| v.visit_each_child(n))`.
// PORT: not in Go. It replaces the `visitor` field of Go `Transformer`.
pub trait TransformerVisit: Sized {
    /// Go `tx.EmitContext()`, cloned so the visitor can borrow it.
    fn emit_context_rc(&self) -> Rc<EmitContext>;

    /// Go `tx.visit`, the callback of the root visitor.
    fn visit(&mut self, node: Node) -> Node;

    /// Runs `f` with Go `tx.Visitor()`.
    fn with_visitor<R>(&mut self, f: impl FnOnce(&mut NodeVisitor<'_, &mut Self>) -> R) -> R {
        let emit_context = self.emit_context_rc();
        let visit = |node: Node, v: &mut NodeVisitor<'_, &mut Self>| v.ctx.visit(node);
        let mut visitor = emit_context.new_node_visitor_ref(&visit, self);
        f(&mut visitor)
    }

    // Go: transformers/transformer.go:39 TransformSourceFile
    /// Go `tx.visitor.VisitSourceFile(file)`.
    fn visit_source_file_root(&mut self, file: Node) -> Node {
        self.with_visitor(|v| v.visit_source_file(file))
    }
}

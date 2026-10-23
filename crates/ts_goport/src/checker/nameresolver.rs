//! Port of `binder/nameresolver.go`.
//!
//! PORT: Go `binder.NameResolver` holds plain Go closures bound to the
//! checker. Here each callback is an `Option<Rc<dyn Fn(&mut Checker, ...)>>`
//! (nil func = `None`), and `resolve` takes the checker explicitly. The
//! resolver's own methods take `&self` so a resolver kept in an `Rc` can be
//! re-entered from its callbacks (Go calls `Resolve` recursively through the
//! checker). The only mutable resolver field, `ArgumentsSymbol`, is a `Cell`.
//!
//! PORT: Go reads symbols through their pointers. Here `resolve` reads them
//! through a `NameResolverHost`: the checker, or the program's binder
//! symbols for the binder reference resolver of JS emit, which runs on
//! threads with no checker (`transformers::reference_resolver`).

use crate::diagnostics::Message;
use crate::prelude::*;
use std::cell::Cell;

// PORT: Go `ast.InternalSymbolNameDefault` ("default"). Kept private here so
// this file does not depend on the constant's Rust name in the ast files.
const INTERNAL_SYMBOL_NAME_DEFAULT: &str = "default";

/// Go `func(location, message, args ...any) *ast.Diagnostic`.
pub type NameResolverErrorFn =
    Rc<dyn Fn(&mut Checker, Node, &'static Message, Vec<String>) -> Diagnostic>;
/// Go `func(node *ast.Node) *ast.Symbol`.
pub type NameResolverGetSymbolOfDeclarationFn = Rc<dyn Fn(&mut Checker, Node) -> SymbolId>;
/// Go `func(symbols ast.SymbolTable, name string, meaning ast.SymbolFlags) *ast.Symbol`.
/// PORT: the name is a `TableKey`. `resolve` passes `TableKey::Name`, so a
/// lookup compares name ids and does not hash or compare the text.
pub type NameResolverLookupFn =
    Rc<dyn Fn(&mut Checker, SymbolTable, TableKey<'_>, SymbolFlags) -> SymbolId>;
/// Go `func(symbol *ast.Symbol, meaning ast.SymbolFlags)`.
pub type NameResolverSymbolReferencedFn = Rc<dyn Fn(&mut Checker, SymbolId, SymbolFlags)>;
/// Go `func(node *ast.Node, value core.Tristate)`.
pub type NameResolverSetRequiresScopeChangeCacheFn = Rc<dyn Fn(&mut Checker, Node, Tristate)>;
/// Go `func(node *ast.Node) core.Tristate`.
pub type NameResolverGetRequiresScopeChangeCacheFn = Rc<dyn Fn(&mut Checker, Node) -> Tristate>;
/// Go `func(location *ast.Node, name string, declaration *ast.Node, result *ast.Symbol) bool`.
pub type NameResolverOnPropertyWithInvalidInitializerFn =
    Rc<dyn Fn(&mut Checker, Node, &str, Node, SymbolId) -> bool>;
/// Go `func(location *ast.Node, name string, meaning ast.SymbolFlags, nameNotFoundMessage *diagnostics.Message)`.
pub type NameResolverOnFailedToResolveSymbolFn =
    Rc<dyn Fn(&mut Checker, Node, &str, SymbolFlags, &'static Message)>;
/// Go `func(location, result, meaning, lastLocation, associatedDeclarationForContainingInitializerOrBindingName, withinDeferredContext)`.
pub type NameResolverOnSuccessfullyResolvedSymbolFn =
    Rc<dyn Fn(&mut Checker, Node, SymbolId, SymbolFlags, Node, Node, bool)>;

/// A non-nil Go `nameNotFoundMessage` of `Resolve`.
// PERF: `CannotFindName(node)` stands for
// `c.get_cannot_find_name_diagnostic_for_name(node)`. `resolve` reads the
// message only when the name is not found, so it builds it only then. That
// function reads only the node and the options, so the message is the same.
#[derive(Clone, Copy)]
pub enum NameNotFound {
    Message(&'static Message),
    CannotFindName(Node),
}

impl NameNotFound {
    /// The Go message this value stands for.
    pub fn message(self, c: &Checker) -> &'static Message {
        match self {
            NameNotFound::Message(message) => message,
            NameNotFound::CannotFindName(node) => c.get_cannot_find_name_diagnostic_for_name(node),
        }
    }
}

/// The symbols that `NameResolver::resolve` reads, and the checker that its
/// callbacks take. The checker is its own host. A host with no checker
/// (`transformers::reference_resolver::BinderSymbols`) serves a resolver
/// with no callbacks, which never asks for it.
// PORT: not in Go. The method names differ from `CheckerShape::symbols` so
// that both traits can be in scope for a `Checker`.
pub trait NameResolverHost {
    /// The arena that holds the symbols and tables the names resolve to.
    fn symbol_arena(&self) -> &SymbolArena;
    /// The same arena, for the transient `arguments` symbol.
    fn symbol_arena_mut(&mut self) -> &mut SymbolArena;
    /// The checker for a resolver callback.
    fn hook_checker(&mut self) -> &mut Checker;
}

impl NameResolverHost for Checker {
    #[inline(always)]
    fn symbol_arena(&self) -> &SymbolArena {
        &self.symbols
    }

    #[inline(always)]
    fn symbol_arena_mut(&mut self) -> &mut SymbolArena {
        &mut self.symbols
    }

    #[inline(always)]
    fn hook_checker(&mut self) -> &mut Checker {
        self
    }
}

/// Go `binder.NameResolver`. The checker builds one with
/// `create_name_resolver` / `create_name_resolver_for_suggestion` and calls
/// `resolve(c, ...)` on it. The binder reference resolver builds one with no
/// callbacks and calls it with the binder symbols.
pub struct NameResolver {
    pub compiler_options: &'static CompilerOptions,
    pub get_symbol_of_declaration: Option<NameResolverGetSymbolOfDeclarationFn>,
    pub error: Option<NameResolverErrorFn>,
    pub globals: SymbolTable,
    // PORT: `Cell` because Go `argumentsSymbol()` lazily assigns it.
    pub arguments_symbol: Cell<SymbolId>,
    pub require_symbol: SymbolId,
    pub lookup: Option<NameResolverLookupFn>,
    pub symbol_referenced: Option<NameResolverSymbolReferencedFn>,
    pub set_requires_scope_change_cache: Option<NameResolverSetRequiresScopeChangeCacheFn>,
    pub get_requires_scope_change_cache: Option<NameResolverGetRequiresScopeChangeCacheFn>,
    pub on_property_with_invalid_initializer:
        Option<NameResolverOnPropertyWithInvalidInitializerFn>,
    pub on_failed_to_resolve_symbol: Option<NameResolverOnFailedToResolveSymbolFn>,
    pub on_successfully_resolved_symbol: Option<NameResolverOnSuccessfullyResolvedSymbolFn>,
}

/// The names that `NameResolver::resolve_name` compares by id.
static CONST_NAME: std::sync::LazyLock<Name> = std::sync::LazyLock::new(|| Name::from("const"));
static ARGUMENTS_NAME: std::sync::LazyLock<Name> =
    std::sync::LazyLock::new(|| Name::from("arguments"));

thread_local! {
    /// U1 (a): the text and name of the last `resolver_name_text` call on
    /// this thread, for the next `NameResolver::resolve`.
    static NAME_HINT: Cell<Option<(&'static str, Name)>> = const { Cell::new(None) };
}

/// `name.as_str()`, and a note for the next `NameResolver::resolve` on this
/// thread that this text is `name`, so it does not intern the text again.
/// Pass the result as the `name` of a resolve call (the checker's
/// `resolve_name` field or method, whose closures take `&str`).
// PERF: U1 (a). The checker reaches the resolver only through closures that
// take the name as `&str` (checker_p01). `resolve` uses the note only when
// its `name` is this same text (same address and length), and then the note
// is the name of that text, so a stale note can never give a wrong name.
#[must_use]
pub fn resolver_name_text(name: Name) -> &'static str {
    let text = name.as_str();
    NAME_HINT.set(Some((text, name)));
    text
}

impl NameResolver {
    // Go: binder/nameresolver.go:25 Resolve
    // PORT: Go `nameNotFoundMessage *diagnostics.Message` may be nil, so it is
    // `Option<NameNotFound>`.
    // PERF: `name` is interned once here (see `resolve_name`), or taken from
    // the note of `resolver_name_text`.
    pub fn resolve<H: NameResolverHost>(
        &self,
        c: &mut H,
        location: Node,
        name: &str,
        meaning: SymbolFlags,
        name_not_found_message: Option<NameNotFound>,
        is_use: bool,
        exclude_globals: bool,
    ) -> SymbolId {
        let name_key = match NAME_HINT.take() {
            Some((text, key)) if std::ptr::eq(text, name) => key,
            _ => Name::from(name),
        };
        debug_assert_eq!(name_key, Name::from(name));
        self.resolve_name(
            c,
            location,
            &name_key,
            meaning,
            name_not_found_message,
            is_use,
            exclude_globals,
        )
    }

    /// `resolve` with the name already interned. It holds the body of Go
    /// `Resolve`.
    // PERF: each scope lookup below uses `name_key`, so a table lookup reads
    // the hash the interner keeps and compares name ids, with no hashing
    // and no text compare per scope.
    pub fn resolve_name<H: NameResolverHost>(
        &self,
        c: &mut H,
        location: Node,
        name_key: &Name,
        meaning: SymbolFlags,
        name_not_found_message: Option<NameNotFound>,
        is_use: bool,
        exclude_globals: bool,
    ) -> SymbolId {
        let name_key = name_key.clone();
        let mut location = location;
        let mut result = SymbolId::NIL;
        let mut last_location = Node::NIL;
        let mut last_self_reference_location = Node::NIL;
        let mut property_with_invalid_initializer = Node::NIL;
        let mut associated_declaration_for_containing_initializer_or_binding_name = Node::NIL;
        let mut within_deferred_context = false;
        let mut grandparent: Node;
        let original_location = location; // needed for did-you-mean error reporting, which gathers candidates starting from the original location
        // PERF: intern1 fix 4. The name is compared by id here and below;
        // its text is read only for a diagnostic.
        let name_is_const = name_key == *CONST_NAME;
        // PERF: `kind` is `location.kind()`, read once per scope. Each
        // `kind()` call reads the file's kind table, and the Go tests below
        // (`IsModuleOrEnumDeclaration`, `IsGlobalSourceFile`,
        // `IsFunctionLike`, `getIsDeferredContext`, the switch and
        // `isSelfReferenceLocation`) read the kind about 8 times per scope.
        // It is read again each time `location` changes.
        // PERF: chkA. The step to the parent at the end of the loop reads
        // the parent's kind with it; `next` holds the parent for the next
        // pass. noderef1 G1: `location_ref` is the view (`NodeRef`) of
        // `location`, so a step to a parent in the same store does no
        // registry lookup.
        let mut next: Option<NodeRef> = None;
        'loop_: while location.is_some() {
            let location_ref = match next.take() {
                Some(location_ref) => location_ref,
                None => NodeRef::new(location),
            };
            let mut kind = location_ref.kind();
            if name_is_const && is_const_assertion(location) {
                // `const` in an `as const` has no symbol, but issues no error because there is no *actual* lookup of the type
                // (it refers to the constant type of the expression instead)
                return SymbolId::NIL;
            }
            if (kind == SyntaxKind::ModuleDeclaration || kind == SyntaxKind::EnumDeclaration)
                && last_location.is_some()
                && location.name() == last_location
            {
                // If lastLocation is the name of a namespace or enum, skip the parent since it will have is own locals that could
                // conflict.
                last_location = location;
                location = location.parent();
                kind = location.kind();
            }
            // PORT: `is_module_declaration(location)` with the kind read.
            let is_module_attributes = kind == SyntaxKind::ModuleDeclaration && {
                let attributes = location.attributes();
                attributes.is_some() && last_location == attributes
            };
            // PERF: chkA. Only a locals container has locals; `kind` tells
            // that with no second kind read in `locals()`.
            let locals = if is_locals_container_kind(kind) {
                location.locals()
            } else {
                debug_assert!(location.locals().is_nil());
                SymbolTable::NIL
            };
            // Locals of a source file are not in scope (because they get merged into the global symbol table)
            // PORT: `!is_global_source_file(location)` with the kind read.
            if locals.is_some()
                && !(kind == SyntaxKind::SourceFile && !is_external_or_common_js_module(location))
            {
                result = self.lookup(c, locals, &name_key, meaning);
                if result.is_some() {
                    let mut use_result = true;
                    // PORT: `is_function_like(location)`; `location` is not nil.
                    if is_module_attributes {
                        use_result = false;
                    } else if is_function_like_kind(kind)
                        && last_location.is_some()
                        && last_location != location.body()
                    {
                        // symbol lookup restrictions for function-like declarations
                        // - Type parameters of a function are in scope in the entire function declaration, including the parameter
                        //   list and return type. However, local types are only in scope in the function body.
                        // - parameters are only in the scope of function body
                        // This restriction does not apply to JSDoc comment types because they are parented
                        // at a higher level than type parameters would normally be
                        let result_flags = c.symbol_arena().sym_flags(result);
                        if (meaning & result_flags).intersects(SymbolFlags::TYPE)
                            && last_location.kind() != SyntaxKind::JsDoc
                        {
                            // type parameters are visible in parameter list, return type and type parameter list.
                            // Synthetic fake scopes are added for signatures so type parameters are accessible from them.
                            use_result = result_flags.intersects(SymbolFlags::TYPE_PARAMETER)
                                && (last_location.flags().intersects(NodeFlags::SYNTHESIZED)
                                    || last_location == location.type_()
                                    || last_location.kind() == SyntaxKind::Parameter
                                    || last_location.kind() == SyntaxKind::JsDocParameterTag
                                    || last_location.kind() == SyntaxKind::JsDocReturnTag
                                    || last_location.kind() == SyntaxKind::TypeParameter);
                        }
                        if (meaning & result_flags).intersects(SymbolFlags::VARIABLE) {
                            // expression inside parameter will lookup as normal variable scope when targeting es2015+
                            if self.use_outer_variable_scope_in_parameter(
                                c,
                                result,
                                location,
                                last_location,
                            ) {
                                use_result = false;
                            } else if result_flags.intersects(SymbolFlags::FUNCTION_SCOPED_VARIABLE)
                            {
                                // parameters are visible only inside function body, parameter list and return type
                                // technically for parameter list case here we might mix parameters and variables declared in function,
                                // however it is detected separately when checking initializers of parameters
                                // to make sure that they reference no variables declared after them.
                                let value_declaration =
                                    c.symbol_arena().sym(result).value_declaration;
                                use_result = last_location.kind() == SyntaxKind::Parameter
                                    || last_location.flags().intersects(NodeFlags::SYNTHESIZED)
                                    || last_location == location.type_()
                                        && find_ancestor(
                                            value_declaration,
                                            is_parameter_declaration,
                                        )
                                        .is_some();
                            }
                        }
                    } else if kind == SyntaxKind::ConditionalType {
                        // A type parameter declared using 'infer T' in a conditional type is visible only in
                        // the true branch of the conditional type.
                        use_result = last_location == location.true_type();
                    }
                    if use_result {
                        break 'loop_;
                    }
                    result = SymbolId::NIL;
                }
            }
            within_deferred_context =
                within_deferred_context || get_is_deferred_context(location, kind, last_location);
            // PORT: Go `break` inside the switch leaves the switch; `break 'switch_` does the same here.
            'switch_: {
                match kind {
                    SyntaxKind::SourceFile | SyntaxKind::ModuleDeclaration => {
                        // PORT: Go `case KindSourceFile: if !external { break }; fallthrough`.
                        if kind == SyntaxKind::SourceFile
                            && !is_external_or_common_js_module(location)
                        {
                            break 'switch_;
                        }
                        if is_module_attributes {
                            break 'switch_;
                        }
                        let module_symbol = self.get_symbol_of_declaration(c, location);
                        if module_symbol.is_nil() {
                            break 'switch_;
                        }
                        let module_exports = c.symbol_arena().sym(module_symbol).exports;
                        if kind == SyntaxKind::SourceFile
                            || (kind == SyntaxKind::ModuleDeclaration
                                && location.flags().intersects(NodeFlags::AMBIENT)
                                && !is_global_scope_augmentation(location))
                        {
                            // It's an external module. First see if the module has an export default and if the local
                            // name of that export default matches.
                            result = c
                                .symbol_arena()
                                .get(module_exports, INTERNAL_SYMBOL_NAME_DEFAULT);
                            if result.is_some() {
                                let local_symbol =
                                    get_local_symbol_for_export_default(c.symbol_arena(), result);
                                if local_symbol.is_some()
                                    && c.symbol_arena().sym_flags(result).intersects(meaning)
                                    && c.symbol_arena().sym(local_symbol).name == name_key
                                {
                                    break 'loop_;
                                }
                                result = SymbolId::NIL;
                            }
                            // Because of module/namespace merging, a module's exports are in scope,
                            // yet we never want to treat an export specifier as putting a member in scope.
                            // Therefore, if the name we find is purely an export specifier, it is not actually considered in scope.
                            // Two things to note about this:
                            //     1. We have to check this without calling getSymbol. The problem with calling getSymbol
                            //        on an export specifier is that it might find the export specifier itself, and try to
                            //        resolve it as an alias. This will cause the checker to consider the export specifier
                            //        a circular alias reference when it might not be.
                            //     2. We check === SymbolFlags.Alias in order to check that the symbol is *purely*
                            //        an alias. If we used &, we'd be throwing out symbols that have non alias aspects,
                            //        which is not the desired behavior.
                            let module_export =
                                c.symbol_arena().get_name(module_exports, &name_key);
                            if module_export.is_some()
                                && c.symbol_arena().sym_flags(module_export) == SymbolFlags::ALIAS
                                && (get_declaration_of_kind(
                                    c.symbol_arena(),
                                    module_export,
                                    SyntaxKind::ExportSpecifier,
                                )
                                .is_some()
                                    || get_declaration_of_kind(
                                        c.symbol_arena(),
                                        module_export,
                                        SyntaxKind::NamespaceExport,
                                    )
                                    .is_some())
                            {
                                break 'switch_;
                            }
                        }
                        if !name_key.is_default_symbol_name() {
                            result = self.lookup(
                                c,
                                module_exports,
                                &name_key,
                                meaning & SymbolFlags::MODULE_MEMBER,
                            );
                            if result.is_some() {
                                if kind == SyntaxKind::SourceFile
                                    && with_source_file_info(location, |info| {
                                        info.common_js_module_indicator
                                    })
                                    .is_some()
                                    && !c
                                        .symbol_arena()
                                        .sym_flags(result)
                                        .intersects(SymbolFlags::TYPE)
                                {
                                    result = SymbolId::NIL;
                                } else {
                                    break 'loop_;
                                }
                            }
                        }
                    }
                    SyntaxKind::EnumDeclaration => {
                        let enum_symbol = self.get_symbol_of_declaration(c, location);
                        if enum_symbol.is_nil() {
                            break 'switch_;
                        }
                        let enum_exports = c.symbol_arena().sym(enum_symbol).exports;
                        result = self.lookup(
                            c,
                            enum_exports,
                            &name_key,
                            meaning & SymbolFlags::ENUM_MEMBER,
                        );
                        if result.is_some() {
                            if name_not_found_message.is_some()
                                && self.compiler_options.get_isolated_modules()
                                && !location.flags().intersects(NodeFlags::AMBIENT)
                                && get_source_file_of_node(location)
                                    != get_source_file_of_node(
                                        c.symbol_arena().sym(result).value_declaration,
                                    )
                            {
                                let isolated_modules_like_flag_name =
                                    if self.compiler_options.verbatim_module_syntax
                                        == Tristate::True
                                    {
                                        "verbatimModuleSyntax"
                                    } else {
                                        "isolatedModules"
                                    };
                                let qualified = c.symbol_arena().sym(enum_symbol).name.to_string()
                                    + "."
                                    + name_key.as_str();
                                self.error(
                                    c,
                                    original_location,
                                    diag::Cannot_access_0_from_another_file_without_qualification_when_1_is_enabled_Use_2_instead,
                                    args![
                                        name_key.as_str(),
                                        isolated_modules_like_flag_name,
                                        qualified
                                    ],
                                );
                            }
                            break 'loop_;
                        }
                    }
                    SyntaxKind::PropertyDeclaration => {
                        if !is_static(location) {
                            let ctor = find_constructor_declaration(location.parent());
                            if ctor.is_some() && ctor.locals().is_some() {
                                if self
                                    .lookup(
                                        c,
                                        ctor.locals(),
                                        &name_key,
                                        meaning & SymbolFlags::VALUE,
                                    )
                                    .is_some()
                                {
                                    // Remember the property node, it will be used later to report appropriate error
                                    property_with_invalid_initializer = location;
                                }
                            }
                        }
                    }
                    SyntaxKind::ClassDeclaration
                    | SyntaxKind::ClassExpression
                    | SyntaxKind::InterfaceDeclaration => {
                        let decl_symbol = self.get_symbol_of_declaration(c, location);
                        let members = c.symbol_arena().sym(decl_symbol).members;
                        result = self.lookup(c, members, &name_key, meaning & SymbolFlags::TYPE);
                        if result.is_some() {
                            if !is_type_parameter_symbol_declared_in_container(
                                c.symbol_arena(),
                                result,
                                location,
                            ) {
                                // ignore type parameters not declared in this container
                                result = SymbolId::NIL;
                                break 'switch_;
                            }
                            if last_location.is_some() && is_static(last_location) {
                                // TypeScript 1.0 spec (April 2014): 3.4.1
                                // The scope of a type parameter extends over the entire declaration with which the type
                                // parameter list is associated, with the exception of static member declarations in classes.
                                if name_not_found_message.is_some() {
                                    self.error(
                                        c,
                                        original_location,
                                        diag::Static_members_cannot_reference_class_type_parameters,
                                        args![],
                                    );
                                }
                                return SymbolId::NIL;
                            }
                            break 'loop_;
                        }
                        if kind == SyntaxKind::ClassExpression
                            && meaning.intersects(SymbolFlags::CLASS)
                        {
                            let class_name = location.name();
                            if class_name.is_some() && class_name.text_is(&name_key) {
                                result = location.symbol();
                                break 'loop_;
                            }
                        }
                    }
                    SyntaxKind::ExpressionWithTypeArguments => {
                        if last_location == location.expression()
                            && is_heritage_clause(location.parent())
                            && location.parent().token() == SyntaxKind::ExtendsKeyword
                        {
                            let container = location.parent().parent();
                            if is_class_like(container) {
                                let container_symbol = self.get_symbol_of_declaration(c, container);
                                let members = c.symbol_arena().sym(container_symbol).members;
                                result =
                                    self.lookup(c, members, &name_key, meaning & SymbolFlags::TYPE);
                                if result.is_some() {
                                    if name_not_found_message.is_some() {
                                        self.error(c, original_location, diag::Base_class_expressions_cannot_reference_class_type_parameters, args![]);
                                    }
                                    return SymbolId::NIL;
                                }
                            }
                        }
                    }
                    // It is not legal to reference a class's own type parameters from a computed property name that
                    // belongs to the class. For example:
                    //
                    //   function foo<T>() { return '' }
                    //   class C<T> { // <-- Class's own type parameter T
                    //       [foo<T>()]() { } // <-- Reference to T from class's own computed property
                    //   }
                    SyntaxKind::ComputedPropertyName => {
                        grandparent = location.parent().parent();
                        if is_class_like(grandparent) || is_interface_declaration(grandparent) {
                            // A reference to this grandparent's type parameters would be an error
                            let grandparent_symbol = self.get_symbol_of_declaration(c, grandparent);
                            let members = c.symbol_arena().sym(grandparent_symbol).members;
                            result =
                                self.lookup(c, members, &name_key, meaning & SymbolFlags::TYPE);
                            if result.is_some() {
                                if name_not_found_message.is_some() {
                                    self.error(
                                        c,
                                        original_location,
                                        diag::A_computed_property_name_cannot_reference_a_type_parameter_from_its_containing_type,
                                        args![],
                                    );
                                }
                                return SymbolId::NIL;
                            }
                        }
                    }
                    SyntaxKind::MethodDeclaration
                    | SyntaxKind::Constructor
                    | SyntaxKind::GetAccessor
                    | SyntaxKind::SetAccessor
                    | SyntaxKind::FunctionDeclaration => {
                        if meaning.intersects(SymbolFlags::VARIABLE) && name_key == *ARGUMENTS_NAME
                        {
                            result = self.arguments_symbol(c);
                            break 'loop_;
                        }
                    }
                    SyntaxKind::FunctionExpression => {
                        if meaning.intersects(SymbolFlags::VARIABLE) && name_key == *ARGUMENTS_NAME
                        {
                            result = self.arguments_symbol(c);
                            break 'loop_;
                        }
                        if meaning.intersects(SymbolFlags::FUNCTION) {
                            let function_name = location.name();
                            if function_name.is_some() && function_name.text_is(&name_key) {
                                result = location.symbol();
                                break 'loop_;
                            }
                        }
                    }
                    SyntaxKind::Decorator => {
                        // Decorators are resolved at the class declaration. Resolving at the parameter
                        // or member would result in looking up locals in the method.
                        //
                        //   function y() {}
                        //   class C {
                        //       method(@y x, y) {} // <-- decorator y should be resolved at the class declaration, not the parameter.
                        //   }
                        //
                        if location.parent().is_some()
                            && location.parent().kind() == SyntaxKind::Parameter
                        {
                            location = location.parent();
                        }
                        //   function y() {}
                        //   class C {
                        //       @y method(x, y) {} // <-- decorator y should be resolved at the class declaration, not the method.
                        //   }
                        //
                        // class Decorators are resolved outside of the class to avoid referencing type parameters of that class.
                        //
                        //   type T = number;
                        //   declare function y(x: T): any;
                        //   @param(1 as T) // <-- T should resolve to the type alias outside of class C
                        //   class C<T> {}
                        if location.parent().is_some()
                            && (is_class_element(location.parent())
                                || location.parent().kind() == SyntaxKind::ClassDeclaration)
                        {
                            location = location.parent();
                        }
                        kind = location.kind();
                    }
                    SyntaxKind::Parameter => {
                        if last_location.is_some()
                            && (last_location == location.initializer()
                                || last_location == location.name()
                                    && is_binding_pattern(last_location))
                        {
                            if associated_declaration_for_containing_initializer_or_binding_name
                                .is_nil()
                            {
                                associated_declaration_for_containing_initializer_or_binding_name =
                                    location;
                            }
                        }
                    }
                    SyntaxKind::BindingElement => {
                        if last_location.is_some()
                            && (last_location == location.initializer()
                                || last_location == location.name()
                                    && is_binding_pattern(last_location))
                        {
                            if is_part_of_parameter_declaration(location)
                                && associated_declaration_for_containing_initializer_or_binding_name
                                    .is_nil()
                            {
                                associated_declaration_for_containing_initializer_or_binding_name =
                                    location;
                            }
                        }
                    }
                    SyntaxKind::InferType => {
                        if meaning.intersects(SymbolFlags::TYPE_PARAMETER) {
                            let parameter_name = location.type_parameter().name();
                            if parameter_name.is_some() && parameter_name.text_is(&name_key) {
                                result = location.type_parameter().symbol();
                                break 'loop_;
                            }
                        }
                    }
                    SyntaxKind::ExportSpecifier => {
                        if last_location.is_some()
                            && last_location == location.property_name()
                            && location.parent().parent().module_specifier().is_some()
                        {
                            location = location.parent().parent().parent();
                            kind = location.kind();
                        }
                    }
                    _ => {}
                }
            }
            if is_self_reference_location(location, kind, last_location) {
                last_self_reference_location = location;
            }
            last_location = location;
            // !!! In Strada, JSDocTemplateTag/JSDocParameterTag/JSDocReturnTag locations skip to
            // getEffectiveContainerForJSDocTemplateTag/getHostSignatureFromJSDoc instead of location.parent.
            // This is a no-op currently because JSDoc nodes have no locals and getEffectiveJSDocHost is not
            // fully ported for JS assignment patterns.
            // The switch above can move `location`.
            let parent = if location_ref.node() == location {
                location_ref.parent()
            } else {
                NodeRef::new(location).parent()
            };
            location = parent.node();
            next = Some(parent);
        }
        // We just climbed up parents looking for the name, meaning that we started in a descendant node of `lastLocation`.
        // If `result === lastSelfReferenceLocation.symbol`, that means that we are somewhere inside `lastSelfReferenceLocation` looking up a name, and resolving to `lastLocation` itself.
        // That means that this is a self-reference of `lastLocation`, and shouldn't count this when considering whether `lastLocation` is used.
        if is_use
            && result.is_some()
            && (last_self_reference_location.is_nil()
                || result != last_self_reference_location.symbol())
        {
            if let Some(symbol_referenced) = &self.symbol_referenced {
                symbol_referenced(c.hook_checker(), result, meaning);
            }
        }
        if result.is_nil() && !exclude_globals {
            result = self.lookup(
                c,
                self.globals,
                &name_key,
                meaning | SymbolFlags::GLOBAL_LOOKUP,
            );
        }
        if result.is_nil() {
            if original_location.is_some()
                && is_in_js_file(original_location)
                && original_location.parent().is_some()
            {
                if is_require_call(
                    original_location.parent(),
                    false, /*requireStringLiteralLikeArgument*/
                ) {
                    return self.require_symbol;
                }
            }
        }
        if let Some(name_not_found_message) = name_not_found_message {
            if property_with_invalid_initializer.is_some() {
                if let Some(on_property_with_invalid_initializer) =
                    &self.on_property_with_invalid_initializer
                {
                    if on_property_with_invalid_initializer(
                        c.hook_checker(),
                        original_location,
                        name_key.as_str(),
                        property_with_invalid_initializer,
                        result,
                    ) {
                        return SymbolId::NIL;
                    }
                }
            }
            if result.is_nil() {
                if let Some(on_failed_to_resolve_symbol) = &self.on_failed_to_resolve_symbol {
                    let name_not_found_message = name_not_found_message.message(c.hook_checker());
                    on_failed_to_resolve_symbol(
                        c.hook_checker(),
                        original_location,
                        name_key.as_str(),
                        meaning,
                        name_not_found_message,
                    );
                }
            } else if let Some(on_successfully_resolved_symbol) =
                &self.on_successfully_resolved_symbol
            {
                on_successfully_resolved_symbol(
                    c.hook_checker(),
                    original_location,
                    result,
                    meaning,
                    last_location,
                    associated_declaration_for_containing_initializer_or_binding_name,
                    within_deferred_context,
                );
            }
        }
        result
    }

    // Go: binder/nameresolver.go:354 useOuterVariableScopeInParameter
    pub fn use_outer_variable_scope_in_parameter<H: NameResolverHost>(
        &self,
        c: &mut H,
        result: SymbolId,
        location: Node,
        last_location: Node,
    ) -> bool {
        if is_parameter_declaration(last_location) {
            let body = location.body();
            let value_declaration = c.symbol_arena().sym(result).value_declaration;
            if body.is_some()
                && value_declaration.is_some()
                && value_declaration.pos() >= body.pos()
                && value_declaration.end() <= body.end()
            {
                // check for several cases where we introduce temporaries that require moving the name/initializer of the parameter to the body
                // - static field in a class expression
                // - optional chaining pre-es2020
                // - nullish coalesce pre-es2020
                // - spread assignment in binding pattern pre-es2017
                let function_location = location;
                let mut declaration_requires_scope_change = Tristate::Unknown;
                if let Some(get_requires_scope_change_cache) = &self.get_requires_scope_change_cache
                {
                    declaration_requires_scope_change =
                        get_requires_scope_change_cache(c.hook_checker(), function_location);
                }
                if declaration_requires_scope_change == Tristate::Unknown {
                    declaration_requires_scope_change = if function_location
                        .parameters()
                        .iter()
                        .any(|p| self.requires_scope_change(p))
                    {
                        Tristate::True
                    } else {
                        Tristate::False
                    };
                    if let Some(set_requires_scope_change_cache) =
                        &self.set_requires_scope_change_cache
                    {
                        set_requires_scope_change_cache(
                            c.hook_checker(),
                            function_location,
                            declaration_requires_scope_change,
                        );
                    }
                }
                return declaration_requires_scope_change != Tristate::True;
            }
        }
        false
    }

    // Go: binder/nameresolver.go:380 requiresScopeChange
    pub fn requires_scope_change(&self, node: Node) -> bool {
        self.requires_scope_change_worker(node.name())
            || node.initializer().is_some() && self.requires_scope_change_worker(node.initializer())
    }

    // Go: binder/nameresolver.go:385 requiresScopeChangeWorker
    pub fn requires_scope_change_worker(&self, node: Node) -> bool {
        match node.kind() {
            SyntaxKind::ArrowFunction
            | SyntaxKind::FunctionExpression
            | SyntaxKind::FunctionDeclaration
            | SyntaxKind::Constructor => false,
            SyntaxKind::MethodDeclaration
            | SyntaxKind::GetAccessor
            | SyntaxKind::SetAccessor
            | SyntaxKind::PropertyAssignment => self.requires_scope_change_worker(node.name()),
            SyntaxKind::PropertyDeclaration => {
                if has_static_modifier(node) {
                    return !self.compiler_options.get_emit_standard_class_fields();
                }
                self.requires_scope_change_worker(node.name())
            }
            _ => {
                if is_nullish_coalesce(node) || is_optional_chain(node) {
                    return self.compiler_options.get_emit_script_target() < ScriptTarget::ES2020;
                }
                if is_binding_element(node)
                    && node.dot_dot_dot_token().is_some()
                    && is_object_binding_pattern(node.parent())
                {
                    return self.compiler_options.get_emit_script_target() < ScriptTarget::ES2017;
                }
                if is_type_node(node) {
                    return false;
                }
                node.for_each_child(&mut |child| self.requires_scope_change_worker(child))
            }
        }
    }

    // Go: binder/nameresolver.go:410 error
    pub fn error<H: NameResolverHost>(
        &self,
        c: &mut H,
        location: Node,
        message: &'static Message,
        args: Vec<String>,
    ) {
        if let Some(error) = &self.error {
            error(c.hook_checker(), location, message, args);
        }
        // Default implementation does not report errors
    }

    // Go: binder/nameresolver.go:417 getSymbolOfDeclaration
    pub fn get_symbol_of_declaration<H: NameResolverHost>(
        &self,
        c: &mut H,
        node: Node,
    ) -> SymbolId {
        if let Some(get_symbol_of_declaration) = &self.get_symbol_of_declaration {
            return get_symbol_of_declaration(c.hook_checker(), node);
        }

        // Default implementation does not support merged symbols
        node.symbol()
    }

    // Go: binder/nameresolver.go:426 lookup
    // PORT: takes the name interned (see `resolve`).
    pub fn lookup<H: NameResolverHost>(
        &self,
        c: &mut H,
        symbols: SymbolTable,
        name: &Name,
        meaning: SymbolFlags,
    ) -> SymbolId {
        if let Some(lookup) = &self.lookup {
            return lookup(c.hook_checker(), symbols, TableKey::Name(name), meaning);
        }
        // Default implementation does not support following aliases or merged symbols
        if meaning != SymbolFlags::NONE {
            let symbol = c.symbol_arena().get_name(symbols, name);
            if symbol.is_some() {
                if c.symbol_arena().sym_flags(symbol).intersects(meaning) {
                    return symbol;
                }
            }
        }
        SymbolId::NIL
    }

    // Go: binder/nameresolver.go:442 argumentsSymbol
    pub fn arguments_symbol<H: NameResolverHost>(&self, c: &mut H) -> SymbolId {
        if self.arguments_symbol.get().is_nil() {
            // Default implementation synthesizes a transient symbol for `arguments`
            // PORT: Go allocates a free-standing `&ast.Symbol`; here it lives in the
            // host's arena (the checker's, or the binder copy of `BinderSymbols`).
            let symbol = c
                .symbol_arena_mut()
                .new_symbol(SymbolFlags::PROPERTY | SymbolFlags::TRANSIENT, "arguments");
            self.arguments_symbol.set(symbol);
        }
        self.arguments_symbol.get()
    }
}

// Go: binder/nameresolver.go:452 GetLocalSymbolForExportDefault
pub fn get_local_symbol_for_export_default(symbols: &SymbolArena, symbol: SymbolId) -> SymbolId {
    if !is_export_default_symbol(symbols, symbol) || symbols.sym(symbol).declarations.is_empty() {
        return SymbolId::NIL;
    }
    for &decl in &symbols.sym(symbol).declarations {
        let local_symbol = decl.local_symbol();
        if local_symbol.is_some() {
            return local_symbol;
        }
    }
    SymbolId::NIL
}

// Go: binder/nameresolver.go:465 isExportDefaultSymbol
pub fn is_export_default_symbol(symbols: &SymbolArena, symbol: SymbolId) -> bool {
    symbol.is_some()
        && !symbols.sym(symbol).declarations.is_empty()
        && has_syntactic_modifier(symbols.sym(symbol).declarations[0], ModifierFlags::DEFAULT)
}

// Go: binder/nameresolver.go:469 getIsDeferredContext
// PERF: `kind` is `location.kind()`, which the caller has read (see
// `resolve_name`). `location` is not nil.
pub fn get_is_deferred_context(location: Node, kind: SyntaxKind, last_location: Node) -> bool {
    if kind != SyntaxKind::ArrowFunction && kind != SyntaxKind::FunctionExpression {
        // initializers in instance property declaration of class like entities are executed in constructor and thus deferred
        // A name is evaluated within the enclosing scope - so it shouldn't count as deferred
        // PORT: `ast.IsFunctionLikeDeclaration(location)` on a kind that is
        // not an arrow function or function expression.
        return kind == SyntaxKind::TypeQuery
            || (matches!(
                kind,
                SyntaxKind::FunctionDeclaration
                    | SyntaxKind::MethodDeclaration
                    | SyntaxKind::Constructor
                    | SyntaxKind::GetAccessor
                    | SyntaxKind::SetAccessor
            ) || kind == SyntaxKind::PropertyDeclaration && !is_static(location))
                && (last_location.is_nil() || last_location != location.name());
    }
    if last_location.is_some() && last_location == location.name() {
        return false;
    }
    // generator functions and async functions are not inlined in control flow when immediately invoked
    // PORT: Go `location.BodyData().AsteriskToken` -> the `asterisk_token()` field accessor.
    if location.asterisk_token().is_some() || has_syntactic_modifier(location, ModifierFlags::ASYNC)
    {
        return true;
    }
    get_immediately_invoked_function_expression(location).is_nil()
}

// Go: binder/nameresolver.go:487 isTypeParameterSymbolDeclaredInContainer
pub fn is_type_parameter_symbol_declared_in_container(
    symbols: &SymbolArena,
    symbol: SymbolId,
    container: Node,
) -> bool {
    for &decl in &symbols.sym(symbol).declarations {
        if decl.kind() == SyntaxKind::TypeParameter {
            let parent = decl.parent();
            if parent == container {
                return true;
            }
        }
    }
    false
}

/// True for the kinds of the locals containers (`locals_container_variants!`
/// in `ast/node.rs`, Go `LocalsContainerData`): only a node of one of these
/// kinds can have locals.
pub fn is_locals_container_kind(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::ArrowFunction
            | SyntaxKind::Block
            | SyntaxKind::CallSignature
            | SyntaxKind::CaseBlock
            | SyntaxKind::CatchClause
            | SyntaxKind::ClassDeclaration
            | SyntaxKind::ClassExpression
            | SyntaxKind::ClassStaticBlockDeclaration
            | SyntaxKind::ConditionalType
            | SyntaxKind::ConstructSignature
            | SyntaxKind::Constructor
            | SyntaxKind::ConstructorType
            | SyntaxKind::ForInStatement
            | SyntaxKind::ForOfStatement
            | SyntaxKind::ForStatement
            | SyntaxKind::FunctionDeclaration
            | SyntaxKind::FunctionExpression
            | SyntaxKind::FunctionType
            | SyntaxKind::GetAccessor
            | SyntaxKind::IndexSignature
            | SyntaxKind::JsDocSignature
            | SyntaxKind::MappedType
            | SyntaxKind::MethodDeclaration
            | SyntaxKind::MethodSignature
            | SyntaxKind::ModuleDeclaration
            | SyntaxKind::SetAccessor
            | SyntaxKind::SourceFile
            | SyntaxKind::TypeAliasDeclaration
            | SyntaxKind::JsTypeAliasDeclaration
    )
}

// Go: binder/nameresolver.go:499 isSelfReferenceLocation
// PERF: `kind` is `node.kind()`, which the caller has read.
pub fn is_self_reference_location(node: Node, kind: SyntaxKind, last_location: Node) -> bool {
    match kind {
        SyntaxKind::Parameter => last_location.is_some() && last_location == node.name(),
        SyntaxKind::FunctionDeclaration
        | SyntaxKind::ClassDeclaration
        | SyntaxKind::InterfaceDeclaration
        | SyntaxKind::EnumDeclaration
        | SyntaxKind::TypeAliasDeclaration
        | SyntaxKind::JsTypeAliasDeclaration
        | SyntaxKind::ModuleDeclaration => true, // For `namespace N { N; }`
        _ => false,
    }
}

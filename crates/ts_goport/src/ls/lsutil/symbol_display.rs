//! Port of Go `ls/lsutil/symbol_display.go`.
//!
//! PORT: Go passes a `*checker.Checker` that may be nil and reads symbol
//! fields through the symbol pointer. Rust reads them from an arena, so the
//! checker parameter is `TypeChecker`: a checker (its arena is
//! `Checker.symbols`), or a nil checker with the arena that holds the
//! caller's symbols.

use crate::ls::lsutil::prelude::*;

use crate::flags_macros::{go_enum, go_flags};

// Go: ls/lsutil/symbol_display.go:10 ScriptElementKind
go_enum!(ScriptElementKind, i32 {
    UNKNOWN = 0; // ScriptElementKindUnknown
    WARNING = 1; // ScriptElementKindWarning
    // predefined type (void) or keyword (class)
    KEYWORD = 2; // ScriptElementKindKeyword
    // top level script node
    SCRIPT_ELEMENT = 3; // ScriptElementKindScriptElement
    // module foo {}
    MODULE_ELEMENT = 4; // ScriptElementKindModuleElement
    // class X {}
    CLASS_ELEMENT = 5; // ScriptElementKindClassElement
    // var x = class X {}
    LOCAL_CLASS_ELEMENT = 6; // ScriptElementKindLocalClassElement
    // interface Y {}
    INTERFACE_ELEMENT = 7; // ScriptElementKindInterfaceElement
    // type T = ...
    TYPE_ELEMENT = 8; // ScriptElementKindTypeElement
    // enum E {}
    ENUM_ELEMENT = 9; // ScriptElementKindEnumElement
    ENUM_MEMBER_ELEMENT = 10; // ScriptElementKindEnumMemberElement
    // Inside module and script only.
    // const v = ...
    VARIABLE_ELEMENT = 11; // ScriptElementKindVariableElement
    // Inside function.
    LOCAL_VARIABLE_ELEMENT = 12; // ScriptElementKindLocalVariableElement
    // using foo = ...
    VARIABLE_USING_ELEMENT = 13; // ScriptElementKindVariableUsingElement
    // await using foo = ...
    VARIABLE_AWAIT_USING_ELEMENT = 14; // ScriptElementKindVariableAwaitUsingElement
    // Inside module and script only.
    // function f() {}
    FUNCTION_ELEMENT = 15; // ScriptElementKindFunctionElement
    // Inside function.
    LOCAL_FUNCTION_ELEMENT = 16; // ScriptElementKindLocalFunctionElement
    // class X { [public|private]* foo() {} }
    MEMBER_FUNCTION_ELEMENT = 17; // ScriptElementKindMemberFunctionElement
    // class X { [public|private]* [get|set] foo:number; }
    MEMBER_GET_ACCESSOR_ELEMENT = 18; // ScriptElementKindMemberGetAccessorElement
    MEMBER_SET_ACCESSOR_ELEMENT = 19; // ScriptElementKindMemberSetAccessorElement
    // class X { [public|private]* foo:number; }
    // interface Y { foo:number; }
    MEMBER_VARIABLE_ELEMENT = 20; // ScriptElementKindMemberVariableElement
    // class X { [public|private]* accessor foo: number; }
    MEMBER_ACCESSOR_VARIABLE_ELEMENT = 21; // ScriptElementKindMemberAccessorVariableElement
    // class X { constructor() { } }
    // class X { static { } }
    CONSTRUCTOR_IMPLEMENTATION_ELEMENT = 22; // ScriptElementKindConstructorImplementationElement
    // interface Y { ():number; }
    CALL_SIGNATURE_ELEMENT = 23; // ScriptElementKindCallSignatureElement
    // interface Y { []:number; }
    INDEX_SIGNATURE_ELEMENT = 24; // ScriptElementKindIndexSignatureElement
    // interface Y { new():Y; }
    CONSTRUCT_SIGNATURE_ELEMENT = 25; // ScriptElementKindConstructSignatureElement
    // function foo(*Y*: string)
    PARAMETER_ELEMENT = 26; // ScriptElementKindParameterElement
    TYPE_PARAMETER_ELEMENT = 27; // ScriptElementKindTypeParameterElement
    PRIMITIVE_TYPE = 28; // ScriptElementKindPrimitiveType
    LABEL = 29; // ScriptElementKindLabel
    ALIAS = 30; // ScriptElementKindAlias
    CONST_ELEMENT = 31; // ScriptElementKindConstElement
    LET_ELEMENT = 32; // ScriptElementKindLetElement
    DIRECTORY = 33; // ScriptElementKindDirectory
    EXTERNAL_MODULE_NAME = 34; // ScriptElementKindExternalModuleName
    // String literal
    STRING = 35; // ScriptElementKindString
    // Jsdoc @link: in `{@link C link text}`, the before and after text "{@link " and "}"
    LINK = 36; // ScriptElementKindLink
    // Jsdoc @link: in `{@link C link text}`, the entity name "C"
    LINK_NAME = 37; // ScriptElementKindLinkName
    // Jsdoc @link: in `{@link C link text}`, the link text "link text"
    LINK_TEXT = 38; // ScriptElementKindLinkText
});

// Go: ls/lsutil/symbol_display.go:85 ScriptElementKindModifier
// PORT: Go starts the `1 << iota` run on the second line, so `Public` is
// `1 << 1`.
go_flags!(ScriptElementKindModifier, u32 {
    NONE = 0; // ScriptElementKindModifierNone
    PUBLIC = 1 << 1; // ScriptElementKindModifierPublic
    PRIVATE = 1 << 2; // ScriptElementKindModifierPrivate
    PROTECTED = 1 << 3; // ScriptElementKindModifierProtected
    EXPORTED = 1 << 4; // ScriptElementKindModifierExported
    AMBIENT = 1 << 5; // ScriptElementKindModifierAmbient
    STATIC = 1 << 6; // ScriptElementKindModifierStatic
    ABSTRACT = 1 << 7; // ScriptElementKindModifierAbstract
    OPTIONAL = 1 << 8; // ScriptElementKindModifierOptional
    DEPRECATED = 1 << 9; // ScriptElementKindModifierDeprecated
    DTS = 1 << 10; // ScriptElementKindModifierDts
    TS = 1 << 11; // ScriptElementKindModifierTs
    TSX = 1 << 12; // ScriptElementKindModifierTsx
    JS = 1 << 13; // ScriptElementKindModifierJs
    JSX = 1 << 14; // ScriptElementKindModifierJsx
    JSON = 1 << 15; // ScriptElementKindModifierJson
    DMTS = 1 << 16; // ScriptElementKindModifierDmts
    MTS = 1 << 17; // ScriptElementKindModifierMts
    MJS = 1 << 18; // ScriptElementKindModifierMjs
    DCTS = 1 << 19; // ScriptElementKindModifierDcts
    CTS = 1 << 20; // ScriptElementKindModifierCts
    CJS = 1 << 21; // ScriptElementKindModifierCjs
});

/// One entry of Go `scriptElementKindModifierNames`.
struct ScriptElementKindModifierName {
    flag: ScriptElementKindModifier,
    name: &'static str,
}

// Go: ls/lsutil/symbol_display.go:112 scriptElementKindModifierNames
const SCRIPT_ELEMENT_KIND_MODIFIER_NAMES: [ScriptElementKindModifierName; 21] = [
    ScriptElementKindModifierName {
        flag: ScriptElementKindModifier::PUBLIC,
        name: "public",
    },
    ScriptElementKindModifierName {
        flag: ScriptElementKindModifier::PRIVATE,
        name: "private",
    },
    ScriptElementKindModifierName {
        flag: ScriptElementKindModifier::PROTECTED,
        name: "protected",
    },
    ScriptElementKindModifierName {
        flag: ScriptElementKindModifier::EXPORTED,
        name: "export",
    },
    ScriptElementKindModifierName {
        flag: ScriptElementKindModifier::AMBIENT,
        name: "declare",
    },
    ScriptElementKindModifierName {
        flag: ScriptElementKindModifier::STATIC,
        name: "static",
    },
    ScriptElementKindModifierName {
        flag: ScriptElementKindModifier::ABSTRACT,
        name: "abstract",
    },
    ScriptElementKindModifierName {
        flag: ScriptElementKindModifier::OPTIONAL,
        name: "optional",
    },
    ScriptElementKindModifierName {
        flag: ScriptElementKindModifier::DEPRECATED,
        name: "deprecated",
    },
    ScriptElementKindModifierName {
        flag: ScriptElementKindModifier::DTS,
        name: ".d.ts",
    },
    ScriptElementKindModifierName {
        flag: ScriptElementKindModifier::TS,
        name: ".ts",
    },
    ScriptElementKindModifierName {
        flag: ScriptElementKindModifier::TSX,
        name: ".tsx",
    },
    ScriptElementKindModifierName {
        flag: ScriptElementKindModifier::JS,
        name: ".js",
    },
    ScriptElementKindModifierName {
        flag: ScriptElementKindModifier::JSX,
        name: ".jsx",
    },
    ScriptElementKindModifierName {
        flag: ScriptElementKindModifier::JSON,
        name: ".json",
    },
    ScriptElementKindModifierName {
        flag: ScriptElementKindModifier::DMTS,
        name: ".d.mts",
    },
    ScriptElementKindModifierName {
        flag: ScriptElementKindModifier::MTS,
        name: ".mts",
    },
    ScriptElementKindModifierName {
        flag: ScriptElementKindModifier::MJS,
        name: ".mjs",
    },
    ScriptElementKindModifierName {
        flag: ScriptElementKindModifier::DCTS,
        name: ".d.cts",
    },
    ScriptElementKindModifierName {
        flag: ScriptElementKindModifier::CTS,
        name: ".cts",
    },
    ScriptElementKindModifierName {
        flag: ScriptElementKindModifier::CJS,
        name: ".cjs",
    },
];

impl ScriptElementKindModifier {
    // Go: ls/lsutil/symbol_display.go:139 Strings
    // PORT: Go `collections.Set[string]` is `FxHashSet<String>`.
    #[must_use]
    pub fn strings(self) -> FxHashSet<String> {
        let mut result = FxHashSet::default();
        for entry in &SCRIPT_ELEMENT_KIND_MODIFIER_NAMES {
            if self.intersects(entry.flag) {
                result.insert(entry.name.to_string());
            }
        }
        result
    }
}

// Go: ls/lsutil/symbol_display.go:149 FileExtensionKindModifiers
pub const FILE_EXTENSION_KIND_MODIFIERS: ScriptElementKindModifier = ScriptElementKindModifier::DTS
    .union(ScriptElementKindModifier::TS)
    .union(ScriptElementKindModifier::TSX)
    .union(ScriptElementKindModifier::JS)
    .union(ScriptElementKindModifier::JSX)
    .union(ScriptElementKindModifier::JSON)
    .union(ScriptElementKindModifier::DMTS)
    .union(ScriptElementKindModifier::MTS)
    .union(ScriptElementKindModifier::MJS)
    .union(ScriptElementKindModifier::DCTS)
    .union(ScriptElementKindModifier::CTS)
    .union(ScriptElementKindModifier::CJS);

/// Go `typeChecker *checker.Checker` in this file, which can be nil.
// PORT: see the module comment. `Nil` carries the arena that Go's symbol
// pointers read. The autoimport extractor passes its checker's arena: its
// symbols can be checker symbols (for example a merged module augmentation),
// which the program's binder arena does not hold.
pub enum TypeChecker<'a> {
    Checker(&'a mut Checker),
    Nil(&'a SymbolArena),
    /// Nil, with the current program's binder symbols
    /// (`program::bound_symbols`), which the guard holds.
    Bound(crate::program::BoundSymbols),
}

/// Go `typeChecker` that is a checker or nil. Nil reads the current
/// program's binder arena (`program::bound_symbols`).
impl<'a> From<Option<&'a mut Checker>> for TypeChecker<'a> {
    fn from(type_checker: Option<&'a mut Checker>) -> Self {
        match type_checker {
            Some(c) => TypeChecker::Checker(c),
            None => TypeChecker::Bound(crate::program::bound_symbols()),
        }
    }
}

impl TypeChecker<'_> {
    /// The arena that holds the symbols Go reads through its pointers.
    fn symbols(&self) -> &SymbolArena {
        match self {
            TypeChecker::Checker(c) => &c.symbols,
            TypeChecker::Nil(symbols) => *symbols,
            TypeChecker::Bound(symbols) => &**symbols,
        }
    }

    /// Go `typeChecker`: `None` is nil.
    fn checker(&mut self) -> Option<&mut Checker> {
        match self {
            TypeChecker::Checker(c) => Some(&mut **c),
            TypeChecker::Nil(_) | TypeChecker::Bound(_) => None,
        }
    }
}

// Go: ls/lsutil/symbol_display.go:162 GetSymbolKind
pub fn get_symbol_kind<'a>(
    type_checker: impl Into<TypeChecker<'a>>,
    symbol: SymbolId,
    location: Node,
) -> ScriptElementKind {
    let mut type_checker = type_checker.into();
    let result = get_symbol_kind_of_constructor_property_method_accessor_function_or_var(
        &mut type_checker,
        symbol,
        location,
    );
    if result != ScriptElementKind::UNKNOWN {
        return result;
    }
    let symbols = type_checker.symbols();
    let flags = symbols
        .sym(symbol)
        .combined_local_and_export_symbol_flags(symbols);
    if flags.intersects(SymbolFlags::CLASS) {
        let decl = get_declaration_of_kind(symbols, symbol, SyntaxKind::ClassExpression);
        if decl.is_some() {
            return ScriptElementKind::LOCAL_CLASS_ELEMENT;
        }
        return ScriptElementKind::CLASS_ELEMENT;
    }
    if flags.intersects(SymbolFlags::ENUM) {
        return ScriptElementKind::ENUM_ELEMENT;
    }
    if flags.intersects(SymbolFlags::TYPE_ALIAS) {
        return ScriptElementKind::TYPE_ELEMENT;
    }
    if flags.intersects(SymbolFlags::INTERFACE) {
        return ScriptElementKind::INTERFACE_ELEMENT;
    }
    if flags.intersects(SymbolFlags::TYPE_PARAMETER) {
        return ScriptElementKind::TYPE_PARAMETER_ELEMENT;
    }
    if flags.intersects(SymbolFlags::ENUM_MEMBER) {
        return ScriptElementKind::ENUM_MEMBER_ELEMENT;
    }
    if flags.intersects(SymbolFlags::ALIAS) {
        return ScriptElementKind::ALIAS;
    }
    if flags.intersects(SymbolFlags::MODULE) {
        return ScriptElementKind::MODULE_ELEMENT;
    }

    ScriptElementKind::UNKNOWN
}

// Go: ls/lsutil/symbol_display.go:200 getSymbolKindOfConstructorPropertyMethodAccessorFunctionOrVar
fn get_symbol_kind_of_constructor_property_method_accessor_function_or_var(
    type_checker: &mut TypeChecker<'_>,
    symbol: SymbolId,
    location: Node,
) -> ScriptElementKind {
    let roots: Vec<SymbolId> = match type_checker.checker() {
        Some(c) => c.get_root_symbols(symbol),
        None => vec![symbol],
    };

    // If this is a method from a mapped type, leave as a method so long as it still has a call signature, as opposed to e.g.
    // `{ [K in keyof I]: number }`.
    if roots.len() == 1
        && type_checker
            .symbols()
            .sym_flags(roots[0])
            .intersects(SymbolFlags::METHOD)
        && match type_checker.checker() {
            None => true,
            Some(c) => {
                let t = c.get_type_of_symbol_at_location(symbol, location);
                let t = c.get_non_nullable_type(t);
                c.get_call_signatures(t).len() > 0
            }
        }
    {
        return ScriptElementKind::MEMBER_FUNCTION_ELEMENT;
    }

    if let Some(c) = type_checker.checker() {
        if c.is_undefined_symbol(symbol) {
            return ScriptElementKind::VARIABLE_ELEMENT;
        }
        if c.is_arguments_symbol(symbol) {
            return ScriptElementKind::LOCAL_VARIABLE_ELEMENT;
        }
        if location.kind() == SyntaxKind::ThisKeyword && is_expression(location)
            || is_this_in_type_query(location)
        {
            return ScriptElementKind::PARAMETER_ELEMENT;
        }
    }

    let symbols = type_checker.symbols();
    let flags = symbols
        .sym(symbol)
        .combined_local_and_export_symbol_flags(symbols);
    if flags.intersects(SymbolFlags::VARIABLE) {
        let value_declaration = symbols.sym(symbol).value_declaration;
        if is_first_declaration_of_symbol_parameter(symbols, symbol) {
            return ScriptElementKind::PARAMETER_ELEMENT;
        } else if value_declaration.is_some() && is_var_const(value_declaration) {
            return ScriptElementKind::CONST_ELEMENT;
        } else if value_declaration.is_some() && is_var_using(value_declaration) {
            return ScriptElementKind::VARIABLE_USING_ELEMENT;
        } else if value_declaration.is_some() && is_var_await_using(value_declaration) {
            return ScriptElementKind::VARIABLE_AWAIT_USING_ELEMENT;
        } else if symbols.sym(symbol).declarations.iter().any(|&d| is_let(d)) {
            return ScriptElementKind::LET_ELEMENT;
        }
        if is_local_variable_or_function(symbols, symbol) {
            return ScriptElementKind::LOCAL_VARIABLE_ELEMENT;
        }
        return ScriptElementKind::VARIABLE_ELEMENT;
    }
    if flags.intersects(SymbolFlags::FUNCTION) {
        if is_local_variable_or_function(symbols, symbol) {
            return ScriptElementKind::LOCAL_FUNCTION_ELEMENT;
        }
        return ScriptElementKind::FUNCTION_ELEMENT;
    }
    // FIXME: getter and setter use the same symbol. And it is rare to use only setter without getter, so in most cases the symbol always has getter flag.
    // So, even when the location is just on the declaration of setter, this function returns getter.
    if flags.intersects(SymbolFlags::GET_ACCESSOR) {
        return ScriptElementKind::MEMBER_GET_ACCESSOR_ELEMENT;
    }
    if flags.intersects(SymbolFlags::SET_ACCESSOR) {
        return ScriptElementKind::MEMBER_SET_ACCESSOR_ELEMENT;
    }
    if flags.intersects(SymbolFlags::METHOD) {
        return ScriptElementKind::MEMBER_FUNCTION_ELEMENT;
    }
    if flags.intersects(SymbolFlags::CONSTRUCTOR) {
        return ScriptElementKind::CONSTRUCTOR_IMPLEMENTATION_ELEMENT;
    }
    if flags.intersects(SymbolFlags::SIGNATURE) {
        return ScriptElementKind::INDEX_SIGNATURE_ELEMENT;
    }

    if flags.intersects(SymbolFlags::PROPERTY) {
        if matches!(type_checker, TypeChecker::Checker(_))
            && flags.intersects(SymbolFlags::TRANSIENT)
            && symbols
                .sym_check_flags(symbol)
                .intersects(CheckFlags::SYNTHETIC)
        {
            // If union property is result of union of non method (property/accessors/variables), it is labeled as property
            let mut union_property_kind = ScriptElementKind::UNKNOWN;
            for &root_symbol in &roots {
                if symbols
                    .sym_flags(root_symbol)
                    .intersects(SymbolFlags::PROPERTY_OR_ACCESSOR | SymbolFlags::VARIABLE)
                {
                    union_property_kind = ScriptElementKind::MEMBER_VARIABLE_ELEMENT;
                    break;
                }
            }
            if union_property_kind == ScriptElementKind::UNKNOWN {
                // If this was union of all methods,
                // make sure it has call signatures before we can label it as method.
                let c = type_checker
                    .checker()
                    .expect("checked TypeChecker::Checker above");
                let type_of_union_property = c.get_type_of_symbol_at_location(symbol, location);
                if c.get_call_signatures(type_of_union_property).len() > 0 {
                    return ScriptElementKind::MEMBER_FUNCTION_ELEMENT;
                }
                return ScriptElementKind::MEMBER_VARIABLE_ELEMENT;
            }
            return union_property_kind;
        }

        return ScriptElementKind::MEMBER_VARIABLE_ELEMENT;
    }

    ScriptElementKind::UNKNOWN
}

// Go: ls/lsutil/symbol_display.go:299 isFirstDeclarationOfSymbolParameter
fn is_first_declaration_of_symbol_parameter(symbols: &SymbolArena, symbol: SymbolId) -> bool {
    let mut declaration = Node::NIL;
    let declarations = &symbols.sym(symbol).declarations;
    if !declarations.is_empty() {
        declaration = declarations[0];
    }
    let result = find_ancestor_or_quit(declaration, |n| {
        if is_parameter_declaration(n) {
            return FindAncestorResult::FIND_ANCESTOR_TRUE;
        }
        if is_binding_element(n) || is_object_binding_pattern(n) || is_array_binding_pattern(n) {
            return FindAncestorResult::FIND_ANCESTOR_FALSE;
        }
        FindAncestorResult::FIND_ANCESTOR_QUIT
    });

    result.is_some()
}

// Go: ls/lsutil/symbol_display.go:317 isLocalVariableOrFunction
fn is_local_variable_or_function(symbols: &SymbolArena, symbol: SymbolId) -> bool {
    let s = symbols.sym(symbol);
    if s.parent.is_some() {
        return false; // This is exported symbol
    }

    for &decl in s.declarations.iter() {
        // Function expressions are local
        if decl.kind() == SyntaxKind::FunctionExpression {
            return true;
        }

        if decl.kind() != SyntaxKind::VariableDeclaration
            && decl.kind() != SyntaxKind::FunctionDeclaration
        {
            continue;
        }

        // If the parent is not source file or module block, it is a local variable.
        let mut parent = decl.parent();
        while !is_function_block(parent) {
            // Reached source file or module block
            if parent.kind() == SyntaxKind::SourceFile || parent.kind() == SyntaxKind::ModuleBlock {
                break;
            }
            parent = parent.parent();
        }

        if is_function_block(parent) {
            // Parent is in function block.
            return true;
        }
    }
    false
}

// Go: ls/lsutil/symbol_display.go:349 GetSymbolModifiers
pub fn get_symbol_modifiers<'a>(
    type_checker: impl Into<TypeChecker<'a>>,
    symbol: SymbolId,
) -> ScriptElementKindModifier {
    if symbol.is_nil() {
        return ScriptElementKindModifier::NONE;
    }

    let mut type_checker = type_checker.into();
    let mut modifiers = get_normalized_symbol_modifiers(&mut type_checker, symbol);
    if type_checker
        .symbols()
        .sym_flags(symbol)
        .intersects(SymbolFlags::ALIAS)
        && matches!(type_checker, TypeChecker::Checker(_))
    {
        let resolved_symbol = type_checker
            .checker()
            .expect("checked TypeChecker::Checker above")
            .get_aliased_symbol(symbol);
        if resolved_symbol != symbol {
            modifiers |= get_normalized_symbol_modifiers(&mut type_checker, resolved_symbol);
        }
    }
    if type_checker
        .symbols()
        .sym_flags(symbol)
        .intersects(SymbolFlags::OPTIONAL)
    {
        modifiers |= ScriptElementKindModifier::OPTIONAL;
    }

    modifiers
}

// Go: ls/lsutil/symbol_display.go:368 getNormalizedSymbolModifiers
fn get_normalized_symbol_modifiers(
    type_checker: &mut TypeChecker<'_>,
    symbol: SymbolId,
) -> ScriptElementKindModifier {
    let mut modifier_set = ScriptElementKindModifier::NONE;
    let symbol_declarations: Vec<Node> = type_checker.symbols().sym(symbol).declarations.to_vec();
    if !symbol_declarations.is_empty() {
        let declaration = symbol_declarations[0];
        let declarations = &symbol_declarations[1..];
        // omit deprecated flag if some declarations are not deprecated
        let exclude_flags;
        if !declarations.is_empty()
            && is_deprecated_declaration(type_checker.checker(), declaration) // !!! include jsdoc node flags
            && declarations
                .iter()
                .any(|&d| !is_deprecated_declaration(type_checker.checker(), d))
        {
            exclude_flags = ModifierFlags::DEPRECATED;
        } else {
            exclude_flags = ModifierFlags::NONE;
        }
        modifier_set = get_node_modifiers(type_checker.checker(), declaration, exclude_flags);
    }

    modifier_set
}

// Go: ls/lsutil/symbol_display.go:388 isDeprecatedDeclaration
// PORT: Go unexported and used only in this file. It is `pub` because the
// lsutil prelude picks it over `ast::is_deprecated_declaration`.
pub fn is_deprecated_declaration(type_checker: Option<&mut Checker>, declaration: Node) -> bool {
    if let Some(c) = type_checker {
        return c.is_deprecated_declaration(declaration);
    }
    crate::ast::is_deprecated_declaration(declaration)
}

// Go: ls/lsutil/symbol_display.go:395 getNodeModifiers
fn get_node_modifiers(
    type_checker: Option<&mut Checker>,
    node: Node,
    exclude_flags: ModifierFlags,
) -> ScriptElementKindModifier {
    let mut result = ScriptElementKindModifier::NONE;
    let mut flags = ModifierFlags::NONE;
    if is_declaration(node) {
        flags = get_combined_modifier_flags(node);
        if is_deprecated_declaration(type_checker, node) {
            flags |= ModifierFlags::DEPRECATED;
        }
        flags = flags.without(exclude_flags);
    }

    if flags.intersects(ModifierFlags::PRIVATE) {
        result |= ScriptElementKindModifier::PRIVATE;
    }
    if flags.intersects(ModifierFlags::PROTECTED) {
        result |= ScriptElementKindModifier::PROTECTED;
    }
    if flags.intersects(ModifierFlags::PUBLIC) {
        result |= ScriptElementKindModifier::PUBLIC;
    }
    if flags.intersects(ModifierFlags::STATIC) {
        result |= ScriptElementKindModifier::STATIC;
    }
    if flags.intersects(ModifierFlags::ABSTRACT) {
        result |= ScriptElementKindModifier::ABSTRACT;
    }
    if flags.intersects(ModifierFlags::EXPORT) {
        result |= ScriptElementKindModifier::EXPORTED;
    }
    if flags.intersects(ModifierFlags::DEPRECATED) {
        result |= ScriptElementKindModifier::DEPRECATED;
    }
    if flags.intersects(ModifierFlags::AMBIENT) {
        result |= ScriptElementKindModifier::AMBIENT;
    }
    if node.flags().intersects(NodeFlags::AMBIENT) {
        result |= ScriptElementKindModifier::AMBIENT;
    }
    if node.kind() == SyntaxKind::ExportAssignment {
        result |= ScriptElementKindModifier::EXPORTED;
    }

    result
}

//! Port of Go `ls/findallreferences.go` lines 1246-2654: the search half of
//! find-all-references (special searches, module references and the
//! `refState` search engine).

use crate::ls::prelude::*;

use crate::astnav;
use crate::gostd::Context;
use crate::scanner_util::{
    GoOffsets, contains_go_string_marker, go_byte_offset, go_string_bytes, go_unit_at,
    go_unit_before, go_unit_bytes,
};
use std::borrow::Cow;

// PORT (whole file):
// - The types of Go lines 29-473 (`ReferenceEntry`, `SymbolAndEntries`,
//   `Definition`, `RefOptions`, ...) are in findallreferences_p1.rs. Go
//   `*ReferenceEntry` and `*SymbolAndEntries` are `Rc<RefCell<..>>`
//   (map-ls-navigation 2.2).
// - Go `*checker.Checker` parameters are `&mut Checker`. `refState` holds the
//   checker for the whole search (`RefState<'c, P>`).
// - Go `*compiler.Program` parameters are `program: P` with
//   `P: ProgramView` (program_view.rs), so that the search can run on a
//   search thread.
// - Go `[]*ast.SourceFile` is `&[Node]` (file roots). The source file name set
//   (`*collections.Set[string]`) is `&FxHashSet<String>`.
// - Go positions (`int`) are `i32` byte offsets.
// - Go `func(...)` parameters are `&mut dyn FnMut(...)`. A callback that
//   calls the checker gets it as its first argument.
// - A Go function that returns `[]*SymbolAndEntries` returns a `Vec`; Go nil
//   and empty are the same `Vec` unless a caller tells them apart (then the
//   function returns `Option<Vec<..>>` and says so).

impl<P: ProgramView> LanguageService<P> {
    // Go: ls/findallreferences.go:1246 (*LanguageService).getReferencesForStringLiteral
    pub fn get_references_for_string_literal(
        &self,
        ctx: &Context,
        node: Node, /*StringLiteralLike*/
        source_files: &[Node],
        checker: &mut Checker,
    ) -> Vec<Rc<RefCell<SymbolAndEntries>>> {
        let t = get_contextual_type_from_parent_or_ancestor_type_node(node, checker);
        // core.FlatMap
        let mut references: Vec<Rc<RefCell<ReferenceEntry>>> = Vec::new();
        for &source_file in source_files {
            if ctx.err().is_some() {
                // Go: the FlatMap callback returns nil for this file.
                continue;
            }
            let mut entries: Vec<Rc<RefCell<ReferenceEntry>>> = Vec::new();
            let possible_references = get_possible_symbol_reference_nodes(
                source_file,
                node.text(),
                Node::NIL, /*container*/
            );
            for ref_ in possible_references {
                if is_string_literal_like(ref_) && ref_.text() == node.text() {
                    if t.is_some() {
                        let ref_type =
                            get_contextual_type_from_parent_or_ancestor_type_node(ref_, checker);
                        if t != checker.get_string_type()
                            && (t == ref_type
                                || is_string_literal_property_reference(ref_, checker))
                        {
                            entries.push(new_node_entry_with_kind(ref_, EntryKind::STRING_LITERAL));
                        }
                    } else {
                        // PORT: Go calls `ast.IsNoSubstitutionTemplateLiteral` (the ls
                        // prelude picks the ls function of the same name).
                        if crate::ast::is_no_substitution_template_literal(ref_)
                            && !range_is_on_single_line(ref_.loc(), source_file)
                        {
                            continue;
                        }
                        entries.push(new_node_entry_with_kind(ref_, EntryKind::STRING_LITERAL));
                    }
                }
            }
            references.extend(entries);
        }

        vec![Rc::new(RefCell::new(SymbolAndEntries {
            definition: Some(Definition {
                kind: DefinitionKind::STRING,
                symbol: SymbolId::NIL,
                node,
                triple_slash_file_ref: None,
            }),
            references,
        }))]
    }
}

// Go: ls/findallreferences.go:1394 isStringLiteralPropertyReference
pub fn is_string_literal_property_reference(
    node: Node, /*StringLiteralLike*/
    checker: &mut Checker,
) -> bool {
    if is_property_signature_declaration(node.parent()) {
        let t = checker.get_type_at_location(node.parent().parent());
        return checker
            .get_property_of_type_exported(t, node.text())
            .is_some();
    }
    false
}

impl<P: ProgramView> LanguageService<P> {
    // Go: ls/findallreferences.go:1293 (*LanguageService).getReferencedSymbolsForModuleIfDeclaredBySourceFile
    // PORT: returns None for Go nil. The caller (`getReferencedSymbolsForNode`)
    // tests `moduleReferences != nil`, and an empty non-nil result stops the
    // search there.
    #[allow(clippy::too_many_arguments)]
    pub fn get_referenced_symbols_for_module_if_declared_by_source_file(
        &self,
        ctx: &Context,
        symbol: SymbolId,
        program: &P,
        source_files: &[Node],
        checker: &mut Checker,
        options: RefOptions,
        source_files_set: &FxHashSet<String>,
    ) -> Option<Vec<Rc<RefCell<SymbolAndEntries>>>> {
        if symbol.is_nil()
            || !(checker.sym(symbol).flags.intersects(SymbolFlags::MODULE)
                && !checker.sym(symbol).declarations.is_empty())
        {
            return None;
        }
        let module_source_file_name: &'static str;
        let module_source_file = checker
            .sym(symbol)
            .declarations
            .iter()
            .copied()
            .find(|&decl| is_source_file(decl))
            .unwrap_or(Node::NIL);
        if module_source_file.is_some() {
            module_source_file_name = source_file_file_name(module_source_file);
        } else {
            return None;
        }
        let export_equals = checker.symbols.get(
            checker.sym(symbol).exports,
            INTERNAL_SYMBOL_NAME_EXPORT_EQUALS,
        );
        // If exportEquals != nil, we're about to add references to `import("mod")` anyway, so don't double-count them.
        let module_references = self.get_referenced_symbols_for_module(
            checker,
            program,
            symbol,
            export_equals.is_some(),
            source_files,
            source_files_set,
        );
        if export_equals.is_nil()
            || !checker
                .sym(export_equals)
                .flags
                .intersects(SymbolFlags::ALIAS)
            || !source_files_set.contains(module_source_file_name)
        {
            return Some(module_references);
        }
        let (symbol, _) = checker.resolve_alias_exported(export_equals);
        let symbol_references = get_referenced_symbols_for_symbol(
            ctx,
            program,
            symbol,
            Node::NIL, /*node*/
            source_files,
            source_files_set,
            checker, /*, cancellationToken*/
            options,
        );
        Some(self.merge_references(program, vec![module_references, symbol_references]))
    }
}

// Go: ls/findallreferences.go:1421 getReferencedSymbolsSpecial
// PORT: every non-nil Go result has one element, so an empty `Vec` is Go nil.
pub fn get_referenced_symbols_special(
    node: Node,
    source_files: &[Node],
) -> Vec<Rc<RefCell<SymbolAndEntries>>> {
    if is_type_keyword(node.kind()) {
        // A void expression (i.e., `void foo()`) is not special, but the `void` type is.
        if node.kind() == SyntaxKind::VoidKeyword
            && node.parent().kind() == SyntaxKind::VoidExpression
        {
            return Vec::new();
        }

        // A modifier readonly (like on a property declaration) is not special;
        // a readonly type keyword (like `readonly string[]`) is.
        if node.kind() == SyntaxKind::ReadonlyKeyword && !is_readonly_type_operator(node) {
            return Vec::new();
        }
        // Likewise, when we *are* looking for a special keyword, make sure we
        // *don't* include readonly member modifiers.
        return get_all_references_for_keyword(
            source_files,
            node.kind(),
            // cancellationToken,
            node.kind() == SyntaxKind::ReadonlyKeyword,
        );
    }

    if is_import_meta(node.parent()) && node.parent().name() == node {
        return get_all_references_for_import_meta(source_files);
    }

    if node.kind() == SyntaxKind::StaticKeyword
        && node.parent().kind() == SyntaxKind::ClassStaticBlockDeclaration
    {
        return vec![Rc::new(RefCell::new(SymbolAndEntries {
            definition: Some(Definition {
                kind: DefinitionKind::KEYWORD,
                symbol: SymbolId::NIL,
                node,
                triple_slash_file_ref: None,
            }),
            references: vec![new_node_entry(node)],
        }))];
    }

    // Labels
    if is_jump_statement_target(node) {
        // if we have a label definition, look within its statement for references, if not, then
        // the label is undefined and we have no results..
        let label_definition = get_target_label(node.parent(), node.text());
        if label_definition.is_some() {
            return get_label_references_in_node(label_definition.parent(), label_definition);
        }
        return Vec::new();
    }

    if is_label_of_labeled_statement(node) {
        // it is a label definition and not a target, search within the parent labeledStatement
        return get_label_references_in_node(node.parent(), node);
    }

    if is_this(node) {
        return get_references_for_this_keyword(node, source_files /*, cancellationToken*/);
    }

    if node.kind() == SyntaxKind::SuperKeyword {
        return get_references_for_super_keyword(node);
    }

    Vec::new()
}

// Go: ls/findallreferences.go:1477 getLabelReferencesInNode
pub fn get_label_references_in_node(
    container: Node,
    target_label: Node,
) -> Vec<Rc<RefCell<SymbolAndEntries>>> {
    let source_file = get_source_file_of_node(container);
    let label_name = target_label.text();
    // core.MapNonNil
    let references: Vec<Rc<RefCell<ReferenceEntry>>> =
        get_possible_symbol_reference_nodes(source_file, label_name, container)
            .into_iter()
            .filter_map(|node| {
                // Only pick labels that are either the target label, or have a target that is the target label
                if node == target_label
                    || (is_jump_statement_target(node)
                        && get_target_label(node, label_name) == target_label)
                {
                    return Some(new_node_entry(node));
                }
                None
            })
            .collect();
    vec![new_symbol_and_entries(
        DefinitionKind::LABEL,
        target_label,
        SymbolId::NIL,
        references,
    )]
}

// Go: ls/findallreferences.go:1490 getReferencesForThisKeyword
pub fn get_references_for_this_keyword(
    this_or_super_keyword: Node,
    source_files: &[Node],
) -> Vec<Rc<RefCell<SymbolAndEntries>>> {
    let mut search_space_node = get_this_container(
        this_or_super_keyword,
        false, /*includeArrowFunctions*/
        false, /*includeClassComputedPropertyName*/
    );

    // Whether 'this' occurs in a static context within a class.
    let mut static_flag = ModifierFlags::STATIC;
    // Go: ls/findallreferences.go:1495 isParameterName (closure)
    fn is_parameter_name(node: Node) -> bool {
        node.kind() == SyntaxKind::Identifier
            && node.parent().kind() == SyntaxKind::Parameter
            && node.parent().name() == node
    }

    match search_space_node.kind() {
        SyntaxKind::MethodDeclaration
        | SyntaxKind::MethodSignature
        | SyntaxKind::PropertyDeclaration
        | SyntaxKind::PropertySignature
        | SyntaxKind::Constructor
        | SyntaxKind::GetAccessor
        | SyntaxKind::SetAccessor => {
            if (search_space_node.kind() == SyntaxKind::MethodDeclaration
                || search_space_node.kind() == SyntaxKind::MethodSignature)
                && is_object_literal_method(search_space_node)
            {
                static_flag = static_flag & search_space_node.modifier_flags();
                search_space_node = search_space_node.parent(); // re-assign to be the owning object literals
            } else {
                static_flag = static_flag & search_space_node.modifier_flags();
                search_space_node = search_space_node.parent(); // re-assign to be the owning class
            }
        }
        SyntaxKind::SourceFile => {
            if is_external_module(search_space_node) || is_parameter_name(this_or_super_keyword) {
                return Vec::new();
            }
        }
        SyntaxKind::FunctionDeclaration | SyntaxKind::FunctionExpression => {
            // Computed properties in classes are not handled here because references to this are illegal,
            // so there is no point finding references to them.
        }
        _ => return Vec::new(),
    }

    let files_to_search: Vec<Node> = if search_space_node.kind() != SyntaxKind::SourceFile {
        vec![get_source_file_of_node(search_space_node)]
    } else {
        source_files.to_vec()
    };
    // core.Map(core.FlatMap(filesToSearch, ..), newNodeEntry)
    let mut reference_nodes: Vec<Node> = Vec::new();
    for &source_file in &files_to_search {
        // cancellationToken.throwIfCancellationRequested();
        let container = if search_space_node.kind() == SyntaxKind::SourceFile {
            source_file
        } else {
            search_space_node
        };
        for node in get_possible_symbol_reference_nodes(source_file, "this", container) {
            let keep = (|| {
                if !is_this(node) {
                    return false;
                }
                let container = get_this_container(
                    node, false, /*includeArrowFunctions*/
                    false, /*includeClassComputedPropertyName*/
                );
                if !can_have_symbol(container) {
                    return false;
                }
                match search_space_node.kind() {
                    SyntaxKind::FunctionExpression | SyntaxKind::FunctionDeclaration => {
                        search_space_node.symbol() == container.symbol()
                    }
                    SyntaxKind::MethodDeclaration | SyntaxKind::MethodSignature => {
                        is_object_literal_method(search_space_node)
                            && search_space_node.symbol() == container.symbol()
                    }
                    SyntaxKind::ClassExpression
                    | SyntaxKind::ClassDeclaration
                    | SyntaxKind::ObjectLiteralExpression => {
                        // Make sure the container belongs to the same class/object literals
                        // and has the appropriate static modifier from the original container.
                        container.parent().is_some()
                            && can_have_symbol(container.parent())
                            && search_space_node.symbol() == container.parent().symbol()
                            && is_static(container) == (static_flag != ModifierFlags::NONE)
                    }
                    SyntaxKind::SourceFile => {
                        container.kind() == SyntaxKind::SourceFile
                            && !is_external_module(container)
                            && !is_parameter_name(node)
                    }
                    _ => false,
                }
            })();
            if keep {
                reference_nodes.push(node);
            }
        }
    }
    let references: Vec<Rc<RefCell<ReferenceEntry>>> =
        reference_nodes.into_iter().map(new_node_entry).collect();

    // core.FirstNonNil
    let mut this_parameter = references
        .iter()
        .find_map(|reference| {
            let node = reference.borrow().node;
            if node.parent().kind() == SyntaxKind::Parameter {
                return Some(node);
            }
            None
        })
        .unwrap_or(Node::NIL);
    if this_parameter.is_nil() {
        this_parameter = this_or_super_keyword;
    }
    vec![new_symbol_and_entries(
        DefinitionKind::THIS,
        this_parameter,
        search_space_node.symbol(),
        references,
    )]
}

// Go: ls/findallreferences.go:1568 getReferencesForSuperKeyword
pub fn get_references_for_super_keyword(super_keyword: Node) -> Vec<Rc<RefCell<SymbolAndEntries>>> {
    let mut search_space_node = get_super_container(super_keyword, false /*stopOnFunctions*/);
    if search_space_node.is_nil() {
        return Vec::new();
    }
    // Whether 'super' occurs in a static context within a class.
    let mut static_flag = ModifierFlags::STATIC;

    match search_space_node.kind() {
        SyntaxKind::PropertyDeclaration
        | SyntaxKind::PropertySignature
        | SyntaxKind::MethodDeclaration
        | SyntaxKind::MethodSignature
        | SyntaxKind::Constructor
        | SyntaxKind::GetAccessor
        | SyntaxKind::SetAccessor => {
            static_flag = static_flag & search_space_node.modifier_flags();
            search_space_node = search_space_node.parent(); // re-assign to be the owning class
        }
        _ => return Vec::new(),
    }

    let source_file = get_source_file_of_node(search_space_node);
    // core.MapNonNil
    let references: Vec<Rc<RefCell<ReferenceEntry>>> =
        get_possible_symbol_reference_nodes(source_file, "super", search_space_node)
            .into_iter()
            .filter_map(|node| {
                if node.kind() != SyntaxKind::SuperKeyword {
                    return None;
                }

                let container = get_super_container(node, false /*stopOnFunctions*/);

                // If we have a 'super' container, we must have an enclosing class.
                // Now make sure the owning class is the same as the search-space
                // and has the same static qualifier as the original 'super's owner.
                if container.is_some()
                    && is_static(container) == (static_flag != ModifierFlags::NONE)
                    && container.parent().symbol() == search_space_node.symbol()
                {
                    return Some(new_node_entry(node));
                }
                None
            })
            .collect();

    vec![new_symbol_and_entries(
        DefinitionKind::SYMBOL,
        Node::NIL,
        search_space_node.symbol(),
        references,
    )]
}

// Go: ls/findallreferences.go:1604 getAllReferencesForImportMeta
pub fn get_all_references_for_import_meta(
    source_files: &[Node],
) -> Vec<Rc<RefCell<SymbolAndEntries>>> {
    // core.FlatMap / core.MapNonNil
    let mut references: Vec<Rc<RefCell<ReferenceEntry>>> = Vec::new();
    for &source_file in source_files {
        for node in get_possible_symbol_reference_nodes(source_file, "meta", source_file) {
            let parent = node.parent();
            if is_import_meta(parent) {
                references.push(new_node_entry(parent));
            }
        }
    }
    if references.is_empty() {
        return Vec::new();
    }
    let first_node = references[0].borrow().node;
    vec![Rc::new(RefCell::new(SymbolAndEntries {
        definition: Some(Definition {
            kind: DefinitionKind::KEYWORD,
            symbol: SymbolId::NIL,
            node: first_node,
            triple_slash_file_ref: None,
        }),
        references,
    }))]
}

// Go: ls/findallreferences.go:1620 getAllReferencesForKeyword
pub fn get_all_references_for_keyword(
    source_files: &[Node],
    keyword_kind: SyntaxKind,
    filter_read_only_type_operator: bool,
) -> Vec<Rc<RefCell<SymbolAndEntries>>> {
    // references is a list of NodeEntry
    // core.FlatMap / core.MapNonNil
    let mut references: Vec<Rc<RefCell<ReferenceEntry>>> = Vec::new();
    for &source_file in source_files {
        // cancellationToken.throwIfCancellationRequested();
        for reference_location in get_possible_symbol_reference_nodes(
            source_file,
            token_to_string(keyword_kind),
            source_file,
        ) {
            if reference_location.kind() == keyword_kind
                && (!filter_read_only_type_operator
                    || is_readonly_type_operator(reference_location))
            {
                references.push(new_node_entry(reference_location));
            }
        }
    }
    if references.is_empty() {
        return Vec::new();
    }
    let first_node = references[0].borrow().node;
    vec![new_symbol_and_entries(
        DefinitionKind::KEYWORD,
        first_node,
        SymbolId::NIL,
        references,
    )]
}

// Go: ls/findallreferences.go:1637 getPossibleSymbolReferenceNodes
// PORT: the crate prelude also exports the checker/services.go function of
// this name; this file uses its own, as Go package `ls` does.
pub fn get_possible_symbol_reference_nodes(
    source_file: Node,
    symbol_name: &str,
    container: Node,
) -> Vec<Node> {
    // core.MapNonNil
    let mut result: Vec<Node> = Vec::new();
    for pos in get_possible_symbol_reference_positions(source_file, symbol_name, container) {
        let reference_location = astnav::get_touching_property_name(source_file, pos);
        if reference_location != source_file {
            result.push(reference_location);
        }
    }
    result
}

// Go: ls/findallreferences.go:1646 getPossibleSymbolReferencePositions
// PORT: Go indexes the text by byte. The search runs on the bytes, because
// `position + symbolNameLength + 1` can fall inside a multi-byte character,
// where a `&str` slice would panic.
// PORT: Go searches the Go bytes. The text is in the source form and a
// string literal's name in the value form (see
// `scanner_util::GO_STRING_MARKER`): a lone surrogate is 3 invalid byte
// units in the text and one surrogate unit in the name, so a port search
// does not find it. A name with a marker unit, or with a char that can be
// the second char of one (lead byte 0xF4), is searched on the Go bytes
// (`go_string_bytes`) and the Go positions are mapped back
// (`port_byte_offset`, with one scan for all of them: `GoOffsets`). So is
// any name in a text with a marker when the container does not start the
// text: as in Go, the first index is relative
// to the container start and is then used as an absolute position, which
// is only the same position in both forms when the bytes before agree.
// Otherwise the port search finds the same positions. When the container
// starts the text, each is a match of whole units, and the bytes next to it
// are read as Go bytes (`go_unit_before`, `go_unit_at`), since a marker
// unit has other bytes than Go's. With another container the text has no
// marker, so its bytes are Go's.
// PERF: Go `strings.Index` is one `memmem::Finder`, built once per call. It
// returns the same first index. A byte-by-byte compare cost 29% to 56% of
// find-references time on large files such as lib.dom.d.ts. The usual name
// costs no scan for markers: a byte before a match is a unit's only when it
// is not ASCII, and a byte after one only when it is the marker's lead byte.
pub fn get_possible_symbol_reference_positions(
    source_file: Node,
    symbol_name: &str,
    container: Node,
) -> Vec<i32> {
    let mut positions: Vec<i32> = Vec::new();

    // TODO: Cache symbol existence for files to save text search
    // Also, need to make this work for unicode escapes.

    // Be resilient in the face of a symbol with no name or zero length name
    if symbol_name.is_empty() {
        return positions;
    }

    let text_text = source_file_text(source_file);

    let container = if container.is_nil() {
        source_file
    } else {
        container
    };

    let go_search = contains_go_string_marker(symbol_name)
        || symbol_name.as_bytes().contains(&0xF4)
        || (container.pos() > 0 && contains_go_string_marker(&text_text));
    let (text, symbol_name, container_pos, end_pos) = if go_search {
        (
            go_string_bytes(&text_text),
            go_string_bytes(symbol_name),
            go_byte_offset(&text_text, container.pos()),
            go_byte_offset(&text_text, container.end()),
        )
    } else {
        (
            Cow::Borrowed(text_text.as_bytes()),
            Cow::Borrowed(symbol_name.as_bytes()),
            container.pos(),
            container.end(),
        )
    };
    // Go `text[position-1]` and `text[endPosition]`. The port search reads
    // a unit next to a match only when the container starts the text. With
    // another container the first position, relative to the container, can
    // fall inside a char, and the text has no marker.
    let units = !go_search && container.pos() == 0;
    let go_byte_before = |position: usize| -> u8 {
        let b = text[position - 1];
        if !units || b < 0x80 {
            return b;
        }
        let mut buf = [0u8; 4];
        let (unit, _) = go_unit_before(&text_text, position);
        *go_unit_bytes(unit, &mut buf)
            .last()
            .expect("a unit has Go bytes")
    };
    let go_byte_at = |position: usize| -> u8 {
        let b = text[position];
        if !units || b != 0xEF {
            return b;
        }
        let mut buf = [0u8; 4];
        let (unit, _) = go_unit_at(&text_text, position);
        go_unit_bytes(unit, &mut buf)[0]
    };
    let source_length = text.len() as i32;
    let symbol_name_length = symbol_name.len() as i32;
    // Go `strings.Index(s, symbolName)` is `finder.find(s)`; -1 is `None`.
    let finder = memchr::memmem::Finder::new(&symbol_name);

    // PORT: as in Go, the first index is relative to `container.Pos()` and is
    // compared with the absolute `container.End()`.
    let mut position = finder
        .find(&text[container_pos as usize..])
        .map_or(-1, |index| index as i32);
    while position >= 0 && position < end_pos {
        // We found a match.  Make sure it's not part of a larger word (i.e. the char
        // before and after it have to be a non-identifier char).
        let end_position = position + symbol_name_length;

        // PORT: Go `rune(text[i])` converts one byte to a rune; `char::from`
        // does the same for a `u8`.
        if (position == 0 || !is_identifier_part(char::from(go_byte_before(position as usize))))
            && (end_position == source_length
                || !is_identifier_part(char::from(go_byte_at(end_position as usize))))
        {
            // Found a real match.  Keep searching.
            positions.push(position);
        }
        let start_index = position + symbol_name_length + 1;
        if start_index > source_length {
            break;
        }
        let found_index = finder
            .find(&text[start_index as usize..])
            .map_or(-1, |index| index as i32);
        if found_index != -1 {
            position = start_index + found_index;
        } else {
            break;
        }
    }

    if go_search {
        let offsets = GoOffsets::new(&text_text);
        for position in &mut positions {
            *position = offsets.port_offset(*position);
        }
    }
    positions
}

// Go: ls/findallreferences.go:1692 findFirstJsxNode
// findFirstJsxNode recursively searches for the first JSX element, self-closing element, or fragment
pub fn find_first_jsx_node(root: Node) -> Node {
    // Go: ls/findallreferences.go:1694 visit (closure)
    fn visit(node: Node) -> Node {
        // Check if this is a JSX node we're looking for
        match node.kind() {
            SyntaxKind::JsxElement
            | SyntaxKind::JsxSelfClosingElement
            | SyntaxKind::JsxFragment => {
                return node;
            }
            _ => {}
        }

        // Skip subtree if it doesn't contain JSX
        if !node
            .subtree_facts()
            .intersects(SubtreeFacts::SUBTREE_CONTAINS_JSX)
        {
            return Node::NIL;
        }

        // Traverse children to find JSX node
        let mut result = Node::NIL;
        node.for_each_child(|child| {
            result = visit(child);
            result.is_some() // Stop if found
        });
        result
    }

    visit(root)
}

// Go: ls/findallreferences.go:1718 getReferencesForNonModule
pub fn get_references_for_non_module<P: ProgramView>(
    _referenced_file: Node,
    _program: &P,
) -> Vec<Rc<RefCell<ReferenceEntry>>> {
    // !!! not implemented
    Vec::new()
}

// Go: ls/findallreferences.go:1723 getMergedAliasedSymbolOfNamespaceExportDeclaration
pub fn get_merged_aliased_symbol_of_namespace_export_declaration(
    node: Node,
    symbol: SymbolId,
    checker: &mut Checker,
) -> SymbolId {
    if node.parent().is_some() && node.parent().kind() == SyntaxKind::NamespaceExportDeclaration {
        let (aliased_symbol, ok) = checker.resolve_alias_exported(symbol);
        if ok {
            let target_symbol = checker.get_merged_symbol_exported(aliased_symbol);
            if aliased_symbol != target_symbol {
                return target_symbol;
            }
        }
    }
    SymbolId::NIL
}

impl<P: ProgramView> LanguageService<P> {
    // Go: ls/findallreferences.go:1735 (*LanguageService).getReferencedSymbolsForModule
    // ts#64543: the caller passes the checker it holds (acquisitions are not
    // reentrant). Before ts#64543 Go acquired it here.
    pub fn get_referenced_symbols_for_module(
        &self,
        checker: &mut Checker,
        program: &P,
        symbol: SymbolId,
        exclude_import_type_of_export_equals: bool,
        source_files: &[Node],
        source_files_set: &FxHashSet<String>,
    ) -> Vec<Rc<RefCell<SymbolAndEntries>>> {
        crate::go_assert!(checker.sym(symbol).value_declaration.is_some());

        let module_refs = find_module_references(program, source_files, symbol, checker);
        // core.MapNonNil
        let mut references: Vec<Rc<RefCell<ReferenceEntry>>> = module_refs
            .iter()
            .filter_map(|reference| -> Option<Rc<RefCell<ReferenceEntry>>> {
                match reference.kind {
                    ModuleReferenceKind::IMPORT => {
                        let parent = reference.literal.parent();
                        if is_literal_type_node(parent) {
                            let import_type = parent.parent();
                            if is_import_type_node(import_type)
                                && exclude_import_type_of_export_equals
                                && import_type.qualifier().is_nil()
                            {
                                return None;
                            }
                        }
                        // import("foo") with no qualifier will reference the `export =` of the module, which may be referenced anyway.
                        Some(new_node_entry(reference.literal))
                    }
                    ModuleReferenceKind::IMPLICIT => {
                        // For implicit references (e.g., JSX runtime imports), return the first JSX node,
                        // the first statement, or the whole file
                        let mut range_node = Node::NIL;

                        // Skip the JSX search for tslib imports
                        if reference.literal.text() != "tslib" {
                            range_node = find_first_jsx_node(reference.referencing_file);
                        }

                        if range_node.is_nil() {
                            let statements = reference.referencing_file.statements();
                            if !statements.is_empty() {
                                range_node = statements.get(0);
                            } else {
                                range_node = reference.referencing_file;
                            }
                        }
                        Some(new_node_entry(range_node))
                    }
                    ModuleReferenceKind::REFERENCE => {
                        // PORT: Go reads `reference.ref.TextRange`; this kind always has a ref.
                        let text_range = reference
                            .ref_
                            .as_ref()
                            .unwrap_or_else(|| crate::core::go_nil_dereference())
                            .range;
                        Some(Rc::new(RefCell::new(ReferenceEntry {
                            kind: EntryKind::RANGE,
                            source_file: reference.referencing_file,
                            text_range: Some(text_range),
                            ..Default::default()
                        })))
                    }
                    _ => None,
                }
            })
            .collect();

        // Add references to the module declarations themselves
        let declarations = checker.sym(symbol).declarations.clone();
        if !declarations.is_empty() {
            for decl in declarations {
                match decl.kind() {
                    SyntaxKind::SourceFile => {
                        // Don't include the source file itself. (This may not be ideal behavior, but awkward to include an entire file as a reference.)
                        continue;
                    }
                    SyntaxKind::ModuleDeclaration => {
                        if source_files_set
                            .contains(source_file_file_name(get_source_file_of_node(decl)))
                        {
                            references.push(new_node_entry(decl.name()));
                        }
                    }
                    _ => {
                        // This may be merged with something (e.g. a class merged with a namespace).
                        continue;
                    }
                }
            }
        }

        // Handle export equals declarations
        let exported = checker.symbols.get(
            checker.sym(symbol).exports,
            INTERNAL_SYMBOL_NAME_EXPORT_EQUALS,
        );
        if exported.is_some() && !checker.sym(exported).declarations.is_empty() {
            let exported_declarations = checker.sym(exported).declarations.clone();
            for decl in exported_declarations {
                let source_file = get_source_file_of_node(decl);
                if source_files_set.contains(source_file_file_name(source_file)) {
                    let node: Node;
                    // At `module.exports = ...`, reference node is `module`
                    if is_binary_expression(decl) && is_property_access_expression(decl.left()) {
                        node = decl.left().expression();
                    } else if is_export_assignment(decl) {
                        // Find the export keyword
                        node = astnav::find_child_of_kind(
                            decl,
                            SyntaxKind::ExportKeyword,
                            source_file,
                        );
                        crate::go_assert!(node.is_some(), "Expected to find export keyword");
                    } else {
                        let name = get_name_of_declaration(decl);
                        node = if name.is_nil() { decl } else { name };
                    }
                    references.push(new_node_entry(node));
                }
            }
        }

        if !references.is_empty() {
            return vec![Rc::new(RefCell::new(SymbolAndEntries {
                definition: Some(Definition {
                    kind: DefinitionKind::SYMBOL,
                    symbol,
                    node: Node::NIL,
                    triple_slash_file_ref: None,
                }),
                references,
            }))];
        }
        Vec::new()
    }
}

// -- Core algorithm for find all references --

// Go: ls/findallreferences.go:1838 getSpecialSearchKind
pub fn get_special_search_kind(node: Node) -> &'static str {
    if node.is_nil() {
        return "none";
    }
    match node.kind() {
        SyntaxKind::Constructor | SyntaxKind::ConstructorKeyword => "constructor",
        SyntaxKind::Identifier => {
            if is_class_like(node.parent()) {
                crate::go_assert!(node.parent().name() == node);
                return "class";
            }
            // fallthrough
            "none"
        }
        _ => "none",
    }
}

// Go: ls/findallreferences.go:1856 getReferencedSymbolsForSymbol
#[allow(clippy::too_many_arguments)]
pub fn get_referenced_symbols_for_symbol<P: ProgramView>(
    ctx: &Context,
    program: &P,
    original_symbol: SymbolId,
    node: Node,
    source_files: &[Node],
    source_files_set: &FxHashSet<String>,
    checker: &mut Checker,
    options: RefOptions,
) -> Vec<Rc<RefCell<SymbolAndEntries>>> {
    // Core find-all-references algorithm for a normal symbol.

    // core.Coalesce
    let mut symbol = skip_past_export_or_import_specifier_or_union(
        original_symbol,
        node,
        checker, /*useLocalSymbolForExportSpecifier*/
        !is_for_rename_with_prefix_and_suffix_text(options),
    );
    if symbol.is_nil() {
        symbol = original_symbol;
    }

    // Compute the meaning from the location and the symbol it references
    let mut search_meaning = SemanticMeaning::ALL;
    if options.use_ != ReferenceUse::RENAME {
        search_meaning = get_intersecting_meaning_from_declarations(
            &checker.symbols,
            node,
            symbol,
            SemanticMeaning::ALL,
        );
    }
    let mut state = new_state(
        ctx,
        program,
        source_files,
        source_files_set,
        node,
        checker,
        search_meaning,
        options,
    );

    let mut export_specifier = Node::NIL;
    if is_for_rename_with_prefix_and_suffix_text(options)
        && !state.checker.sym(symbol).declarations.is_empty()
    {
        // core.Find
        export_specifier = state
            .checker
            .sym(symbol)
            .declarations
            .iter()
            .copied()
            .find(|&decl| is_export_specifier(decl))
            .unwrap_or(Node::NIL);
    }
    if export_specifier.is_some() {
        // When renaming at an export specifier, rename the export and not the thing being exported.
        let search = state.create_search(
            node,
            original_symbol,
            ImpExpKind::UNKNOWN, /*comingFrom*/
            "",
            Vec::new(),
        );
        state.get_references_at_export_specifier(
            export_specifier.name(),
            symbol,
            export_specifier,
            &search,
            true, /*addReferencesHere*/
            true, /*alwaysGetReferences*/
        );
    } else if node.is_some()
        && node.kind() == SyntaxKind::DefaultKeyword
        && state.checker.sym(symbol).name.as_str() == INTERNAL_SYMBOL_NAME_DEFAULT
        && state.checker.sym(symbol).parent.is_some()
    {
        state.add_reference(node, symbol, EntryKind::NODE);
        let exporting_module_symbol = state.checker.sym(symbol).parent;
        state.search_for_imports_of_export(
            node,
            symbol,
            &ExportInfo {
                exporting_module_symbol,
                export_kind: ExportKind::DEFAULT,
            },
        );
    } else {
        let all_search_symbols = state.populate_search_symbol_set(
            symbol,
            node,
            options.use_ == ReferenceUse::RENAME,
            options.use_aliases_for_rename,
            options.implementations,
        );
        let search = state.create_search(
            node,
            symbol,
            ImpExpKind::UNKNOWN, /*comingFrom*/
            "",
            all_search_symbols,
        );
        state.get_references_in_container_or_files(symbol, &search);
    }

    state.result
}

// Go: ls/findallreferences.go:1888 refSearch
// Symbol that is currently being searched for.
// This will be replaced if we find an alias for the symbol.
// PORT: Go `includes` is a closure over `allSearchSymbols`; here it owns a
// copy of the list (map-ls-navigation 3.1).
pub struct RefSearch {
    // If coming from an export, we will not recursively search for the imported symbol (since that's where we came from).
    pub coming_from: ImpExpKind, // import, export

    pub symbol: SymbolId,
    pub text: String,
    pub escaped_text: String,

    // Only set if `options.implementations` is true. These are the symbols checked to get the implementations of a property access.
    pub parents: Vec<SymbolId>,

    pub all_search_symbols: Vec<SymbolId>,

    // Whether a symbol is in the search set.
    // Do not compare directly to `symbol` because there may be related symbols to search for. See `populateSearchSymbolSet`.
    pub includes: Rc<dyn Fn(SymbolId) -> bool>,
}

// Go: ls/findallreferences.go:1906 inheritKey
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct InheritKey {
    pub symbol: SymbolId,
    pub parent: SymbolId,
}

// Go: ls/findallreferences.go:1911 refState
// PORT: the state borrows the checker, the context and the source file list
// and set for the whole search (`'c`). Go `collections.Set[*ast.Node]` node
// seen trackers are `FxHashSet<Node>`. `symbolToReferences` and
// `sourceFileToSeenSymbols` are lookups only; `result` keeps the Go order.
pub struct RefState<'c, P> {
    pub source_files: &'c [Node],
    pub source_files_set: &'c FxHashSet<String>,
    pub special_search_kind: &'static str, // "none", "constructor", or "class"
    pub checker: &'c mut Checker,
    pub ctx: &'c Context,
    pub program: &'c P,
    pub search_meaning: SemanticMeaning,
    pub options: RefOptions,
    pub result: Vec<Rc<RefCell<SymbolAndEntries>>>,
    pub inherits_from_cache: FxHashMap<InheritKey, bool>,
    pub seen_containing_type_references: FxHashSet<Node>, // node seen tracker
    pub seen_re_export_rhs: FxHashSet<Node>,              // node seen tracker
    pub import_tracker: Option<ImportTracker<'c>>,
    pub symbol_to_references: FxHashMap<SymbolId, Rc<RefCell<SymbolAndEntries>>>,
    pub source_file_to_seen_symbols: FxHashMap<Node, FxHashSet<SymbolId>>,
}

// Go: ls/findallreferences.go:1929 newState
// PORT: Go returns `*refState`; the state is returned by value.
#[allow(clippy::too_many_arguments)]
pub fn new_state<'c, P: ProgramView>(
    ctx: &'c Context,
    program: &'c P,
    source_files: &'c [Node],
    source_files_set: &'c FxHashSet<String>,
    node: Node,
    checker: &'c mut Checker,
    search_meaning: SemanticMeaning,
    options: RefOptions,
) -> RefState<'c, P> {
    RefState {
        source_files,
        source_files_set,
        special_search_kind: get_special_search_kind(node),
        checker,
        ctx,
        program,
        search_meaning,
        options,
        result: Vec::new(),
        inherits_from_cache: FxHashMap::default(),
        seen_containing_type_references: FxHashSet::default(),
        seen_re_export_rhs: FxHashSet::default(),
        import_tracker: None,
        symbol_to_references: FxHashMap::default(),
        source_file_to_seen_symbols: FxHashMap::default(),
    }
}

impl<'c, P: ProgramView> RefState<'c, P> {
    // Go: ls/findallreferences.go:1837 (*refState).includesSourceFile
    pub fn includes_source_file(&self, source_file: Node) -> bool {
        self.source_files_set
            .contains(source_file_file_name(source_file))
    }

    // Go: ls/findallreferences.go:1841 (*refState).getImportSearches
    // PORT: Go returns `*ImportsResult`; the result is returned by value.
    pub fn get_import_searches(
        &mut self,
        export_symbol: SymbolId,
        export_info: &ExportInfo,
    ) -> ImportsResult {
        if self.import_tracker.is_none() {
            self.import_tracker = Some(create_import_tracker(
                self.ctx,
                self.program,
                self.source_files,
                self.source_files_set,
                self.checker,
            ));
        }
        let import_tracker = Rc::clone(
            self.import_tracker
                .as_ref()
                .expect("import tracker is set above"),
        );
        import_tracker(
            &mut *self.checker,
            export_symbol,
            export_info,
            self.options.use_ == ReferenceUse::RENAME,
        )
    }

    // Go: ls/findallreferences.go:1849 (*refState).createSearch
    // @param allSearchSymbols set of additional symbols for use by `includes`
    // PORT: Go `nil` for `allSearchSymbols` is an empty `Vec`.
    pub fn create_search(
        &mut self,
        location: Node,
        symbol: SymbolId,
        coming_from: ImpExpKind,
        text: &str,
        all_search_symbols: Vec<SymbolId>,
    ) -> RefSearch {
        // Note: if this is an external module symbol, the name doesn't include quotes.
        // Note: getLocalSymbolForExportDefault handles `export default class C {}`, but not `export default C` or `export { C as default }`.
        // The other two forms seem to be handled downstream (e.g. in `skipPastExportOrImportSpecifier`), so special-casing the first form
        // here appears to be intentional).
        let mut text = text.to_string();
        if text.is_empty() {
            let mut s = get_local_symbol_for_export_default(&self.checker.symbols, symbol);
            if s.is_nil() {
                s = get_non_module_symbol_of_merged_module_symbol(&self.checker.symbols, symbol);
                if s.is_nil() {
                    s = symbol;
                }
            }
            let symbol_name = symbol_name(&self.checker.symbols, s);
            if let Some(module_name) = try_get_ambient_module_name_from_symbol_name(&symbol_name) {
                text = module_name.to_string();
            } else {
                text = symbol_name.to_string();
            }
        }
        let mut all_search_symbols = all_search_symbols;
        if all_search_symbols.is_empty() {
            all_search_symbols = vec![symbol];
        }
        let includes_symbols = all_search_symbols.clone();
        let mut search = RefSearch {
            symbol,
            coming_from,
            text: text.clone(),
            escaped_text: text,
            parents: Vec::new(),
            all_search_symbols,
            includes: Rc::new(move |sym: SymbolId| includes_symbols.contains(&sym)),
        };
        if self.options.implementations && location.is_some() {
            search.parents = get_parent_symbols_of_property_access(location, symbol, self.checker);
        }
        search
    }

    // Go: ls/findallreferences.go:1881 (*refState).referenceAdder
    // PORT: the returned Go closure appends to the shared `SymbolAndEntries`;
    // here it owns an `Rc` to it, so it does not borrow the state.
    pub fn reference_adder(&mut self, search_symbol: SymbolId) -> Box<dyn Fn(Node, EntryKind)> {
        let symbol_and_entries = match self.symbol_to_references.get(&search_symbol) {
            Some(symbol_and_entries) => Rc::clone(symbol_and_entries),
            None => {
                let symbol_and_entries = new_symbol_and_entries(
                    DefinitionKind::SYMBOL,
                    Node::NIL,
                    search_symbol,
                    Vec::new(),
                );
                self.symbol_to_references
                    .insert(search_symbol, Rc::clone(&symbol_and_entries));
                self.result.push(Rc::clone(&symbol_and_entries));
                symbol_and_entries
            }
        };
        Box::new(move |node: Node, kind: EntryKind| {
            symbol_and_entries
                .borrow_mut()
                .references
                .push(new_node_entry_with_kind(node, kind));
        })
    }

    // Go: ls/findallreferences.go:1893 (*refState).addReference
    pub fn add_reference(&mut self, reference_location: Node, symbol: SymbolId, kind: EntryKind) {
        // if rename symbol from default export anonymous function, for example `export default function() {}`, we do not need to add reference
        if self.options.use_ == ReferenceUse::RENAME
            && reference_location.kind() == SyntaxKind::DefaultKeyword
        {
            return;
        }

        let add_ref = self.reference_adder(symbol);
        if self.options.implementations {
            self.add_implementation_references(reference_location, &mut |n| add_ref(n, kind));
        } else {
            add_ref(reference_location, kind);
        }
    }
}

// Go: ls/findallreferences.go:2020 getReferenceEntriesForShorthandPropertyAssignment
pub fn get_reference_entries_for_shorthand_property_assignment(
    node: Node,
    checker: &mut Checker,
    add_reference: &mut dyn FnMut(Node),
) {
    let ref_symbol = checker.get_symbol_at_location_exported(node);
    if ref_symbol.is_nil() || checker.sym(ref_symbol).value_declaration.is_nil() {
        return;
    }
    let ref_value_declaration = checker.sym(ref_symbol).value_declaration;
    let shorthand_symbol = checker.get_shorthand_assignment_value_symbol(ref_value_declaration);
    if shorthand_symbol.is_some() && !checker.sym(shorthand_symbol).declarations.is_empty() {
        let declarations = checker.sym(shorthand_symbol).declarations.clone();
        for declaration in declarations {
            // PORT: Go calls `ast.GetMeaningFromDeclaration` (the ls prelude
            // picks the ls function of the same name, which differs).
            if crate::ast::get_meaning_from_declaration(declaration)
                .intersects(SemanticMeaning::VALUE)
            {
                add_reference(declaration);
            }
        }
    }
}

// Go: ls/findallreferences.go:2035 isMethodOrAccessor
pub fn is_method_or_accessor(node: Node) -> bool {
    node.kind() == SyntaxKind::MethodDeclaration
        || node.kind() == SyntaxKind::GetAccessor
        || node.kind() == SyntaxKind::SetAccessor
}

// Go: ls/findallreferences.go:2039 tryGetClassByExtendingIdentifier
pub fn try_get_class_by_extending_identifier(node: Node) -> Node /*ClassLikeDeclaration*/ {
    try_get_class_extending_expression_with_type_arguments(
        climb_past_property_access(node).parent(),
    )
}

// Go: ls/findallreferences.go:2043 getClassConstructorSymbol
// PORT: Go reads the symbol fields directly; they are in the checker's arena.
pub fn get_class_constructor_symbol(symbols: &SymbolArena, class_symbol: SymbolId) -> SymbolId {
    let members = symbols.sym(class_symbol).members;
    if members.is_nil() {
        return SymbolId::NIL;
    }
    symbols.get(members, INTERNAL_SYMBOL_NAME_CONSTRUCTOR)
}

// Go: ls/findallreferences.go:2050 hasOwnConstructor
pub fn has_own_constructor(
    symbols: &SymbolArena,
    class_declaration: Node, /*ClassLikeDeclaration*/
) -> bool {
    get_class_constructor_symbol(symbols, class_declaration.symbol()).is_some()
}

// Go: ls/findallreferences.go:2054 findOwnConstructorReferences
pub fn find_own_constructor_references(
    symbols: &SymbolArena,
    class_symbol: SymbolId,
    source_file: Node,
    add_node: &mut dyn FnMut(Node),
) {
    let constructor_symbol = get_class_constructor_symbol(symbols, class_symbol);
    if constructor_symbol.is_some() && !symbols.sym(constructor_symbol).declarations.is_empty() {
        for &decl in symbols.sym(constructor_symbol).declarations.iter() {
            if decl.kind() == SyntaxKind::Constructor {
                let ctr_keyword =
                    astnav::find_child_of_kind(decl, SyntaxKind::ConstructorKeyword, source_file);
                if ctr_keyword.is_some() {
                    add_node(ctr_keyword);
                }
            }
        }
    }

    let class_exports = symbols.sym(class_symbol).exports;
    if class_exports.is_some() {
        // PORT: Go map order is random; this walks the table in insertion order.
        for (_, member) in symbols.iter(class_exports) {
            let decl = symbols.sym(member).value_declaration;
            if decl.is_some() && decl.kind() == SyntaxKind::MethodDeclaration {
                let body = decl.body();
                if body.is_some() {
                    for_each_descendant_of_kind(
                        body,
                        SyntaxKind::ThisKeyword,
                        &mut |this_keyword| {
                            if is_new_expression_target(this_keyword, false, false) {
                                add_node(this_keyword);
                            }
                        },
                    );
                }
            }
        }
    }
}

// Go: ls/findallreferences.go:2083 findSuperConstructorAccesses
pub fn find_super_constructor_accesses(
    symbols: &SymbolArena,
    class_declaration: Node, /*ClassLikeDeclaration*/
    add_node: &mut dyn FnMut(Node),
) {
    let constructor_symbol = get_class_constructor_symbol(symbols, class_declaration.symbol());
    if constructor_symbol.is_nil() || symbols.sym(constructor_symbol).declarations.is_empty() {
        return;
    }

    for &decl in symbols.sym(constructor_symbol).declarations.iter() {
        if decl.kind() == SyntaxKind::Constructor {
            let body = decl.body();
            if body.is_some() {
                for_each_descendant_of_kind(body, SyntaxKind::SuperKeyword, &mut |node| {
                    if is_call_expression_target(node, false, false) {
                        add_node(node);
                    }
                });
            }
        }
    }
}

// Go: ls/findallreferences.go:2103 forEachDescendantOfKind
pub fn for_each_descendant_of_kind(node: Node, kind: SyntaxKind, action: &mut dyn FnMut(Node)) {
    node.for_each_child(|child| {
        if child.kind() == kind {
            action(child);
        }
        for_each_descendant_of_kind(child, kind, &mut *action);
        false
    });
}

impl<'c, P: ProgramView> RefState<'c, P> {
    // Go: ls/findallreferences.go:2000 (*refState).addImplementationReferences
    pub fn add_implementation_references(&mut self, ref_node: Node, add_ref: &mut dyn FnMut(Node)) {
        // Check if we found a function/propertyAssignment/method with an implementation or initializer
        if is_declaration_name(ref_node) && is_implementation(ref_node.parent()) {
            add_ref(ref_node);
            return;
        }

        if ref_node.kind() != SyntaxKind::Identifier {
            return;
        }

        if ref_node.parent().kind() == SyntaxKind::ShorthandPropertyAssignment {
            // Go ahead and dereference the shorthand assignment by going to its definition
            get_reference_entries_for_shorthand_property_assignment(
                ref_node,
                self.checker,
                &mut *add_ref,
            );
        }

        // Check if the node is within an extends or implements clause

        let containing_node = get_containing_node_if_in_heritage_clause(ref_node);
        if containing_node.is_some() {
            add_ref(containing_node);
            return;
        }

        // If we got a type reference, try and see if the reference applies to any expressions that can implement an interface
        // Find the first node whose parent isn't a type node -- i.e., the highest type node.
        let type_node = find_ancestor(ref_node, |a| {
            !is_qualified_name(a.parent())
                && !is_type_node(a.parent())
                && !is_type_element(a.parent())
        });

        if type_node.is_nil() || type_node.parent().type_().is_nil() {
            return;
        }

        let type_having_node = type_node.parent();
        if type_having_node.type_() == type_node
            && self
                .seen_containing_type_references
                .insert(type_having_node)
        {
            // Go: ls/findallreferences.go:2148 addIfImplementation (closure)
            let mut add_if_implementation = |e: Node /*Expression*/| {
                if is_implementation_expression(e) {
                    add_ref(e);
                }
            };
            if has_initializer(type_having_node) {
                add_if_implementation(type_having_node.initializer());
            } else if is_function_like(type_having_node) && type_having_node.body().is_some() {
                let body = type_having_node.body();
                if body.kind() == SyntaxKind::Block {
                    for_each_return_statement(body, |return_statement| {
                        let expr = return_statement.expression();
                        if expr.is_some() {
                            add_if_implementation(expr);
                        }
                        false
                    });
                } else {
                    add_if_implementation(body);
                }
            } else if is_assertion_expression(type_having_node)
                || is_satisfies_expression(type_having_node)
            {
                add_if_implementation(type_having_node.expression());
            }
        }
    }

    // Go: ls/findallreferences.go:2060 (*refState).getReferencesInContainerOrFiles
    // PORT: `getSymbolScope` (findallreferences_p1.rs) takes the checker as its
    // first parameter, because it reads the symbol and calls
    // `IsExternalModuleSymbol` through it.
    pub fn get_references_in_container_or_files(&mut self, symbol: SymbolId, search: &RefSearch) {
        // Try to get the smallest valid scope that we can limit our search to;
        // otherwise we'll need to search globally (i.e. include each file).
        let scope = get_symbol_scope(&*self.checker, symbol);
        if scope.is_some() {
            let add_references_here =
                scope.kind() != SyntaxKind::SourceFile || self.source_files.contains(&scope);
            self.get_references_in_container(
                scope,
                get_source_file_of_node(scope),
                search,
                add_references_here,
            );
        } else {
            // Global search
            let source_files = self.source_files;
            for &source_file in source_files {
                // state.cancellationToken.throwIfCancellationRequested();
                self.search_for_name(source_file, search);
            }
        }
    }

    // Go: ls/findallreferences.go:2075 (*refState).getReferencesInSourceFile
    pub fn get_references_in_source_file(
        &mut self,
        source_file: Node,
        search: &RefSearch,
        add_references_here: bool,
    ) {
        // state.cancellationToken.throwIfCancellationRequested();
        self.get_references_in_container(source_file, source_file, search, add_references_here);
    }

    // Go: ls/findallreferences.go:2080 (*refState).getReferencesInContainer
    pub fn get_references_in_container(
        &mut self,
        container: Node,
        source_file: Node,
        search: &RefSearch,
        add_references_here: bool,
    ) {
        // Search within node "container" for references for a search value, where the search value is defined as a
        //     tuple of (searchSymbol, searchText, searchLocation, and searchMeaning).
        // searchLocation: a node where the search value
        if !self.mark_searched_symbols(source_file, &search.all_search_symbols) {
            return;
        }

        for position in
            get_possible_symbol_reference_positions(source_file, &search.text, container)
        {
            self.get_references_at_location(source_file, position, search, add_references_here);
        }
    }

    // Go: ls/findallreferences.go:2093 (*refState).markSearchedSymbols
    pub fn mark_searched_symbols(&mut self, source_file: Node, symbols: &[SymbolId]) -> bool {
        let seen_symbols = self
            .source_file_to_seen_symbols
            .entry(source_file)
            .or_default();
        let mut any_new_symbols = false;
        for &sym in symbols {
            if seen_symbols.insert(sym) {
                any_new_symbols = true;
            }
        }
        any_new_symbols
    }

    // Go: ls/findallreferences.go:2108 (*refState).getReferencesAtLocation
    pub fn get_references_at_location(
        &mut self,
        source_file: Node,
        position: i32,
        search: &RefSearch,
        add_references_here: bool,
    ) {
        let reference_location = astnav::get_touching_property_name(source_file, position);

        if !is_valid_reference_position(reference_location, &search.text) {
            // This wasn't the start of a token.  Check to see if it might be a
            // match in a comment or string if that's what the caller is asking
            // for.

            // !!! not implemented
            // if (!state.options.implementations && (state.options.findInStrings && isInString(sourceFile, position) || state.options.findInComments && isInNonReferenceComment(sourceFile, position))) {
            // 	// In the case where we're looking inside comments/strings, we don't have
            // 	// an actual definition.  So just use 'undefined' here.  Features like
            // 	// 'Rename' won't care (as they ignore the definitions), and features like
            // 	// 'FindReferences' will just filter out these results.
            // 	state.addStringOrCommentReference(sourceFile.FileName, createTextSpan(position, search.text.length));
            // }

            return;
        }

        if !get_meaning_from_location(reference_location).intersects(self.search_meaning) {
            return;
        }

        let mut reference_symbol = self
            .checker
            .get_symbol_at_location_exported(reference_location);
        if reference_symbol.is_nil() {
            return;
        }

        let parent = reference_location.parent();
        if parent.kind() == SyntaxKind::ImportSpecifier
            && parent.property_name() == reference_location
        {
            // This is added through `singleReferences` in ImportsResult. If we happen to see it again, don't add it again.
            return;
        }

        if parent.kind() == SyntaxKind::ExportSpecifier {
            self.get_references_at_export_specifier(
                reference_location,
                reference_symbol,
                parent,
                search,
                add_references_here,
                false, /*alwaysGetReferences*/
            );
            return;
        }

        let (related_symbol, related_symbol_kind) =
            self.get_related_symbol(search, reference_symbol, reference_location);
        if related_symbol.is_nil() {
            self.get_reference_for_shorthand_property(reference_symbol, search);
            return;
        }

        match self.special_search_kind {
            "none" => {
                if add_references_here {
                    self.add_reference(reference_location, related_symbol, related_symbol_kind);
                }
            }
            "constructor" => {
                self.add_constructor_references(
                    reference_location,
                    related_symbol,
                    search,
                    add_references_here,
                );
            }
            "class" => {
                self.add_class_static_this_references(
                    reference_location,
                    related_symbol,
                    search,
                    add_references_here,
                );
            }
            _ => {}
        }

        // Use the parent symbol if the location is commonjs require syntax on javascript files only.
        if is_in_js_file(reference_location)
            && reference_location.parent().kind() == SyntaxKind::BindingElement
            && is_variable_declaration_initialized_to_bare_or_accessed_require(
                reference_location.parent().parent().parent(),
            )
        {
            reference_symbol = reference_location.parent().symbol();
            // The parent will not have a symbol if it's an ObjectBindingPattern (when destructuring is used).  In
            // this case, just skip it, since the bound identifiers are not an alias of the import.
            if reference_symbol.is_nil() {
                return;
            }
        }

        self.get_import_or_export_references(reference_location, reference_symbol, search);
    }

    // Go: ls/findallreferences.go:2179 (*refState).addConstructorReferences
    // PORT: Go `pusher` calls `referenceAdder` once for each node that the
    // walks find. The walks read no search state, so they collect the nodes
    // and `pusher()` runs after each walk, in the same order.
    pub fn add_constructor_references(
        &mut self,
        reference_location: Node,
        symbol: SymbolId,
        search: &RefSearch,
        add_references_here: bool,
    ) {
        if is_new_expression_target(reference_location, false, false) && add_references_here {
            self.add_reference(reference_location, symbol, EntryKind::NODE);
        }

        // Go: ls/findallreferences.go:2297 pusher (closure)
        let pusher = |state: &mut Self| state.reference_adder(search.symbol);

        if is_class_like(reference_location.parent()) {
            // This is the class declaration containing the constructor.
            let source_file = get_source_file_of_node(reference_location);
            let mut nodes: Vec<Node> = Vec::new();
            find_own_constructor_references(
                &self.checker.symbols,
                search.symbol,
                source_file,
                &mut |n| nodes.push(n),
            );
            for n in nodes {
                pusher(&mut *self)(n, EntryKind::NODE);
            }
        } else {
            // If this class appears in `extends C`, then the extending class' "super" calls are references.
            let class_extending = try_get_class_by_extending_identifier(reference_location);
            if class_extending.is_some() {
                let mut nodes: Vec<Node> = Vec::new();
                find_super_constructor_accesses(&self.checker.symbols, class_extending, &mut |n| {
                    nodes.push(n)
                });
                for n in nodes {
                    pusher(&mut *self)(n, EntryKind::NODE);
                }
                self.find_inherited_constructor_references(class_extending);
            }
        }
    }

    // Go: ls/findallreferences.go:2205 (*refState).addClassStaticThisReferences
    pub fn add_class_static_this_references(
        &mut self,
        reference_location: Node,
        symbol: SymbolId,
        search: &RefSearch,
        add_references_here: bool,
    ) {
        if add_references_here {
            self.add_reference(reference_location, symbol, EntryKind::NODE);
        }

        let class_like = reference_location.parent();
        if self.options.use_ == ReferenceUse::RENAME || !is_class_like(class_like) {
            return;
        }

        let add_ref = self.reference_adder(search.symbol);
        let members = class_like.members();
        if members.is_empty() {
            // Go: `members == nil`; an empty list has no members to walk either.
            return;
        }
        // Go: ls/findallreferences.go:2340 cb (closure)
        fn cb(node: Node, add_ref: &dyn Fn(Node, EntryKind)) {
            if node.kind() == SyntaxKind::ThisKeyword {
                add_ref(node, EntryKind::NODE);
            } else if !is_function_like(node) && !is_class_like(node) {
                node.for_each_child(|child| {
                    cb(child, add_ref);
                    false
                });
            }
        }
        for member in members {
            if !(is_method_or_accessor(member) && has_static_modifier(member)) {
                continue;
            }
            let body = member.body();
            if body.is_some() {
                cb(body, &*add_ref);
            }
        }
    }

    // Go: ls/findallreferences.go:2242 (*refState).findInheritedConstructorReferences
    pub fn find_inherited_constructor_references(
        &mut self,
        class_declaration: Node, /*ClassLikeDeclaration*/
    ) {
        if has_own_constructor(&self.checker.symbols, class_declaration) {
            return;
        }
        let class_symbol = class_declaration.symbol();
        let search =
            self.create_search(Node::NIL, class_symbol, ImpExpKind::UNKNOWN, "", Vec::new());
        self.get_references_in_container_or_files(class_symbol, &search);
    }

    // Go: ls/findallreferences.go:2251 (*refState).getImportOrExportReferences
    pub fn get_import_or_export_references(
        &mut self,
        reference_location: Node,
        reference_symbol: SymbolId,
        search: &RefSearch,
    ) {
        let import_or_export = get_import_or_export_symbol(
            reference_location,
            reference_symbol,
            self.checker,
            search.coming_from == ImpExpKind::EXPORT,
        );
        let Some(import_or_export) = import_or_export else {
            return;
        };
        if import_or_export.kind == ImpExpKind::IMPORT {
            if !is_for_rename_with_prefix_and_suffix_text(self.options) {
                self.search_for_imported_symbol(import_or_export.symbol);
            }
        } else {
            // PORT: Go passes `importOrExport.exportInfo`, which an export
            // result always sets; Go would fail on nil inside the tracker.
            let export_info = import_or_export
                .export_info
                .unwrap_or_else(|| crate::core::go_nil_dereference());
            self.search_for_imports_of_export(
                reference_location,
                import_or_export.symbol,
                &export_info,
            );
        }
    }

    // Go: ls/findallreferences.go:2265 (*refState).markSeenReExportRHS
    pub fn mark_seen_re_export_rhs(&mut self, node: Node) -> bool {
        self.seen_re_export_rhs.insert(node)
    }

    // Go: ls/findallreferences.go:2269 (*refState).getReferencesAtExportSpecifier
    pub fn get_references_at_export_specifier(
        &mut self,
        reference_location: Node,
        reference_symbol: SymbolId,
        export_specifier: Node, /*ExportSpecifier*/
        search: &RefSearch,
        add_references_here: bool,
        always_get_references: bool,
    ) {
        crate::go_assert!(
            !always_get_references || self.options.use_aliases_for_rename,
            "If alwaysGetReferences is true, then prefix/suffix text must be enabled"
        );

        let export_declaration = export_specifier.parent().parent();
        let property_name = export_specifier.property_name();
        let name = export_specifier.name();
        let local_symbol = get_local_symbol_for_export_specifier(
            reference_location,
            reference_symbol,
            export_specifier,
            self.checker,
        );

        if !always_get_references && !(search.includes)(local_symbol) {
            return;
        }

        // Go: ls/findallreferences.go:2288 addRef (closure)
        let add_ref = |state: &mut Self| {
            if add_references_here {
                state.add_reference(reference_location, local_symbol, EntryKind::NODE);
            }
        };

        if property_name.is_nil() {
            // Don't rename at `export { default } from "m";`. (but do continue to search for imports of the re-export)
            if !(self.options.use_ == ReferenceUse::RENAME && module_export_name_is_default(name)) {
                add_ref(&mut *self);
            }
        } else if reference_location == property_name {
            // For `export { foo as bar } from "baz"`, "`foo`" will be added from the singleReferences for import searches of the original export.
            // For `export { foo as bar };`, where `foo` is a local, so add it now.
            if export_declaration.module_specifier().is_nil() {
                add_ref(&mut *self);
            }

            if add_references_here
                && self.options.use_ != ReferenceUse::RENAME
                && self.mark_seen_re_export_rhs(name)
            {
                let export_symbol = export_specifier.symbol();
                crate::go_assert!(
                    export_symbol.is_some(),
                    "exportSpecifier.Symbol() should not be nil"
                );
                self.add_reference(name, export_symbol, EntryKind::NODE);
            }
        } else if self.mark_seen_re_export_rhs(reference_location) {
            add_ref(&mut *self);
        }

        // For `export { foo as bar }`, rename `foo`, but not `bar`.
        if !is_for_rename_with_prefix_and_suffix_text(self.options) || always_get_references {
            let is_default_export = module_export_name_is_default(reference_location)
                || module_export_name_is_default(export_specifier.name());
            let mut export_kind = ExportKind::NAMED;
            if is_default_export {
                export_kind = ExportKind::DEFAULT;
            }
            let export_symbol = export_specifier.symbol();
            crate::go_assert!(
                export_symbol.is_some(),
                "exportSpecifier.Symbol() should not be nil"
            );
            let export_info = get_export_info(export_symbol, export_kind, self.checker);
            if let Some(export_info) = export_info {
                self.search_for_imports_of_export(reference_location, export_symbol, &export_info);
            }
        }

        // At `export { x } from "foo"`, also search for the imported symbol `"foo".x`.
        if search.coming_from != ImpExpKind::EXPORT
            && export_declaration.module_specifier().is_some()
            && property_name.is_nil()
            && !is_for_rename_with_prefix_and_suffix_text(self.options)
        {
            let imported = self
                .checker
                .get_export_specifier_local_target_symbol(export_specifier);
            if imported.is_some() {
                self.search_for_imported_symbol(imported);
            }
        }
    }

    // Go: ls/findallreferences.go:2342 (*refState).searchForImportedSymbol
    // Go to the symbol we imported from and find references for it.
    pub fn search_for_imported_symbol(&mut self, symbol: SymbolId) {
        let declarations = self.checker.sym(symbol).declarations.clone();
        for declaration in declarations {
            let exporting_file = get_source_file_of_node(declaration);
            // Need to search in the file even if it's not in the search-file set, because it might export the symbol.
            let search =
                self.create_search(declaration, symbol, ImpExpKind::IMPORT, "", Vec::new());
            let add_references_here = self.includes_source_file(exporting_file);
            self.get_references_in_source_file(exporting_file, &search, add_references_here);
        }
    }

    // Go: ls/findallreferences.go:2351 (*refState).searchForImportsOfExport
    // Search for all imports of a given exported symbol using `State.getImportSearches`. */
    pub fn search_for_imports_of_export(
        &mut self,
        export_location: Node,
        export_symbol: SymbolId,
        export_info: &ExportInfo,
    ) {
        let r = self.get_import_searches(export_symbol, export_info);

        // For `import { foo as bar }` just add the reference to `foo`, and don't otherwise search in the file.
        if !r.single_references.is_empty() {
            let add_ref = self.reference_adder(export_symbol);
            for &single_ref in &r.single_references {
                if self.should_add_single_reference(single_ref) {
                    add_ref(single_ref, EntryKind::NODE);
                }
            }
        }

        // For each import, find all references to that import in its source file.
        for i in &r.import_searches {
            let source_file = get_source_file_of_node(i.import_location);
            let search = self.create_search(
                i.import_location,
                i.import_symbol,
                ImpExpKind::EXPORT,
                "",
                Vec::new(),
            );
            self.get_references_in_source_file(
                source_file,
                &search,
                true, /*addReferencesHere*/
            );
        }

        if !r.indirect_users.is_empty() {
            let mut indirect_search: Option<RefSearch> = None;
            match export_info.export_kind {
                ExportKind::NAMED => {
                    indirect_search = Some(self.create_search(
                        export_location,
                        export_symbol,
                        ImpExpKind::EXPORT,
                        "",
                        Vec::new(),
                    ));
                }
                ExportKind::DEFAULT => {
                    // Search for a property access to '.default'. This can't be renamed.
                    if self.options.use_ != ReferenceUse::RENAME {
                        indirect_search = Some(self.create_search(
                            export_location,
                            export_symbol,
                            ImpExpKind::EXPORT,
                            "default",
                            Vec::new(),
                        ));
                    }
                }
                _ => {}
            }
            if let Some(indirect_search) = indirect_search {
                for &indirect_user in &r.indirect_users {
                    self.search_for_name(indirect_user, &indirect_search);
                }
            }
        }
    }

    // Go: ls/findallreferences.go:2388 (*refState).shouldAddSingleReference
    pub fn should_add_single_reference(&self, single_ref: Node) -> bool {
        if !self.has_matching_meaning(single_ref) {
            return false;
        }
        if self.options.use_ != ReferenceUse::RENAME {
            return true;
        }
        // Don't rename an import type `import("./module-name")` when renaming `name` in `export = name;`
        if !is_identifier(single_ref) && !is_import_or_export_specifier(single_ref.parent()) {
            return false;
        }
        // At `default` in `import { default as x }` or `export { default as x }`, do add a reference, but do not rename.
        !(is_import_or_export_specifier(single_ref.parent())
            && module_export_name_is_default(single_ref))
    }

    // Go: ls/findallreferences.go:2403 (*refState).hasMatchingMeaning
    pub fn has_matching_meaning(&self, reference_location: Node) -> bool {
        get_meaning_from_location(reference_location).intersects(self.search_meaning)
    }

    // Go: ls/findallreferences.go:2407 (*refState).getReferenceForShorthandProperty
    pub fn get_reference_for_shorthand_property(
        &mut self,
        reference_symbol: SymbolId,
        search: &RefSearch,
    ) {
        let reference_flags = self.checker.sym(reference_symbol).flags;
        let value_declaration = self.checker.sym(reference_symbol).value_declaration;
        if reference_flags.intersects(SymbolFlags::TRANSIENT) || value_declaration.is_nil() {
            return;
        }
        let shorthand_value_symbol = self
            .checker
            .get_shorthand_assignment_value_symbol(value_declaration);
        let name = get_name_of_declaration(value_declaration);

        // Because in short-hand property assignment, an identifier which stored as name of the short-hand property assignment
        // has two meanings: property name and property value. Therefore when we do findAllReference at the position where
        // an identifier is declared, the language service should return the position of the variable declaration as well as
        // the position in short-hand property assignment excluding property accessing. However, if we do findAllReference at the
        // position of property accessing, the referenceEntry of such position will be handled in the first case.
        if name.is_some() && (search.includes)(shorthand_value_symbol) {
            self.add_reference(name, shorthand_value_symbol, EntryKind::NODE);
        }
    }

    // === search ===

    // Go: ls/findallreferences.go:2425 (*refState).populateSearchSymbolSet
    pub fn populate_search_symbol_set(
        &mut self,
        symbol: SymbolId,
        location: Node,
        is_for_rename: bool,
        provide_prefix_and_suffix_text: bool,
        implementations: bool,
    ) -> Vec<SymbolId> {
        if location.is_nil() {
            return vec![symbol];
        }
        let mut result: Vec<SymbolId> = Vec::new();
        self.for_each_related_symbol(
            symbol,
            location,
            is_for_rename,
            !(is_for_rename && provide_prefix_and_suffix_text),
            &mut |c: &mut Checker, sym: SymbolId, root: SymbolId, mut base: SymbolId| -> SymbolId {
                // static method/property and instance method/property might have the same name. Only include static or only include instance.
                if base.is_some()
                    && is_static_symbol(&c.symbols, symbol) != is_static_symbol(&c.symbols, base)
                {
                    base = SymbolId::NIL;
                }
                // core.OrElse(base, core.OrElse(root, sym))
                result.push(if base.is_some() {
                    base
                } else if root.is_some() {
                    root
                } else {
                    sym
                });
                SymbolId::NIL
            }, // when try to find implementation, implementations is true, and not allowed to find base class
            /*allowBaseTypes*/
            &mut |_: &mut Self, _: SymbolId| !implementations,
        );
        result
    }

    // Go: ls/findallreferences.go:2450 (*refState).getRelatedSymbol
    pub fn get_related_symbol(
        &mut self,
        search: &RefSearch,
        reference_symbol: SymbolId,
        reference_location: Node,
    ) -> (SymbolId, EntryKind) {
        let only_include_binding_element_at_reference_location =
            self.options.use_ != ReferenceUse::RENAME || self.options.use_aliases_for_rename;
        self.for_each_related_symbol(
            reference_symbol,
            reference_location,
            false, /*isForRenamePopulateSearchSymbolSet*/
            only_include_binding_element_at_reference_location,
            &mut |c: &mut Checker,
                  sym: SymbolId,
                  root_symbol: SymbolId,
                  mut base_symbol: SymbolId|
             -> SymbolId {
                // check whether the symbol used to search itself is just the searched one.
                if base_symbol.is_some() {
                    // static method/property and instance method/property might have the same name. Only check static or only check instance.
                    if is_static_symbol(&c.symbols, reference_symbol)
                        != is_static_symbol(&c.symbols, base_symbol)
                    {
                        base_symbol = SymbolId::NIL;
                    }
                }
                // core.Coalesce(baseSymbol, core.Coalesce(rootSymbol, sym))
                let search_sym = if base_symbol.is_some() {
                    base_symbol
                } else if root_symbol.is_some() {
                    root_symbol
                } else {
                    sym
                };
                if search_sym.is_some() && (search.includes)(search_sym) {
                    if root_symbol.is_some()
                        && !c.sym(sym).check_flags.intersects(CheckFlags::SYNTHETIC)
                    {
                        return root_symbol;
                    }
                    return sym;
                }
                // For a base type, use the symbol for the derived type. For a synthetic (e.g. union) property, use the union symbol.
                SymbolId::NIL
            },
            &mut |state: &mut Self, root_symbol: SymbolId| -> bool {
                let root_symbol_parent = state.checker.sym(root_symbol).parent;
                !(!search.parents.is_empty()
                    && !search
                        .parents
                        .iter()
                        .any(|&parent| state.explicitly_inherits_from(root_symbol_parent, parent)))
            },
        )
    }

    // Go: ls/findallreferences.go:2603 fromRoot (closure in forEachRelatedSymbol)
    // PORT: the closure captures `cbSymbol` and `allowBaseTypes`, which
    // `forEachRelatedSymbol` also calls directly, so it is a method that takes
    // them as arguments.
    fn for_each_related_symbol_from_root(
        &mut self,
        sym: SymbolId,
        cb_symbol: &mut dyn FnMut(&mut Checker, SymbolId, SymbolId, SymbolId) -> SymbolId,
        allow_base_types: &mut dyn FnMut(&mut Self, SymbolId) -> bool,
    ) -> SymbolId {
        // If this is a union property:
        //   - In populateSearchSymbolsSet we will add all the symbols from all its source symbols in all unioned types.
        //   - In findRelatedSymbol, we will just use the union symbol if any source symbol is included in the search.
        // If the symbol is an instantiation from a another symbol (e.g. widened symbol):
        //   - In populateSearchSymbolsSet, add the root the list
        //   - In findRelatedSymbol, return the source symbol if that is in the search. (Do not return the instantiation symbol.)
        for root_symbol in self.checker.get_root_symbols(sym) {
            let result = cb_symbol(
                &mut *self.checker,
                sym,
                root_symbol,
                SymbolId::NIL, /*baseSymbol*/
            );
            if result.is_some() {
                return result;
            }
            // Add symbol of properties/methods of the same name in base classes and implemented interfaces definitions
            let root_symbol_parent = self.checker.sym(root_symbol).parent;
            if root_symbol_parent.is_some()
                && self
                    .checker
                    .sym(root_symbol_parent)
                    .flags
                    .intersects(SymbolFlags::CLASS | SymbolFlags::INTERFACE)
                && allow_base_types(&mut *self, root_symbol)
            {
                let root_symbol_name = self.checker.sym(root_symbol).name.as_str();
                let result = get_property_symbols_from_base_types(
                    root_symbol_parent,
                    root_symbol_name,
                    self.checker,
                    &mut |c: &mut Checker, base: SymbolId| -> SymbolId {
                        cb_symbol(c, sym, root_symbol, base)
                    },
                );
                if result.is_some() {
                    return result;
                }
            }
        }
        SymbolId::NIL
    }

    // Go: ls/findallreferences.go:2482 (*refState).forEachRelatedSymbol
    // PORT: `cbSymbol` gets the checker as its first argument, and
    // `allowBaseTypes` gets the state, because the callers' closures call
    // them.
    pub fn for_each_related_symbol(
        &mut self,
        symbol: SymbolId,
        location: Node,
        is_for_rename_populate_search_symbol_set: bool,
        only_include_binding_element_at_reference_location: bool,
        cb_symbol: &mut dyn FnMut(&mut Checker, SymbolId, SymbolId, SymbolId) -> SymbolId,
        allow_base_types: &mut dyn FnMut(&mut Self, SymbolId) -> bool,
    ) -> (SymbolId, EntryKind) {
        let containing_object_literal_element = get_containing_object_literal_element(location);
        if containing_object_literal_element.is_some() {
            /* Because in short-hand property assignment, location has two meaning : property name and as value of the property
             * When we do findAllReference at the position of the short-hand property assignment, we would want to have references to position of
             * property name and variable declaration of the identifier.
             * Like in below example, when querying for all references for an identifier 'name', of the property assignment, the language service
             * should show both 'name' in 'obj' and 'name' in variable declaration
             *      const name = "Foo";
             *      const obj = { name };
             * In order to do that, we will populate the search set with the value symbol of the identifier as a value of the property assignment
             * so that when matching with potential reference symbol, both symbols from property declaration and variable declaration
             * will be included correctly.
             */
            let shorthand_value_symbol = self
                .checker
                .get_shorthand_assignment_value_symbol(location.parent());
            // gets the local symbol
            if shorthand_value_symbol.is_some() && is_for_rename_populate_search_symbol_set {
                // When renaming 'x' in `const o = { x }`, just rename the local variable, not the property.
                return (
                    cb_symbol(
                        &mut *self.checker,
                        shorthand_value_symbol,
                        SymbolId::NIL, /*rootSymbol*/
                        SymbolId::NIL, /*baseSymbol*/
                    ),
                    EntryKind::SEARCHED_LOCAL_FOUND_PROPERTY,
                );
            }
            // If the location is in a context sensitive location (i.e. in an object literal) try
            // to get a contextual type for it, and add the property symbol from the contextual
            // type to the search set
            let contextual_type = self.checker.get_contextual_type_exported(
                containing_object_literal_element.parent(),
                ContextFlags::NONE,
            );
            if contextual_type.is_some() {
                let symbols = self.checker.get_property_symbols_from_contextual_type(
                    containing_object_literal_element,
                    contextual_type,
                    true, /*unionSymbolOk*/
                );
                for sym in symbols {
                    let res = self.for_each_related_symbol_from_root(
                        sym,
                        &mut *cb_symbol,
                        &mut *allow_base_types,
                    );
                    if res.is_some() {
                        return (res, EntryKind::SEARCHED_PROPERTY_FOUND_LOCAL);
                    }
                }
            }
            // If the location is name of property symbol from object literal destructuring pattern
            // Search the property symbol
            //      for ( { property: p2 } of elems) { }
            let property_symbol = self
                .checker
                .get_property_symbol_of_destructuring_assignment(location);
            if property_symbol.is_some() {
                let res = cb_symbol(
                    &mut *self.checker,
                    property_symbol,
                    SymbolId::NIL, /*rootSymbol*/
                    SymbolId::NIL, /*baseSymbol*/
                );
                if res.is_some() {
                    return (res, EntryKind::SEARCHED_PROPERTY_FOUND_LOCAL);
                }
            }
            if shorthand_value_symbol.is_some() {
                let res = cb_symbol(
                    &mut *self.checker,
                    shorthand_value_symbol,
                    SymbolId::NIL, /*rootSymbol*/
                    SymbolId::NIL, /*baseSymbol*/
                );
                if res.is_some() {
                    return (res, EntryKind::SEARCHED_LOCAL_FOUND_PROPERTY);
                }
            }
        }

        let aliased_symbol = get_merged_aliased_symbol_of_namespace_export_declaration(
            location,
            symbol,
            self.checker,
        );
        if aliased_symbol.is_some() {
            // In case of UMD module and global merging, search for global as well
            let res = cb_symbol(
                &mut *self.checker,
                aliased_symbol,
                SymbolId::NIL, /*rootSymbol*/
                SymbolId::NIL, /*baseSymbol*/
            );
            if res.is_some() {
                return (res, EntryKind::NODE);
            }
        }

        let res =
            self.for_each_related_symbol_from_root(symbol, &mut *cb_symbol, &mut *allow_base_types);
        if res.is_some() {
            return (res, EntryKind::NODE);
        }

        let value_declaration = self.checker.sym(symbol).value_declaration;
        if value_declaration.is_some()
            && is_parameter_property_declaration(value_declaration, value_declaration.parent())
        {
            let symbol_name = self.checker.sym(symbol).name.as_str();
            let (param_prop1, param_prop2) = self
                .checker
                .get_symbols_of_parameter_property_declaration(value_declaration, symbol_name);
            crate::go_assert!(
                self.checker
                    .sym(param_prop1)
                    .flags
                    .intersects(SymbolFlags::FUNCTION_SCOPED_VARIABLE)
                    && self
                        .checker
                        .sym(param_prop2)
                        .flags
                        .intersects(SymbolFlags::CLASS_MEMBER),
                "GetSymbolsOfParameterPropertyDeclaration must return (parameter, member) pair"
            );
            // core.IfElse
            let param_prop = if self
                .checker
                .sym(symbol)
                .flags
                .intersects(SymbolFlags::FUNCTION_SCOPED_VARIABLE)
            {
                param_prop2
            } else {
                param_prop1
            };
            return (
                self.for_each_related_symbol_from_root(
                    param_prop,
                    &mut *cb_symbol,
                    &mut *allow_base_types,
                ),
                EntryKind::NODE,
            );
        }

        let export_specifier =
            get_declaration_of_kind(&self.checker.symbols, symbol, SyntaxKind::ExportSpecifier);
        if export_specifier.is_some()
            && (!is_for_rename_populate_search_symbol_set
                || export_specifier.property_name().is_nil())
        {
            let local_symbol = self
                .checker
                .get_export_specifier_local_target_symbol(export_specifier);
            if local_symbol.is_some() {
                let res = cb_symbol(
                    &mut *self.checker,
                    local_symbol,
                    SymbolId::NIL, /*rootSymbol*/
                    SymbolId::NIL, /*baseSymbol*/
                );
                if res.is_some() {
                    return (res, EntryKind::NODE);
                }
            }
        }

        // symbolAtLocation for a binding element is the local symbol. See if the search symbol is the property.
        // Don't do this when populating search set for a rename when prefix and suffix text will be provided -- just rename the local.
        if !is_for_rename_populate_search_symbol_set {
            let binding_element_property_symbol: SymbolId;
            if only_include_binding_element_at_reference_location {
                if !is_object_binding_element_without_property_name(location.parent()) {
                    return (SymbolId::NIL, EntryKind::NONE);
                }
                binding_element_property_symbol =
                    get_property_symbol_from_binding_element(self.checker, location.parent());
            } else {
                binding_element_property_symbol =
                    get_property_symbol_of_object_binding_pattern_without_property_name(
                        symbol,
                        self.checker,
                    );
            }
            if binding_element_property_symbol.is_nil() {
                return (SymbolId::NIL, EntryKind::NONE);
            }
            return (
                self.for_each_related_symbol_from_root(
                    binding_element_property_symbol,
                    &mut *cb_symbol,
                    &mut *allow_base_types,
                ),
                EntryKind::SEARCHED_PROPERTY_FOUND_LOCAL,
            );
        }

        crate::go_assert!(is_for_rename_populate_search_symbol_set);

        // due to the above assert and the arguments at the uses of this function,
        // (onlyIncludeBindingElementAtReferenceLocation <=> !providePrefixAndSuffixTextForRename) holds
        let include_original_symbol_of_binding_element =
            only_include_binding_element_at_reference_location;

        if include_original_symbol_of_binding_element {
            let binding_element_property_symbol =
                get_property_symbol_of_object_binding_pattern_without_property_name(
                    symbol,
                    self.checker,
                );
            if binding_element_property_symbol.is_some() {
                return (
                    self.for_each_related_symbol_from_root(
                        binding_element_property_symbol,
                        &mut *cb_symbol,
                        &mut *allow_base_types,
                    ),
                    EntryKind::SEARCHED_PROPERTY_FOUND_LOCAL,
                );
            }
        }
        (SymbolId::NIL, EntryKind::NONE)
    }

    // Go: ls/findallreferences.go:2619 (*refState).searchForName
    // Search for all occurrences of an identifier in a source file (and filter out the ones that match).
    pub fn search_for_name(&mut self, source_file: Node, search: &RefSearch) {
        if source_file_get_name_table(source_file).contains_key(search.escaped_text.as_str()) {
            self.get_references_in_source_file(
                source_file,
                search,
                true, /*addReferencesHere*/
            );
        }
    }

    // Go: ls/findallreferences.go:2625 (*refState).explicitlyInheritsFrom
    pub fn explicitly_inherits_from(&mut self, symbol: SymbolId, parent: SymbolId) -> bool {
        if symbol == parent {
            return true;
        }

        // Check cache first
        let key = InheritKey { symbol, parent };
        if let Some(&cached) = self.inherits_from_cache.get(&key) {
            return cached;
        }

        // Set to false initially to prevent infinite recursion
        self.inherits_from_cache.insert(key, false);

        // PORT: Go tests `symbol.Declarations == nil`; an empty list gives the
        // same result below (nothing inherits, the cache stays false).
        if self.checker.sym(symbol).declarations.is_empty() {
            return false;
        }

        // core.Some(declarations, core.Some(superTypeNodes, ..))
        let declarations = self.checker.sym(symbol).declarations.clone();
        let mut inherits = false;
        'declarations: for declaration in declarations {
            let super_type_nodes = get_all_super_type_nodes(declaration);
            for type_reference in super_type_nodes {
                let typ = self.checker.get_type_at_location(type_reference);
                if typ.is_some() {
                    let typ_symbol = self.checker.ty(typ).symbol;
                    if typ_symbol.is_some() && self.explicitly_inherits_from(typ_symbol, parent) {
                        inherits = true;
                        break 'declarations;
                    }
                }
            }
        }

        // Update cache with the actual result
        self.inherits_from_cache.insert(key, inherits);
        inherits
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::parser::{SourceFileParseOptions, parse_source_file};
    use crate::frontend::tspath::Path;
    use crate::scanner_util::port_byte_offset;
    use crate::scanner_util::{go_string_from_bytes, push_js_string_rune};

    // PORT: no Go test. Go searches the Go bytes of the text. The text is in
    // the source form, where a lone surrogate is 3 invalid byte units, and a
    // string literal's name in the value form, where it is one surrogate unit
    // (see `scanner_util::GO_STRING_MARKER`). The bytes next to a match are
    // Go bytes: 0xC5 (`Å`) and 0xFF (`ÿ`) are identifier parts, 0x80 is not.
    // The checker's copy of the search gives the same positions.
    #[test]
    fn possible_reference_positions_search_go_bytes() {
        let mut lone = String::new();
        push_js_string_rune(&mut lone, 0xD800);
        let cases: [(&[u8], &str, &[i32]); 3] = [
            (
                b"a = \"\xed\xa0\x80\"; b = \"\xed\xa0\x80\";",
                &lone,
                &[5, 16],
            ),
            (b"\xc5foo foo\x80 foo\xff", "foo", &[5]),
            // A real U+FDD0 is M + M in both forms.
            ("x\u{FDD0}y \u{FDD0}y".as_bytes(), "\u{FDD0}\u{FDD0}y", &[6]),
        ];
        for (bytes, name, want) in cases {
            let text = go_string_from_bytes(bytes.to_vec());
            let file = parse_source_file(
                &SourceFileParseOptions {
                    file_name: "/a.ts".to_string(),
                    path: Path("/a.ts".to_string()),
                    ..Default::default()
                },
                crate::ast::FileText::new(text.clone(), false),
                ScriptKind::TS,
            );
            let got = get_possible_symbol_reference_positions(file.root, name, Node::NIL);
            let want: Vec<i32> = want.iter().map(|&g| port_byte_offset(&text, g)).collect();
            assert_eq!(got, want, "{bytes:?} {name:?}");
            // Go checker/services.go:655, the identifier search of
            // `IsSymbolReferencedInFile`, has the same body.
            let checker_got = crate::checker::services::get_possible_symbol_reference_positions(
                file.root, name, file.root,
            );
            assert_eq!(checker_got, want, "checker: {bytes:?} {name:?}");
        }
        // A container that does not start the text: as in Go, the first
        // index is relative to it and is used as an absolute position. Here
        // that is Go offset 11, inside an `é` (0xC3 before it) or after an
        // invalid byte 0xFF, both identifier parts, so it is no match.
        let cases: [(&[u8], i32, &[i32]); 2] = [
            (
                "let ééééé = 1;function f(x = 1) { return x; }".as_bytes(),
                19,
                &[30, 46],
            ),
            (
                b"let a = \"\xff\xff\xff\xff\xff\";function f(x = 1) { return x; }",
                16,
                &[27, 43],
            ),
        ];
        for (bytes, go_container_pos, want) in cases {
            let text = go_string_from_bytes(bytes.to_vec());
            let file = parse_source_file(
                &SourceFileParseOptions {
                    file_name: "/a.ts".to_string(),
                    path: Path("/a.ts".to_string()),
                    ..Default::default()
                },
                crate::ast::FileText::new(text.clone(), false),
                ScriptKind::TS,
            );
            let x = port_byte_offset(&text, go_container_pos + 11);
            let function = astnav::get_touching_property_name(file.root, x)
                .parent()
                .parent();
            assert_eq!(function.kind(), SyntaxKind::FunctionDeclaration);
            assert_eq!(function.pos(), port_byte_offset(&text, go_container_pos));
            let got = get_possible_symbol_reference_positions(file.root, "x", function);
            let want: Vec<i32> = want.iter().map(|&g| port_byte_offset(&text, g)).collect();
            assert_eq!(got, want, "{bytes:?}");
        }
    }
}

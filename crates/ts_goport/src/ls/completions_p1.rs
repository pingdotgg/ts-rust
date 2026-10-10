use crate::ls::prelude::*;

// Port of Go `ls/completions.go` lines 1-1708: the completion entry points,
// the shared completion types and `getCompletionData`.
//
// PORT (whole file):
// - The shared completion types follow map-ls-completions.md section 2.3.
// - Go `(T, error)` results are `Result<T, GoError>`. A Go
//   `return globalsSearchFail, err` is `Err(err)`; the caller never reads the
//   search value when the error is set.
// - `getCompletionData` has 18 closures over shared locals. They are methods
//   of the private `GetCompletionDataState`, which holds every local that the
//   closures read or change. The methods keep the closure names and Go order.
// - Checker lease: PORTING "Programs and checkers". Helpers take
//   `type_checker: &mut Checker`.

use crate::flags_macros::{go_enum, go_flags};
use crate::frontend::scanner::scanner_p1::{RUNE_ERROR, rune_to_char, utf8_decode_rune_in_string};
use crate::scanner_util::go_byte_offset;
use crate::spanmap::Feature;
use std::sync::LazyLock;

// Go: ls/completions.go:35 ErrNeedsAutoImports
pub static ERR_NEEDS_AUTO_IMPORTS: LazyLock<GoError> =
    LazyLock::new(|| crate::gostd::errors::new("completion list needs auto imports"));

impl LanguageService {
    // Go: ls/completions.go:37 ProvideCompletion
    // PORT: Go `context *lsproto.CompletionContext` is `Option<&..>`; the
    // document URI is borrowed like a Go string parameter.
    pub fn provide_completion(
        &self,
        ctx: &Context,
        document_uri: &lsproto::DocumentUri,
        lsp_position: lsproto::Position,
        context: Option<&lsproto::CompletionContext>,
    ) -> Result<lsproto::CompletionResponse, GoError> {
        let (program, file) = self.get_program_and_file(document_uri);
        let mut trigger_character: Option<String> = None;
        if let Some(context) = context {
            trigger_character = context.trigger_character.clone();
        }
        let ctx = &crate::format::with_format_code_settings(
            ctx,
            &self.format_options(),
            &self.format_options().editor_settings.new_line_character,
        );
        let positions = lsconv::from_lsp_position_for_source_file(
            &self.converters,
            file,
            lsp_position,
            Feature::COMPLETION,
        );
        if positions.is_empty() || !positions[0].fidelity.is_exact() {
            // In a content-mapped file the cursor is outside a verbatim span, so any completion committed here
            // could not be applied to the original text. Offer nothing rather than edits at a bogus location.
            return Ok(lsproto::CompletionItemsOrListOrNull::default());
        }
        let file = positions[0].script;
        let position = positions[0].position;
        let completion_list_internal = self.get_completions_at_position(
            ctx,
            file,
            position,
            trigger_character,
            false, /*includeSymbols*/
        )?;
        let mut completion_list = ensure_item_data(
            file,
            position,
            CompletionList::to_lsp(completion_list_internal.as_ref()),
        );
        if lsconv::Script::span_map(&file).is_some() {
            self.filter_content_mapped_auto_imports(ctx, program, file, completion_list.as_mut());
        }
        Ok(lsproto::CompletionItemsOrListOrNull {
            items: None,
            list: completion_list,
        })
    }

    // Go: ls/completions.go:78 filterContentMappedAutoImports
    // filterContentMappedAutoImports eagerly resolves auto-import edits for a content-mapped file and drops any
    // completion whose import edit cannot be placed entirely within verbatim spans (it would otherwise insert
    // an import into synthesized virtual code with no counterpart in the original file). Surviving auto-imports carry
    // their additional edits directly so the client applies correct original-text positions on commit.
    // PORT: Go filters `list.Items` in place (`list.Items[:0]`); here
    // `retain_mut`, which keeps the order.
    pub fn filter_content_mapped_auto_imports(
        &self,
        ctx: &Context,
        program: &compiler::NewProgram,
        file: Node,
        list: Option<&mut lsproto::CompletionList>,
    ) {
        let Some(list) = list else {
            return;
        };
        list.items.retain_mut(|item| {
            let Some(auto_import) = item
                .data
                .as_ref()
                .and_then(|data| data.auto_import.as_ref())
            else {
                return true;
            };
            let (edits, description, ok) = autoimport::Fix {
                auto_import_fix: auto_import.clone(),
                ..Default::default()
            }
            .edits(
                ctx,
                file,
                program.options(),
                &self.format_options(),
                &self.converters,
                &self.user_preferences(),
            );
            if !ok {
                return false;
            }
            // Go `&edits`: a `[]*lsproto.TextEdit` of non-nil edits.
            item.additional_text_edits = Some(edits.into_iter().map(Some).collect());
            item.detail = str_ptr_to(&description);
            true
        });
    }

    // Go: ls/completions.go:106 GetCompletionsAtPosition
    // PORT: Go has `GetCompletionsAtPosition` and `getCompletionsAtPosition`;
    // the exported one gets `_exported`.
    pub fn get_completions_at_position_exported(
        &self,
        ctx: &Context,
        file: Node,
        position: i32,
        trigger_character: Option<String>,
        include_symbols: bool,
    ) -> Result<Option<CompletionList>, GoError> {
        self.get_completions_at_position(ctx, file, position, trigger_character, include_symbols)
    }
}

// Go: ls/completions.go:110 CompletionItem
// PORT: Go embeds `*lsproto.CompletionItem` and never stores a nil one. The
// embedded item is the field `completion_item`; `Deref`/`DerefMut` give the Go
// promoted fields (`item.label`, `item.sort_text`, ...).
#[derive(Clone, Debug, Default)]
pub struct CompletionItem {
    pub completion_item: lsproto::CompletionItem,
    pub symbol: SymbolId, // non-nil for symbol completions when IncludeSymbols is set; nil otherwise
}

impl std::ops::Deref for CompletionItem {
    type Target = lsproto::CompletionItem;
    fn deref(&self) -> &lsproto::CompletionItem {
        &self.completion_item
    }
}

impl std::ops::DerefMut for CompletionItem {
    fn deref_mut(&mut self) -> &mut lsproto::CompletionItem {
        &mut self.completion_item
    }
}

// Go: ls/completions.go:115 CompletionList
#[derive(Clone, Debug, Default)]
pub struct CompletionList {
    pub is_incomplete: bool,
    pub item_defaults: Option<lsproto::CompletionItemDefaults>,
    pub apply_kind: Option<lsproto::CompletionItemApplyKinds>,
    pub items: Vec<CompletionItem>,
}

// Go: ls/completions.go:122 ensureItemData
// PORT: Go fills `item.Data` through the list pointer; the list is moved in
// and returned. `data.position` is a Go byte offset in Go, and the client
// sends it back on resolve. Port offsets differ from Go offsets after a
// marker unit (see `scanner_util::GO_STRING_MARKER`), so the list leaves
// with Go offsets (`go_byte_offset`) and `resolve_completion_item` maps them
// back. The items that `create_lsp_completion_item` makes hold the port
// `pos` until here, so the text is scanned once for the list, not once for
// each item.
pub fn ensure_item_data(
    file: Node,
    pos: i32,
    list: Option<lsproto::CompletionList>,
) -> Option<lsproto::CompletionList> {
    let Some(mut list) = list else {
        return None;
    };
    let text = source_file_text(file);
    let go_pos = go_byte_offset(&text, pos);
    for item in &mut list.items {
        if let Some(data) = &mut item.data {
            data.position = if data.position == pos {
                go_pos
            } else {
                go_byte_offset(&text, data.position)
            };
        } else {
            item.data = Some(lsproto::CompletionItemData {
                file_name: source_file_original_file_name(file).to_string(),
                position: go_pos,
                supplemental_file_index: supplemental_file_index(file),
                name: item.label.clone(),
                ..Default::default()
            });
        }
    }
    Some(list)
}

// Go: ls/completions.go:139 supplementalFileIndex
// PORT: Go `*int32`; nil is `None`.
pub fn supplemental_file_index(file: Node) -> Option<i32> {
    let canonical = source_file_canonical_source_file(file);
    if canonical.is_nil() {
        return None;
    }
    for (i, &supplemental) in source_file_supplemental_source_files(canonical)
        .iter()
        .enumerate()
    {
        if supplemental == file {
            return Some(i as i32);
        }
    }
    crate::core::go_panic(
        "supplemental source file is not linked from its canonical source file".to_string(),
    );
}

// Go: ls/completions.go:152 sourceFileForSupplementalFileIndex
// PORT: Go `*ast.SourceFile` is the file root `Node` (nil is `Node::NIL`).
// Go `*int32` is `Option<i32>`.
pub fn source_file_for_supplemental_file_index(file: Node, index: Option<i32>) -> Node {
    let Some(index) = index else {
        return file;
    };
    let supplemental = source_file_supplemental_source_files(file);
    if index >= 0 && (index as usize) < supplemental.len() {
        return supplemental[index as usize];
    }
    Node::NIL
}

// Go: ls/completions.go:164 completionData
// *completionDataData | *completionDataKeyword | *completionDataJSDocTagName | *completionDataJSDocTag | *completionDataJSDocParameterName
// PORT: Go `type completionData = any` is an enum over the five pointer types.
// A nil `completionData` is `Option::None` around it.
#[derive(Clone, Debug)]
pub enum CompletionData {
    Data(Box<CompletionDataData>),
    Keyword(CompletionDataKeyword),
    JSDocTagName(CompletionDataJSDocTagName),
    JSDocTag(CompletionDataJSDocTag),
    JSDocParameterName(CompletionDataJSDocParameterName),
}

// Go: ls/completions.go:166 completionDataData
#[derive(Clone, Debug, Default)]
pub struct CompletionDataData {
    pub symbols: Vec<SymbolId>,
    pub auto_imports: Vec<autoimport::FixAndExport>,
    pub completion_kind: CompletionKind,
    pub is_in_snippet_scope: bool,
    // Note that the presence of this alone doesn't mean that we need a conversion. Only do that if the completion is not an ordinary identifier.
    pub property_access_to_convert: Node,
    pub is_new_identifier_location: bool,
    pub location: Node,
    pub keyword_filters: KeywordCompletionFilters,
    pub literals: Vec<LiteralValue>,
    // PORT: key is the index in `symbols` (Go `int`).
    pub symbol_to_origin_info_map: FxHashMap<i32, SymbolOriginInfo>,
    // PORT: key is Go `ast.SymbolId` (`get_symbol_id`).
    pub symbol_to_sort_text_map: FxHashMap<u64, SortText>,
    pub recommended_completion: SymbolId,
    pub previous_token: Node,
    pub context_token: Node,
    pub jsx_initializer: JsxInitializer,
    pub inside_js_doc_tag_type_expression: bool,
    pub is_type_only_location: bool,
    // In JSX tag name and attribute names, identifiers like "my-tag" or "aria-name" is valid identifier.
    pub is_jsx_identifier_expected: bool,
    pub is_right_of_open_tag: bool,
    pub is_right_of_dot_or_question_dot: bool,
    pub import_statement_completion: Option<ImportStatementCompletionInfo>, // !!!
    pub has_unresolved_auto_imports: bool,                                  // !!!
    // flags CompletionInfoFlags // !!!
    // PORT: Go keeps nil and empty slices apart here; `None` is nil.
    pub default_commit_characters: Option<Vec<String>>,
}

// Go: ls/completions.go:195 completionDataKeyword
#[derive(Clone, Debug, Default)]
pub struct CompletionDataKeyword {
    pub keyword_completions: Vec<CompletionItem>,
    pub is_new_identifier_location: bool,
}

// Go: ls/completions.go:200 completionDataJSDocTagName
#[derive(Clone, Copy, Debug, Default)]
pub struct CompletionDataJSDocTagName;

// Go: ls/completions.go:202 completionDataJSDocTag
#[derive(Clone, Copy, Debug, Default)]
pub struct CompletionDataJSDocTag;

// Go: ls/completions.go:204 completionDataJSDocParameterName
#[derive(Clone, Copy, Debug, Default)]
pub struct CompletionDataJSDocParameterName {
    pub tag: Node,
}

// Go: ls/completions.go:208 importStatementCompletionInfo
#[derive(Clone, Debug)]
pub struct ImportStatementCompletionInfo {
    pub is_keyword_only_completion: bool,
    pub keyword_completion: SyntaxKind, // TokenKind
    pub is_new_identifier_location: bool,
    pub is_top_level_type_only: bool,
    pub could_be_type_only_import_specifier: bool,
    pub replacement_span: Option<lsproto::Range>,
}

// PORT: the Go zero value has `keywordCompletion == ast.KindUnknown`.
impl Default for ImportStatementCompletionInfo {
    fn default() -> Self {
        ImportStatementCompletionInfo {
            is_keyword_only_completion: false,
            keyword_completion: SyntaxKind::Unknown,
            is_new_identifier_location: false,
            is_top_level_type_only: false,
            could_be_type_only_import_specifier: false,
            replacement_span: None,
        }
    }
}

// Go: ls/completions.go:219 jsxInitializer
// If we're after the `=` sign but no identifier has been typed yet,
// value will be `true` but initializer will be `nil`.
#[derive(Clone, Copy, Debug, Default)]
pub struct JsxInitializer {
    pub is_initializer: bool,
    pub initializer: Node,
}

// Go: ls/completions.go:224 KeywordCompletionFilters
go_enum!(KeywordCompletionFilters, i32 {
    NONE = 0; // No keywords
    ALL = 1; // Every possible kewyord
    CLASS_ELEMENT_KEYWORDS = 2; // Keywords inside class body
    INTERFACE_ELEMENT_KEYWORDS = 3; // Keywords inside interface body
    CONSTRUCTOR_PARAMETER_KEYWORDS = 4; // Keywords at constructor parameter
    FUNCTION_LIKE_BODY_KEYWORDS = 5; // Keywords at function like body
    TYPE_ASSERTION_KEYWORDS = 6;
    TYPE_KEYWORDS = 7;
    TYPE_KEYWORD = 8; // Literally just `type`
    LAST = 8;
});

// Go: ls/completions.go:239 keywordFiltersFromSyntaxKind
// PORT: Go `ast.Kind.String()` is "Kind" + the kind name; the Rust `Debug`
// name spells JSDoc kinds `JsDoc`.
pub fn keyword_filters_from_syntax_kind(
    keyword_completion: SyntaxKind,
) -> KeywordCompletionFilters {
    match keyword_completion {
        SyntaxKind::TypeKeyword => KeywordCompletionFilters::TYPE_KEYWORD,
        _ => crate::core::go_panic(format!(
            "Unknown mapping from ast.Kind `{}` to KeywordCompletionFilters",
            crate::gostd::debug::kind_string(keyword_completion)
        )),
    }
}

// Go: ls/completions.go:248 CompletionKind
go_enum!(CompletionKind, i32 {
    NONE = 0;
    OBJECT_PROPERTY_DECLARATION = 1;
    GLOBAL = 2;
    PROPERTY_ACCESS = 3;
    MEMBER_LIKE = 4;
    STRING = 5;
});

// Go: ls/completions.go:259 CompletionTriggerCharacters
pub static COMPLETION_TRIGGER_CHARACTERS: [&str; 10] =
    [".", "\"", "'", "`", "/", "@", "<", "#", " ", "*"];

// Go: ls/completions.go:262 allCommitCharacters
// All commit characters, valid when `isNewIdentifierLocation` is false.
pub static ALL_COMMIT_CHARACTERS: &[&str] = &[".", ",", ";"];

// Go: ls/completions.go:265 noCommaCommitCharacters
// Commit characters valid at expression positions where we could be inside a parameter list.
pub static NO_COMMA_COMMIT_CHARACTERS: &[&str] = &[".", ";"];

// Go: ls/completions.go:267 emptyCommitCharacters
pub static EMPTY_COMMIT_CHARACTERS: &[&str] = &[];

// Go: ls/completions.go:269 SortText
pub type SortText = String;

// Go: ls/completions.go:269 SortText consts
pub const SORT_TEXT_LOCAL_DECLARATION_PRIORITY: &str = "10";
pub const SORT_TEXT_LOCATION_PRIORITY: &str = "11";
pub const SORT_TEXT_OPTIONAL_MEMBER: &str = "12";
pub const SORT_TEXT_MEMBER_DECLARED_BY_SPREAD_ASSIGNMENT: &str = "13";
pub const SORT_TEXT_SUGGESTED_CLASS_MEMBERS: &str = "14";
pub const SORT_TEXT_GLOBALS_OR_KEYWORDS: &str = "15";
pub const SORT_TEXT_AUTO_IMPORT_SUGGESTIONS: &str = "16";
pub const SORT_TEXT_CLASS_MEMBER_SNIPPETS: &str = "17";
pub const SORT_TEXT_JAVASCRIPT_IDENTIFIERS: &str = "18";

// Go: ls/completions.go:283 DeprecateSortText
pub fn deprecate_sort_text(original: &str) -> SortText {
    format!("z{original}")
}

// Go: ls/completions.go:287 ObjectLiteralPropertySortText
pub fn object_literal_property_sort_text(
    preset_sort_text: &str,
    symbol_display_name: &str,
) -> SortText {
    format!("{preset_sort_text}\x00{symbol_display_name}\x00")
}

// Go: ls/completions.go:291 SortBelow
pub fn sort_below(original: &str) -> SortText {
    format!("{original}1")
}

// Go: ls/completions.go:295 symbolOriginInfoKind
go_flags!(SymbolOriginInfoKind, i32 {
    THIS_TYPE = 1; // 1 << iota
    SYMBOL_MEMBER = 2;
    PROMISE = 4;
    NULLABLE = 8;
    TYPE_ONLY_ALIAS = 16;
    OBJECT_LITERAL_METHOD = 32;
    IGNORE = 64;
    COMPUTED_PROPERTY_NAME = 128;
});

// Go: ls/completions.go:308 symbolOriginInfo
#[derive(Clone, Debug, Default)]
pub struct SymbolOriginInfo {
    pub kind: SymbolOriginInfoKind,
    pub is_default_export: bool,
    pub is_from_package_json: bool,
    pub file_name: String,
    pub data: SymbolOriginInfoData,
}

// PORT: Go `data any` holds nil or one of three pointer types.
#[derive(Clone, Debug, Default)]
pub enum SymbolOriginInfoData {
    #[default]
    None,
    ObjectLiteralMethod(SymbolOriginInfoObjectLiteralMethod),
    TypeOnlyAlias(SymbolOriginInfoTypeOnlyAlias),
    ComputedPropertyName(SymbolOriginInfoComputedPropertyName),
}

impl SymbolOriginInfoData {
    /// Go `%T` of `symbolOriginInfo.data`, for the Go panic texts.
    fn go_type_name(&self) -> &'static str {
        match self {
            SymbolOriginInfoData::None => "<nil>",
            SymbolOriginInfoData::ObjectLiteralMethod(_) => {
                "*ls.symbolOriginInfoObjectLiteralMethod"
            }
            SymbolOriginInfoData::TypeOnlyAlias(_) => "*ls.symbolOriginInfoTypeOnlyAlias",
            SymbolOriginInfoData::ComputedPropertyName(_) => {
                "*ls.symbolOriginInfoComputedPropertyName"
            }
        }
    }
}

impl SymbolOriginInfo {
    // Go: ls/completions.go:315 (*symbolOriginInfo).symbolName
    pub fn symbol_name(&self) -> String {
        match &self.data {
            SymbolOriginInfoData::ComputedPropertyName(data) => data.symbol_name.clone(),
            _ => crate::core::go_panic(format!(
                "symbolOriginInfo: unknown data type for symbolName(): {}",
                self.data.go_type_name()
            )),
        }
    }

    // Go: ls/completions.go:330 (*symbolOriginInfo).asObjectLiteralMethod
    // PORT: a failed Go type assertion panics; the text follows the Go runtime.
    pub fn as_object_literal_method(&self) -> &SymbolOriginInfoObjectLiteralMethod {
        match &self.data {
            SymbolOriginInfoData::ObjectLiteralMethod(data) => data,
            _ => crate::core::go_panic(format!(
                "interface conversion: interface {{}} is {}, not *ls.symbolOriginInfoObjectLiteralMethod",
                self.data.go_type_name()
            )),
        }
    }
}

// Go: ls/completions.go:324 symbolOriginInfoObjectLiteralMethod
#[derive(Clone, Debug, Default)]
pub struct SymbolOriginInfoObjectLiteralMethod {
    pub insert_text: String,
    pub label_details: Option<lsproto::CompletionItemLabelDetails>,
    pub is_snippet: bool,
}

// Go: ls/completions.go:334 symbolOriginInfoTypeOnlyAlias
#[derive(Clone, Copy, Debug, Default)]
pub struct SymbolOriginInfoTypeOnlyAlias {
    pub declaration: Node,
}

// Go: ls/completions.go:338 symbolOriginInfoComputedPropertyName
#[derive(Clone, Debug, Default)]
pub struct SymbolOriginInfoComputedPropertyName {
    pub symbol_name: String,
}

// Go: ls/completions.go:351 completionSource
// Special values for `CompletionInfo['source']` used to disambiguate
// completion items with the same `name`. (Each completion item must
// have a unique name/source combination, because those two fields
// comprise `CompletionEntryIdentifier` in `getCompletionEntryDetails`.
//
// When the completion item is an auto-import suggestion, the source
// is the module specifier of the suggestion. To avoid collisions,
// the values here should not be a module specifier we would ever
// generate for an auto-import.
pub type CompletionSource = &'static str;

// Completions that require `this.` insertion text.
pub const COMPLETION_SOURCE_THIS_PROPERTY: CompletionSource = "ThisProperty/";
// Auto-import that comes attached to a class member snippet.
pub const COMPLETION_SOURCE_CLASS_MEMBER_SNIPPET: CompletionSource = "ClassMemberSnippet/";
// A type-only import that needs to be promoted in order to be used at the completion location.
pub const COMPLETION_SOURCE_TYPE_ONLY_ALIAS: CompletionSource = "TypeOnlyAlias/";
// Auto-import that comes attached to an object literal method snippet.
pub const COMPLETION_SOURCE_OBJECT_LITERAL_METHOD_SNIPPET: CompletionSource =
    "ObjectLiteralMethodSnippet/";
// Case completions for switch statements.
pub const COMPLETION_SOURCE_SWITCH_CASES: CompletionSource = "SwitchCases/";
// Completions for an object literal expression.
pub const COMPLETION_SOURCE_OBJECT_LITERAL_MEMBER_WITH_COMMA: CompletionSource =
    "ObjectLiteralMemberWithComma/";

// Go: ls/completions.go:370 uniqueNamesMap
// Value is set to false for global variables or completions from external module exports,
// true otherwise.
pub type UniqueNamesMap = FxHashMap<String, bool>;

// Go: ls/completions.go:373 literalValue
// PORT: Go `literalValue any` (string | jsnum.Number | PseudoBigInt) is
// `LiteralValue` from checker/types.rs; Go nil is `None`.

// Go: ls/completions.go:375 globalsSearch
go_enum!(GlobalsSearch, i32 {
    CONTINUE = 0;
    SUCCESS = 1;
    FAIL = 2;
});

impl CompletionList {
    // Go: ls/completions.go:383 (*CompletionList).toLSP
    // PORT: a nil receiver is `None` (the lsproto `resolve` rule). Go shares
    // the item pointers; the items are cloned.
    pub fn to_lsp(l: Option<&CompletionList>) -> Option<lsproto::CompletionList> {
        let Some(l) = l else {
            return None;
        };
        let mut items: Vec<lsproto::CompletionItem> = Vec::with_capacity(l.items.len());
        for entry in &l.items {
            // PORT: Go skips nil entries and nil embedded items; a Rust
            // `CompletionItem` always holds its item.
            items.push(entry.completion_item.clone());
        }
        Some(lsproto::CompletionList {
            is_incomplete: l.is_incomplete,
            item_defaults: l.item_defaults.clone(),
            apply_kind: l.apply_kind.clone(),
            items,
        })
    }
}

impl LanguageService {
    // Go: ls/completions.go:401 getCompletionsAtPosition
    pub fn get_completions_at_position(
        &self,
        ctx: &Context,
        file: Node,
        position: i32,
        trigger_character: Option<String>,
        include_symbols: bool,
    ) -> Result<Option<CompletionList>, GoError> {
        let (_, previous_token) = get_relevant_tokens(position, file);
        if let Some(trigger_character) = trigger_character.as_deref() {
            if !is_in_string(file, position, previous_token)
                && !is_valid_trigger(file, trigger_character, previous_token, position)
            {
                return Ok(None);
            }
        }

        if trigger_character.as_deref() == Some(" ") {
            // `isValidTrigger` ensures we are at `import |`
            if self
                .user_preferences()
                .include_completions_for_import_statements
                .is_true()
            {
                return Ok(Some(CompletionList {
                    is_incomplete: true,
                    ..Default::default()
                }));
            }
            return Ok(None);
        }

        if let Some(js_doc_snippet_completion) =
            self.get_js_doc_snippet_completion(ctx, file, position)
        {
            return Ok(Some(js_doc_snippet_completion));
        }

        let compiler_options = self.get_program().options();

        // !!! see if incomplete completion list and continue or clean

        // Go: checker, done := l.GetProgram().GetTypeCheckerForFile(ctx, file); defer done()
        let (checker, _done) = ls_program::get_type_checker_for_file(self.get_program(), ctx, file);
        let c = &mut *checker.borrow_mut();

        let string_completions = self.get_string_literal_completions(
            ctx,
            file,
            position,
            previous_token,
            c,
            compiler_options,
            include_symbols,
        );
        if string_completions.is_some() {
            return Ok(string_completions);
        }

        if previous_token.is_some()
            && (previous_token.kind() == SyntaxKind::BreakKeyword
                || previous_token.kind() == SyntaxKind::ContinueKeyword
                || previous_token.kind() == SyntaxKind::Identifier)
            && is_break_or_continue_statement(previous_token.parent())
        {
            return Ok(self.get_label_completions_at_position(
                ctx,
                previous_token.parent(),
                file,
                position,
                self.get_optional_replacement_span(previous_token, file),
            ));
        }

        let preferences = self.user_preferences();
        let data = self.get_completion_data(
            ctx,
            c,
            file,
            position,
            &preferences,
            false, /*forItemResolve*/
        )?;
        let Some(data) = data else {
            return Ok(None);
        };

        // PORT: the Go `default` panic ("getCompletionData() returned
        // unexpected type") cannot happen with the enum.
        match data {
            CompletionData::Data(mut data) => {
                let optional_replacement_span =
                    self.get_optional_replacement_span(data.location, file);
                let response = self.completion_info_from_data(
                    ctx,
                    c,
                    file,
                    compiler_options,
                    &mut data,
                    position,
                    optional_replacement_span,
                    include_symbols,
                )?;
                Ok(response)
            }
            CompletionData::Keyword(data) => {
                let optional_replacement_span =
                    self.get_optional_replacement_span(previous_token, file);
                Ok(self.specific_keyword_completion_info(
                    ctx,
                    position,
                    file,
                    data.keyword_completions,
                    data.is_new_identifier_location,
                    optional_replacement_span,
                ))
            }
            CompletionData::JSDocTagName(_) => {
                // If the current position is a jsDoc tag name, only tag names should be provided for completion
                let mut items = get_js_doc_tag_name_completions();
                items.extend(get_js_doc_parameter_completions(
                    ctx,
                    file,
                    position,
                    c,
                    compiler_options,
                    &preferences,
                    true, /*tagNameOnly*/
                ));
                Ok(self.js_doc_completion_info(ctx, position, file, items))
            }
            CompletionData::JSDocTag(_) => {
                // If the current position is a jsDoc tag, only tags should be provided for completion
                let mut items = get_js_doc_tag_completions();
                items.extend(get_js_doc_parameter_completions(
                    ctx,
                    file,
                    position,
                    c,
                    compiler_options,
                    &preferences,
                    false, /*tagNameOnly*/
                ));
                Ok(self.js_doc_completion_info(ctx, position, file, items))
            }
            CompletionData::JSDocParameterName(data) => Ok(self.js_doc_completion_info(
                ctx,
                position,
                file,
                get_js_doc_parameter_name_completions(data.tag),
            )),
        }
    }

    // Go: ls/completions.go:527 getCompletionData
    pub fn get_completion_data(
        &self,
        ctx: &Context,
        type_checker: &mut Checker,
        file: Node,
        position: i32,
        preferences: &lsutil::UserPreferences,
        for_item_resolve: bool,
    ) -> Result<Option<CompletionData>, GoError> {
        let in_checked_file = is_checked_file(file, self.get_program().options());

        let mut current_token = astnav::get_token_at_position(file, position);

        let inside_comment = is_in_comment(file, position, current_token);

        let mut inside_js_doc_tag_type_expression = false;
        let mut inside_js_doc_import_tag = false;
        let is_in_snippet_scope = false;
        if inside_comment.is_some() {
            if has_doc_comment(file, position) {
                if position > 0
                    && source_file_text(file).as_bytes()[(position - 1) as usize] == b'@'
                {
                    // The current position is next to the '@' sign, when no tag name being provided yet.
                    // Provide a full list of tag names
                    return Ok(Some(CompletionData::JSDocTagName(
                        CompletionDataJSDocTagName,
                    )));
                } else {
                    // When completion is requested without "@", we will have check to make sure that
                    // there are no comments prefix the request position. We will only allow "*" and space.
                    // e.g
                    //   /** |c| /*
                    //
                    //   /**
                    //     |c|
                    //    */
                    //
                    //   /**
                    //    * |c|
                    //    */
                    //
                    //   /**
                    //    *         |c|
                    //    */
                    let line_start =
                        crate::format::get_line_start_position_for_position(position, file);
                    let mut no_comment_prefix = true;
                    // PORT: Go ranges over the runes of `file.Text()[lineStart:position]`
                    // (byte offsets). A rune cut by `position` decodes as RuneError
                    // with width 1, as in Go.
                    let text = source_file_text(file);
                    let end = position as usize;
                    let mut pos = line_start as usize;
                    while pos < end {
                        let (mut r, mut size) = utf8_decode_rune_in_string(&text, pos);
                        if pos + size as usize > end {
                            r = RUNE_ERROR;
                            size = 1;
                        }
                        let r = rune_to_char(r);
                        if !(is_white_space_single_line(r)
                            || r == '*'
                            || r == '/'
                            || r == '('
                            || r == ')'
                            || r == '|')
                        {
                            no_comment_prefix = false;
                            break;
                        }
                        pos += size as usize;
                    }
                    if no_comment_prefix {
                        return Ok(Some(CompletionData::JSDocTag(CompletionDataJSDocTag)));
                    }
                }
            }

            // Completion should work inside certain JSDoc tags. For example:
            //     /** @type {number | string} */
            // Completion should work in the brackets
            let tag = get_js_doc_tag_at_position(current_token, position);
            if tag.is_some() {
                if tag.tag_name().pos() <= position && position <= tag.tag_name().end() {
                    return Ok(Some(CompletionData::JSDocTagName(
                        CompletionDataJSDocTagName,
                    )));
                }
                if is_js_doc_import_tag(tag) {
                    inside_js_doc_import_tag = true;
                } else {
                    let type_expression = try_get_type_expression_from_tag(tag);
                    if type_expression.is_some() {
                        current_token = astnav::get_token_at_position(file, position);
                        if current_token.is_nil()
                            || (!is_declaration_name(current_token)
                                && (current_token.parent().kind() != SyntaxKind::JsDocPropertyTag
                                    || current_token.parent().name() != current_token))
                        {
                            // Use as type location if inside tag's type expression
                            inside_js_doc_tag_type_expression =
                                is_currently_editing_node(type_expression, file, position);
                        }
                    }
                    if !inside_js_doc_tag_type_expression
                        && is_js_doc_parameter_tag(tag)
                        && (node_is_missing(tag.name())
                            || tag.name().pos() <= position && position <= tag.name().end())
                    {
                        return Ok(Some(CompletionData::JSDocParameterName(
                            CompletionDataJSDocParameterName { tag },
                        )));
                    }
                }
            }

            if !inside_js_doc_tag_type_expression && !inside_js_doc_import_tag {
                // Proceed if the current position is in JSDoc tag expression; otherwise it is a normal
                // comment or the plain text part of a JSDoc comment, so no completion should be available
                return Ok(None);
            }
        }

        // The decision to provide completion depends on the contextToken, which is determined through the previousToken.
        // Note: 'previousToken' (and thus 'contextToken') can be undefined if we are the beginning of the file
        let is_js_only_location = !inside_js_doc_tag_type_expression
            && !inside_js_doc_import_tag
            && is_source_file_js(file);
        let (mut context_token, previous_token) = get_relevant_tokens(position, file);

        // Find the node where completion is requested on.
        // Also determine whether we are trying to complete with members of that node
        // or attributes of a JSX tag.
        let mut node = current_token;
        let mut property_access_to_convert = Node::NIL;
        let mut is_right_of_dot = false;
        let mut is_right_of_question_dot = false;
        let mut is_right_of_open_tag = false;
        let mut is_starting_close_tag = false;
        let mut jsx_initializer = JsxInitializer::default();
        let mut is_jsx_identifier_expected = false;
        let mut import_statement_completion: Option<ImportStatementCompletionInfo> = None;
        let mut location = astnav::get_touching_property_name(file, position);
        let mut keyword_filters = KeywordCompletionFilters::NONE;
        let mut is_new_identifier_location = false;
        // !!! flags := CompletionInfoFlagsNone
        let default_commit_characters: Option<Vec<String>> = None;

        if context_token.is_some() {
            let import_statement_completion_info =
                self.get_import_statement_completion_info(context_token, file);
            if import_statement_completion_info.keyword_completion != SyntaxKind::Unknown {
                if import_statement_completion_info.is_keyword_only_completion {
                    return Ok(Some(CompletionData::Keyword(CompletionDataKeyword {
                        keyword_completions: vec![CompletionItem {
                            completion_item: lsproto::CompletionItem {
                                label: token_to_string(
                                    import_statement_completion_info.keyword_completion,
                                )
                                .to_string(),
                                kind: Some(lsproto::CompletionItemKind::KEYWORD),
                                sort_text: Some(SORT_TEXT_GLOBALS_OR_KEYWORDS.to_string()),
                                ..Default::default()
                            },
                            symbol: SymbolId::NIL,
                        }],
                        is_new_identifier_location: import_statement_completion_info
                            .is_new_identifier_location,
                    })));
                }
                keyword_filters = keyword_filters_from_syntax_kind(
                    import_statement_completion_info.keyword_completion,
                );
            }
            if import_statement_completion_info.replacement_span.is_some()
                && preferences
                    .include_completions_for_import_statements
                    .is_true()
            {
                // !!! flags |= CompletionInfoFlags.IsImportStatementCompletion;
                import_statement_completion = Some(import_statement_completion_info.clone());
                is_new_identifier_location =
                    import_statement_completion_info.is_new_identifier_location;
            }
            // Bail out if this is a known invalid completion location.
            if import_statement_completion_info.replacement_span.is_none()
                && is_completion_list_blocker(
                    context_token,
                    previous_token,
                    location,
                    file,
                    position,
                    type_checker,
                )
            {
                if keyword_filters != KeywordCompletionFilters::NONE {
                    let (is_new_identifier_location, _) =
                        compute_commit_characters_and_is_new_identifier(
                            context_token,
                            file,
                            position,
                        );
                    return Ok(Some(CompletionData::Keyword(keyword_completion_data(
                        keyword_filters,
                        is_js_only_location,
                        is_new_identifier_location,
                    ))));
                }
                return Ok(None);
            }

            let mut parent = context_token.parent();
            if context_token.kind() == SyntaxKind::DotToken
                || context_token.kind() == SyntaxKind::QuestionDotToken
            {
                is_right_of_dot = context_token.kind() == SyntaxKind::DotToken;
                is_right_of_question_dot = context_token.kind() == SyntaxKind::QuestionDotToken;
                match parent.kind() {
                    SyntaxKind::PropertyAccessExpression => {
                        property_access_to_convert = parent;
                        node = property_access_to_convert.expression();
                        let left_most_access_expression = get_leftmost_access_expression(parent);
                        if node_is_missing(left_most_access_expression)
                            || ((is_call_expression(node) || is_function_like(node))
                                && node.end() == context_token.pos()
                                && lsutil::get_last_child(node, file).kind()
                                    != SyntaxKind::CloseParenToken)
                        {
                            // This is likely dot from incorrectly parsed expression and user is starting to write spread
                            // eg: Math.min(./**/)
                            // const x = function (./**/) {}
                            // ({./**/})
                            return Ok(None);
                        }
                    }
                    SyntaxKind::QualifiedName => {
                        node = parent.left();
                    }
                    SyntaxKind::ModuleDeclaration => {
                        node = parent.name();
                    }
                    SyntaxKind::ImportType => {
                        node = parent;
                    }
                    SyntaxKind::MetaProperty => {
                        node = lsutil::get_first_token(parent, file);
                        if node.kind() != SyntaxKind::ImportKeyword
                            && node.kind() != SyntaxKind::NewKeyword
                        {
                            crate::core::go_panic(format!(
                                "Unexpected token kind: {}",
                                crate::gostd::debug::kind_string(node.kind())
                            ));
                        }
                    }
                    _ => {
                        // There is nothing that precedes the dot, so this likely just a stray character
                        // or leading into a '...' token. Just bail out instead.
                        return Ok(None);
                    }
                }
            } else if import_statement_completion.is_none() {
                // <UI.Test /* completion position */ />
                // If the tagname is a property access expression, we will then walk up to the top most of property access expression.
                // Then, try to get a JSX container and its associated attributes type.
                if parent.is_some() && parent.kind() == SyntaxKind::PropertyAccessExpression {
                    context_token = parent;
                    parent = parent.parent();
                }

                // Fix location
                if parent == location {
                    match current_token.kind() {
                        SyntaxKind::GreaterThanToken => {
                            if parent.kind() == SyntaxKind::JsxElement
                                || parent.kind() == SyntaxKind::JsxOpeningElement
                            {
                                location = current_token;
                            }
                        }
                        SyntaxKind::LessThanSlashToken => {
                            if parent.kind() == SyntaxKind::JsxSelfClosingElement {
                                location = current_token;
                            }
                        }
                        _ => {}
                    }
                }

                match parent.kind() {
                    SyntaxKind::JsxClosingElement => {
                        if context_token.kind() == SyntaxKind::LessThanSlashToken {
                            is_starting_close_tag = true;
                            location = context_token;
                        }
                    }
                    // PORT: Go `case KindBinaryExpression: if !may { break }; fallthrough`
                    // into the JSX case. The guard keeps that order.
                    SyntaxKind::BinaryExpression
                    | SyntaxKind::JsxSelfClosingElement
                    | SyntaxKind::JsxElement
                    | SyntaxKind::JsxOpeningElement => {
                        if parent.kind() != SyntaxKind::BinaryExpression
                            || binary_expression_may_be_open_tag(parent)
                        {
                            is_jsx_identifier_expected = true;
                            if context_token.kind() == SyntaxKind::LessThanToken {
                                is_right_of_open_tag = true;
                                location = context_token;
                            }
                        }
                    }
                    SyntaxKind::JsxExpression | SyntaxKind::JsxSpreadAttribute => {
                        // First case is for `<div foo={true} [||] />` or `<div foo={true} [||] ></div>`,
                        // `parent` will be `{true}` and `previousToken` will be `}`.
                        // Second case is for `<div foo={true} t[||] ></div>`.
                        // Second case must not match for `<div foo={undefine[||]}></div>`.
                        if previous_token.kind() == SyntaxKind::CloseBraceToken
                            || previous_token.kind() == SyntaxKind::Identifier
                                && previous_token.parent().kind() == SyntaxKind::JsxAttribute
                        {
                            is_jsx_identifier_expected = true;
                        }
                    }
                    SyntaxKind::JsxAttribute => {
                        // For `<div className="x" [||] ></div>`, `parent` will be JsxAttribute and `previousToken` will be its initializer.
                        if parent.initializer() == previous_token && previous_token.end() < position
                        {
                            is_jsx_identifier_expected = true;
                        } else {
                            match previous_token.kind() {
                                SyntaxKind::EqualsToken => {
                                    jsx_initializer.is_initializer = true;
                                }
                                SyntaxKind::Identifier => {
                                    is_jsx_identifier_expected = true;
                                    // For `<div x=[|f/**/|]`, `parent` will be `x` and `previousToken.parent` will be `f` (which is its own JsxAttribute).
                                    // Note for `<div someBool f>` we don't want to treat this as a jsx inializer, instead it's the attribute name.
                                    if parent != previous_token.parent()
                                        && parent.initializer().is_nil()
                                        && astnav::find_child_of_kind(
                                            parent,
                                            SyntaxKind::EqualsToken,
                                            file,
                                        )
                                        .is_some()
                                    {
                                        jsx_initializer.initializer = previous_token;
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        let completion_kind = CompletionKind::NONE;
        let has_unresolved_auto_imports = false;
        // This also gets mutated in nested-functions after the return
        let symbols: Vec<SymbolId> = Vec::new();
        let auto_imports: Vec<autoimport::FixAndExport> = Vec::new();
        // Keys are indexes of `symbols`.
        let symbol_to_origin_info_map: FxHashMap<i32, SymbolOriginInfo> = FxHashMap::default();
        let symbol_to_sort_text_map: FxHashMap<u64, SortText> = FxHashMap::default();
        let seen_property_symbols: FxHashSet<u64> = FxHashSet::default();
        let is_type_only_location = inside_js_doc_tag_type_expression
            || inside_js_doc_import_tag
            || import_statement_completion.is_some()
                && location.parent().is_some()
                && is_type_only_import_or_export_declaration(location.parent())
            || !is_context_token_value_location(context_token)
                && (is_possibly_type_argument_position(context_token, file, type_checker)
                    || is_part_of_type_node(location)
                    || is_context_token_type_location(context_token));

        // PORT: every local that the Go closures below capture moves into the
        // state; the rest of this function reads them through `s`.
        let mut s = GetCompletionDataState {
            l: self,
            ctx,
            type_checker,
            file,
            position,
            preferences,
            for_item_resolve,
            in_checked_file,
            inside_js_doc_tag_type_expression,
            context_token,
            previous_token,
            node,
            is_right_of_dot,
            is_right_of_question_dot,
            is_right_of_open_tag,
            import_statement_completion,
            location,
            is_type_only_location,
            keyword_filters,
            is_new_identifier_location,
            default_commit_characters,
            completion_kind,
            is_in_snippet_scope,
            symbols,
            auto_imports,
            symbol_to_origin_info_map,
            symbol_to_sort_text_map,
            seen_property_symbols,
        };

        if s.is_right_of_dot || s.is_right_of_question_dot {
            s.get_type_script_member_symbols();
        } else if s.is_right_of_open_tag {
            s.symbols = s.type_checker.get_jsx_intrinsic_tag_names_at(s.location);
            // Go: core.CheckEachDefined
            for &symbol in &s.symbols {
                if symbol.is_nil() {
                    crate::core::go_panic(
                        "GetJsxIntrinsicTagNamesAt() should all be defined".to_string(),
                    );
                }
            }
            s.try_get_global_symbols()?;
            s.completion_kind = CompletionKind::GLOBAL;
            s.keyword_filters = KeywordCompletionFilters::NONE;
        } else if is_starting_close_tag {
            let tag_name = s
                .context_token
                .parent()
                .parent()
                .opening_element()
                .tag_name();
            let tag_symbol = s.type_checker.get_symbol_at_location_exported(tag_name);
            if tag_symbol.is_some() {
                s.symbols = vec![tag_symbol];
            }
            s.completion_kind = CompletionKind::GLOBAL;
            s.keyword_filters = KeywordCompletionFilters::NONE;
        } else {
            // For JavaScript or TypeScript, if we're not after a dot, then just try to get the
            // global symbols in scope.  These results should be valid for either language as
            // the set of symbols that can be referenced from this location.
            match s.try_get_global_symbols() {
                Ok(true) => {}
                Err(err) => return Err(err),
                Ok(false) => {
                    if s.keyword_filters != KeywordCompletionFilters::NONE {
                        return Ok(Some(CompletionData::Keyword(keyword_completion_data(
                            s.keyword_filters,
                            is_js_only_location,
                            s.is_new_identifier_location,
                        ))));
                    }
                    return Ok(None);
                }
            }
        }

        let mut contextual_type_or_constraint = TypeId::NIL;
        if previous_token.is_some() {
            contextual_type_or_constraint =
                get_contextual_type(previous_token, position, file, s.type_checker);
            if contextual_type_or_constraint.is_nil() {
                contextual_type_or_constraint =
                    get_constraint_of_type_argument_property(previous_token, s.type_checker);
            }
        }

        // exclude literal suggestions after <input type="text" [||] /> microsoft/TypeScript#51667) and after closing quote (microsoft/TypeScript#52675)
        // for strings getStringLiteralCompletions handles completions
        let is_literal_expected = !(previous_token.is_some()
            && is_string_literal_like(previous_token))
            && !is_jsx_identifier_expected;
        let mut literals: Vec<LiteralValue> = Vec::new();
        if is_literal_expected {
            let mut types: Vec<TypeId> = Vec::new();
            if contextual_type_or_constraint.is_some()
                && s.type_checker.ty(contextual_type_or_constraint).is_union()
            {
                types = s
                    .type_checker
                    .ty(contextual_type_or_constraint)
                    .types()
                    .to_vec();
            } else if contextual_type_or_constraint.is_some() {
                types = vec![contextual_type_or_constraint];
            }
            // Go: core.MapNonNil (a nil value is dropped)
            for t in types {
                if is_literal(s.type_checker, t) && !s.type_checker.ty(t).is_enum_literal() {
                    if let Some(value) = s.type_checker.ty(t).as_literal_type().value() {
                        literals.push(value.clone());
                    }
                }
            }
        }

        let mut recommended_completion = SymbolId::NIL;
        if previous_token.is_some() && contextual_type_or_constraint.is_some() {
            recommended_completion = get_recommended_completion(
                previous_token,
                contextual_type_or_constraint,
                s.type_checker,
            );
        }

        if s.default_commit_characters.is_none() {
            s.default_commit_characters =
                Some(get_default_commit_characters(s.is_new_identifier_location));
        }

        Ok(Some(CompletionData::Data(Box::new(CompletionDataData {
            symbols: s.symbols,
            auto_imports: s.auto_imports,
            completion_kind: s.completion_kind,
            is_in_snippet_scope: s.is_in_snippet_scope,
            property_access_to_convert,
            is_new_identifier_location: s.is_new_identifier_location,
            location: s.location,
            keyword_filters: s.keyword_filters,
            literals,
            symbol_to_origin_info_map: s.symbol_to_origin_info_map,
            symbol_to_sort_text_map: s.symbol_to_sort_text_map,
            recommended_completion,
            previous_token,
            context_token: s.context_token,
            jsx_initializer,
            inside_js_doc_tag_type_expression,
            is_type_only_location: s.is_type_only_location,
            is_jsx_identifier_expected,
            is_right_of_open_tag: s.is_right_of_open_tag,
            is_right_of_dot_or_question_dot: s.is_right_of_dot || s.is_right_of_question_dot,
            import_statement_completion: s.import_statement_completion,
            has_unresolved_auto_imports,
            default_commit_characters: s.default_commit_characters,
        }))))
    }
}

/// The locals of Go `getCompletionData` that its closures capture.
// PORT: Go closures over shared locals become methods of this state. The
// first group is read only after the closures are made; the second group is
// changed by the closures (and read after them).
struct GetCompletionDataState<'a> {
    l: &'a LanguageService,
    ctx: &'a Context,
    type_checker: &'a mut Checker,
    file: Node,
    position: i32,
    preferences: &'a lsutil::UserPreferences,
    for_item_resolve: bool,
    in_checked_file: bool,
    inside_js_doc_tag_type_expression: bool,
    context_token: Node,
    previous_token: Node,
    node: Node,
    is_right_of_dot: bool,
    is_right_of_question_dot: bool,
    is_right_of_open_tag: bool,
    import_statement_completion: Option<ImportStatementCompletionInfo>,
    location: Node,
    is_type_only_location: bool,

    keyword_filters: KeywordCompletionFilters,
    is_new_identifier_location: bool,
    default_commit_characters: Option<Vec<String>>,
    completion_kind: CompletionKind,
    is_in_snippet_scope: bool,
    symbols: Vec<SymbolId>,
    auto_imports: Vec<autoimport::FixAndExport>,
    // Keys are indexes of `symbols`.
    symbol_to_origin_info_map: FxHashMap<i32, SymbolOriginInfo>,
    symbol_to_sort_text_map: FxHashMap<u64, SortText>,
    seen_property_symbols: FxHashSet<u64>,
}

impl GetCompletionDataState<'_> {
    // Go: ls/completions.go:795 getCompletionData.addSymbolOriginInfo
    fn add_symbol_origin_info(
        &mut self,
        symbol: SymbolId,
        insert_question_dot: bool,
        insert_await: bool,
    ) {
        let symbol_id = get_symbol_id(&self.type_checker.symbols, symbol);
        if insert_await && self.seen_property_symbols.insert(symbol_id) {
            self.symbol_to_origin_info_map.insert(
                (self.symbols.len() - 1) as i32,
                SymbolOriginInfo {
                    kind: get_nullable_symbol_origin_info_kind(
                        SymbolOriginInfoKind::PROMISE,
                        insert_question_dot,
                    ),
                    ..Default::default()
                },
            );
        } else if insert_question_dot {
            self.symbol_to_origin_info_map.insert(
                (self.symbols.len() - 1) as i32,
                SymbolOriginInfo {
                    kind: SymbolOriginInfoKind::NULLABLE,
                    ..Default::default()
                },
            );
        }
    }

    // Go: ls/completions.go:804 getCompletionData.addSymbolSortInfo
    fn add_symbol_sort_info(&mut self, symbol: SymbolId) {
        let symbol_id = get_symbol_id(&self.type_checker.symbols, symbol);
        if is_static_property(&self.type_checker.symbols, symbol) {
            self.symbol_to_sort_text_map
                .insert(symbol_id, SORT_TEXT_LOCAL_DECLARATION_PRIORITY.to_string());
        }
    }

    // Go: ls/completions.go:811 getCompletionData.addPropertySymbol
    fn add_property_symbol(
        &mut self,
        symbol: SymbolId,
        insert_await: bool,
        insert_question_dot: bool,
    ) {
        // For a computed property with an accessible name like `Symbol.iterator`,
        // we'll add a completion for the *name* `Symbol` instead of for the property.
        // If this is e.g. [Symbol.iterator], add a completion for `Symbol`.
        // Go: core.FirstNonNil
        let mut computed_property_name = Node::NIL;
        for decl in self.type_checker.sym(symbol).declarations.iter().copied() {
            let name = get_name_of_declaration(decl);
            if name.is_some() && name.kind() == SyntaxKind::ComputedPropertyName {
                computed_property_name = name;
                break;
            }
        }

        if computed_property_name.is_some() {
            let left_most_name = get_left_most_name(computed_property_name.expression()); // The completion is for `Symbol`, not `iterator`.
            let mut name_symbol = SymbolId::NIL;
            if left_most_name.is_some() {
                name_symbol = self
                    .type_checker
                    .get_symbol_at_location_exported(left_most_name);
            }
            // If this is nested like for `namespace N { export const sym = Symbol(); }`, we'll add the completion for `N`.
            let mut first_accessible_symbol = SymbolId::NIL;
            if name_symbol.is_some() {
                first_accessible_symbol =
                    get_first_symbol_in_chain(name_symbol, self.context_token, self.type_checker);
            }
            let mut first_accessible_symbol_id: u64 = 0;
            if first_accessible_symbol.is_some() {
                first_accessible_symbol_id =
                    get_symbol_id(&self.type_checker.symbols, first_accessible_symbol);
            }
            if first_accessible_symbol_id != 0
                && self
                    .seen_property_symbols
                    .insert(first_accessible_symbol_id)
            {
                self.symbols.push(first_accessible_symbol);
                self.symbol_to_sort_text_map.insert(
                    first_accessible_symbol_id,
                    SORT_TEXT_GLOBALS_OR_KEYWORDS.to_string(),
                );
                let module_symbol = self.type_checker.sym(first_accessible_symbol).parent;
                // PORT: the Go `||` chain, split so the last test runs only
                // when the first two are false.
                let mut not_module_export = module_symbol.is_nil()
                    || !self.type_checker.is_external_module_symbol(module_symbol);
                if !not_module_export {
                    let name = self.type_checker.sym(first_accessible_symbol).name.as_str();
                    not_module_export = self
                        .type_checker
                        .try_get_member_in_module_exports_and_properties(name, module_symbol)
                        != first_accessible_symbol;
                }
                if not_module_export {
                    self.symbol_to_origin_info_map.insert(
                        (self.symbols.len() - 1) as i32,
                        SymbolOriginInfo {
                            kind: get_nullable_symbol_origin_info_kind(
                                SymbolOriginInfoKind::SYMBOL_MEMBER,
                                insert_question_dot,
                            ),
                            ..Default::default()
                        },
                    );
                } else {
                    // !!! auto-import symbol
                }
            } else if first_accessible_symbol_id == 0
                || !self
                    .seen_property_symbols
                    .contains(&first_accessible_symbol_id)
            {
                self.symbols.push(symbol);
                self.add_symbol_origin_info(symbol, insert_question_dot, insert_await);
                self.add_symbol_sort_info(symbol);
            }
        } else {
            self.symbols.push(symbol);
            self.add_symbol_origin_info(symbol, insert_question_dot, insert_await);
            self.add_symbol_sort_info(symbol);
        }
    }

    // Go: ls/completions.go:861 getCompletionData.addTypeProperties
    fn add_type_properties(&mut self, t: TypeId, insert_await: bool, insert_question_dot: bool) {
        if self.type_checker.get_string_index_type(t).is_some() {
            self.is_new_identifier_location = true;
            self.default_commit_characters = Some(Vec::new());
        }
        if self.is_right_of_question_dot && !self.type_checker.get_call_signatures(t).is_empty() {
            self.is_new_identifier_location = true;
            if self.default_commit_characters.is_none() {
                // Only invalid commit character here would be `(`.
                self.default_commit_characters = Some(
                    ALL_COMMIT_CHARACTERS
                        .iter()
                        .map(|c| c.to_string())
                        .collect(),
                );
            }
        }

        let property_access = if self.node.kind() == SyntaxKind::ImportType {
            self.node
        } else {
            self.node.parent()
        };

        if self.in_checked_file {
            for symbol in self.type_checker.get_apparent_properties(t) {
                if self
                    .type_checker
                    .is_valid_property_access_for_completions_exported(property_access, t, symbol)
                {
                    self.add_property_symbol(
                        symbol,
                        false, /*insertAwait*/
                        insert_question_dot,
                    );
                }
            }
        } else {
            // In javascript files, for union types, we don't just get the members that
            // the individual types have in common, we also include all the members that
            // each individual type has. This is because we're going to add all identifiers
            // anyways. So we might as well elevate the members that were at least part
            // of the individual types to a higher status since we know what they are.
            for symbol in get_properties_for_completion(t, self.type_checker) {
                if self
                    .type_checker
                    .is_valid_property_access_for_completions_exported(property_access, t, symbol)
                {
                    self.symbols.push(symbol);
                }
            }
        }

        if insert_await {
            let promise_type = self.type_checker.get_promised_type_of_promise(t);
            if promise_type.is_some() {
                for symbol in self.type_checker.get_apparent_properties(promise_type) {
                    if self
                        .type_checker
                        .is_valid_property_access_for_completions_exported(
                            property_access,
                            promise_type,
                            symbol,
                        )
                    {
                        self.add_property_symbol(
                            symbol,
                            true, /*insertAwait*/
                            insert_question_dot,
                        );
                    }
                }
            }
        }
    }

    // Go: ls/completions.go:911 getCompletionData.getTypeScriptMemberSymbols
    fn get_type_script_member_symbols(&mut self) {
        // Right of dot member completion list
        self.completion_kind = CompletionKind::PROPERTY_ACCESS;

        let node = self.node;
        // Since this is qualified name check it's a type node location
        let is_import_type = is_literal_import_type_node(node);
        let is_type_location = (is_import_type && !node.is_type_of())
            || is_part_of_type_node(node.parent())
            || is_possibly_type_argument_position(self.context_token, self.file, self.type_checker);
        let is_rhs_of_import_declaration =
            is_in_right_side_of_internal_import_equals_declaration(node);
        if is_entity_name(node) || is_import_type || is_property_access_expression(node) {
            let is_namespace_name = is_module_declaration(node.parent());
            if is_namespace_name {
                self.is_new_identifier_location = true;
                self.default_commit_characters = Some(Vec::new());
            }
            let symbol = self.type_checker.get_symbol_at_location_exported(node);
            if symbol.is_some() {
                let symbol = self.type_checker.skip_alias(symbol);
                if self
                    .type_checker
                    .sym(symbol)
                    .flags
                    .intersects(SymbolFlags::MODULE | SymbolFlags::ENUM)
                {
                    let value_access_node = if is_import_type { node } else { node.parent() };
                    // Extract module or enum members
                    let exported_symbols = self.type_checker.get_exports_of_module_exported(symbol);
                    for exported_symbol in exported_symbols {
                        if exported_symbol.is_nil() {
                            crate::core::go_panic(
                                "getExporsOfModule() should all be defined".to_string(),
                            );
                        }
                        let is_valid_value_access = |c: &mut Checker, s: SymbolId| -> bool {
                            let name = c.sym(s).name.as_str();
                            c.is_valid_property_access_exported(value_access_node, name)
                        };
                        let is_valid_type_access = |c: &mut Checker, s: SymbolId| -> bool {
                            // Go passes a new `collections.Set[ast.SymbolId]{}`
                            // (nil map), which is `None` in p2's set value.
                            symbol_can_be_referenced_at_type_location(s, c, None)
                        };
                        let is_valid_access;
                        if is_namespace_name {
                            // At `namespace N.M/**/`, if this is the only declaration of `M`, don't include `M` as a completion.
                            let exported = self.type_checker.sym(exported_symbol);
                            is_valid_access = exported.flags.intersects(SymbolFlags::NAMESPACE)
                                && !exported
                                    .declarations
                                    .iter()
                                    .all(|declaration| declaration.parent() == node.parent());
                        } else if is_rhs_of_import_declaration {
                            // Any kind is allowed when dotting off namespace in internal import equals declaration
                            is_valid_access =
                                is_valid_type_access(&mut *self.type_checker, exported_symbol)
                                    || is_valid_value_access(
                                        &mut *self.type_checker,
                                        exported_symbol,
                                    );
                        } else if is_type_location || self.inside_js_doc_tag_type_expression {
                            is_valid_access =
                                is_valid_type_access(&mut *self.type_checker, exported_symbol);
                        } else {
                            is_valid_access =
                                is_valid_value_access(&mut *self.type_checker, exported_symbol);
                        }
                        if is_valid_access {
                            self.symbols.push(exported_symbol);
                        }
                    }

                    // If the module is merged with a value, we must get the type of the class and add its properties (for inherited static methods).
                    if !is_type_location
                        && !self.inside_js_doc_tag_type_expression
                        && self
                            .type_checker
                            .sym(symbol)
                            .declarations
                            .iter()
                            .any(|decl| {
                                decl.kind() != SyntaxKind::SourceFile
                                    && decl.kind() != SyntaxKind::ModuleDeclaration
                                    && decl.kind() != SyntaxKind::EnumDeclaration
                            })
                    {
                        let symbol_type = self
                            .type_checker
                            .get_type_of_symbol_at_location(symbol, node);
                        let mut t = self.type_checker.get_non_optional_type(symbol_type);
                        let mut insert_question_dot = false;
                        if self.type_checker.is_nullable_type(t) {
                            let can_correct_to_question_dot = self.is_right_of_dot
                                && !self.is_right_of_question_dot
                                && !self
                                    .preferences
                                    .include_automatic_optional_chain_completions
                                    .is_false();
                            if can_correct_to_question_dot || self.is_right_of_question_dot {
                                t = self.type_checker.get_non_nullable_type(t);
                                if can_correct_to_question_dot {
                                    insert_question_dot = true;
                                }
                            }
                        }
                        self.add_type_properties(
                            t,
                            node.flags().intersects(NodeFlags::AWAIT_CONTEXT),
                            insert_question_dot,
                        );
                    }

                    return;
                }
            }
        }

        if !is_type_location || is_in_type_query(node) {
            // microsoft/TypeScript#39946. Pulling on the type of a node inside of a function with a contextual `this` parameter can result in a circularity
            // if the `node` is part of the exprssion of a `yield` or `return`. This circularity doesn't exist at compile time because
            // we will check (and cache) the type of `this` *before* checking the type of the node.
            self.type_checker.try_get_this_type_at_ex_exported(
                node,
                false, /*includeGlobalThis*/
                Node::NIL,
            );
            let node_type = self.type_checker.get_type_at_location(node);
            let mut t = self.type_checker.get_non_optional_type(node_type);

            if !is_type_location {
                let mut insert_question_dot = false;
                if self.type_checker.is_nullable_type(t) {
                    let can_correct_to_question_dot = self.is_right_of_dot
                        && !self.is_right_of_question_dot
                        && !self
                            .preferences
                            .include_automatic_optional_chain_completions
                            .is_false();

                    if can_correct_to_question_dot || self.is_right_of_question_dot {
                        t = self.type_checker.get_non_nullable_type(t);
                        if can_correct_to_question_dot {
                            insert_question_dot = true;
                        }
                    }
                }
                self.add_type_properties(
                    t,
                    node.flags().intersects(NodeFlags::AWAIT_CONTEXT),
                    insert_question_dot,
                );
            } else {
                let non_nullable = self.type_checker.get_non_nullable_type(t);
                self.add_type_properties(
                    non_nullable,
                    false, /*insertAwait*/
                    false, /*insertQuestionDot*/
                );
            }
        }
    }

    // Go: ls/completions.go:1025 getCompletionData.tryGetObjectTypeLiteralInTypeArgumentCompletionSymbols
    // Aggregates relevant symbols for completion in object literals in type argument positions.
    fn try_get_object_type_literal_in_type_argument_completion_symbols(
        &mut self,
    ) -> Result<GlobalsSearch, GoError> {
        let type_literal_node = try_get_type_literal_node(self.context_token);
        if type_literal_node.is_nil() {
            return Ok(GlobalsSearch::CONTINUE);
        }

        let intersection_type_node = if is_intersection_type_node(type_literal_node.parent()) {
            type_literal_node.parent()
        } else {
            Node::NIL
        };
        let container_type_node = if intersection_type_node.is_some() {
            intersection_type_node
        } else {
            type_literal_node
        };

        let container_expected_type =
            get_constraint_of_type_argument_property(container_type_node, self.type_checker);
        if container_expected_type.is_nil() {
            return Ok(GlobalsSearch::CONTINUE);
        }

        let container_actual_type = self
            .type_checker
            .get_type_from_type_node_exported(container_type_node);

        let members = get_properties_for_completion(container_expected_type, self.type_checker);
        let existing_members =
            get_properties_for_completion(container_actual_type, self.type_checker);

        let mut existing_member_names: FxHashSet<&'static str> = FxHashSet::default();
        for member in existing_members {
            existing_member_names.insert(self.type_checker.sym(member).name.as_str());
        }

        // Go: core.Filter
        for member in members {
            if !existing_member_names.contains(self.type_checker.sym(member).name.as_str()) {
                self.symbols.push(member);
            }
        }

        self.completion_kind = CompletionKind::OBJECT_PROPERTY_DECLARATION;
        self.is_new_identifier_location = true;

        Ok(GlobalsSearch::SUCCESS)
    }

    // Go: ls/completions.go:1070 getCompletionData.tryGetObjectLikeCompletionSymbols
    // Aggregates relevant symbols for completion in object literals and object binding patterns.
    // Relevant symbols are stored in the captured 'symbols' variable.
    fn try_get_object_like_completion_symbols(&mut self) -> Result<GlobalsSearch, GoError> {
        if self.context_token.is_some() && self.context_token.kind() == SyntaxKind::DotDotDotToken {
            return Ok(GlobalsSearch::CONTINUE);
        }
        let object_like_container =
            try_get_object_like_completion_container(self.context_token, self.position, self.file);
        if object_like_container.is_nil() {
            return Ok(GlobalsSearch::CONTINUE);
        }

        // We're looking up possible property names from contextual/inferred/declared type.
        self.completion_kind = CompletionKind::OBJECT_PROPERTY_DECLARATION;

        let mut type_members: Vec<SymbolId> = Vec::new();
        let mut existing_members: Vec<Node> = Vec::new();

        if object_like_container.kind() == SyntaxKind::ObjectLiteralExpression {
            let instantiated_type =
                try_get_object_literal_contextual_type(object_like_container, self.type_checker);

            // Check completions for Object property value shorthand
            if instantiated_type.is_nil() {
                if object_like_container
                    .flags()
                    .intersects(NodeFlags::IN_WITH_STATEMENT)
                {
                    return Ok(GlobalsSearch::FAIL);
                }
                return Ok(GlobalsSearch::CONTINUE);
            }
            let completions_type = self.type_checker.get_contextual_type_exported(
                object_like_container,
                ContextFlags::IGNORE_NODE_INFERENCES,
            );
            let t = if completions_type.is_some() {
                completions_type
            } else {
                instantiated_type
            };
            let string_index_type = self.type_checker.get_string_index_type(t);
            let number_index_type = self.type_checker.get_number_index_type(t);
            self.is_new_identifier_location =
                string_index_type.is_some() || number_index_type.is_some();
            type_members = get_properties_for_object_expression(
                instantiated_type,
                completions_type,
                object_like_container,
                self.type_checker,
            );
            existing_members = object_like_container.properties().to_vec();

            if type_members.is_empty() {
                // Edge case: If NumberIndexType exists
                if number_index_type.is_nil() {
                    return Ok(GlobalsSearch::CONTINUE);
                }
            }
        } else {
            if object_like_container.kind() != SyntaxKind::ObjectBindingPattern {
                crate::core::go_panic(
                    "Expected 'objectLikeContainer' to be an object binding pattern.".to_string(),
                );
            }
            // We are *only* completing on properties from the type being destructured.
            self.is_new_identifier_location = false;
            let root_declaration = get_root_declaration(object_like_container.parent());
            // PORT: Go `ast.IsVariableLike`, named in full because ls
            // callhierarchy.go has its own `isVariableLike`.
            if !crate::ast::is_variable_like(root_declaration) {
                crate::core::go_panic("Root declaration is not variable-like.".to_string());
            }

            // We don't want to complete using the type acquired by the shape
            // of the binding pattern; we are only interested in types acquired
            // through type declaration or inference.
            // Also proceed if rootDeclaration is a parameter and if its containing function expression/arrow function is contextually typed -
            // type of parameter will flow in from the contextual type of the function.
            let mut can_get_type = has_initializer(root_declaration)
                || get_type_annotation_node(root_declaration).is_some()
                || root_declaration.parent().parent().kind() == SyntaxKind::ForOfStatement;
            if !can_get_type && root_declaration.kind() == SyntaxKind::Parameter {
                if is_expression(root_declaration.parent()) {
                    can_get_type = self
                        .type_checker
                        .get_contextual_type_exported(root_declaration.parent(), ContextFlags::NONE)
                        .is_some();
                } else if root_declaration.parent().kind() == SyntaxKind::MethodDeclaration
                    || root_declaration.parent().kind() == SyntaxKind::SetAccessor
                {
                    can_get_type = is_expression(root_declaration.parent().parent())
                        && self
                            .type_checker
                            .get_contextual_type_exported(
                                root_declaration.parent().parent(),
                                ContextFlags::NONE,
                            )
                            .is_some();
                }
            }
            if can_get_type {
                let type_for_object = self
                    .type_checker
                    .get_type_at_location(object_like_container);
                if type_for_object.is_nil() {
                    return Ok(GlobalsSearch::FAIL);
                }
                // Go: core.Filter
                let properties = self
                    .type_checker
                    .get_properties_of_type_exported(type_for_object);
                type_members = Vec::new();
                for property_symbol in properties {
                    if self.type_checker.is_property_accessible_exported(
                        object_like_container,
                        false, /*isSuper*/
                        false, /*isWrite*/
                        type_for_object,
                        property_symbol,
                    ) {
                        type_members.push(property_symbol);
                    }
                }
                existing_members = object_like_container.elements().to_vec();
            }
        }

        if !type_members.is_empty() {
            // Go: core.CheckEachDefined
            for member in &existing_members {
                if member.is_nil() {
                    crate::core::go_panic(
                        "object like properties or elements should all be defined".to_string(),
                    );
                }
            }
            // Add filtered items to the completion list.
            let (filtered_members, spread_member_names) = filter_object_members_list(
                &type_members,
                &existing_members,
                self.file,
                self.position,
                self.type_checker,
            );
            self.symbols.extend(filtered_members.iter().copied());

            // Set sort texts.
            for &member in &filtered_members {
                let symbol_id = get_symbol_id(&self.type_checker.symbols, member);
                if spread_member_names.contains(self.type_checker.sym(member).name.as_str()) {
                    self.symbol_to_sort_text_map.insert(
                        symbol_id,
                        SORT_TEXT_MEMBER_DECLARED_BY_SPREAD_ASSIGNMENT.to_string(),
                    );
                }
                if self
                    .type_checker
                    .sym(member)
                    .flags
                    .intersects(SymbolFlags::OPTIONAL)
                {
                    if !self.symbol_to_sort_text_map.contains_key(&symbol_id) {
                        self.symbol_to_sort_text_map
                            .insert(symbol_id, SORT_TEXT_OPTIONAL_MEMBER.to_string());
                    }
                }
                if object_like_container.kind() == SyntaxKind::ObjectLiteralExpression
                    && self
                        .preferences
                        .include_completions_with_object_literal_method_snippets
                        .is_true()
                {
                    let (display_name, _) = get_completion_entry_display_name_for_symbol(
                        self.file,
                        self.preferences,
                        &self.type_checker.symbols,
                        member,
                        None, /*origin*/
                        CompletionKind::OBJECT_PROPERTY_DECLARATION,
                        false, /*isJsxIdentifierExpected*/
                    );
                    if !display_name.is_empty() {
                        // Go: core.OrElse(symbolToSortTextMap[symbolId], SortTextLocationPriority)
                        let original_sort_text = match self.symbol_to_sort_text_map.get(&symbol_id)
                        {
                            Some(sort_text) if !sort_text.is_empty() => sort_text.clone(),
                            _ => SORT_TEXT_LOCATION_PRIORITY.to_string(),
                        };
                        self.symbol_to_sort_text_map.insert(
                            symbol_id,
                            object_literal_property_sort_text(&original_sort_text, &display_name),
                        );
                    }
                }
            }

            if object_like_container.kind() == SyntaxKind::ObjectLiteralExpression
                && self
                    .preferences
                    .include_completions_with_object_literal_method_snippets
                    .is_true()
            {
                for entry in self.l.collect_object_literal_method_symbols(
                    self.ctx,
                    self.type_checker,
                    &filtered_members,
                    object_like_container,
                    self.file,
                ) {
                    self.symbol_to_origin_info_map
                        .insert(self.symbols.len() as i32, entry.origin);
                    self.symbols.push(entry.symbol);
                }
            }
        }

        Ok(GlobalsSearch::SUCCESS)
    }

    // Go: ls/completions.go:1201 getCompletionData.shouldOfferImportCompletions
    fn should_offer_import_completions(&self) -> bool {
        if tspath::is_dynamic_file_name(source_file_file_name(self.file)) {
            return false;
        }
        // If already typing an import statement, provide completions for it.
        if self.import_statement_completion.is_some() {
            return true;
        }
        // If not already a module, must have modules enabled.
        if self
            .preferences
            .include_completions_for_module_exports
            .is_false()
        {
            return false;
        }
        // Always using ES modules in 6.0+
        true
    }

    // Go: ls/completions.go:1218 getCompletionData.collectAutoImports
    // Mutates `symbols`, `symbolToOriginInfoMap`, and `symbolToSortTextMap`
    // PORT: `View.GetCompletions` takes the request checker (w3 decision).
    fn collect_auto_imports(&mut self) -> Result<(), GoError> {
        // `completionItem/resolve` for auto-import completions should be resolved via the completion item data,
        // so we don't need to collect auto-import entries again.
        if self.for_item_resolve {
            return Ok(());
        }
        if !self.should_offer_import_completions() {
            return Ok(());
        }

        // import { type | -> token text should be blank
        let mut lower_case_token_text = String::new();
        let (mut usage_position, mut fidelity) =
            self.l.create_lsp_position(self.position, self.file);
        if !fidelity.is_exact() {
            return Ok(());
        }
        if self.previous_token.is_some() && is_identifier(self.previous_token) {
            (usage_position, fidelity) = self.l.create_lsp_position(
                get_token_pos_of_node(self.previous_token, self.file, false /*includeJSDoc*/),
                self.file,
            );
            if !fidelity.is_exact() {
                return Ok(());
            }
            if !(self.previous_token == self.context_token
                && self.import_statement_completion.is_some())
            {
                lower_case_token_text = strings_to_lower(self.previous_token.text());
            }
        }

        let view = self.l.get_prepared_auto_import_view(self.file)?;
        let Some(view) = view else {
            return Ok(());
        };

        self.auto_imports = view.get_completions(
            self.type_checker,
            &lower_case_token_text,
            usage_position,
            self.is_right_of_open_tag,
            self.is_type_only_location,
        );
        Ok(())
    }

    // Go: ls/completions.go:1256 getCompletionData.tryGetImportCompletionSymbols
    fn try_get_import_completion_symbols(&mut self) -> Result<GlobalsSearch, GoError> {
        if self.import_statement_completion.is_none() {
            return Ok(GlobalsSearch::CONTINUE);
        }
        self.is_new_identifier_location = true;
        self.collect_auto_imports()?;
        Ok(GlobalsSearch::SUCCESS)
    }

    // Go: ls/completions.go:1278 getCompletionData.tryGetImportOrExportClauseCompletionSymbols
    // Aggregates relevant symbols for completion in import clauses and export clauses
    // whose declarations have a module specifier; for instance, symbols will be aggregated for
    //
    //      import { | } from "moduleName";
    //      export { a as foo, | } from "moduleName";
    //
    // but not for
    //
    //      export { | };
    //
    // Relevant symbols are stored in the captured 'symbols' variable.
    fn try_get_import_or_export_clause_completion_symbols(
        &mut self,
    ) -> Result<GlobalsSearch, GoError> {
        let context_token = self.context_token;
        if context_token.is_nil() {
            return Ok(GlobalsSearch::CONTINUE);
        }

        // `import { |` or `import { a as 0, | }` or `import { type | }`
        let mut named_imports_or_exports = Node::NIL;
        if context_token.kind() == SyntaxKind::OpenBraceToken
            || context_token.kind() == SyntaxKind::CommaToken
        {
            named_imports_or_exports = if is_named_imports_or_exports(context_token.parent()) {
                context_token.parent()
            } else {
                Node::NIL
            };
        } else if is_type_keyword_token_or_identifier(context_token) {
            named_imports_or_exports =
                if is_named_imports_or_exports(context_token.parent().parent()) {
                    context_token.parent().parent()
                } else {
                    Node::NIL
                };
        }

        if named_imports_or_exports.is_nil() {
            return Ok(GlobalsSearch::CONTINUE);
        }

        // We can at least offer `type` at `import { |`
        if !is_type_keyword_token_or_identifier(context_token) {
            self.keyword_filters = KeywordCompletionFilters::TYPE_KEYWORD;
        }

        // try to show exported member for imported/re-exported module
        let module_specifier = (if named_imports_or_exports.kind() == SyntaxKind::NamedImports {
            named_imports_or_exports.parent().parent()
        } else {
            named_imports_or_exports.parent()
        })
        .module_specifier();
        if module_specifier.is_nil() {
            self.is_new_identifier_location = true;
            if named_imports_or_exports.kind() == SyntaxKind::NamedImports {
                return Ok(GlobalsSearch::FAIL);
            }
            return Ok(GlobalsSearch::CONTINUE);
        }

        let module_specifier_symbol = self
            .type_checker
            .get_symbol_at_location_exported(module_specifier);
        if module_specifier_symbol.is_nil() {
            self.is_new_identifier_location = true;
            return Ok(GlobalsSearch::FAIL);
        }

        self.completion_kind = CompletionKind::MEMBER_LIKE;
        self.is_new_identifier_location = false;
        let exports = self
            .type_checker
            .get_exports_and_properties_of_module(module_specifier_symbol);

        let mut existing: FxHashSet<&'static str> = FxHashSet::default();
        for element in named_imports_or_exports.elements() {
            if is_currently_editing_node(element, self.file, self.position) {
                continue;
            }
            existing.insert(element.property_name_or_name().text());
        }
        // Go: core.Filter
        let mut uniques: Vec<SymbolId> = Vec::new();
        for symbol in exports {
            let name = symbol_name(&self.type_checker.symbols, symbol);
            if name != INTERNAL_SYMBOL_NAME_DEFAULT && !existing.contains(name.as_str()) {
                uniques.push(symbol);
            }
        }

        let uniques_empty = uniques.is_empty();
        self.symbols.extend(uniques);
        if uniques_empty {
            // If there's nothing else to import, don't offer `type` either.
            self.keyword_filters = KeywordCompletionFilters::NONE;
        }
        Ok(GlobalsSearch::SUCCESS)
    }

    // Go: ls/completions.go:1348 getCompletionData.tryGetImportAttributesCompletionSymbols
    // import { x } from "foo" with { | }
    fn try_get_import_attributes_completion_symbols(&mut self) -> Result<GlobalsSearch, GoError> {
        let context_token = self.context_token;
        if context_token.is_nil() {
            return Ok(GlobalsSearch::CONTINUE);
        }

        let mut import_attributes = Node::NIL;
        match context_token.kind() {
            SyntaxKind::OpenBraceToken | SyntaxKind::CommaToken => {
                import_attributes = context_token.parent();
            }
            SyntaxKind::ColonToken => {
                import_attributes = context_token.parent().parent();
            }
            _ => {}
        }
        if import_attributes.is_nil() || !is_import_attributes(import_attributes) {
            return Ok(GlobalsSearch::CONTINUE);
        }

        let mut elements: Vec<Node> = Vec::new();
        if !import_attributes.attribute_list().is_nil() {
            elements = import_attributes.attribute_list().nodes().to_vec();
        }
        // Go: core.Map, collections.NewSetFromItems
        let existing: FxHashSet<&'static str> =
            elements.iter().map(|el| el.name().text()).collect();
        let attributes_type = self.type_checker.get_type_at_location(import_attributes);
        // Go: core.Filter
        let mut uniques: Vec<SymbolId> = Vec::new();
        for symbol in self.type_checker.get_apparent_properties(attributes_type) {
            if !existing.contains(symbol_name(&self.type_checker.symbols, symbol).as_str()) {
                uniques.push(symbol);
            }
        }
        self.symbols.extend(uniques);
        Ok(GlobalsSearch::SUCCESS)
    }

    // Go: ls/completions.go:1387 getCompletionData.tryGetLocalNamedExportCompletionSymbols
    // Adds local declarations for completions in named exports:
    //   export { | };
    // Does not check for the absence of a module specifier (`export {} from "./other"`)
    // because `tryGetImportOrExportClauseCompletionSymbols` runs first and handles that,
    // preventing this function from running.
    fn try_get_local_named_export_completion_symbols(&mut self) -> Result<GlobalsSearch, GoError> {
        let context_token = self.context_token;
        if context_token.is_nil() {
            return Ok(GlobalsSearch::CONTINUE);
        }
        let mut named_exports = Node::NIL;
        if context_token.kind() == SyntaxKind::OpenBraceToken
            || context_token.kind() == SyntaxKind::CommaToken
        {
            named_exports = if is_named_exports(context_token.parent()) {
                context_token.parent()
            } else {
                Node::NIL
            };
        }

        if named_exports.is_nil() {
            return Ok(GlobalsSearch::CONTINUE);
        }

        let locals_container = find_ancestor(named_exports, |node| {
            is_source_file(node) || is_module_declaration(node)
        });
        self.completion_kind = CompletionKind::NONE;
        self.is_new_identifier_location = false;
        let local_symbol = locals_container.symbol();
        let mut local_exports = SymbolTable::NIL;
        if local_symbol.is_some() {
            local_exports = self.type_checker.sym(local_symbol).exports;
        }
        // PORT: Go map order is random; the table keeps insertion order.
        for (name, symbol) in self.type_checker.symbols.entries(locals_container.locals()) {
            self.symbols.push(symbol);
            if self
                .type_checker
                .symbols
                .get_name(local_exports, &name)
                .is_some()
            {
                let symbol_id = get_symbol_id(&self.type_checker.symbols, symbol);
                self.symbol_to_sort_text_map
                    .insert(symbol_id, SORT_TEXT_OPTIONAL_MEMBER.to_string());
            }
        }

        Ok(GlobalsSearch::SUCCESS)
    }

    // Go: ls/completions.go:1421 getCompletionData.tryGetConstructorCompletion
    fn try_get_constructor_completion(&mut self) -> Result<GlobalsSearch, GoError> {
        if try_get_constructor_like_completion_container(self.context_token).is_nil() {
            return Ok(GlobalsSearch::CONTINUE);
        }

        // no members, only keywords
        self.completion_kind = CompletionKind::NONE;
        // Declaring new property/method/accessor
        self.is_new_identifier_location = true;
        // Has keywords for constructor parameter
        self.keyword_filters = KeywordCompletionFilters::CONSTRUCTOR_PARAMETER_KEYWORDS;
        Ok(GlobalsSearch::SUCCESS)
    }

    // Go: ls/completions.go:1437 getCompletionData.tryGetClassLikeCompletionSymbols
    // Aggregates relevant symbols for completion in class declaration
    // Relevant symbols are stored in the captured 'symbols' variable.
    fn try_get_class_like_completion_symbols(&mut self) -> Result<GlobalsSearch, GoError> {
        let context_token = self.context_token;
        let decl = try_get_object_type_declaration_completion_container(
            self.file,
            context_token,
            self.location,
            self.position,
        );
        if decl.is_nil() {
            return Ok(GlobalsSearch::CONTINUE);
        }

        // We're looking up possible property names from parent type.
        self.completion_kind = CompletionKind::MEMBER_LIKE;
        // Declaring new property/method/accessor
        self.is_new_identifier_location = true;
        if context_token.kind() == SyntaxKind::AsteriskToken {
            self.keyword_filters = KeywordCompletionFilters::NONE;
        } else if is_class_like(decl) {
            self.keyword_filters = KeywordCompletionFilters::CLASS_ELEMENT_KEYWORDS;
        } else {
            self.keyword_filters = KeywordCompletionFilters::INTERFACE_ELEMENT_KEYWORDS;
        }

        // If you're in an interface you don't want to repeat things from super-interface. So just stop here.
        if !is_class_like(decl) {
            return Ok(GlobalsSearch::SUCCESS);
        }

        let class_element = if context_token.kind() == SyntaxKind::SemicolonToken {
            context_token.parent().parent()
        } else {
            context_token.parent()
        };
        let mut class_element_modifier_flags = ModifierFlags::NONE;
        if is_class_element(class_element) {
            class_element_modifier_flags = class_element.modifier_flags();
        }
        // If this is context token is not something we are editing now, consider if this would lead to be modifier.
        if context_token.kind() == SyntaxKind::Identifier
            && !is_currently_editing_node(context_token, self.file, self.position)
        {
            match context_token.text() {
                "private" => class_element_modifier_flags |= ModifierFlags::PRIVATE,
                "static" => class_element_modifier_flags |= ModifierFlags::STATIC,
                "override" => class_element_modifier_flags |= ModifierFlags::OVERRIDE,
                _ => {}
            }
        }
        if is_class_static_block_declaration(class_element) {
            class_element_modifier_flags |= ModifierFlags::STATIC;
        }

        // No member list for private methods
        if !class_element_modifier_flags.intersects(ModifierFlags::PRIVATE) {
            // List of property symbols of base type that are not private and already implemented
            let base_type_nodes: Vec<Node> = if is_class_like(decl)
                && class_element_modifier_flags.intersects(ModifierFlags::OVERRIDE)
            {
                // Go: core.SingleElementSlice (nil gives an empty list)
                let extends_element = get_class_extends_heritage_element(decl);
                if extends_element.is_nil() {
                    Vec::new()
                } else {
                    vec![extends_element]
                }
            } else {
                get_all_super_type_nodes(decl)
            };
            let mut base_symbols: Vec<SymbolId> = Vec::new();
            for base_type_node in base_type_nodes {
                let t = self.type_checker.get_type_at_location(base_type_node);
                if class_element_modifier_flags.intersects(ModifierFlags::STATIC) {
                    let t_symbol = self.type_checker.ty(t).symbol;
                    if t_symbol.is_some() {
                        let symbol_type = self
                            .type_checker
                            .get_type_of_symbol_at_location(t_symbol, decl);
                        base_symbols.extend(
                            self.type_checker
                                .get_properties_of_type_exported(symbol_type),
                        );
                    }
                } else if t.is_some() {
                    base_symbols.extend(self.type_checker.get_properties_of_type_exported(t));
                }
            }

            // PORT: Go `filterClassMembersList` reads symbols without a checker
            // parameter; the Rust helper takes the checker first.
            let filtered = filter_class_members_list(
                self.type_checker,
                &base_symbols,
                &decl.members().to_vec(),
                class_element_modifier_flags,
                self.file,
                self.position,
            );
            self.symbols.extend(filtered);
            // PORT: Go `range` reads the slice once; iterate a copy.
            for (index, symbol) in self.symbols.clone().into_iter().enumerate() {
                let declaration = self.type_checker.sym(symbol).value_declaration;
                if declaration.is_some()
                    && is_class_element(declaration)
                    && declaration.name().is_some()
                    && is_computed_property_name(declaration.name())
                {
                    let origin = SymbolOriginInfo {
                        kind: SymbolOriginInfoKind::COMPUTED_PROPERTY_NAME,
                        data: SymbolOriginInfoData::ComputedPropertyName(
                            SymbolOriginInfoComputedPropertyName {
                                symbol_name: self.type_checker.symbol_to_string_exported(symbol),
                            },
                        ),
                        ..Default::default()
                    };
                    self.symbol_to_origin_info_map.insert(index as i32, origin);
                }
            }
        }

        Ok(GlobalsSearch::SUCCESS)
    }

    // Go: ls/completions.go:1528 getCompletionData.tryGetJsxCompletionSymbols
    fn try_get_jsx_completion_symbols(&mut self) -> Result<GlobalsSearch, GoError> {
        let jsx_container = try_get_containing_jsx_element(self.context_token, self.file);
        if jsx_container.is_nil() {
            return Ok(GlobalsSearch::CONTINUE);
        }
        // Cursor is inside a JSX self-closing element or opening element.
        let attrs_type = self
            .type_checker
            .get_contextual_type_exported(jsx_container.attributes(), ContextFlags::NONE);
        if attrs_type.is_nil() {
            return Ok(GlobalsSearch::CONTINUE);
        }
        let completions_type = self.type_checker.get_contextual_type_exported(
            jsx_container.attributes(),
            ContextFlags::IGNORE_NODE_INFERENCES,
        );
        let properties = get_properties_for_object_expression(
            attrs_type,
            completions_type,
            jsx_container.attributes(),
            self.type_checker,
        );
        let (filtered_symbols, spread_member_names) = filter_jsx_attributes(
            &properties,
            &jsx_container.attributes().properties().to_vec(),
            self.file,
            self.position,
            self.type_checker,
        );

        self.symbols.extend(filtered_symbols.iter().copied());
        // Set sort texts.
        for symbol in filtered_symbols {
            let symbol_id = get_symbol_id(&self.type_checker.symbols, symbol);
            if spread_member_names
                .contains(symbol_name(&self.type_checker.symbols, symbol).as_str())
            {
                self.symbol_to_sort_text_map.insert(
                    symbol_id,
                    SORT_TEXT_MEMBER_DECLARED_BY_SPREAD_ASSIGNMENT.to_string(),
                );
            }
            if self
                .type_checker
                .sym(symbol)
                .flags
                .intersects(SymbolFlags::OPTIONAL)
            {
                if !self.symbol_to_sort_text_map.contains_key(&symbol_id) {
                    self.symbol_to_sort_text_map
                        .insert(symbol_id, SORT_TEXT_OPTIONAL_MEMBER.to_string());
                }
            }
        }

        self.completion_kind = CompletionKind::MEMBER_LIKE;
        self.is_new_identifier_location = false;
        Ok(GlobalsSearch::SUCCESS)
    }

    // Go: ls/completions.go:1567 getCompletionData.getGlobalCompletions
    fn get_global_completions(&mut self) -> Result<GlobalsSearch, GoError> {
        if try_get_function_like_body_completion_container(self.context_token).is_some() {
            self.keyword_filters = KeywordCompletionFilters::FUNCTION_LIKE_BODY_KEYWORDS;
        } else {
            self.keyword_filters = KeywordCompletionFilters::ALL;
        }
        // Get all entities in the current scope.
        self.completion_kind = CompletionKind::GLOBAL;
        let (is_new_identifier_location, default_commit_characters) =
            compute_commit_characters_and_is_new_identifier(
                self.context_token,
                self.file,
                self.position,
            );
        self.is_new_identifier_location = is_new_identifier_location;
        self.default_commit_characters = Some(default_commit_characters);

        if self.previous_token != self.context_token {
            if self.previous_token.is_nil() {
                crate::core::go_panic(
                    "Expected 'contextToken' to be defined when different from 'previousToken'."
                        .to_string(),
                );
            }
        }

        // We need to find the node that will give us an appropriate scope to begin
        // aggregating completion candidates. This is achieved in 'getScopeNode'
        // by finding the first node that encompasses a position, accounting for whether a node
        // is "complete" to decide whether a position belongs to the node.
        //
        // However, at the end of an identifier, we are interested in the scope of the identifier
        // itself, but fall outside of the identifier. For instance:
        //
        //      xyz => x$
        //
        // the cursor is outside of both the 'x' and the arrow function 'xyz => x',
        // so 'xyz' is not returned in our results.
        //
        // We define 'adjustedPosition' so that we may appropriately account for
        // being at the end of an identifier. The intention is that if requesting completion
        // at the end of an identifier, it should be effectively equivalent to requesting completion
        // anywhere inside/at the beginning of the identifier. So in the previous case, the
        // 'adjustedPosition' will work as if requesting completion in the following:
        //
        //      xyz => $x
        //
        // If previousToken !== contextToken, then
        //   - 'contextToken' was adjusted to the token prior to 'previousToken'
        //      because we were at the end of an identifier.
        //   - 'previousToken' is defined.
        let adjusted_position = if self.previous_token != self.context_token {
            astnav::get_start_of_node(self.previous_token, self.file, false /*includeJSDoc*/)
        } else {
            self.position
        };

        let mut scope_node = get_scope_node(self.context_token, adjusted_position, self.file);
        if scope_node.is_nil() {
            scope_node = self.file;
        }
        self.is_in_snippet_scope = is_snippet_scope(scope_node);

        let symbol_meanings = (if self.is_type_only_location {
            SymbolFlags::NONE
        } else {
            SymbolFlags::VALUE
        }) | SymbolFlags::TYPE
            | SymbolFlags::NAMESPACE
            | SymbolFlags::ALIAS;
        let type_only_alias_needs_promotion = self.previous_token.is_some()
            && !is_valid_type_only_alias_use_site(self.previous_token);

        let symbols_in_scope = self
            .type_checker
            .get_symbols_in_scope_exported(scope_node, symbol_meanings);
        self.symbols.extend(symbols_in_scope);
        // Go: core.CheckEachDefined
        for &symbol in &self.symbols {
            if symbol.is_nil() {
                crate::core::go_panic("getSymbolsInScope() should all be defined".to_string());
            }
        }
        // PORT: Go `range` reads the slice once; iterate a copy.
        for (index, symbol) in self.symbols.clone().into_iter().enumerate() {
            let symbol_id = get_symbol_id(&self.type_checker.symbols, symbol);
            if !self.type_checker.is_arguments_symbol(symbol)
                && !self
                    .type_checker
                    .sym(symbol)
                    .declarations
                    .iter()
                    .any(|&decl| get_source_file_of_node(decl) == self.file)
            {
                self.symbol_to_sort_text_map
                    .insert(symbol_id, SORT_TEXT_GLOBALS_OR_KEYWORDS.to_string());
            }
            if type_only_alias_needs_promotion
                && !self
                    .type_checker
                    .sym(symbol)
                    .flags
                    .intersects(SymbolFlags::VALUE)
            {
                // Go: core.Find
                let type_only_alias_declaration = self
                    .type_checker
                    .sym(symbol)
                    .declarations
                    .iter()
                    .copied()
                    .find(|&decl| is_type_only_import_declaration(decl))
                    .unwrap_or(Node::NIL);
                if type_only_alias_declaration.is_some() {
                    let origin = SymbolOriginInfo {
                        kind: SymbolOriginInfoKind::TYPE_ONLY_ALIAS,
                        data: SymbolOriginInfoData::TypeOnlyAlias(SymbolOriginInfoTypeOnlyAlias {
                            declaration: type_only_alias_declaration,
                        }),
                        ..Default::default()
                    };
                    self.symbol_to_origin_info_map.insert(index as i32, origin);
                }
            }
        }

        // Need to insert 'this.' before properties of `this` type.
        if scope_node.kind() != SyntaxKind::SourceFile {
            let this_type = self.type_checker.try_get_this_type_at_ex_exported(
                scope_node,
                false, /*includeGlobalThis*/
                if is_class_like(scope_node.parent()) {
                    scope_node
                } else {
                    Node::NIL
                },
            );
            if this_type.is_some()
                && !is_probably_global_type(this_type, self.file, self.type_checker)
            {
                for symbol in get_properties_for_completion(this_type, self.type_checker) {
                    let symbol_id = get_symbol_id(&self.type_checker.symbols, symbol);
                    self.symbols.push(symbol);
                    self.symbol_to_origin_info_map.insert(
                        (self.symbols.len() - 1) as i32,
                        SymbolOriginInfo {
                            kind: SymbolOriginInfoKind::THIS_TYPE,
                            ..Default::default()
                        },
                    );
                    self.symbol_to_sort_text_map
                        .insert(symbol_id, SORT_TEXT_SUGGESTED_CLASS_MEMBERS.to_string());
                }
            }
        }

        self.collect_auto_imports()?;
        if self.is_type_only_location {
            if self.context_token.is_some() && is_assertion_expression(self.context_token.parent())
            {
                self.keyword_filters = KeywordCompletionFilters::TYPE_ASSERTION_KEYWORDS;
            } else {
                self.keyword_filters = KeywordCompletionFilters::TYPE_KEYWORDS;
            }
        }

        Ok(GlobalsSearch::SUCCESS)
    }

    // Go: ls/completions.go:1678 getCompletionData.tryGetGlobalSymbols
    // PORT: Go loops over a slice of the ten closures below; the index match
    // keeps their order.
    fn try_get_global_symbols(&mut self) -> Result<bool, GoError> {
        let mut result = GlobalsSearch::CONTINUE;
        for global_search_func in 0..10 {
            result = match global_search_func {
                0 => self.try_get_object_type_literal_in_type_argument_completion_symbols(),
                1 => self.try_get_object_like_completion_symbols(),
                2 => self.try_get_import_completion_symbols(),
                3 => self.try_get_import_or_export_clause_completion_symbols(),
                4 => self.try_get_import_attributes_completion_symbols(),
                5 => self.try_get_local_named_export_completion_symbols(),
                6 => self.try_get_constructor_completion(),
                7 => self.try_get_class_like_completion_symbols(),
                8 => self.try_get_jsx_completion_symbols(),
                _ => self.get_global_completions(),
            }?;
            if result != GlobalsSearch::CONTINUE {
                break;
            }
        }
        Ok(result == GlobalsSearch::SUCCESS)
    }
}

/// Go `strings.ToLower`: `unicode.ToLower` on each rune.
// PORT: not `str::to_lowercase`, which maps a final sigma to U+03C2 and
// U+0130 to two runes. Go maps each rune on its own; the first char of
// `char::to_lowercase` is that one-rune mapping.
fn strings_to_lower(s: &str) -> String {
    s.chars()
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect()
}

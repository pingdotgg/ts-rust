use crate::format::prelude::*;

//
// Contexts
//

// Go: format/rulecontext.go:18 optionSelector
// PORT: Go selectors take `lsutil.FormatCodeSettings` by value; these read it
// by reference. They are plain fn pointers, so the static rules can hold
// them.
pub type OptionSelector = fn(&lsutil::FormatCodeSettings) -> Tristate;
// Go: format/rulecontext.go:19 anyOptionSelector
pub type AnyOptionSelector<T> = fn(&lsutil::FormatCodeSettings) -> T;

// Go: format/rulecontext.go:22 semicolonOption
pub fn semicolon_option(options: &lsutil::FormatCodeSettings) -> lsutil::SemicolonPreference {
    options.semicolons.clone()
}

// Go: format/rulecontext.go:26 insertSpaceAfterCommaDelimiterOption
pub fn insert_space_after_comma_delimiter_option(options: &lsutil::FormatCodeSettings) -> Tristate {
    options.insert_space_after_comma_delimiter
}

// Go: format/rulecontext.go:30 insertSpaceAfterSemicolonInForStatementsOption
pub fn insert_space_after_semicolon_in_for_statements_option(
    options: &lsutil::FormatCodeSettings,
) -> Tristate {
    options.insert_space_after_semicolon_in_for_statements
}

// Go: format/rulecontext.go:34 insertSpaceBeforeAndAfterBinaryOperatorsOption
pub fn insert_space_before_and_after_binary_operators_option(
    options: &lsutil::FormatCodeSettings,
) -> Tristate {
    options.insert_space_before_and_after_binary_operators
}

// Go: format/rulecontext.go:38 insertSpaceAfterConstructorOption
pub fn insert_space_after_constructor_option(options: &lsutil::FormatCodeSettings) -> Tristate {
    options.insert_space_after_constructor
}

// Go: format/rulecontext.go:42 insertSpaceAfterKeywordsInControlFlowStatementsOption
pub fn insert_space_after_keywords_in_control_flow_statements_option(
    options: &lsutil::FormatCodeSettings,
) -> Tristate {
    options.insert_space_after_keywords_in_control_flow_statements
}

// Go: format/rulecontext.go:46 insertSpaceAfterFunctionKeywordForAnonymousFunctionsOption
pub fn insert_space_after_function_keyword_for_anonymous_functions_option(
    options: &lsutil::FormatCodeSettings,
) -> Tristate {
    options.insert_space_after_function_keyword_for_anonymous_functions
}

// Go: format/rulecontext.go:50 insertSpaceAfterOpeningAndBeforeClosingNonemptyParenthesisOption
pub fn insert_space_after_opening_and_before_closing_nonempty_parenthesis_option(
    options: &lsutil::FormatCodeSettings,
) -> Tristate {
    options.insert_space_after_opening_and_before_closing_nonempty_parenthesis
}

// Go: format/rulecontext.go:54 insertSpaceAfterOpeningAndBeforeClosingNonemptyBracketsOption
pub fn insert_space_after_opening_and_before_closing_nonempty_brackets_option(
    options: &lsutil::FormatCodeSettings,
) -> Tristate {
    options.insert_space_after_opening_and_before_closing_nonempty_brackets
}

// Go: format/rulecontext.go:58 insertSpaceAfterOpeningAndBeforeClosingNonemptyBracesOption
pub fn insert_space_after_opening_and_before_closing_nonempty_braces_option(
    options: &lsutil::FormatCodeSettings,
) -> Tristate {
    options.insert_space_after_opening_and_before_closing_nonempty_braces
}

// Go: format/rulecontext.go:62 insertSpaceAfterOpeningAndBeforeClosingEmptyBracesOption
pub fn insert_space_after_opening_and_before_closing_empty_braces_option(
    options: &lsutil::FormatCodeSettings,
) -> Tristate {
    options.insert_space_after_opening_and_before_closing_empty_braces
}

// Go: format/rulecontext.go:66 insertSpaceAfterOpeningAndBeforeClosingTemplateStringBracesOption
pub fn insert_space_after_opening_and_before_closing_template_string_braces_option(
    options: &lsutil::FormatCodeSettings,
) -> Tristate {
    options.insert_space_after_opening_and_before_closing_template_string_braces
}

// Go: format/rulecontext.go:70 insertSpaceAfterOpeningAndBeforeClosingJsxExpressionBracesOption
pub fn insert_space_after_opening_and_before_closing_jsx_expression_braces_option(
    options: &lsutil::FormatCodeSettings,
) -> Tristate {
    options.insert_space_after_opening_and_before_closing_jsx_expression_braces
}

// Go: format/rulecontext.go:74 insertSpaceAfterTypeAssertionOption
pub fn insert_space_after_type_assertion_option(options: &lsutil::FormatCodeSettings) -> Tristate {
    options.insert_space_after_type_assertion
}

// Go: format/rulecontext.go:78 insertSpaceBeforeFunctionParenthesisOption
pub fn insert_space_before_function_parenthesis_option(
    options: &lsutil::FormatCodeSettings,
) -> Tristate {
    options.insert_space_before_function_parenthesis
}

// Go: format/rulecontext.go:82 placeOpenBraceOnNewLineForFunctionsOption
pub fn place_open_brace_on_new_line_for_functions_option(
    options: &lsutil::FormatCodeSettings,
) -> Tristate {
    options.place_open_brace_on_new_line_for_functions
}

// Go: format/rulecontext.go:86 placeOpenBraceOnNewLineForControlBlocksOption
pub fn place_open_brace_on_new_line_for_control_blocks_option(
    options: &lsutil::FormatCodeSettings,
) -> Tristate {
    options.place_open_brace_on_new_line_for_control_blocks
}

// Go: format/rulecontext.go:90 insertSpaceBeforeTypeAnnotationOption
pub fn insert_space_before_type_annotation_option(
    options: &lsutil::FormatCodeSettings,
) -> Tristate {
    options.insert_space_before_type_annotation
}

// Go: format/rulecontext.go:94 indentMultiLineObjectLiteralBeginningOnBlankLineOption
pub fn indent_multi_line_object_literal_beginning_on_blank_line_option(
    options: &lsutil::FormatCodeSettings,
) -> Tristate {
    options.indent_multi_line_object_literal_beginning_on_blank_line
}

// Go: format/rulecontext.go:98 indentSwitchCaseOption
pub fn indent_switch_case_option(options: &lsutil::FormatCodeSettings) -> Tristate {
    options.indent_switch_case
}

// Go: format/rulecontext.go:102 optionEquals
// PORT: Go is generic over the option type; the one use is the semicolons
// option, so this takes that type.
#[must_use]
pub const fn option_equals(
    option_name: AnyOptionSelector<lsutil::SemicolonPreference>,
    option_value: lsutil::SemicolonPreference,
) -> ContextPredicate {
    ContextPredicate::OptionEquals(option_name, option_value)
}

// Go: format/rulecontext.go:108 isOptionEnabled
#[must_use]
pub const fn is_option_enabled(option_name: OptionSelector) -> ContextPredicate {
    ContextPredicate::OptionEnabled(option_name)
}

// Go: format/rulecontext.go:114 isOptionDisabled
#[must_use]
pub const fn is_option_disabled(option_name: OptionSelector) -> ContextPredicate {
    ContextPredicate::OptionDisabled(option_name)
}

// Go: format/rulecontext.go:120 isOptionDisabledOrUndefined
#[must_use]
pub const fn is_option_disabled_or_undefined(option_name: OptionSelector) -> ContextPredicate {
    ContextPredicate::OptionDisabledOrUndefined(option_name)
}

// Go: format/rulecontext.go:126 isOptionDisabledOrUndefinedOrTokensOnSameLine
#[must_use]
pub const fn is_option_disabled_or_undefined_or_tokens_on_same_line(
    option_name: OptionSelector,
) -> ContextPredicate {
    ContextPredicate::OptionDisabledOrUndefinedOrTokensOnSameLine(option_name)
}

// Go: format/rulecontext.go:132 isOptionEnabledOrUndefined
#[must_use]
pub const fn is_option_enabled_or_undefined(option_name: OptionSelector) -> ContextPredicate {
    ContextPredicate::OptionEnabledOrUndefined(option_name)
}

impl ContextPredicate {
    /// Runs the predicate: Go `p(context)`. The option arms are the bodies of
    /// the closures that the helpers above return in Go.
    pub fn call(self, context: &mut FormattingContext) -> bool {
        match self {
            ContextPredicate::Fn(f) => f(context),
            // Go: format/rulecontext.go:102 optionEquals
            ContextPredicate::OptionEquals(option_name, option_value) => {
                option_name(&context.options) == option_value
            }
            // Go: format/rulecontext.go:108 isOptionEnabled
            ContextPredicate::OptionEnabled(option_name) => option_name(&context.options).is_true(),
            // Go: format/rulecontext.go:114 isOptionDisabled
            ContextPredicate::OptionDisabled(option_name) => {
                option_name(&context.options).is_false()
            }
            // Go: format/rulecontext.go:120 isOptionDisabledOrUndefined
            ContextPredicate::OptionDisabledOrUndefined(option_name) => {
                option_name(&context.options).is_false_or_unknown()
            }
            // Go: format/rulecontext.go:126 isOptionDisabledOrUndefinedOrTokensOnSameLine
            ContextPredicate::OptionDisabledOrUndefinedOrTokensOnSameLine(option_name) => {
                option_name(&context.options).is_false_or_unknown()
                    || context.tokens_are_on_same_line()
            }
            // Go: format/rulecontext.go:132 isOptionEnabledOrUndefined
            ContextPredicate::OptionEnabledOrUndefined(option_name) => {
                option_name(&context.options).is_true_or_unknown()
            }
        }
    }
}

// Go: format/rulecontext.go:138 isForContext
pub fn is_for_context(context: &mut FormattingContext) -> bool {
    context.context_node.kind() == SyntaxKind::ForStatement
}

// Go: format/rulecontext.go:142 isNotForContext
pub fn is_not_for_context(context: &mut FormattingContext) -> bool {
    !is_for_context(context)
}

// Go: format/rulecontext.go:146 isBinaryOpContext
pub fn is_binary_op_context(context: &mut FormattingContext) -> bool {
    match context.context_node.kind() {
        SyntaxKind::BinaryExpression => {
            return context.context_node.operator_token().kind() != SyntaxKind::CommaToken;
        }
        SyntaxKind::ConditionalExpression
        | SyntaxKind::ConditionalType
        | SyntaxKind::AsExpression
        | SyntaxKind::ExportSpecifier
        | SyntaxKind::ImportSpecifier
        | SyntaxKind::TypePredicate
        | SyntaxKind::UnionType
        | SyntaxKind::IntersectionType
        | SyntaxKind::SatisfiesExpression => {
            return true;
        }

        // equals in binding elements func foo([[x, y] = [1, 2]])
        // PORT: the Go cases BindingElement, TypeAliasDeclaration,
        // ImportEqualsDeclaration, ExportAssignment and VariableDeclaration
        // fall through to the Parameter case; they are one arm here.
        SyntaxKind::BindingElement
        // equals in type X = ...
        | SyntaxKind::TypeAliasDeclaration
        // equal in import a = module('a');
        | SyntaxKind::ImportEqualsDeclaration
        // equal in export = 1
        | SyntaxKind::ExportAssignment
        // equal in let a = 0
        | SyntaxKind::VariableDeclaration
        // equal in p = 0
        | SyntaxKind::Parameter
        | SyntaxKind::EnumMember
        | SyntaxKind::PropertyDeclaration
        | SyntaxKind::PropertySignature => {
            return context.current_token_span.kind == SyntaxKind::EqualsToken
                || context.next_token_span.kind == SyntaxKind::EqualsToken;
        }
        // "in" keyword in for (let x in []) { }
        // PORT: the Go ForInStatement case falls through to TypeParameter.
        SyntaxKind::ForInStatement
        // "in" keyword in [P in keyof T] T[P]
        | SyntaxKind::TypeParameter => {
            return context.current_token_span.kind == SyntaxKind::InKeyword
                || context.next_token_span.kind == SyntaxKind::InKeyword
                || context.current_token_span.kind == SyntaxKind::EqualsToken
                || context.next_token_span.kind == SyntaxKind::EqualsToken;
        }
        // Technically, "of" is not a binary operator, but format it the same way as "in"
        SyntaxKind::ForOfStatement => {
            return context.current_token_span.kind == SyntaxKind::OfKeyword
                || context.next_token_span.kind == SyntaxKind::OfKeyword;
        }
        _ => {}
    }
    false
}

// Go: format/rulecontext.go:195 isNotBinaryOpContext
pub fn is_not_binary_op_context(context: &mut FormattingContext) -> bool {
    !is_binary_op_context(context)
}

// Go: format/rulecontext.go:199 isNotTypeAnnotationContext
pub fn is_not_type_annotation_context(context: &mut FormattingContext) -> bool {
    !is_type_annotation_context(context)
}

// Go: format/rulecontext.go:203 isTypeAnnotationContext
pub fn is_type_annotation_context(context: &mut FormattingContext) -> bool {
    let context_kind = context.context_node.kind();
    context_kind == SyntaxKind::PropertyDeclaration
        || context_kind == SyntaxKind::PropertySignature
        || context_kind == SyntaxKind::Parameter
        || context_kind == SyntaxKind::VariableDeclaration
        || is_function_like_kind(context_kind)
}

// Go: format/rulecontext.go:212 isOptionalPropertyContext
pub fn is_optional_property_context(context: &mut FormattingContext) -> bool {
    is_property_declaration(context.context_node) && has_question_token(context.context_node)
}

// Go: format/rulecontext.go:216 isNonOptionalPropertyContext
pub fn is_non_optional_property_context(context: &mut FormattingContext) -> bool {
    !is_optional_property_context(context)
}

// Go: format/rulecontext.go:220 isConditionalOperatorContext
pub fn is_conditional_operator_context(context: &mut FormattingContext) -> bool {
    context.context_node.kind() == SyntaxKind::ConditionalExpression
        || context.context_node.kind() == SyntaxKind::ConditionalType
}

// Go: format/rulecontext.go:225 isSameLineTokenOrBeforeBlockContext
pub fn is_same_line_token_or_before_block_context(context: &mut FormattingContext) -> bool {
    context.tokens_are_on_same_line() || is_before_block_context(context)
}

// Go: format/rulecontext.go:229 isBraceWrappedContext
pub fn is_brace_wrapped_context(context: &mut FormattingContext) -> bool {
    context.context_node.kind() == SyntaxKind::ObjectBindingPattern
        || context.context_node.kind() == SyntaxKind::MappedType
        || is_single_line_block_context(context)
}

// This check is done before an open brace in a control construct, a function, or a typescript block declaration
// Go: format/rulecontext.go:236 isBeforeMultilineBlockContext
pub fn is_before_multiline_block_context(context: &mut FormattingContext) -> bool {
    is_before_block_context(context)
        && !(context.next_node_all_on_same_line() || context.next_node_block_is_on_one_line())
}

// Go: format/rulecontext.go:240 isMultilineBlockContext
pub fn is_multiline_block_context(context: &mut FormattingContext) -> bool {
    is_block_context(context)
        && !(context.context_node_all_on_same_line() || context.context_node_block_is_on_one_line())
}

// Go: format/rulecontext.go:244 isSingleLineBlockContext
pub fn is_single_line_block_context(context: &mut FormattingContext) -> bool {
    is_block_context(context)
        && (context.context_node_all_on_same_line() || context.context_node_block_is_on_one_line())
}

// Go: format/rulecontext.go:248 isBlockContext
pub fn is_block_context(context: &mut FormattingContext) -> bool {
    node_is_block_context(context.context_node)
}

// Go: format/rulecontext.go:252 isBeforeBlockContext
pub fn is_before_block_context(context: &mut FormattingContext) -> bool {
    node_is_block_context(context.next_token_parent)
}

// IMPORTANT!!! This method must return true ONLY for nodes with open and close braces as immediate children
// Go: format/rulecontext.go:257 nodeIsBlockContext
pub fn node_is_block_context(node: Node) -> bool {
    if node_is_type_script_decl_with_block_context(node) {
        // This means we are in a context that looks like a block to the user, but in the grammar is actually not a node (it's a class, module, enum, object type literal, etc).
        return true;
    }

    match node.kind() {
        SyntaxKind::Block
        | SyntaxKind::CaseBlock
        | SyntaxKind::ObjectLiteralExpression
        | SyntaxKind::ModuleBlock => {
            return true;
        }
        _ => {}
    }

    false
}

// Go: format/rulecontext.go:274 isFunctionDeclContext
pub fn is_function_decl_context(context: &mut FormattingContext) -> bool {
    match context.context_node.kind() {
        // PORT: every Go case falls through to the InterfaceDeclaration case,
        // so they are one arm here.
        SyntaxKind::FunctionDeclaration
        | SyntaxKind::MethodDeclaration
        | SyntaxKind::MethodSignature
        // case ast.KindMemberFunctionDeclaration:
        | SyntaxKind::GetAccessor
        | SyntaxKind::SetAccessor
        // case ast.KindMethodSignature:
        | SyntaxKind::CallSignature
        | SyntaxKind::FunctionExpression
        | SyntaxKind::Constructor
        | SyntaxKind::ArrowFunction
        // case ast.KindConstructorDeclaration:
        // case ast.KindSimpleArrowFunctionExpression:
        // case ast.KindParenthesizedArrowFunctionExpression:
        | SyntaxKind::InterfaceDeclaration => {
            // This one is not truly a function, but for formatting purposes, it acts just like one
            return true;
        }
        _ => {}
    }

    false
}

// Go: format/rulecontext.go:300 isNotFunctionDeclContext
pub fn is_not_function_decl_context(context: &mut FormattingContext) -> bool {
    !is_function_decl_context(context)
}

// Go: format/rulecontext.go:304 isFunctionDeclarationOrFunctionExpressionContext
pub fn is_function_declaration_or_function_expression_context(
    context: &mut FormattingContext,
) -> bool {
    context.context_node.kind() == SyntaxKind::FunctionDeclaration
        || context.context_node.kind() == SyntaxKind::FunctionExpression
}

// Go: format/rulecontext.go:308 isTypeScriptDeclWithBlockContext
pub fn is_type_script_decl_with_block_context(context: &mut FormattingContext) -> bool {
    node_is_type_script_decl_with_block_context(context.context_node)
}

// Go: format/rulecontext.go:312 nodeIsTypeScriptDeclWithBlockContext
pub fn node_is_type_script_decl_with_block_context(node: Node) -> bool {
    match node.kind() {
        SyntaxKind::ClassDeclaration
        | SyntaxKind::ClassExpression
        | SyntaxKind::InterfaceDeclaration
        | SyntaxKind::EnumDeclaration
        | SyntaxKind::TypeLiteral
        | SyntaxKind::ModuleDeclaration
        | SyntaxKind::ExportDeclaration
        | SyntaxKind::NamedExports
        | SyntaxKind::ImportDeclaration
        | SyntaxKind::NamedImports => {
            return true;
        }
        _ => {}
    }

    false
}

// Go: format/rulecontext.go:330 isAfterCodeBlockContext
pub fn is_after_code_block_context(context: &mut FormattingContext) -> bool {
    match context.current_token_parent.kind() {
        SyntaxKind::ClassDeclaration
        | SyntaxKind::ModuleDeclaration
        | SyntaxKind::EnumDeclaration
        | SyntaxKind::CatchClause
        | SyntaxKind::ModuleBlock
        | SyntaxKind::SwitchStatement => {
            return true;
        }
        SyntaxKind::Block => {
            let block_parent = context.current_token_parent.parent();
            // In a codefix scenario, we can't rely on parents being set. So just always return true.
            if block_parent.is_nil()
                || block_parent.kind() != SyntaxKind::ArrowFunction
                    && block_parent.kind() != SyntaxKind::FunctionExpression
            {
                return true;
            }
        }
        _ => {}
    }
    false
}

// Go: format/rulecontext.go:349 isControlDeclContext
pub fn is_control_decl_context(context: &mut FormattingContext) -> bool {
    match context.context_node.kind() {
        // PORT: the Go statement cases fall through to CatchClause; one arm here.
        SyntaxKind::IfStatement
        | SyntaxKind::SwitchStatement
        | SyntaxKind::ForStatement
        | SyntaxKind::ForInStatement
        | SyntaxKind::ForOfStatement
        | SyntaxKind::WhileStatement
        | SyntaxKind::TryStatement
        | SyntaxKind::DoStatement
        | SyntaxKind::WithStatement
        // TODO
        // case ast.KindElseClause:
        | SyntaxKind::CatchClause => true,

        _ => false,
    }
}

// Go: format/rulecontext.go:371 isObjectContext
pub fn is_object_context(context: &mut FormattingContext) -> bool {
    context.context_node.kind() == SyntaxKind::ObjectLiteralExpression
}

// Go: format/rulecontext.go:375 isFunctionCallContext
pub fn is_function_call_context(context: &mut FormattingContext) -> bool {
    context.context_node.kind() == SyntaxKind::CallExpression
}

// Go: format/rulecontext.go:379 isNewContext
pub fn is_new_context(context: &mut FormattingContext) -> bool {
    context.context_node.kind() == SyntaxKind::NewExpression
}

// Go: format/rulecontext.go:383 isFunctionCallOrNewContext
pub fn is_function_call_or_new_context(context: &mut FormattingContext) -> bool {
    is_function_call_context(context) || is_new_context(context)
}

// Go: format/rulecontext.go:387 isPreviousTokenNotComma
pub fn is_previous_token_not_comma(context: &mut FormattingContext) -> bool {
    context.current_token_span.kind != SyntaxKind::CommaToken
}

// Go: format/rulecontext.go:391 isNextTokenNotCloseBracket
pub fn is_next_token_not_close_bracket(context: &mut FormattingContext) -> bool {
    context.next_token_span.kind != SyntaxKind::CloseBracketToken
}

// Go: format/rulecontext.go:395 isNextTokenNotCloseParen
pub fn is_next_token_not_close_paren(context: &mut FormattingContext) -> bool {
    context.next_token_span.kind != SyntaxKind::CloseParenToken
}

// Go: format/rulecontext.go:399 isArrowFunctionContext
pub fn is_arrow_function_context(context: &mut FormattingContext) -> bool {
    context.context_node.kind() == SyntaxKind::ArrowFunction
}

// Go: format/rulecontext.go:403 isImportTypeContext
pub fn is_import_type_context(context: &mut FormattingContext) -> bool {
    context.context_node.kind() == SyntaxKind::ImportType
}

// Go: format/rulecontext.go:407 isNonJsxSameLineTokenContext
pub fn is_non_jsx_same_line_token_context(context: &mut FormattingContext) -> bool {
    context.tokens_are_on_same_line() && context.context_node.kind() != SyntaxKind::JsxText
}

// Go: format/rulecontext.go:411 isNonJsxTextContext
pub fn is_non_jsx_text_context(context: &mut FormattingContext) -> bool {
    context.context_node.kind() != SyntaxKind::JsxText
}

// Go: format/rulecontext.go:415 isNonJsxElementOrFragmentContext
pub fn is_non_jsx_element_or_fragment_context(context: &mut FormattingContext) -> bool {
    context.context_node.kind() != SyntaxKind::JsxElement
        && context.context_node.kind() != SyntaxKind::JsxFragment
}

// Go: format/rulecontext.go:419 isJsxExpressionContext
pub fn is_jsx_expression_context(context: &mut FormattingContext) -> bool {
    context.context_node.kind() == SyntaxKind::JsxExpression
        || context.context_node.kind() == SyntaxKind::JsxSpreadAttribute
}

// Go: format/rulecontext.go:423 isNextTokenParentJsxAttribute
pub fn is_next_token_parent_jsx_attribute(context: &mut FormattingContext) -> bool {
    context.next_token_parent.kind() == SyntaxKind::JsxAttribute
        || (context.next_token_parent.kind() == SyntaxKind::JsxNamespacedName
            && context.next_token_parent.parent().kind() == SyntaxKind::JsxAttribute)
}

// Go: format/rulecontext.go:427 isJsxAttributeContext
pub fn is_jsx_attribute_context(context: &mut FormattingContext) -> bool {
    context.context_node.kind() == SyntaxKind::JsxAttribute
}

// Go: format/rulecontext.go:431 isNextTokenParentNotJsxNamespacedName
pub fn is_next_token_parent_not_jsx_namespaced_name(context: &mut FormattingContext) -> bool {
    context.next_token_parent.kind() != SyntaxKind::JsxNamespacedName
}

// Go: format/rulecontext.go:435 isNextTokenParentJsxNamespacedName
pub fn is_next_token_parent_jsx_namespaced_name(context: &mut FormattingContext) -> bool {
    context.next_token_parent.kind() == SyntaxKind::JsxNamespacedName
}

// Go: format/rulecontext.go:439 isJsxSelfClosingElementContext
pub fn is_jsx_self_closing_element_context(context: &mut FormattingContext) -> bool {
    context.context_node.kind() == SyntaxKind::JsxSelfClosingElement
}

// Go: format/rulecontext.go:443 isNotBeforeBlockInFunctionDeclarationContext
pub fn is_not_before_block_in_function_declaration_context(
    context: &mut FormattingContext,
) -> bool {
    !is_function_decl_context(context) && !is_before_block_context(context)
}

// Go: format/rulecontext.go:447 isEndOfDecoratorContextOnSameLine
pub fn is_end_of_decorator_context_on_same_line(context: &mut FormattingContext) -> bool {
    context.tokens_are_on_same_line()
        && has_decorators(context.context_node)
        && node_is_in_decorator_context(context.current_token_parent)
        && !node_is_in_decorator_context(context.next_token_parent)
}

// Go: format/rulecontext.go:454 nodeIsInDecoratorContext
pub fn node_is_in_decorator_context(mut node: Node) -> bool {
    while node.is_some() && is_expression(node) {
        node = node.parent();
    }
    node.is_some() && node.kind() == SyntaxKind::Decorator
}

// Go: format/rulecontext.go:461 isStartOfVariableDeclarationList
pub fn is_start_of_variable_declaration_list(context: &mut FormattingContext) -> bool {
    context.current_token_parent.kind() == SyntaxKind::VariableDeclarationList
        && get_token_pos_of_node(context.current_token_parent, context.source_file, false)
            == context.current_token_span.loc.pos()
}

// Go: format/rulecontext.go:466 isNotFormatOnEnter
pub fn is_not_format_on_enter(context: &mut FormattingContext) -> bool {
    context.formatting_request_kind != FormatRequestKind::FORMAT_ON_ENTER
}

// Go: format/rulecontext.go:470 isModuleDeclContext
pub fn is_module_decl_context(context: &mut FormattingContext) -> bool {
    context.context_node.kind() == SyntaxKind::ModuleDeclaration
}

// Go: format/rulecontext.go:474 isObjectTypeContext
pub fn is_object_type_context(context: &mut FormattingContext) -> bool {
    context.context_node.kind() == SyntaxKind::TypeLiteral // && context.contextNode.parent.Kind != ast.KindInterfaceDeclaration;
}

// Go: format/rulecontext.go:478 isConstructorSignatureContext
pub fn is_constructor_signature_context(context: &mut FormattingContext) -> bool {
    context.context_node.kind() == SyntaxKind::ConstructSignature
}

// Go: format/rulecontext.go:482 isTypeArgumentOrParameterOrAssertion
// PORT: Go takes the TextRangeWithKind by value; this reads it by reference.
pub fn is_type_argument_or_parameter_or_assertion(token: &TextRangeWithKind, parent: Node) -> bool {
    if token.kind != SyntaxKind::LessThanToken && token.kind != SyntaxKind::GreaterThanToken {
        return false;
    }
    match parent.kind() {
        SyntaxKind::TypeReference
        | SyntaxKind::TypeAssertionExpression
        | SyntaxKind::TypeAliasDeclaration
        | SyntaxKind::ClassDeclaration
        | SyntaxKind::ClassExpression
        | SyntaxKind::InterfaceDeclaration
        | SyntaxKind::FunctionDeclaration
        | SyntaxKind::FunctionExpression
        | SyntaxKind::ArrowFunction
        | SyntaxKind::MethodDeclaration
        | SyntaxKind::MethodSignature
        | SyntaxKind::CallSignature
        | SyntaxKind::ConstructSignature
        | SyntaxKind::CallExpression
        | SyntaxKind::NewExpression
        | SyntaxKind::ExpressionWithTypeArguments => true,
        _ => false,
    }
}

// Go: format/rulecontext.go:509 isTypeArgumentOrParameterOrAssertionContext
pub fn is_type_argument_or_parameter_or_assertion_context(context: &mut FormattingContext) -> bool {
    is_type_argument_or_parameter_or_assertion(
        &context.current_token_span,
        context.current_token_parent,
    ) || is_type_argument_or_parameter_or_assertion(
        &context.next_token_span,
        context.next_token_parent,
    )
}

// Go: format/rulecontext.go:514 isTypeAssertionContext
pub fn is_type_assertion_context(context: &mut FormattingContext) -> bool {
    context.context_node.kind() == SyntaxKind::TypeAssertionExpression
}

// Go: format/rulecontext.go:518 isNonTypeAssertionContext
pub fn is_non_type_assertion_context(context: &mut FormattingContext) -> bool {
    !is_type_assertion_context(context)
}

// Go: format/rulecontext.go:522 isVoidOpContext
pub fn is_void_op_context(context: &mut FormattingContext) -> bool {
    context.current_token_span.kind == SyntaxKind::VoidKeyword
        && context.current_token_parent.kind() == SyntaxKind::VoidExpression
}

// Go: format/rulecontext.go:526 isYieldOrYieldStarWithOperand
pub fn is_yield_or_yield_star_with_operand(context: &mut FormattingContext) -> bool {
    context.context_node.kind() == SyntaxKind::YieldExpression
        && context.context_node.expression().is_some()
}

// Go: format/rulecontext.go:530 isNonNullAssertionContext
pub fn is_non_null_assertion_context(context: &mut FormattingContext) -> bool {
    context.context_node.kind() == SyntaxKind::NonNullExpression
}

// Go: format/rulecontext.go:534 isNotStatementConditionContext
pub fn is_not_statement_condition_context(context: &mut FormattingContext) -> bool {
    !is_statement_condition_context(context)
}

// Go: format/rulecontext.go:538 isStatementConditionContext
pub fn is_statement_condition_context(context: &mut FormattingContext) -> bool {
    match context.context_node.kind() {
        SyntaxKind::IfStatement
        | SyntaxKind::ForStatement
        | SyntaxKind::ForInStatement
        | SyntaxKind::ForOfStatement
        | SyntaxKind::DoStatement
        | SyntaxKind::WhileStatement => true,

        _ => false,
    }
}

// Go: format/rulecontext.go:553 isSemicolonDeletionContext
pub fn is_semicolon_deletion_context(context: &mut FormattingContext) -> bool {
    let mut next_token_kind = context.next_token_span.kind;
    let mut next_token_start = context.next_token_span.loc.pos();
    if is_trivia(next_token_kind) {
        let next_real_token = if context.next_token_parent == context.current_token_parent {
            // !!! TODO: very different from strada, but strada's logic here is wonky - find the first ancestor without a parent? that's just the source file.
            astnav::find_next_token(
                context.next_token_parent,
                context.source_file,
                context.source_file,
            )
        } else {
            lsutil::get_first_token(context.next_token_parent, context.source_file)
        };

        if next_real_token.is_nil() {
            return true;
        }
        next_token_kind = next_real_token.kind();
        next_token_start = get_token_pos_of_node(next_real_token, context.source_file, false);
    }

    let start_line =
        get_ecma_line_of_position(context.source_file, context.current_token_span.loc.pos());
    let end_line = get_ecma_line_of_position(context.source_file, next_token_start);
    if start_line == end_line {
        return next_token_kind == SyntaxKind::CloseBraceToken
            || next_token_kind == SyntaxKind::EndOfFile;
    }

    if next_token_kind == SyntaxKind::SemicolonToken
        && context.current_token_span.kind == SyntaxKind::SemicolonToken
    {
        return true;
    }

    if next_token_kind == SyntaxKind::SemicolonClassElement
        || next_token_kind == SyntaxKind::SemicolonToken
    {
        return false;
    }

    if context.context_node.kind() == SyntaxKind::InterfaceDeclaration
        || context.context_node.kind() == SyntaxKind::TypeAliasDeclaration
    {
        // Can't remove semicolon after `foo`; it would parse as a method declaration:
        //
        // interface I {
        //   foo;
        //   () void
        // }
        return context.current_token_parent.kind() != SyntaxKind::PropertySignature
            || context.current_token_parent.type_().is_some()
            || next_token_kind != SyntaxKind::OpenParenToken;
    }

    if is_property_declaration(context.current_token_parent) {
        return context.current_token_parent.initializer().is_nil();
    }

    context.current_token_parent.kind() != SyntaxKind::ForStatement
        && context.current_token_parent.kind() != SyntaxKind::EmptyStatement
        && context.current_token_parent.kind() != SyntaxKind::SemicolonClassElement
        && next_token_kind != SyntaxKind::OpenBracketToken
        && next_token_kind != SyntaxKind::OpenParenToken
        && next_token_kind != SyntaxKind::PlusToken
        && next_token_kind != SyntaxKind::MinusToken
        && next_token_kind != SyntaxKind::SlashToken
        && next_token_kind != SyntaxKind::RegularExpressionLiteral
        && next_token_kind != SyntaxKind::CommaToken
        && next_token_kind != SyntaxKind::TemplateExpression
        && next_token_kind != SyntaxKind::TemplateHead
        && next_token_kind != SyntaxKind::NoSubstitutionTemplateLiteral
        && next_token_kind != SyntaxKind::DotToken
}

// Go: format/rulecontext.go:621 isSemicolonInsertionContext
pub fn is_semicolon_insertion_context(context: &mut FormattingContext) -> bool {
    lsutil::position_is_asi_candidate(
        context.current_token_span.loc.end(),
        context.current_token_parent,
        context.source_file,
    )
}

// Go: format/rulecontext.go:625 isNotPropertyAccessOnIntegerLiteral
pub fn is_not_property_access_on_integer_literal(context: &mut FormattingContext) -> bool {
    !is_property_access_expression(context.context_node)
        || !is_numeric_literal(context.context_node.expression())
        || context.context_node.expression().text().contains('.')
}

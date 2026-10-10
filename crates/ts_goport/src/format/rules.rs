use crate::format::prelude::*;

use std::sync::Arc;

// Builds a Go `[]contextPredicate{...}` literal. A bare predicate function
// becomes an `Arc`; a call such as `is_option_enabled(x)` or
// `option_equals(x, v)` already returns a `ContextPredicate`.
macro_rules! preds {
    (@one $f:ident ( $($args:tt)* )) => { $f($($args)*) };
    (@one $f:ident) => { Arc::new($f) as ContextPredicate };
    ($($f:ident $(( $($args:tt)* ))?),* $(,)?) => {
        vec![$(preds!(@one $f $(( $($args)* ))?)),*]
    };
}

// Go: format/rules.go:10 getAllRules
pub fn get_all_rules() -> Vec<RuleSpec> {
    let mut all_tokens: Vec<SyntaxKind> = Vec::with_capacity(
        (SyntaxKind::LAST_TOKEN as usize) - (SyntaxKind::FIRST_TOKEN as usize) + 1,
    );
    for token in (SyntaxKind::FIRST_TOKEN as u16)..=(SyntaxKind::LAST_TOKEN as u16) {
        let token = SyntaxKind::try_from(token).expect("token kinds are contiguous");
        if token != SyntaxKind::EndOfFile {
            all_tokens.push(token);
        }
    }

    let any_token_except = |tokens: &[SyntaxKind]| -> TokenRange {
        let mut new_tokens: Vec<SyntaxKind> = Vec::with_capacity(all_tokens.len());
        for &token in &all_tokens {
            if tokens.contains(&token) {
                continue;
            }
            new_tokens.push(token);
        }
        TokenRange {
            is_specific: false,
            tokens: new_tokens,
        }
    };

    let any_token = TokenRange {
        is_specific: false,
        tokens: all_tokens.clone(),
    };

    let mut any_token_including_multiline_comments =
        token_range_from_ex(&all_tokens, &[SyntaxKind::MultiLineCommentTrivia]);
    let any_token_including_eof = token_range_from_ex(&all_tokens, &[SyntaxKind::EndOfFile]);
    // PORT: Go `append(prefix, tokens...)` in tokenRangeFromEx writes into the
    // spare capacity of `allTokens` (cap LastToken-FirstToken+1, len one less
    // because EndOfFile is skipped), so both results above share one backing
    // array and the second call overwrites the last element of the first. In
    // Go, anyTokenIncludingMultilineComments ends with KindEndOfFile, not
    // KindMultiLineCommentTrivia (checked with go1.26 and go1.27.1,
    // followups24 appendcap-go.txt). Reproduce that.
    let last = any_token_including_multiline_comments.tokens.len() - 1;
    any_token_including_multiline_comments.tokens[last] = any_token_including_eof.tokens[last];
    let keywords = token_range_from_range(SyntaxKind::FIRST_KEYWORD, SyntaxKind::LAST_KEYWORD);
    let binary_operators = token_range_from_range(
        SyntaxKind::FIRST_BINARY_OPERATOR,
        SyntaxKind::LAST_BINARY_OPERATOR,
    );
    let binary_keyword_operators: Vec<SyntaxKind> = vec![
        SyntaxKind::InKeyword,
        SyntaxKind::InstanceOfKeyword,
        SyntaxKind::OfKeyword,
        SyntaxKind::AsKeyword,
        SyntaxKind::IsKeyword,
        SyntaxKind::SatisfiesKeyword,
    ];
    let unary_prefix_operators: Vec<SyntaxKind> = vec![
        SyntaxKind::PlusPlusToken,
        SyntaxKind::MinusToken,
        SyntaxKind::TildeToken,
        SyntaxKind::ExclamationToken,
    ];
    let unary_prefix_expressions: Vec<SyntaxKind> = vec![
        SyntaxKind::NumericLiteral,
        SyntaxKind::BigIntLiteral,
        SyntaxKind::Identifier,
        SyntaxKind::OpenParenToken,
        SyntaxKind::OpenBracketToken,
        SyntaxKind::OpenBraceToken,
        SyntaxKind::ThisKeyword,
        SyntaxKind::NewKeyword,
    ];
    let unary_preincrement_expressions: Vec<SyntaxKind> = vec![
        SyntaxKind::Identifier,
        SyntaxKind::OpenParenToken,
        SyntaxKind::ThisKeyword,
        SyntaxKind::NewKeyword,
    ];
    let unary_postincrement_expressions: Vec<SyntaxKind> = vec![
        SyntaxKind::Identifier,
        SyntaxKind::CloseParenToken,
        SyntaxKind::CloseBracketToken,
        SyntaxKind::NewKeyword,
    ];
    let unary_predecrement_expressions: Vec<SyntaxKind> = vec![
        SyntaxKind::Identifier,
        SyntaxKind::OpenParenToken,
        SyntaxKind::ThisKeyword,
        SyntaxKind::NewKeyword,
    ];
    let unary_postdecrement_expressions: Vec<SyntaxKind> = vec![
        SyntaxKind::Identifier,
        SyntaxKind::CloseParenToken,
        SyntaxKind::CloseBracketToken,
        SyntaxKind::NewKeyword,
    ];
    let comments: Vec<SyntaxKind> = vec![
        SyntaxKind::SingleLineCommentTrivia,
        SyntaxKind::MultiLineCommentTrivia,
    ];
    let type_keywords: Vec<SyntaxKind> = vec![
        SyntaxKind::AnyKeyword,
        SyntaxKind::AssertsKeyword,
        SyntaxKind::BigIntKeyword,
        SyntaxKind::BooleanKeyword,
        SyntaxKind::FalseKeyword,
        SyntaxKind::InferKeyword,
        SyntaxKind::KeyOfKeyword,
        SyntaxKind::NeverKeyword,
        SyntaxKind::NullKeyword,
        SyntaxKind::NumberKeyword,
        SyntaxKind::ObjectKeyword,
        SyntaxKind::ReadonlyKeyword,
        SyntaxKind::StringKeyword,
        SyntaxKind::SymbolKeyword,
        SyntaxKind::TypeOfKeyword,
        SyntaxKind::TrueKeyword,
        SyntaxKind::VoidKeyword,
        SyntaxKind::UndefinedKeyword,
        SyntaxKind::UniqueKeyword,
        SyntaxKind::UnknownKeyword,
    ];
    let mut type_names: Vec<SyntaxKind> = vec![SyntaxKind::Identifier];
    type_names.extend_from_slice(&type_keywords);

    // Place a space before open brace in a function declaration
    // TypeScript: Function can have return types, which can be made of tons of different token kinds
    let function_open_brace_left_token_range = any_token_including_multiline_comments.clone();

    // Place a space before open brace in a TypeScript declaration that has braces as children (class, module, enum, etc)
    let type_script_open_brace_left_token_range = token_range_from(&[
        SyntaxKind::Identifier,
        SyntaxKind::GreaterThanToken,
        SyntaxKind::MultiLineCommentTrivia,
        SyntaxKind::ClassKeyword,
        SyntaxKind::ExportKeyword,
        SyntaxKind::ImportKeyword,
    ]);

    // Place a space before open brace in a control flow construct
    let control_open_brace_left_token_range = token_range_from(&[
        SyntaxKind::CloseParenToken,
        SyntaxKind::MultiLineCommentTrivia,
        SyntaxKind::DoKeyword,
        SyntaxKind::TryKeyword,
        SyntaxKind::FinallyKeyword,
        SyntaxKind::ElseKeyword,
        SyntaxKind::CatchKeyword,
    ]);

    // These rules are higher in priority than user-configurable
    let high_priority_common_rules: Vec<RuleSpec> = vec![
        // Leave comments alone
        rule(
            "IgnoreBeforeComment",
            any_token.clone(),
            comments,
            ANY_CONTEXT.clone(),
            RuleAction::STOP_PROCESSING_SPACE_ACTIONS,
            &[],
        ),
        rule(
            "IgnoreAfterLineComment",
            SyntaxKind::SingleLineCommentTrivia,
            any_token.clone(),
            ANY_CONTEXT.clone(),
            RuleAction::STOP_PROCESSING_SPACE_ACTIONS,
            &[],
        ),
        rule(
            "NotSpaceBeforeColon",
            any_token.clone(),
            SyntaxKind::ColonToken,
            preds![
                is_non_jsx_same_line_token_context,
                is_not_binary_op_context,
                is_not_type_annotation_context
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "SpaceAfterColon",
            SyntaxKind::ColonToken,
            any_token.clone(),
            preds![
                is_non_jsx_same_line_token_context,
                is_not_binary_op_context,
                is_next_token_parent_not_jsx_namespaced_name
            ],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "NoSpaceBeforeQuestionMark",
            any_token.clone(),
            SyntaxKind::QuestionToken,
            preds![
                is_non_jsx_same_line_token_context,
                is_not_binary_op_context,
                is_not_type_annotation_context
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        // insert space after '?' only when it is used in conditional operator
        rule(
            "SpaceAfterQuestionMarkInConditionalOperator",
            SyntaxKind::QuestionToken,
            any_token.clone(),
            preds![
                is_non_jsx_same_line_token_context,
                is_conditional_operator_context
            ],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        // in other cases there should be no space between '?' and next token
        rule(
            "NoSpaceAfterQuestionMark",
            SyntaxKind::QuestionToken,
            any_token.clone(),
            preds![
                is_non_jsx_same_line_token_context,
                is_non_optional_property_context
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "NoSpaceBeforeDot",
            any_token.clone(),
            vec![SyntaxKind::DotToken, SyntaxKind::QuestionDotToken],
            preds![
                is_non_jsx_same_line_token_context,
                is_not_property_access_on_integer_literal
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "NoSpaceAfterDot",
            vec![SyntaxKind::DotToken, SyntaxKind::QuestionDotToken],
            any_token.clone(),
            preds![is_non_jsx_same_line_token_context],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "NoSpaceBetweenImportParenInImportType",
            SyntaxKind::ImportKeyword,
            SyntaxKind::OpenParenToken,
            preds![is_non_jsx_same_line_token_context, is_import_type_context],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        // Special handling of unary operators.
        // Prefix operators generally shouldn't have a space between
        // them and their target unary expression.
        rule(
            "NoSpaceAfterUnaryPrefixOperator",
            unary_prefix_operators,
            unary_prefix_expressions,
            preds![is_non_jsx_same_line_token_context, is_not_binary_op_context],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "NoSpaceAfterUnaryPreincrementOperator",
            SyntaxKind::PlusPlusToken,
            unary_preincrement_expressions,
            preds![is_non_jsx_same_line_token_context],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "NoSpaceAfterUnaryPredecrementOperator",
            SyntaxKind::MinusMinusToken,
            unary_predecrement_expressions,
            preds![is_non_jsx_same_line_token_context],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "NoSpaceBeforeUnaryPostincrementOperator",
            unary_postincrement_expressions,
            SyntaxKind::PlusPlusToken,
            preds![
                is_non_jsx_same_line_token_context,
                is_not_statement_condition_context
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "NoSpaceBeforeUnaryPostdecrementOperator",
            unary_postdecrement_expressions,
            SyntaxKind::MinusMinusToken,
            preds![
                is_non_jsx_same_line_token_context,
                is_not_statement_condition_context
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        // More unary operator special-casing.
        // DevDiv 181814: Be careful when removing leading whitespace
        // around unary operators.  Examples:
        //      1 - -2  --X--> 1--2
        //      a + ++b --X--> a+++b
        rule(
            "SpaceAfterPostincrementWhenFollowedByAdd",
            SyntaxKind::PlusPlusToken,
            SyntaxKind::PlusToken,
            preds![is_non_jsx_same_line_token_context, is_binary_op_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "SpaceAfterAddWhenFollowedByUnaryPlus",
            SyntaxKind::PlusToken,
            SyntaxKind::PlusToken,
            preds![is_non_jsx_same_line_token_context, is_binary_op_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "SpaceAfterAddWhenFollowedByPreincrement",
            SyntaxKind::PlusToken,
            SyntaxKind::PlusPlusToken,
            preds![is_non_jsx_same_line_token_context, is_binary_op_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "SpaceAfterPostdecrementWhenFollowedBySubtract",
            SyntaxKind::MinusMinusToken,
            SyntaxKind::MinusToken,
            preds![is_non_jsx_same_line_token_context, is_binary_op_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "SpaceAfterSubtractWhenFollowedByUnaryMinus",
            SyntaxKind::MinusToken,
            SyntaxKind::MinusToken,
            preds![is_non_jsx_same_line_token_context, is_binary_op_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "SpaceAfterSubtractWhenFollowedByPredecrement",
            SyntaxKind::MinusToken,
            SyntaxKind::MinusMinusToken,
            preds![is_non_jsx_same_line_token_context, is_binary_op_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "NoSpaceAfterCloseBrace",
            SyntaxKind::CloseBraceToken,
            vec![SyntaxKind::CommaToken, SyntaxKind::SemicolonToken],
            preds![is_non_jsx_same_line_token_context],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        // For functions and control block place } on a new line []ast.Kind{multi-line rule}
        rule(
            "NewLineBeforeCloseBraceInBlockContext",
            any_token_including_multiline_comments.clone(),
            SyntaxKind::CloseBraceToken,
            preds![is_multiline_block_context],
            RuleAction::INSERT_NEW_LINE,
            &[],
        ),
        // Space/new line after }.
        rule(
            "SpaceAfterCloseBrace",
            SyntaxKind::CloseBraceToken,
            any_token_except(&[SyntaxKind::CloseParenToken]),
            preds![
                is_non_jsx_same_line_token_context,
                is_after_code_block_context
            ],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        // Special case for (}, else) and (}, while) since else & while tokens are not part of the tree which makes SpaceAfterCloseBrace rule not applied
        // Also should not apply to })
        rule(
            "SpaceBetweenCloseBraceAndElse",
            SyntaxKind::CloseBraceToken,
            SyntaxKind::ElseKeyword,
            preds![is_non_jsx_same_line_token_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "SpaceBetweenCloseBraceAndWhile",
            SyntaxKind::CloseBraceToken,
            SyntaxKind::WhileKeyword,
            preds![is_non_jsx_same_line_token_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "NoSpaceBetweenEmptyBraceBrackets",
            SyntaxKind::OpenBraceToken,
            SyntaxKind::CloseBraceToken,
            preds![is_non_jsx_same_line_token_context, is_object_context],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        // Add a space after control dec context if the next character is an open bracket ex: 'if (false)[]ast.Kind{a, b} = []ast.Kind{1, 2};' -> 'if (false) []ast.Kind{a, b} = []ast.Kind{1, 2};'
        rule(
            "SpaceAfterConditionalClosingParen",
            SyntaxKind::CloseParenToken,
            SyntaxKind::OpenBracketToken,
            preds![is_control_decl_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "NoSpaceBetweenFunctionKeywordAndStar",
            SyntaxKind::FunctionKeyword,
            SyntaxKind::AsteriskToken,
            preds![is_function_declaration_or_function_expression_context],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "SpaceAfterStarInGeneratorDeclaration",
            SyntaxKind::AsteriskToken,
            SyntaxKind::Identifier,
            preds![is_function_declaration_or_function_expression_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "SpaceAfterFunctionInFuncDecl",
            SyntaxKind::FunctionKeyword,
            any_token.clone(),
            preds![is_function_decl_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        // Insert new line after { and before } in multi-line contexts.
        rule(
            "NewLineAfterOpenBraceInBlockContext",
            SyntaxKind::OpenBraceToken,
            any_token.clone(),
            preds![is_multiline_block_context],
            RuleAction::INSERT_NEW_LINE,
            &[],
        ),
        // For get/set members, we check for (identifier,identifier) since get/set don't have tokens and they are represented as just an identifier token.
        // Though, we do extra check on the context to make sure we are dealing with get/set node. Example:
        //      get x() {}
        //      set x(val) {}
        rule(
            "SpaceAfterGetSetInMember",
            vec![SyntaxKind::GetKeyword, SyntaxKind::SetKeyword],
            SyntaxKind::Identifier,
            preds![is_function_decl_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "NoSpaceBetweenYieldKeywordAndStar",
            SyntaxKind::YieldKeyword,
            SyntaxKind::AsteriskToken,
            preds![
                is_non_jsx_same_line_token_context,
                is_yield_or_yield_star_with_operand
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "SpaceBetweenYieldOrYieldStarAndOperand",
            vec![SyntaxKind::YieldKeyword, SyntaxKind::AsteriskToken],
            any_token.clone(),
            preds![
                is_non_jsx_same_line_token_context,
                is_yield_or_yield_star_with_operand
            ],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "NoSpaceBetweenReturnAndSemicolon",
            SyntaxKind::ReturnKeyword,
            SyntaxKind::SemicolonToken,
            preds![is_non_jsx_same_line_token_context],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "SpaceAfterCertainKeywords",
            vec![
                SyntaxKind::VarKeyword,
                SyntaxKind::ThrowKeyword,
                SyntaxKind::NewKeyword,
                SyntaxKind::DeleteKeyword,
                SyntaxKind::ReturnKeyword,
                SyntaxKind::TypeOfKeyword,
                SyntaxKind::AwaitKeyword,
            ],
            any_token.clone(),
            preds![is_non_jsx_same_line_token_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "SpaceAfterLetConstInVariableDeclaration",
            vec![SyntaxKind::LetKeyword, SyntaxKind::ConstKeyword],
            any_token.clone(),
            preds![
                is_non_jsx_same_line_token_context,
                is_start_of_variable_declaration_list
            ],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "NoSpaceBeforeOpenParenInFuncCall",
            any_token.clone(),
            SyntaxKind::OpenParenToken,
            preds![
                is_non_jsx_same_line_token_context,
                is_function_call_or_new_context,
                is_previous_token_not_comma
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        // Special case for binary operators (that are keywords). For these we have to add a space and shouldn't follow any user options.
        rule(
            "SpaceBeforeBinaryKeywordOperator",
            any_token.clone(),
            binary_keyword_operators.clone(),
            preds![is_non_jsx_same_line_token_context, is_binary_op_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "SpaceAfterBinaryKeywordOperator",
            binary_keyword_operators,
            any_token.clone(),
            preds![is_non_jsx_same_line_token_context, is_binary_op_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "SpaceAfterVoidOperator",
            SyntaxKind::VoidKeyword,
            any_token.clone(),
            preds![is_non_jsx_same_line_token_context, is_void_op_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        // Async-await
        rule(
            "SpaceBetweenAsyncAndOpenParen",
            SyntaxKind::AsyncKeyword,
            SyntaxKind::OpenParenToken,
            preds![
                is_arrow_function_context,
                is_non_jsx_same_line_token_context
            ],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "SpaceBetweenAsyncAndFunctionKeyword",
            SyntaxKind::AsyncKeyword,
            vec![SyntaxKind::FunctionKeyword, SyntaxKind::Identifier],
            preds![is_non_jsx_same_line_token_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        // Template string
        rule(
            "NoSpaceBetweenTagAndTemplateString",
            vec![SyntaxKind::Identifier, SyntaxKind::CloseParenToken],
            vec![
                SyntaxKind::NoSubstitutionTemplateLiteral,
                SyntaxKind::TemplateHead,
            ],
            preds![is_non_jsx_same_line_token_context],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        // JSX opening elements
        rule(
            "SpaceBeforeJsxAttribute",
            any_token.clone(),
            SyntaxKind::Identifier,
            preds![
                is_next_token_parent_jsx_attribute,
                is_non_jsx_same_line_token_context
            ],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "SpaceBeforeSlashInJsxOpeningElement",
            any_token.clone(),
            SyntaxKind::SlashToken,
            preds![
                is_jsx_self_closing_element_context,
                is_non_jsx_same_line_token_context
            ],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "NoSpaceBeforeGreaterThanTokenInJsxOpeningElement",
            SyntaxKind::SlashToken,
            SyntaxKind::GreaterThanToken,
            preds![
                is_jsx_self_closing_element_context,
                is_non_jsx_same_line_token_context
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "NoSpaceBeforeEqualInJsxAttribute",
            any_token.clone(),
            SyntaxKind::EqualsToken,
            preds![is_jsx_attribute_context, is_non_jsx_same_line_token_context],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "NoSpaceAfterEqualInJsxAttribute",
            SyntaxKind::EqualsToken,
            any_token.clone(),
            preds![is_jsx_attribute_context, is_non_jsx_same_line_token_context],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "NoSpaceBeforeJsxNamespaceColon",
            SyntaxKind::Identifier,
            SyntaxKind::ColonToken,
            preds![is_next_token_parent_jsx_namespaced_name],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "NoSpaceAfterJsxNamespaceColon",
            SyntaxKind::ColonToken,
            SyntaxKind::Identifier,
            preds![is_next_token_parent_jsx_namespaced_name],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        // TypeScript-specific rules
        // Use of module as a function call. e.g.: import m2 = module("m2");
        rule(
            "NoSpaceAfterModuleImport",
            vec![SyntaxKind::ModuleKeyword, SyntaxKind::RequireKeyword],
            SyntaxKind::OpenParenToken,
            preds![is_non_jsx_same_line_token_context],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        // Add a space around certain TypeScript keywords
        rule(
            "SpaceAfterCertainTypeScriptKeywords",
            vec![
                SyntaxKind::AbstractKeyword,
                SyntaxKind::AccessorKeyword,
                SyntaxKind::ClassKeyword,
                SyntaxKind::DeclareKeyword,
                SyntaxKind::DefaultKeyword,
                SyntaxKind::EnumKeyword,
                SyntaxKind::ExportKeyword,
                SyntaxKind::ExtendsKeyword,
                SyntaxKind::GetKeyword,
                SyntaxKind::ImplementsKeyword,
                SyntaxKind::ImportKeyword,
                SyntaxKind::InterfaceKeyword,
                SyntaxKind::ModuleKeyword,
                SyntaxKind::NamespaceKeyword,
                SyntaxKind::OverrideKeyword,
                SyntaxKind::PrivateKeyword,
                SyntaxKind::PublicKeyword,
                SyntaxKind::ProtectedKeyword,
                SyntaxKind::ReadonlyKeyword,
                SyntaxKind::SetKeyword,
                SyntaxKind::StaticKeyword,
                SyntaxKind::TypeKeyword,
                SyntaxKind::FromKeyword,
                SyntaxKind::KeyOfKeyword,
                SyntaxKind::InferKeyword,
            ],
            any_token.clone(),
            preds![is_non_jsx_same_line_token_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "SpaceBeforeCertainTypeScriptKeywords",
            any_token.clone(),
            vec![
                SyntaxKind::ExtendsKeyword,
                SyntaxKind::ImplementsKeyword,
                SyntaxKind::FromKeyword,
            ],
            preds![is_non_jsx_same_line_token_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        // Treat string literals in module names as identifiers, and add a space between the literal and the opening Brace braces, e.g.: module "m2" {
        rule(
            "SpaceAfterModuleName",
            SyntaxKind::StringLiteral,
            SyntaxKind::OpenBraceToken,
            preds![is_module_decl_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        // Lambda expressions
        rule(
            "SpaceBeforeArrow",
            any_token.clone(),
            SyntaxKind::EqualsGreaterThanToken,
            preds![is_non_jsx_same_line_token_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "SpaceAfterArrow",
            SyntaxKind::EqualsGreaterThanToken,
            any_token.clone(),
            preds![is_non_jsx_same_line_token_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        // Optional parameters and let args
        rule(
            "NoSpaceAfterEllipsis",
            SyntaxKind::DotDotDotToken,
            SyntaxKind::Identifier,
            preds![is_non_jsx_same_line_token_context],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "NoSpaceAfterOptionalParameters",
            SyntaxKind::QuestionToken,
            vec![SyntaxKind::CloseParenToken, SyntaxKind::CommaToken],
            preds![is_non_jsx_same_line_token_context, is_not_binary_op_context],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        // Remove spaces in empty interface literals. e.g.: x: {}
        rule(
            "NoSpaceBetweenEmptyInterfaceBraceBrackets",
            SyntaxKind::OpenBraceToken,
            SyntaxKind::CloseBraceToken,
            preds![is_non_jsx_same_line_token_context, is_object_type_context],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        // generics and type assertions
        rule(
            "NoSpaceBeforeOpenAngularBracket",
            type_names.clone(),
            SyntaxKind::LessThanToken,
            preds![
                is_non_jsx_same_line_token_context,
                is_type_argument_or_parameter_or_assertion_context
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "NoSpaceBetweenCloseParenAndAngularBracket",
            SyntaxKind::CloseParenToken,
            SyntaxKind::LessThanToken,
            preds![
                is_non_jsx_same_line_token_context,
                is_type_argument_or_parameter_or_assertion_context
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "NoSpaceAfterOpenAngularBracket",
            SyntaxKind::LessThanToken,
            any_token.clone(),
            preds![
                is_non_jsx_same_line_token_context,
                is_type_argument_or_parameter_or_assertion_context
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "NoSpaceBeforeCloseAngularBracket",
            any_token.clone(),
            SyntaxKind::GreaterThanToken,
            preds![
                is_non_jsx_same_line_token_context,
                is_type_argument_or_parameter_or_assertion_context
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "NoSpaceAfterCloseAngularBracket",
            SyntaxKind::GreaterThanToken,
            vec![
                SyntaxKind::OpenParenToken,
                SyntaxKind::OpenBracketToken,
                SyntaxKind::GreaterThanToken,
                SyntaxKind::CommaToken,
            ],
            preds![
                is_non_jsx_same_line_token_context,
                is_type_argument_or_parameter_or_assertion_context,
                is_not_function_decl_context, /*To prevent an interference with the SpaceBeforeOpenParenInFuncDecl rule*/
                is_non_type_assertion_context,
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        // decorators
        rule(
            "SpaceBeforeAt",
            vec![SyntaxKind::CloseParenToken, SyntaxKind::Identifier],
            SyntaxKind::AtToken,
            preds![is_non_jsx_same_line_token_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "NoSpaceAfterAt",
            SyntaxKind::AtToken,
            any_token.clone(),
            preds![is_non_jsx_same_line_token_context],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        // Insert space after @ in decorator
        rule(
            "SpaceAfterDecorator",
            any_token.clone(),
            vec![
                SyntaxKind::AbstractKeyword,
                SyntaxKind::Identifier,
                SyntaxKind::ExportKeyword,
                SyntaxKind::DefaultKeyword,
                SyntaxKind::ClassKeyword,
                SyntaxKind::StaticKeyword,
                SyntaxKind::PublicKeyword,
                SyntaxKind::PrivateKeyword,
                SyntaxKind::ProtectedKeyword,
                SyntaxKind::GetKeyword,
                SyntaxKind::SetKeyword,
                SyntaxKind::OpenBracketToken,
                SyntaxKind::AsteriskToken,
            ],
            preds![is_end_of_decorator_context_on_same_line],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "NoSpaceBeforeNonNullAssertionOperator",
            any_token.clone(),
            SyntaxKind::ExclamationToken,
            preds![
                is_non_jsx_same_line_token_context,
                is_non_null_assertion_context
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "NoSpaceAfterNewKeywordOnConstructorSignature",
            SyntaxKind::NewKeyword,
            SyntaxKind::OpenParenToken,
            preds![
                is_non_jsx_same_line_token_context,
                is_constructor_signature_context
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "SpaceLessThanAndNonJSXTypeAnnotation",
            SyntaxKind::LessThanToken,
            SyntaxKind::LessThanToken,
            preds![is_non_jsx_same_line_token_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
    ];

    // These rules are applied after high priority
    let user_configurable_rules: Vec<RuleSpec> = vec![
        // Treat constructor as an identifier in a function declaration, and remove spaces between constructor and following left parentheses
        rule(
            "SpaceAfterConstructor",
            SyntaxKind::ConstructorKeyword,
            SyntaxKind::OpenParenToken,
            preds![
                is_option_enabled(insert_space_after_constructor_option),
                is_non_jsx_same_line_token_context
            ],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "NoSpaceAfterConstructor",
            SyntaxKind::ConstructorKeyword,
            SyntaxKind::OpenParenToken,
            preds![
                is_option_disabled_or_undefined(insert_space_after_constructor_option),
                is_non_jsx_same_line_token_context
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "SpaceAfterComma",
            SyntaxKind::CommaToken,
            any_token.clone(),
            preds![
                is_option_enabled(insert_space_after_comma_delimiter_option),
                is_non_jsx_same_line_token_context,
                is_non_jsx_element_or_fragment_context,
                is_next_token_not_close_bracket,
                is_next_token_not_close_paren
            ],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "NoSpaceAfterComma",
            SyntaxKind::CommaToken,
            any_token.clone(),
            preds![
                is_option_disabled_or_undefined(insert_space_after_comma_delimiter_option),
                is_non_jsx_same_line_token_context,
                is_non_jsx_element_or_fragment_context
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        // Insert space after function keyword for anonymous functions
        rule(
            "SpaceAfterAnonymousFunctionKeyword",
            vec![SyntaxKind::FunctionKeyword, SyntaxKind::AsteriskToken],
            SyntaxKind::OpenParenToken,
            preds![
                is_option_enabled(
                    insert_space_after_function_keyword_for_anonymous_functions_option
                ),
                is_function_decl_context
            ],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "NoSpaceAfterAnonymousFunctionKeyword",
            vec![SyntaxKind::FunctionKeyword, SyntaxKind::AsteriskToken],
            SyntaxKind::OpenParenToken,
            preds![
                is_option_disabled_or_undefined(
                    insert_space_after_function_keyword_for_anonymous_functions_option
                ),
                is_function_decl_context
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        // Insert space after keywords in control flow statements
        rule(
            "SpaceAfterKeywordInControl",
            keywords.clone(),
            SyntaxKind::OpenParenToken,
            preds![
                is_option_enabled(insert_space_after_keywords_in_control_flow_statements_option),
                is_control_decl_context
            ],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "NoSpaceAfterKeywordInControl",
            keywords,
            SyntaxKind::OpenParenToken,
            preds![
                is_option_disabled_or_undefined(
                    insert_space_after_keywords_in_control_flow_statements_option
                ),
                is_control_decl_context
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        // Insert space after opening and before closing nonempty parenthesis
        rule(
            "SpaceAfterOpenParen",
            SyntaxKind::OpenParenToken,
            any_token.clone(),
            preds![
                is_option_enabled(
                    insert_space_after_opening_and_before_closing_nonempty_parenthesis_option
                ),
                is_non_jsx_same_line_token_context
            ],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "SpaceBeforeCloseParen",
            any_token.clone(),
            SyntaxKind::CloseParenToken,
            preds![
                is_option_enabled(
                    insert_space_after_opening_and_before_closing_nonempty_parenthesis_option
                ),
                is_non_jsx_same_line_token_context
            ],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "SpaceBetweenOpenParens",
            SyntaxKind::OpenParenToken,
            SyntaxKind::OpenParenToken,
            preds![
                is_option_enabled(
                    insert_space_after_opening_and_before_closing_nonempty_parenthesis_option
                ),
                is_non_jsx_same_line_token_context
            ],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "NoSpaceBetweenParens",
            SyntaxKind::OpenParenToken,
            SyntaxKind::CloseParenToken,
            preds![is_non_jsx_same_line_token_context],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "NoSpaceAfterOpenParen",
            SyntaxKind::OpenParenToken,
            any_token.clone(),
            preds![
                is_option_disabled_or_undefined(
                    insert_space_after_opening_and_before_closing_nonempty_parenthesis_option
                ),
                is_non_jsx_same_line_token_context
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "NoSpaceBeforeCloseParen",
            any_token.clone(),
            SyntaxKind::CloseParenToken,
            preds![
                is_option_disabled_or_undefined(
                    insert_space_after_opening_and_before_closing_nonempty_parenthesis_option
                ),
                is_non_jsx_same_line_token_context
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        // Insert space after opening and before closing nonempty brackets
        rule(
            "SpaceAfterOpenBracket",
            SyntaxKind::OpenBracketToken,
            any_token.clone(),
            preds![
                is_option_enabled(
                    insert_space_after_opening_and_before_closing_nonempty_brackets_option
                ),
                is_non_jsx_same_line_token_context
            ],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "SpaceBeforeCloseBracket",
            any_token.clone(),
            SyntaxKind::CloseBracketToken,
            preds![
                is_option_enabled(
                    insert_space_after_opening_and_before_closing_nonempty_brackets_option
                ),
                is_non_jsx_same_line_token_context
            ],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "NoSpaceBetweenBrackets",
            SyntaxKind::OpenBracketToken,
            SyntaxKind::CloseBracketToken,
            preds![is_non_jsx_same_line_token_context],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "NoSpaceAfterOpenBracket",
            SyntaxKind::OpenBracketToken,
            any_token.clone(),
            preds![
                is_option_disabled_or_undefined(
                    insert_space_after_opening_and_before_closing_nonempty_brackets_option
                ),
                is_non_jsx_same_line_token_context
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "NoSpaceBeforeCloseBracket",
            any_token.clone(),
            SyntaxKind::CloseBracketToken,
            preds![
                is_option_disabled_or_undefined(
                    insert_space_after_opening_and_before_closing_nonempty_brackets_option
                ),
                is_non_jsx_same_line_token_context
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        // Insert a space after { and before } in single-line contexts, but remove space from empty object literals {}.
        rule(
            "SpaceAfterOpenBrace",
            SyntaxKind::OpenBraceToken,
            any_token.clone(),
            preds![
                is_option_enabled_or_undefined(
                    insert_space_after_opening_and_before_closing_nonempty_braces_option
                ),
                is_brace_wrapped_context
            ],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "SpaceBeforeCloseBrace",
            any_token.clone(),
            SyntaxKind::CloseBraceToken,
            preds![
                is_option_enabled_or_undefined(
                    insert_space_after_opening_and_before_closing_nonempty_braces_option
                ),
                is_brace_wrapped_context
            ],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "NoSpaceBetweenEmptyBraceBrackets",
            SyntaxKind::OpenBraceToken,
            SyntaxKind::CloseBraceToken,
            preds![is_non_jsx_same_line_token_context, is_object_context],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "NoSpaceAfterOpenBrace",
            SyntaxKind::OpenBraceToken,
            any_token.clone(),
            preds![
                is_option_disabled(
                    insert_space_after_opening_and_before_closing_nonempty_braces_option
                ),
                is_non_jsx_same_line_token_context
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "NoSpaceBeforeCloseBrace",
            any_token.clone(),
            SyntaxKind::CloseBraceToken,
            preds![
                is_option_disabled(
                    insert_space_after_opening_and_before_closing_nonempty_braces_option
                ),
                is_non_jsx_same_line_token_context
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        // Insert a space after opening and before closing empty brace brackets
        rule(
            "SpaceBetweenEmptyBraceBrackets",
            SyntaxKind::OpenBraceToken,
            SyntaxKind::CloseBraceToken,
            preds![is_option_enabled(
                insert_space_after_opening_and_before_closing_empty_braces_option
            )],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "NoSpaceBetweenEmptyBraceBrackets",
            SyntaxKind::OpenBraceToken,
            SyntaxKind::CloseBraceToken,
            preds![
                is_option_disabled(
                    insert_space_after_opening_and_before_closing_empty_braces_option
                ),
                is_non_jsx_same_line_token_context
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        // Insert space after opening and before closing template string braces
        rule(
            "SpaceAfterTemplateHeadAndMiddle",
            vec![SyntaxKind::TemplateHead, SyntaxKind::TemplateMiddle],
            any_token.clone(),
            preds![
                is_option_enabled(
                    insert_space_after_opening_and_before_closing_template_string_braces_option
                ),
                is_non_jsx_text_context
            ],
            RuleAction::INSERT_SPACE,
            &[RuleFlags::CAN_DELETE_NEW_LINES],
        ),
        rule(
            "SpaceBeforeTemplateMiddleAndTail",
            any_token.clone(),
            vec![SyntaxKind::TemplateMiddle, SyntaxKind::TemplateTail],
            preds![
                is_option_enabled(
                    insert_space_after_opening_and_before_closing_template_string_braces_option
                ),
                is_non_jsx_same_line_token_context
            ],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "NoSpaceAfterTemplateHeadAndMiddle",
            vec![SyntaxKind::TemplateHead, SyntaxKind::TemplateMiddle],
            any_token.clone(),
            preds![
                is_option_disabled_or_undefined(
                    insert_space_after_opening_and_before_closing_template_string_braces_option
                ),
                is_non_jsx_text_context
            ],
            RuleAction::DELETE_SPACE,
            &[RuleFlags::CAN_DELETE_NEW_LINES],
        ),
        rule(
            "NoSpaceBeforeTemplateMiddleAndTail",
            any_token.clone(),
            vec![SyntaxKind::TemplateMiddle, SyntaxKind::TemplateTail],
            preds![
                is_option_disabled_or_undefined(
                    insert_space_after_opening_and_before_closing_template_string_braces_option
                ),
                is_non_jsx_same_line_token_context
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        // No space after { and before } in JSX expression
        rule(
            "SpaceAfterOpenBraceInJsxExpression",
            SyntaxKind::OpenBraceToken,
            any_token.clone(),
            preds![
                is_option_enabled(
                    insert_space_after_opening_and_before_closing_jsx_expression_braces_option
                ),
                is_non_jsx_same_line_token_context,
                is_jsx_expression_context
            ],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "SpaceBeforeCloseBraceInJsxExpression",
            any_token.clone(),
            SyntaxKind::CloseBraceToken,
            preds![
                is_option_enabled(
                    insert_space_after_opening_and_before_closing_jsx_expression_braces_option
                ),
                is_non_jsx_same_line_token_context,
                is_jsx_expression_context
            ],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "NoSpaceAfterOpenBraceInJsxExpression",
            SyntaxKind::OpenBraceToken,
            any_token.clone(),
            preds![
                is_option_disabled_or_undefined(
                    insert_space_after_opening_and_before_closing_jsx_expression_braces_option
                ),
                is_non_jsx_same_line_token_context,
                is_jsx_expression_context
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "NoSpaceBeforeCloseBraceInJsxExpression",
            any_token.clone(),
            SyntaxKind::CloseBraceToken,
            preds![
                is_option_disabled_or_undefined(
                    insert_space_after_opening_and_before_closing_jsx_expression_braces_option
                ),
                is_non_jsx_same_line_token_context,
                is_jsx_expression_context
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        // Insert space after semicolon in for statement
        rule(
            "SpaceAfterSemicolonInFor",
            SyntaxKind::SemicolonToken,
            any_token.clone(),
            preds![
                is_option_enabled(insert_space_after_semicolon_in_for_statements_option),
                is_non_jsx_same_line_token_context,
                is_for_context
            ],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "NoSpaceAfterSemicolonInFor",
            SyntaxKind::SemicolonToken,
            any_token.clone(),
            preds![
                is_option_disabled_or_undefined(
                    insert_space_after_semicolon_in_for_statements_option
                ),
                is_non_jsx_same_line_token_context,
                is_for_context
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        // Insert space before and after binary operators
        rule(
            "SpaceBeforeBinaryOperator",
            any_token.clone(),
            binary_operators.clone(),
            preds![
                is_option_enabled(insert_space_before_and_after_binary_operators_option),
                is_non_jsx_same_line_token_context,
                is_binary_op_context
            ],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "SpaceAfterBinaryOperator",
            binary_operators.clone(),
            any_token.clone(),
            preds![
                is_option_enabled(insert_space_before_and_after_binary_operators_option),
                is_non_jsx_same_line_token_context,
                is_binary_op_context
            ],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "NoSpaceBeforeBinaryOperator",
            any_token.clone(),
            binary_operators.clone(),
            preds![
                is_option_disabled_or_undefined(
                    insert_space_before_and_after_binary_operators_option
                ),
                is_non_jsx_same_line_token_context,
                is_binary_op_context
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "NoSpaceAfterBinaryOperator",
            binary_operators,
            any_token.clone(),
            preds![
                is_option_disabled_or_undefined(
                    insert_space_before_and_after_binary_operators_option
                ),
                is_non_jsx_same_line_token_context,
                is_binary_op_context
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "SpaceBeforeOpenParenInFuncDecl",
            any_token.clone(),
            SyntaxKind::OpenParenToken,
            preds![
                is_option_enabled(insert_space_before_function_parenthesis_option),
                is_non_jsx_same_line_token_context,
                is_function_decl_context
            ],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "NoSpaceBeforeOpenParenInFuncDecl",
            any_token.clone(),
            SyntaxKind::OpenParenToken,
            preds![
                is_option_disabled_or_undefined(insert_space_before_function_parenthesis_option),
                is_non_jsx_same_line_token_context,
                is_function_decl_context
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        // Open Brace braces after control block
        rule(
            "NewLineBeforeOpenBraceInControl",
            control_open_brace_left_token_range.clone(),
            SyntaxKind::OpenBraceToken,
            preds![
                is_option_enabled(place_open_brace_on_new_line_for_control_blocks_option),
                is_control_decl_context,
                is_before_multiline_block_context
            ],
            RuleAction::INSERT_NEW_LINE,
            &[RuleFlags::CAN_DELETE_NEW_LINES],
        ),
        // Open Brace braces after function
        // TypeScript: Function can have return types, which can be made of tons of different token kinds
        rule(
            "NewLineBeforeOpenBraceInFunction",
            function_open_brace_left_token_range.clone(),
            SyntaxKind::OpenBraceToken,
            preds![
                is_option_enabled(place_open_brace_on_new_line_for_functions_option),
                is_function_decl_context,
                is_before_multiline_block_context
            ],
            RuleAction::INSERT_NEW_LINE,
            &[RuleFlags::CAN_DELETE_NEW_LINES],
        ),
        // Open Brace braces after TypeScript module/class/interface
        rule(
            "NewLineBeforeOpenBraceInTypeScriptDeclWithBlock",
            type_script_open_brace_left_token_range.clone(),
            SyntaxKind::OpenBraceToken,
            preds![
                is_option_enabled(place_open_brace_on_new_line_for_functions_option),
                is_type_script_decl_with_block_context,
                is_before_multiline_block_context
            ],
            RuleAction::INSERT_NEW_LINE,
            &[RuleFlags::CAN_DELETE_NEW_LINES],
        ),
        rule(
            "SpaceAfterTypeAssertion",
            SyntaxKind::GreaterThanToken,
            any_token.clone(),
            preds![
                is_option_enabled(insert_space_after_type_assertion_option),
                is_non_jsx_same_line_token_context,
                is_type_assertion_context
            ],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "NoSpaceAfterTypeAssertion",
            SyntaxKind::GreaterThanToken,
            any_token.clone(),
            preds![
                is_option_disabled_or_undefined(insert_space_after_type_assertion_option),
                is_non_jsx_same_line_token_context,
                is_type_assertion_context
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "SpaceBeforeTypeAnnotation",
            any_token.clone(),
            vec![SyntaxKind::QuestionToken, SyntaxKind::ColonToken],
            preds![
                is_option_enabled(insert_space_before_type_annotation_option),
                is_non_jsx_same_line_token_context,
                is_type_annotation_context
            ],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "NoSpaceBeforeTypeAnnotation",
            any_token.clone(),
            vec![SyntaxKind::QuestionToken, SyntaxKind::ColonToken],
            preds![
                is_option_disabled_or_undefined(insert_space_before_type_annotation_option),
                is_non_jsx_same_line_token_context,
                is_type_annotation_context
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "NoOptionalSemicolon",
            SyntaxKind::SemicolonToken,
            any_token_including_eof.clone(),
            preds![
                option_equals(semicolon_option, lsutil::SemicolonPreference::REMOVE),
                is_semicolon_deletion_context
            ],
            RuleAction::DELETE_TOKEN,
            &[],
        ),
        rule(
            "OptionalSemicolon",
            any_token.clone(),
            any_token_including_eof,
            preds![
                option_equals(semicolon_option, lsutil::SemicolonPreference::INSERT),
                is_semicolon_insertion_context
            ],
            RuleAction::INSERT_TRAILING_SEMICOLON,
            &[],
        ),
    ];

    // These rules are lower in priority than user-configurable. Rules earlier in this list have priority over rules later in the list.
    let low_priority_common_rules: Vec<RuleSpec> = vec![
        // Space after keyword but not before ; or : or ?
        rule(
            "NoSpaceBeforeSemicolon",
            any_token.clone(),
            SyntaxKind::SemicolonToken,
            preds![is_non_jsx_same_line_token_context],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "SpaceBeforeOpenBraceInControl",
            control_open_brace_left_token_range,
            SyntaxKind::OpenBraceToken,
            preds![
                is_option_disabled_or_undefined_or_tokens_on_same_line(
                    place_open_brace_on_new_line_for_control_blocks_option
                ),
                is_control_decl_context,
                is_not_format_on_enter,
                is_same_line_token_or_before_block_context
            ],
            RuleAction::INSERT_SPACE,
            &[RuleFlags::CAN_DELETE_NEW_LINES],
        ),
        rule(
            "SpaceBeforeOpenBraceInFunction",
            function_open_brace_left_token_range,
            SyntaxKind::OpenBraceToken,
            preds![
                is_option_disabled_or_undefined_or_tokens_on_same_line(
                    place_open_brace_on_new_line_for_functions_option
                ),
                is_function_decl_context,
                is_before_block_context,
                is_not_format_on_enter,
                is_same_line_token_or_before_block_context
            ],
            RuleAction::INSERT_SPACE,
            &[RuleFlags::CAN_DELETE_NEW_LINES],
        ),
        rule(
            "SpaceBeforeOpenBraceInTypeScriptDeclWithBlock",
            type_script_open_brace_left_token_range,
            SyntaxKind::OpenBraceToken,
            preds![
                is_option_disabled_or_undefined_or_tokens_on_same_line(
                    place_open_brace_on_new_line_for_functions_option
                ),
                is_type_script_decl_with_block_context,
                is_not_format_on_enter,
                is_same_line_token_or_before_block_context
            ],
            RuleAction::INSERT_SPACE,
            &[RuleFlags::CAN_DELETE_NEW_LINES],
        ),
        rule(
            "NoSpaceBeforeComma",
            any_token.clone(),
            SyntaxKind::CommaToken,
            preds![is_non_jsx_same_line_token_context],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        // No space before and after indexer `x[]ast.Kind{}`
        rule(
            "NoSpaceBeforeOpenBracket",
            any_token_except(&[SyntaxKind::AsyncKeyword, SyntaxKind::CaseKeyword]),
            SyntaxKind::OpenBracketToken,
            preds![is_non_jsx_same_line_token_context],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "NoSpaceAfterCloseBracket",
            SyntaxKind::CloseBracketToken,
            any_token.clone(),
            preds![
                is_non_jsx_same_line_token_context,
                is_not_before_block_in_function_declaration_context
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "SpaceAfterSemicolon",
            SyntaxKind::SemicolonToken,
            any_token.clone(),
            preds![is_non_jsx_same_line_token_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        // Remove extra space between for and await
        rule(
            "SpaceBetweenForAndAwaitKeyword",
            SyntaxKind::ForKeyword,
            SyntaxKind::AwaitKeyword,
            preds![is_non_jsx_same_line_token_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        // Remove extra spaces between ... and type name in tuple spread
        rule(
            "SpaceBetweenDotDotDotAndTypeName",
            SyntaxKind::DotDotDotToken,
            type_names.clone(),
            preds![is_non_jsx_same_line_token_context],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        // Add a space between statements. All keywords except (do,else,case) has open/close parens after them.
        // So, we have a rule to add a space for []ast.Kind{),Any}, []ast.Kind{do,Any}, []ast.Kind{else,Any}, and []ast.Kind{case,Any}
        rule(
            "SpaceBetweenStatements",
            vec![
                SyntaxKind::CloseParenToken,
                SyntaxKind::DoKeyword,
                SyntaxKind::ElseKeyword,
                SyntaxKind::CaseKeyword,
            ],
            any_token,
            preds![
                is_non_jsx_same_line_token_context,
                is_non_jsx_element_or_fragment_context,
                is_not_for_context
            ],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        // This low-pri rule takes care of "try {", "catch {" and "finally {" in case the rule SpaceBeforeOpenBraceInControl didn't execute on FormatOnEnter.
        rule(
            "SpaceAfterTryCatchFinally",
            vec![
                SyntaxKind::TryKeyword,
                SyntaxKind::CatchKeyword,
                SyntaxKind::FinallyKeyword,
            ],
            SyntaxKind::OpenBraceToken,
            preds![is_non_jsx_same_line_token_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
    ];

    let mut result: Vec<RuleSpec> = Vec::with_capacity(
        high_priority_common_rules.len()
            + user_configurable_rules.len()
            + low_priority_common_rules.len(),
    );
    result.extend(high_priority_common_rules);
    result.extend(user_configurable_rules);
    result.extend(low_priority_common_rules);
    result
}

// Go: format/rules.go:428 tokenRangeFrom
pub fn token_range_from(tokens: &[SyntaxKind]) -> TokenRange {
    TokenRange {
        is_specific: true,
        tokens: tokens.to_vec(),
    }
}

// Go: format/rules.go:435 tokenRangeFromEx
// PORT: Go appends to `prefix` in place when it has spare capacity; see the
// note in get_all_rules for the one call site where that matters.
pub fn token_range_from_ex(prefix: &[SyntaxKind], tokens: &[SyntaxKind]) -> TokenRange {
    let mut all = prefix.to_vec();
    all.extend_from_slice(tokens);
    TokenRange {
        is_specific: true,
        tokens: all,
    }
}

// Go: format/rules.go:443 tokenRangeFromRange
pub fn token_range_from_range(start: SyntaxKind, end: SyntaxKind) -> TokenRange {
    let mut tokens: Vec<SyntaxKind> = Vec::with_capacity((end as usize) - (start as usize) + 1);
    for token in (start as u16)..=(end as u16) {
        tokens.push(SyntaxKind::try_from(token).expect("token kinds are contiguous"));
    }

    token_range_from(&tokens)
}

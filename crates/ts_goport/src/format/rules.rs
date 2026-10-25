use crate::format::prelude::*;

use RuleTokens::{
    AnyToken, AnyTokenExcept, AnyTokenIncludingEof, AnyTokenIncludingMultilineComments,
    BinaryOperators, Keywords, Kind, Kinds,
};

// Builds a Go `[]contextPredicate{...}` literal as a static slice. A bare
// predicate function becomes `ContextPredicate::Fn`; a call such as
// `is_option_enabled(x)` or `option_equals(x, v)` already makes a
// `ContextPredicate`.
macro_rules! preds {
    (@one $f:ident ( $($args:tt)* )) => { $f($($args)*) };
    (@one $f:ident) => { ContextPredicate::Fn($f) };
    ($($f:ident $(( $($args:tt)* ))?),* $(,)?) => {
        &[$(preds!(@one $f $(( $($args)* ))?)),*]
    };
}

// Go: format/rules.go:10 getAllRules
pub fn get_all_rules() -> Vec<RuleSpec> {
    // PORT: Go builds every rule here at run time. The rule lists and the
    // fixed token lists are static data in Go's order; the token ranges that
    // Go computes from allTokens are made below, and the result is the same
    // []ruleSpec.
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

    // Go: format/rule.go:71 toTokenRange (`case ast.Kind`, `case []ast.Kind`
    // and `case tokenRange`) for the left and right side of each rule.
    let token_range = |tokens: RuleTokens| -> TokenRange {
        match tokens {
            Kind(kind) => token_range_from(&[kind]),
            Kinds(kinds) => token_range_from(kinds),
            AnyToken => any_token.clone(),
            AnyTokenExcept(except) => any_token_except(except),
            AnyTokenIncludingMultilineComments => any_token_including_multiline_comments.clone(),
            AnyTokenIncludingEof => any_token_including_eof.clone(),
            Keywords => keywords.clone(),
            BinaryOperators => binary_operators.clone(),
        }
    };

    // Go: format/rules.go getAllRules binaryKeywordOperators
    const BINARY_KEYWORD_OPERATORS: &[SyntaxKind] = &[
        SyntaxKind::InKeyword,
        SyntaxKind::InstanceOfKeyword,
        SyntaxKind::OfKeyword,
        SyntaxKind::AsKeyword,
        SyntaxKind::IsKeyword,
        SyntaxKind::SatisfiesKeyword,
    ];
    // Go: format/rules.go getAllRules unaryPrefixOperators
    const UNARY_PREFIX_OPERATORS: &[SyntaxKind] = &[
        SyntaxKind::PlusPlusToken,
        SyntaxKind::MinusToken,
        SyntaxKind::TildeToken,
        SyntaxKind::ExclamationToken,
    ];
    // Go: format/rules.go getAllRules unaryPrefixExpressions
    const UNARY_PREFIX_EXPRESSIONS: &[SyntaxKind] = &[
        SyntaxKind::NumericLiteral,
        SyntaxKind::BigIntLiteral,
        SyntaxKind::Identifier,
        SyntaxKind::OpenParenToken,
        SyntaxKind::OpenBracketToken,
        SyntaxKind::OpenBraceToken,
        SyntaxKind::ThisKeyword,
        SyntaxKind::NewKeyword,
    ];
    // Go: format/rules.go getAllRules unaryPreincrementExpressions
    const UNARY_PREINCREMENT_EXPRESSIONS: &[SyntaxKind] = &[
        SyntaxKind::Identifier,
        SyntaxKind::OpenParenToken,
        SyntaxKind::ThisKeyword,
        SyntaxKind::NewKeyword,
    ];
    // Go: format/rules.go getAllRules unaryPostincrementExpressions
    const UNARY_POSTINCREMENT_EXPRESSIONS: &[SyntaxKind] = &[
        SyntaxKind::Identifier,
        SyntaxKind::CloseParenToken,
        SyntaxKind::CloseBracketToken,
        SyntaxKind::NewKeyword,
    ];
    // Go: format/rules.go getAllRules unaryPredecrementExpressions
    const UNARY_PREDECREMENT_EXPRESSIONS: &[SyntaxKind] = &[
        SyntaxKind::Identifier,
        SyntaxKind::OpenParenToken,
        SyntaxKind::ThisKeyword,
        SyntaxKind::NewKeyword,
    ];
    // Go: format/rules.go getAllRules unaryPostdecrementExpressions
    const UNARY_POSTDECREMENT_EXPRESSIONS: &[SyntaxKind] = &[
        SyntaxKind::Identifier,
        SyntaxKind::CloseParenToken,
        SyntaxKind::CloseBracketToken,
        SyntaxKind::NewKeyword,
    ];
    // Go: format/rules.go getAllRules comments
    const COMMENTS: &[SyntaxKind] = &[
        SyntaxKind::SingleLineCommentTrivia,
        SyntaxKind::MultiLineCommentTrivia,
    ];
    // Go: format/rules.go getAllRules typeKeywords
    const TYPE_KEYWORDS: &[SyntaxKind] = &[
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
    // Go: format/rules.go getAllRules typeNames := append([]ast.Kind{ast.KindIdentifier}, typeKeywords...)
    const TYPE_NAMES: &[SyntaxKind] = &{
        let mut names = [SyntaxKind::Identifier; TYPE_KEYWORDS.len() + 1];
        let mut i = 0;
        while i < TYPE_KEYWORDS.len() {
            names[i + 1] = TYPE_KEYWORDS[i];
            i += 1;
        }
        names
    };

    // Place a space before open brace in a function declaration
    // TypeScript: Function can have return types, which can be made of tons of different token kinds
    // Go: format/rules.go getAllRules functionOpenBraceLeftTokenRange
    const FUNCTION_OPEN_BRACE_LEFT_TOKEN_RANGE: RuleTokens = AnyTokenIncludingMultilineComments;

    // Place a space before open brace in a TypeScript declaration that has braces as children (class, module, enum, etc)
    // Go: format/rules.go getAllRules typeScriptOpenBraceLeftTokenRange
    const TYPE_SCRIPT_OPEN_BRACE_LEFT_TOKEN_RANGE: RuleTokens = Kinds(&[
        SyntaxKind::Identifier,
        SyntaxKind::GreaterThanToken,
        SyntaxKind::MultiLineCommentTrivia,
        SyntaxKind::ClassKeyword,
        SyntaxKind::ExportKeyword,
        SyntaxKind::ImportKeyword,
    ]);

    // Place a space before open brace in a control flow construct
    // Go: format/rules.go getAllRules controlOpenBraceLeftTokenRange
    const CONTROL_OPEN_BRACE_LEFT_TOKEN_RANGE: RuleTokens = Kinds(&[
        SyntaxKind::CloseParenToken,
        SyntaxKind::MultiLineCommentTrivia,
        SyntaxKind::DoKeyword,
        SyntaxKind::TryKeyword,
        SyntaxKind::FinallyKeyword,
        SyntaxKind::ElseKeyword,
        SyntaxKind::CatchKeyword,
    ]);

    // These rules are higher in priority than user-configurable
    // Go: format/rules.go getAllRules highPriorityCommonRules
    static HIGH_PRIORITY_COMMON_RULES: &[RuleDef] = &[
        // Leave comments alone
        rule(
            "IgnoreBeforeComment",
            AnyToken,
            Kinds(COMMENTS),
            ANY_CONTEXT,
            RuleAction::STOP_PROCESSING_SPACE_ACTIONS,
            &[],
        ),
        rule(
            "IgnoreAfterLineComment",
            Kind(SyntaxKind::SingleLineCommentTrivia),
            AnyToken,
            ANY_CONTEXT,
            RuleAction::STOP_PROCESSING_SPACE_ACTIONS,
            &[],
        ),
        rule(
            "NotSpaceBeforeColon",
            AnyToken,
            Kind(SyntaxKind::ColonToken),
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
            Kind(SyntaxKind::ColonToken),
            AnyToken,
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
            AnyToken,
            Kind(SyntaxKind::QuestionToken),
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
            Kind(SyntaxKind::QuestionToken),
            AnyToken,
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
            Kind(SyntaxKind::QuestionToken),
            AnyToken,
            preds![
                is_non_jsx_same_line_token_context,
                is_non_optional_property_context
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "NoSpaceBeforeDot",
            AnyToken,
            Kinds(&[SyntaxKind::DotToken, SyntaxKind::QuestionDotToken]),
            preds![
                is_non_jsx_same_line_token_context,
                is_not_property_access_on_integer_literal
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "NoSpaceAfterDot",
            Kinds(&[SyntaxKind::DotToken, SyntaxKind::QuestionDotToken]),
            AnyToken,
            preds![is_non_jsx_same_line_token_context],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "NoSpaceBetweenImportParenInImportType",
            Kind(SyntaxKind::ImportKeyword),
            Kind(SyntaxKind::OpenParenToken),
            preds![is_non_jsx_same_line_token_context, is_import_type_context],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        // Special handling of unary operators.
        // Prefix operators generally shouldn't have a space between
        // them and their target unary expression.
        rule(
            "NoSpaceAfterUnaryPrefixOperator",
            Kinds(UNARY_PREFIX_OPERATORS),
            Kinds(UNARY_PREFIX_EXPRESSIONS),
            preds![is_non_jsx_same_line_token_context, is_not_binary_op_context],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "NoSpaceAfterUnaryPreincrementOperator",
            Kind(SyntaxKind::PlusPlusToken),
            Kinds(UNARY_PREINCREMENT_EXPRESSIONS),
            preds![is_non_jsx_same_line_token_context],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "NoSpaceAfterUnaryPredecrementOperator",
            Kind(SyntaxKind::MinusMinusToken),
            Kinds(UNARY_PREDECREMENT_EXPRESSIONS),
            preds![is_non_jsx_same_line_token_context],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "NoSpaceBeforeUnaryPostincrementOperator",
            Kinds(UNARY_POSTINCREMENT_EXPRESSIONS),
            Kind(SyntaxKind::PlusPlusToken),
            preds![
                is_non_jsx_same_line_token_context,
                is_not_statement_condition_context
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "NoSpaceBeforeUnaryPostdecrementOperator",
            Kinds(UNARY_POSTDECREMENT_EXPRESSIONS),
            Kind(SyntaxKind::MinusMinusToken),
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
            Kind(SyntaxKind::PlusPlusToken),
            Kind(SyntaxKind::PlusToken),
            preds![is_non_jsx_same_line_token_context, is_binary_op_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "SpaceAfterAddWhenFollowedByUnaryPlus",
            Kind(SyntaxKind::PlusToken),
            Kind(SyntaxKind::PlusToken),
            preds![is_non_jsx_same_line_token_context, is_binary_op_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "SpaceAfterAddWhenFollowedByPreincrement",
            Kind(SyntaxKind::PlusToken),
            Kind(SyntaxKind::PlusPlusToken),
            preds![is_non_jsx_same_line_token_context, is_binary_op_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "SpaceAfterPostdecrementWhenFollowedBySubtract",
            Kind(SyntaxKind::MinusMinusToken),
            Kind(SyntaxKind::MinusToken),
            preds![is_non_jsx_same_line_token_context, is_binary_op_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "SpaceAfterSubtractWhenFollowedByUnaryMinus",
            Kind(SyntaxKind::MinusToken),
            Kind(SyntaxKind::MinusToken),
            preds![is_non_jsx_same_line_token_context, is_binary_op_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "SpaceAfterSubtractWhenFollowedByPredecrement",
            Kind(SyntaxKind::MinusToken),
            Kind(SyntaxKind::MinusMinusToken),
            preds![is_non_jsx_same_line_token_context, is_binary_op_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "NoSpaceAfterCloseBrace",
            Kind(SyntaxKind::CloseBraceToken),
            Kinds(&[SyntaxKind::CommaToken, SyntaxKind::SemicolonToken]),
            preds![is_non_jsx_same_line_token_context],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        // For functions and control block place } on a new line []ast.Kind{multi-line rule}
        rule(
            "NewLineBeforeCloseBraceInBlockContext",
            AnyTokenIncludingMultilineComments,
            Kind(SyntaxKind::CloseBraceToken),
            preds![is_multiline_block_context],
            RuleAction::INSERT_NEW_LINE,
            &[],
        ),
        // Space/new line after }.
        rule(
            "SpaceAfterCloseBrace",
            Kind(SyntaxKind::CloseBraceToken),
            AnyTokenExcept(&[SyntaxKind::CloseParenToken]),
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
            Kind(SyntaxKind::CloseBraceToken),
            Kind(SyntaxKind::ElseKeyword),
            preds![is_non_jsx_same_line_token_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "SpaceBetweenCloseBraceAndWhile",
            Kind(SyntaxKind::CloseBraceToken),
            Kind(SyntaxKind::WhileKeyword),
            preds![is_non_jsx_same_line_token_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "NoSpaceBetweenEmptyBraceBrackets",
            Kind(SyntaxKind::OpenBraceToken),
            Kind(SyntaxKind::CloseBraceToken),
            preds![is_non_jsx_same_line_token_context, is_object_context],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        // Add a space after control dec context if the next character is an open bracket ex: 'if (false)[]ast.Kind{a, b} = []ast.Kind{1, 2};' -> 'if (false) []ast.Kind{a, b} = []ast.Kind{1, 2};'
        rule(
            "SpaceAfterConditionalClosingParen",
            Kind(SyntaxKind::CloseParenToken),
            Kind(SyntaxKind::OpenBracketToken),
            preds![is_control_decl_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "NoSpaceBetweenFunctionKeywordAndStar",
            Kind(SyntaxKind::FunctionKeyword),
            Kind(SyntaxKind::AsteriskToken),
            preds![is_function_declaration_or_function_expression_context],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "SpaceAfterStarInGeneratorDeclaration",
            Kind(SyntaxKind::AsteriskToken),
            Kind(SyntaxKind::Identifier),
            preds![is_function_declaration_or_function_expression_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "SpaceAfterFunctionInFuncDecl",
            Kind(SyntaxKind::FunctionKeyword),
            AnyToken,
            preds![is_function_decl_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        // Insert new line after { and before } in multi-line contexts.
        rule(
            "NewLineAfterOpenBraceInBlockContext",
            Kind(SyntaxKind::OpenBraceToken),
            AnyToken,
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
            Kinds(&[SyntaxKind::GetKeyword, SyntaxKind::SetKeyword]),
            Kind(SyntaxKind::Identifier),
            preds![is_function_decl_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "NoSpaceBetweenYieldKeywordAndStar",
            Kind(SyntaxKind::YieldKeyword),
            Kind(SyntaxKind::AsteriskToken),
            preds![
                is_non_jsx_same_line_token_context,
                is_yield_or_yield_star_with_operand
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "SpaceBetweenYieldOrYieldStarAndOperand",
            Kinds(&[SyntaxKind::YieldKeyword, SyntaxKind::AsteriskToken]),
            AnyToken,
            preds![
                is_non_jsx_same_line_token_context,
                is_yield_or_yield_star_with_operand
            ],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "NoSpaceBetweenReturnAndSemicolon",
            Kind(SyntaxKind::ReturnKeyword),
            Kind(SyntaxKind::SemicolonToken),
            preds![is_non_jsx_same_line_token_context],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "SpaceAfterCertainKeywords",
            Kinds(&[
                SyntaxKind::VarKeyword,
                SyntaxKind::ThrowKeyword,
                SyntaxKind::NewKeyword,
                SyntaxKind::DeleteKeyword,
                SyntaxKind::ReturnKeyword,
                SyntaxKind::TypeOfKeyword,
                SyntaxKind::AwaitKeyword,
            ]),
            AnyToken,
            preds![is_non_jsx_same_line_token_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "SpaceAfterLetConstInVariableDeclaration",
            Kinds(&[SyntaxKind::LetKeyword, SyntaxKind::ConstKeyword]),
            AnyToken,
            preds![
                is_non_jsx_same_line_token_context,
                is_start_of_variable_declaration_list
            ],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "NoSpaceBeforeOpenParenInFuncCall",
            AnyToken,
            Kind(SyntaxKind::OpenParenToken),
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
            AnyToken,
            Kinds(BINARY_KEYWORD_OPERATORS),
            preds![is_non_jsx_same_line_token_context, is_binary_op_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "SpaceAfterBinaryKeywordOperator",
            Kinds(BINARY_KEYWORD_OPERATORS),
            AnyToken,
            preds![is_non_jsx_same_line_token_context, is_binary_op_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "SpaceAfterVoidOperator",
            Kind(SyntaxKind::VoidKeyword),
            AnyToken,
            preds![is_non_jsx_same_line_token_context, is_void_op_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        // Async-await
        rule(
            "SpaceBetweenAsyncAndOpenParen",
            Kind(SyntaxKind::AsyncKeyword),
            Kind(SyntaxKind::OpenParenToken),
            preds![
                is_arrow_function_context,
                is_non_jsx_same_line_token_context
            ],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "SpaceBetweenAsyncAndFunctionKeyword",
            Kind(SyntaxKind::AsyncKeyword),
            Kinds(&[SyntaxKind::FunctionKeyword, SyntaxKind::Identifier]),
            preds![is_non_jsx_same_line_token_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        // Template string
        rule(
            "NoSpaceBetweenTagAndTemplateString",
            Kinds(&[SyntaxKind::Identifier, SyntaxKind::CloseParenToken]),
            Kinds(&[
                SyntaxKind::NoSubstitutionTemplateLiteral,
                SyntaxKind::TemplateHead,
            ]),
            preds![is_non_jsx_same_line_token_context],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        // JSX opening elements
        rule(
            "SpaceBeforeJsxAttribute",
            AnyToken,
            Kind(SyntaxKind::Identifier),
            preds![
                is_next_token_parent_jsx_attribute,
                is_non_jsx_same_line_token_context
            ],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "SpaceBeforeSlashInJsxOpeningElement",
            AnyToken,
            Kind(SyntaxKind::SlashToken),
            preds![
                is_jsx_self_closing_element_context,
                is_non_jsx_same_line_token_context
            ],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "NoSpaceBeforeGreaterThanTokenInJsxOpeningElement",
            Kind(SyntaxKind::SlashToken),
            Kind(SyntaxKind::GreaterThanToken),
            preds![
                is_jsx_self_closing_element_context,
                is_non_jsx_same_line_token_context
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "NoSpaceBeforeEqualInJsxAttribute",
            AnyToken,
            Kind(SyntaxKind::EqualsToken),
            preds![is_jsx_attribute_context, is_non_jsx_same_line_token_context],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "NoSpaceAfterEqualInJsxAttribute",
            Kind(SyntaxKind::EqualsToken),
            AnyToken,
            preds![is_jsx_attribute_context, is_non_jsx_same_line_token_context],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "NoSpaceBeforeJsxNamespaceColon",
            Kind(SyntaxKind::Identifier),
            Kind(SyntaxKind::ColonToken),
            preds![is_next_token_parent_jsx_namespaced_name],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "NoSpaceAfterJsxNamespaceColon",
            Kind(SyntaxKind::ColonToken),
            Kind(SyntaxKind::Identifier),
            preds![is_next_token_parent_jsx_namespaced_name],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        // TypeScript-specific rules
        // Use of module as a function call. e.g.: import m2 = module("m2");
        rule(
            "NoSpaceAfterModuleImport",
            Kinds(&[SyntaxKind::ModuleKeyword, SyntaxKind::RequireKeyword]),
            Kind(SyntaxKind::OpenParenToken),
            preds![is_non_jsx_same_line_token_context],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        // Add a space around certain TypeScript keywords
        rule(
            "SpaceAfterCertainTypeScriptKeywords",
            Kinds(&[
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
            ]),
            AnyToken,
            preds![is_non_jsx_same_line_token_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "SpaceBeforeCertainTypeScriptKeywords",
            AnyToken,
            Kinds(&[
                SyntaxKind::ExtendsKeyword,
                SyntaxKind::ImplementsKeyword,
                SyntaxKind::FromKeyword,
            ]),
            preds![is_non_jsx_same_line_token_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        // Treat string literals in module names as identifiers, and add a space between the literal and the opening Brace braces, e.g.: module "m2" {
        rule(
            "SpaceAfterModuleName",
            Kind(SyntaxKind::StringLiteral),
            Kind(SyntaxKind::OpenBraceToken),
            preds![is_module_decl_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        // Lambda expressions
        rule(
            "SpaceBeforeArrow",
            AnyToken,
            Kind(SyntaxKind::EqualsGreaterThanToken),
            preds![is_non_jsx_same_line_token_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "SpaceAfterArrow",
            Kind(SyntaxKind::EqualsGreaterThanToken),
            AnyToken,
            preds![is_non_jsx_same_line_token_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        // Optional parameters and let args
        rule(
            "NoSpaceAfterEllipsis",
            Kind(SyntaxKind::DotDotDotToken),
            Kind(SyntaxKind::Identifier),
            preds![is_non_jsx_same_line_token_context],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "NoSpaceAfterOptionalParameters",
            Kind(SyntaxKind::QuestionToken),
            Kinds(&[SyntaxKind::CloseParenToken, SyntaxKind::CommaToken]),
            preds![is_non_jsx_same_line_token_context, is_not_binary_op_context],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        // Remove spaces in empty interface literals. e.g.: x: {}
        rule(
            "NoSpaceBetweenEmptyInterfaceBraceBrackets",
            Kind(SyntaxKind::OpenBraceToken),
            Kind(SyntaxKind::CloseBraceToken),
            preds![is_non_jsx_same_line_token_context, is_object_type_context],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        // generics and type assertions
        rule(
            "NoSpaceBeforeOpenAngularBracket",
            Kinds(TYPE_NAMES),
            Kind(SyntaxKind::LessThanToken),
            preds![
                is_non_jsx_same_line_token_context,
                is_type_argument_or_parameter_or_assertion_context
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "NoSpaceBetweenCloseParenAndAngularBracket",
            Kind(SyntaxKind::CloseParenToken),
            Kind(SyntaxKind::LessThanToken),
            preds![
                is_non_jsx_same_line_token_context,
                is_type_argument_or_parameter_or_assertion_context
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "NoSpaceAfterOpenAngularBracket",
            Kind(SyntaxKind::LessThanToken),
            AnyToken,
            preds![
                is_non_jsx_same_line_token_context,
                is_type_argument_or_parameter_or_assertion_context
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "NoSpaceBeforeCloseAngularBracket",
            AnyToken,
            Kind(SyntaxKind::GreaterThanToken),
            preds![
                is_non_jsx_same_line_token_context,
                is_type_argument_or_parameter_or_assertion_context
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "NoSpaceAfterCloseAngularBracket",
            Kind(SyntaxKind::GreaterThanToken),
            Kinds(&[
                SyntaxKind::OpenParenToken,
                SyntaxKind::OpenBracketToken,
                SyntaxKind::GreaterThanToken,
                SyntaxKind::CommaToken,
            ]),
            preds![
                is_non_jsx_same_line_token_context,
                is_type_argument_or_parameter_or_assertion_context,
                is_not_function_decl_context,
                /*To prevent an interference with the SpaceBeforeOpenParenInFuncDecl rule*/
                is_non_type_assertion_context,
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        // decorators
        rule(
            "SpaceBeforeAt",
            Kinds(&[SyntaxKind::CloseParenToken, SyntaxKind::Identifier]),
            Kind(SyntaxKind::AtToken),
            preds![is_non_jsx_same_line_token_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "NoSpaceAfterAt",
            Kind(SyntaxKind::AtToken),
            AnyToken,
            preds![is_non_jsx_same_line_token_context],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        // Insert space after @ in decorator
        rule(
            "SpaceAfterDecorator",
            AnyToken,
            Kinds(&[
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
            ]),
            preds![is_end_of_decorator_context_on_same_line],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "NoSpaceBeforeNonNullAssertionOperator",
            AnyToken,
            Kind(SyntaxKind::ExclamationToken),
            preds![
                is_non_jsx_same_line_token_context,
                is_non_null_assertion_context
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "NoSpaceAfterNewKeywordOnConstructorSignature",
            Kind(SyntaxKind::NewKeyword),
            Kind(SyntaxKind::OpenParenToken),
            preds![
                is_non_jsx_same_line_token_context,
                is_constructor_signature_context
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "SpaceLessThanAndNonJSXTypeAnnotation",
            Kind(SyntaxKind::LessThanToken),
            Kind(SyntaxKind::LessThanToken),
            preds![is_non_jsx_same_line_token_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
    ];

    // These rules are applied after high priority
    // Go: format/rules.go getAllRules userConfigurableRules
    static USER_CONFIGURABLE_RULES: &[RuleDef] = &[
        // Treat constructor as an identifier in a function declaration, and remove spaces between constructor and following left parentheses
        rule(
            "SpaceAfterConstructor",
            Kind(SyntaxKind::ConstructorKeyword),
            Kind(SyntaxKind::OpenParenToken),
            preds![
                is_option_enabled(insert_space_after_constructor_option),
                is_non_jsx_same_line_token_context
            ],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "NoSpaceAfterConstructor",
            Kind(SyntaxKind::ConstructorKeyword),
            Kind(SyntaxKind::OpenParenToken),
            preds![
                is_option_disabled_or_undefined(insert_space_after_constructor_option),
                is_non_jsx_same_line_token_context
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "SpaceAfterComma",
            Kind(SyntaxKind::CommaToken),
            AnyToken,
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
            Kind(SyntaxKind::CommaToken),
            AnyToken,
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
            Kinds(&[SyntaxKind::FunctionKeyword, SyntaxKind::AsteriskToken]),
            Kind(SyntaxKind::OpenParenToken),
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
            Kinds(&[SyntaxKind::FunctionKeyword, SyntaxKind::AsteriskToken]),
            Kind(SyntaxKind::OpenParenToken),
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
            Keywords,
            Kind(SyntaxKind::OpenParenToken),
            preds![
                is_option_enabled(insert_space_after_keywords_in_control_flow_statements_option),
                is_control_decl_context
            ],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "NoSpaceAfterKeywordInControl",
            Keywords,
            Kind(SyntaxKind::OpenParenToken),
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
            Kind(SyntaxKind::OpenParenToken),
            AnyToken,
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
            AnyToken,
            Kind(SyntaxKind::CloseParenToken),
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
            Kind(SyntaxKind::OpenParenToken),
            Kind(SyntaxKind::OpenParenToken),
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
            Kind(SyntaxKind::OpenParenToken),
            Kind(SyntaxKind::CloseParenToken),
            preds![is_non_jsx_same_line_token_context],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "NoSpaceAfterOpenParen",
            Kind(SyntaxKind::OpenParenToken),
            AnyToken,
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
            AnyToken,
            Kind(SyntaxKind::CloseParenToken),
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
            Kind(SyntaxKind::OpenBracketToken),
            AnyToken,
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
            AnyToken,
            Kind(SyntaxKind::CloseBracketToken),
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
            Kind(SyntaxKind::OpenBracketToken),
            Kind(SyntaxKind::CloseBracketToken),
            preds![is_non_jsx_same_line_token_context],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "NoSpaceAfterOpenBracket",
            Kind(SyntaxKind::OpenBracketToken),
            AnyToken,
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
            AnyToken,
            Kind(SyntaxKind::CloseBracketToken),
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
            Kind(SyntaxKind::OpenBraceToken),
            AnyToken,
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
            AnyToken,
            Kind(SyntaxKind::CloseBraceToken),
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
            Kind(SyntaxKind::OpenBraceToken),
            Kind(SyntaxKind::CloseBraceToken),
            preds![is_non_jsx_same_line_token_context, is_object_context],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "NoSpaceAfterOpenBrace",
            Kind(SyntaxKind::OpenBraceToken),
            AnyToken,
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
            AnyToken,
            Kind(SyntaxKind::CloseBraceToken),
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
            Kind(SyntaxKind::OpenBraceToken),
            Kind(SyntaxKind::CloseBraceToken),
            preds![is_option_enabled(
                insert_space_after_opening_and_before_closing_empty_braces_option
            )],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        rule(
            "NoSpaceBetweenEmptyBraceBrackets",
            Kind(SyntaxKind::OpenBraceToken),
            Kind(SyntaxKind::CloseBraceToken),
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
            Kinds(&[SyntaxKind::TemplateHead, SyntaxKind::TemplateMiddle]),
            AnyToken,
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
            AnyToken,
            Kinds(&[SyntaxKind::TemplateMiddle, SyntaxKind::TemplateTail]),
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
            Kinds(&[SyntaxKind::TemplateHead, SyntaxKind::TemplateMiddle]),
            AnyToken,
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
            AnyToken,
            Kinds(&[SyntaxKind::TemplateMiddle, SyntaxKind::TemplateTail]),
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
            Kind(SyntaxKind::OpenBraceToken),
            AnyToken,
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
            AnyToken,
            Kind(SyntaxKind::CloseBraceToken),
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
            Kind(SyntaxKind::OpenBraceToken),
            AnyToken,
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
            AnyToken,
            Kind(SyntaxKind::CloseBraceToken),
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
            Kind(SyntaxKind::SemicolonToken),
            AnyToken,
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
            Kind(SyntaxKind::SemicolonToken),
            AnyToken,
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
            AnyToken,
            BinaryOperators,
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
            BinaryOperators,
            AnyToken,
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
            AnyToken,
            BinaryOperators,
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
            BinaryOperators,
            AnyToken,
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
            AnyToken,
            Kind(SyntaxKind::OpenParenToken),
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
            AnyToken,
            Kind(SyntaxKind::OpenParenToken),
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
            CONTROL_OPEN_BRACE_LEFT_TOKEN_RANGE,
            Kind(SyntaxKind::OpenBraceToken),
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
            FUNCTION_OPEN_BRACE_LEFT_TOKEN_RANGE,
            Kind(SyntaxKind::OpenBraceToken),
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
            TYPE_SCRIPT_OPEN_BRACE_LEFT_TOKEN_RANGE,
            Kind(SyntaxKind::OpenBraceToken),
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
            Kind(SyntaxKind::GreaterThanToken),
            AnyToken,
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
            Kind(SyntaxKind::GreaterThanToken),
            AnyToken,
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
            AnyToken,
            Kinds(&[SyntaxKind::QuestionToken, SyntaxKind::ColonToken]),
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
            AnyToken,
            Kinds(&[SyntaxKind::QuestionToken, SyntaxKind::ColonToken]),
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
            Kind(SyntaxKind::SemicolonToken),
            AnyTokenIncludingEof,
            preds![
                option_equals(semicolon_option, lsutil::SemicolonPreference::REMOVE),
                is_semicolon_deletion_context
            ],
            RuleAction::DELETE_TOKEN,
            &[],
        ),
        rule(
            "OptionalSemicolon",
            AnyToken,
            AnyTokenIncludingEof,
            preds![
                option_equals(semicolon_option, lsutil::SemicolonPreference::INSERT),
                is_semicolon_insertion_context
            ],
            RuleAction::INSERT_TRAILING_SEMICOLON,
            &[],
        ),
    ];

    // These rules are lower in priority than user-configurable. Rules earlier in this list have priority over rules later in the list.
    // Go: format/rules.go getAllRules lowPriorityCommonRules
    static LOW_PRIORITY_COMMON_RULES: &[RuleDef] = &[
        // Space after keyword but not before ; or : or ?
        rule(
            "NoSpaceBeforeSemicolon",
            AnyToken,
            Kind(SyntaxKind::SemicolonToken),
            preds![is_non_jsx_same_line_token_context],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "SpaceBeforeOpenBraceInControl",
            CONTROL_OPEN_BRACE_LEFT_TOKEN_RANGE,
            Kind(SyntaxKind::OpenBraceToken),
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
            FUNCTION_OPEN_BRACE_LEFT_TOKEN_RANGE,
            Kind(SyntaxKind::OpenBraceToken),
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
            TYPE_SCRIPT_OPEN_BRACE_LEFT_TOKEN_RANGE,
            Kind(SyntaxKind::OpenBraceToken),
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
            AnyToken,
            Kind(SyntaxKind::CommaToken),
            preds![is_non_jsx_same_line_token_context],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        // No space before and after indexer `x[]ast.Kind{}`
        rule(
            "NoSpaceBeforeOpenBracket",
            AnyTokenExcept(&[SyntaxKind::AsyncKeyword, SyntaxKind::CaseKeyword]),
            Kind(SyntaxKind::OpenBracketToken),
            preds![is_non_jsx_same_line_token_context],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "NoSpaceAfterCloseBracket",
            Kind(SyntaxKind::CloseBracketToken),
            AnyToken,
            preds![
                is_non_jsx_same_line_token_context,
                is_not_before_block_in_function_declaration_context
            ],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        rule(
            "SpaceAfterSemicolon",
            Kind(SyntaxKind::SemicolonToken),
            AnyToken,
            preds![is_non_jsx_same_line_token_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        // Remove extra space between for and await
        rule(
            "SpaceBetweenForAndAwaitKeyword",
            Kind(SyntaxKind::ForKeyword),
            Kind(SyntaxKind::AwaitKeyword),
            preds![is_non_jsx_same_line_token_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
        // Remove extra spaces between ... and type name in tuple spread
        rule(
            "SpaceBetweenDotDotDotAndTypeName",
            Kind(SyntaxKind::DotDotDotToken),
            Kinds(TYPE_NAMES),
            preds![is_non_jsx_same_line_token_context],
            RuleAction::DELETE_SPACE,
            &[],
        ),
        // Add a space between statements. All keywords except (do,else,case) has open/close parens after them.
        // So, we have a rule to add a space for []ast.Kind{),Any}, []ast.Kind{do,Any}, []ast.Kind{else,Any}, and []ast.Kind{case,Any}
        rule(
            "SpaceBetweenStatements",
            Kinds(&[
                SyntaxKind::CloseParenToken,
                SyntaxKind::DoKeyword,
                SyntaxKind::ElseKeyword,
                SyntaxKind::CaseKeyword,
            ]),
            AnyToken,
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
            Kinds(&[
                SyntaxKind::TryKeyword,
                SyntaxKind::CatchKeyword,
                SyntaxKind::FinallyKeyword,
            ]),
            Kind(SyntaxKind::OpenBraceToken),
            preds![is_non_jsx_same_line_token_context],
            RuleAction::INSERT_SPACE,
            &[],
        ),
    ];

    HIGH_PRIORITY_COMMON_RULES
        .iter()
        .chain(USER_CONFIGURABLE_RULES)
        .chain(LOW_PRIORITY_COMMON_RULES)
        .map(|def| RuleSpec {
            left_token_range: token_range(def.left),
            right_token_range: token_range(def.right),
            rule: &def.rule,
        })
        .collect()
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

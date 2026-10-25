use crate::format::prelude::*;

use crate::flags_macros::{go_enum, go_flags};

// Go: format/rule.go:5 ruleImpl
// PORT: the rules are static data (rules.rs), so the name and the context are
// `&'static` and a rule is shared as `&'static RuleImpl`.
pub struct RuleImpl {
    pub debug_name: &'static str,
    pub context: &'static [ContextPredicate],
    pub action: RuleAction,
    pub flags: RuleFlags,
}

impl RuleImpl {
    // Go: format/rule.go:12 Action
    #[must_use]
    pub fn action(&self) -> RuleAction {
        self.action
    }

    // Go: format/rule.go:16 Context
    #[must_use]
    pub fn context(&self) -> &'static [ContextPredicate] {
        self.context
    }

    // Go: format/rule.go:20 Flags
    #[must_use]
    pub fn flags(&self) -> RuleFlags {
        self.flags
    }

    // Go: format/rule.go:24 String
    #[must_use]
    pub fn string(&self) -> String {
        self.debug_name.to_string()
    }
}

impl std::fmt::Display for RuleImpl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.debug_name)
    }
}

// Go: format/rule.go:28 tokenRange
#[derive(Clone, Debug, Default)]
pub struct TokenRange {
    pub tokens: Vec<SyntaxKind>,
    pub is_specific: bool,
}

// Go: format/rule.go:33 ruleSpec
pub struct RuleSpec {
    pub left_token_range: TokenRange,
    pub right_token_range: TokenRange,
    pub rule: &'static RuleImpl,
}

/// One rule of the static rule lists in rules.rs: Go's `rule(...)` call
/// before it builds the token ranges.
pub struct RuleDef {
    pub left: RuleTokens,
    pub right: RuleTokens,
    pub rule: RuleImpl,
}

// Go: format/rule.go:51 rule's `left any, right any`
// PORT: Go switches on the dynamic type in toTokenRange (`ast.Kind`,
// `[]ast.Kind` or `tokenRange`). A static rule names its side: `Kind` and
// `Kinds` are the first two cases, and the other variants are the
// tokenRange values that getAllRules computes from allTokens
// (`AnyTokenExcept` is `anyTokenExcept(...)`). get_all_rules makes the
// tokenRange.
#[derive(Clone, Copy)]
pub enum RuleTokens {
    Kind(SyntaxKind),
    Kinds(&'static [SyntaxKind]),
    AnyToken,
    AnyTokenExcept(&'static [SyntaxKind]),
    AnyTokenIncludingMultilineComments,
    AnyTokenIncludingEof,
    Keywords,
    BinaryOperators,
}

/**
 * A rule takes a two tokens (left/right) and a particular context
 * for which you're meant to look at them. You then declare what should the
 * whitespace annotation be between these tokens via the action param.
 *
 * @param debugName Name to print
 * @param left The left side of the comparison
 * @param right The right side of the comparison
 * @param context A set of filters to narrow down the space in which this formatter rule applies
 * @param action a declaration of the expected whitespace
 * @param flags whether the rule deletes a line or not, defaults to no-op
 */
// Go: format/rule.go:51 rule
// PORT: a `const fn` for the static rule lists; the token ranges are made
// later (see RuleTokens). Go `flags ...ruleFlags` is a slice; only the first
// element is read.
#[must_use]
pub const fn rule(
    debug_name: &'static str,
    left: RuleTokens,
    right: RuleTokens,
    context: &'static [ContextPredicate],
    action: RuleAction,
    flags: &[RuleFlags],
) -> RuleDef {
    let mut flag = RuleFlags::NONE;
    if !flags.is_empty() {
        flag = flags[0];
    }
    RuleDef {
        left,
        right,
        rule: RuleImpl {
            debug_name,
            context,
            action,
            flags: flag,
        },
    }
}

// Go: format/rule.go:83 contextPredicate
// PORT: Go `func(ctx *FormattingContext) bool`. The rules are static data, so
// a predicate is a value: `Fn` is a plain predicate function and the other
// variants are the closures that rulecontext.go's option helpers return
// (isOptionEnabled and so on; see `ContextPredicate::call`). `&mut` because
// the FormattingContext getters cache tristates (Go mutates through the
// pointer).
#[derive(Clone, Copy)]
pub enum ContextPredicate {
    Fn(fn(&mut FormattingContext) -> bool),
    OptionEquals(
        AnyOptionSelector<lsutil::SemicolonPreference>,
        lsutil::SemicolonPreference,
    ),
    OptionEnabled(OptionSelector),
    OptionDisabled(OptionSelector),
    OptionDisabledOrUndefined(OptionSelector),
    OptionDisabledOrUndefinedOrTokensOnSameLine(OptionSelector),
    OptionEnabledOrUndefined(OptionSelector),
}

// Go: format/rule.go:85 anyContext
pub const ANY_CONTEXT: &[ContextPredicate] = &[];

// Go: format/rule.go:87 ruleAction
go_flags!(RuleAction, i32 {
    NONE = 0; // ruleActionNone
    STOP_PROCESSING_SPACE_ACTIONS = 1 << 0; // ruleActionStopProcessingSpaceActions
    STOP_PROCESSING_TOKEN_ACTIONS = 1 << 1; // ruleActionStopProcessingTokenActions
    INSERT_SPACE = 1 << 2; // ruleActionInsertSpace
    INSERT_NEW_LINE = 1 << 3; // ruleActionInsertNewLine
    DELETE_SPACE = 1 << 4; // ruleActionDeleteSpace
    DELETE_TOKEN = 1 << 5; // ruleActionDeleteToken
    INSERT_TRAILING_SEMICOLON = 1 << 6; // ruleActionInsertTrailingSemicolon

    STOP_ACTION = (1 << 0) | (1 << 1); // ruleActionStopProcessingSpaceActions | ruleActionStopProcessingTokenActions
    MODIFY_SPACE_ACTION = (1 << 2) | (1 << 3) | (1 << 4); // ruleActionInsertSpace | ruleActionInsertNewLine | ruleActionDeleteSpace
    MODIFY_TOKEN_ACTION = (1 << 5) | (1 << 6); // ruleActionDeleteToken | ruleActionInsertTrailingSemicolon
});

// Go: format/rule.go:104 ruleFlags
go_enum!(RuleFlags, i32 {
    NONE = 0; // ruleFlagsNone
    CAN_DELETE_NEW_LINES = 1; // ruleFlagsCanDeleteNewLines
});

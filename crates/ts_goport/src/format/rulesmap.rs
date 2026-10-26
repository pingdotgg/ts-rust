use crate::format::prelude::*;

use crate::flags_macros::go_enum;
use std::sync::LazyLock;

// Go: format/rulesmap.go:11 getRules
// PORT: Go appends to the caller's slice and returns it. The caller (span.go)
// reuses one slice, so this appends to `rules` in place.
pub fn get_rules(context: &mut FormattingContext, rules: &mut Vec<&'static RuleImpl>) {
    let bucket = &get_rules_map()[get_rule_bucket_index(
        context.current_token_span.kind,
        context.next_token_span.kind,
    ) as usize];
    if !bucket.is_empty() {
        let mut rule_action_mask = RuleAction::NONE;
        'outer: for &rule in bucket {
            let accept_rule_actions = !get_rule_action_exclusion(rule_action_mask);
            if rule.action().intersects(accept_rule_actions) {
                let preds = rule.context();
                for p in preds {
                    if !p.call(context) {
                        continue 'outer;
                    }
                }
                rules.push(rule);
                rule_action_mask |= rule.action();
            }
        }
    }
}

// Go: format/rulesmap.go:34 getRuleBucketIndex
pub fn get_rule_bucket_index(row: SyntaxKind, column: SyntaxKind) -> i32 {
    crate::go_assert!(
        row as u16 <= SyntaxKind::LAST_KEYWORD as u16
            && column as u16 <= SyntaxKind::LAST_KEYWORD as u16,
        "Must compute formatting context from tokens"
    );
    (row as i32 * MAP_ROW_LENGTH) + column as i32
}

// Go: format/rulesmap.go:40 maskBitSize
pub const MASK_BIT_SIZE: i32 = 5;
// Go: format/rulesmap.go:41 mask
pub const MASK: i32 = 0b11111; // MaskBitSize bits
// Go: format/rulesmap.go:42 mapRowLength
pub const MAP_ROW_LENGTH: i32 = SyntaxKind::LAST_TOKEN as i32 + 1;

/**
 * For a given rule action, gets a mask of other rule actions that
 * cannot be applied at the same position.
 */
// Go: format/rulesmap.go:49 getRuleActionExclusion
pub fn get_rule_action_exclusion(rule_action: RuleAction) -> RuleAction {
    let mut mask = RuleAction::NONE;
    if rule_action.intersects(RuleAction::STOP_PROCESSING_SPACE_ACTIONS) {
        mask |= RuleAction::MODIFY_SPACE_ACTION;
    }
    if rule_action.intersects(RuleAction::STOP_PROCESSING_TOKEN_ACTIONS) {
        mask |= RuleAction::MODIFY_TOKEN_ACTION;
    }
    if rule_action.intersects(RuleAction::MODIFY_SPACE_ACTION) {
        mask |= RuleAction::MODIFY_SPACE_ACTION;
    }
    if rule_action.intersects(RuleAction::MODIFY_TOKEN_ACTION) {
        mask |= RuleAction::MODIFY_TOKEN_ACTION;
    }
    mask
}

// Go: format/rulesmap.go:66 getRulesMap
// PORT: Go `sync.OnceValue(buildRulesMap)` is a process-wide LazyLock.
pub fn get_rules_map() -> &'static [Vec<&'static RuleImpl>] {
    static RULES_MAP: LazyLock<Vec<Vec<&'static RuleImpl>>> = LazyLock::new(build_rules_map);
    &RULES_MAP
}

// Go: format/rulesmap.go:68 buildRulesMap
pub fn build_rules_map() -> Vec<Vec<&'static RuleImpl>> {
    let rules = get_all_rules();
    // Map from bucket index to array of rules
    let mut m: Vec<Vec<&'static RuleImpl>> =
        vec![Vec::new(); (MAP_ROW_LENGTH * MAP_ROW_LENGTH) as usize];
    // This array is used only during construction of the rulesbucket in the map
    let mut rules_bucket_construction_state_list: Vec<i32> = vec![0; m.len()];
    for rule in &rules {
        let specific_rule = rule.left_token_range.is_specific && rule.right_token_range.is_specific;

        for &left in &rule.left_token_range.tokens {
            for &right in &rule.right_token_range.tokens {
                let index = get_rule_bucket_index(left, right);
                // PORT: Go `m[index] = addRule(m[index], ...)`; add_rule
                // inserts into the bucket in place.
                add_rule(
                    &mut m[index as usize],
                    rule.rule,
                    specific_rule,
                    &mut rules_bucket_construction_state_list,
                    index,
                );
            }
        }
    }
    m
}

// Go: format/rulesmap.go:87 RulesPosition
go_enum!(RulesPosition, i32 {
    STOP_RULES_SPECIFIC = 0; // RulesPositionStopRulesSpecific
    STOP_RULES_ANY = MASK_BIT_SIZE * 1; // RulesPositionStopRulesAny
    CONTEXT_RULES_SPECIFIC = MASK_BIT_SIZE * 2; // RulesPositionContextRulesSpecific
    CONTEXT_RULES_ANY = MASK_BIT_SIZE * 3; // RulesPositionContextRulesAny
    NO_CONTEXT_RULES_SPECIFIC = MASK_BIT_SIZE * 4; // RulesPositionNoContextRulesSpecific
    NO_CONTEXT_RULES_ANY = MASK_BIT_SIZE * 5; // RulesPositionNoContextRulesAny
});

// The Rules list contains all the inserted rules into a rulebucket in the following order:
//
//	1- Ignore rules with specific token combination
//	2- Ignore rules with any token combination
//	3- Context rules with specific token combination
//	4- Context rules with any token combination
//	5- Non-context rules with specific token combination
//	6- Non-context rules with any token combination
//
// The member rulesInsertionIndexBitmap is used to describe the number of rules
// in each sub-bucket (above) hence can be used to know the index of where to insert
// the next rule. It's a bitmap which contains 6 different sections each is given 5 bits.
//
// Example:
// In order to insert a rule to the end of sub-bucket (3), we get the index by adding
// the values in the bitmap segments 3rd, 2nd, and 1st.
// Go: format/rulesmap.go:114 addRule
// PORT: Go returns the grown slice; here the bucket is updated in place.
pub fn add_rule(
    rules: &mut Vec<&'static RuleImpl>,
    rule: &'static RuleImpl,
    specific_tokens: bool,
    construction_state: &mut [i32],
    rules_bucket_index: i32,
) {
    let position = if rule.action().intersects(RuleAction::STOP_ACTION) {
        if specific_tokens {
            RulesPosition::STOP_RULES_SPECIFIC
        } else {
            RulesPosition::STOP_RULES_ANY
        }
    } else if !rule.context().is_empty() {
        if specific_tokens {
            RulesPosition::CONTEXT_RULES_SPECIFIC
        } else {
            RulesPosition::CONTEXT_RULES_ANY
        }
    } else if specific_tokens {
        RulesPosition::NO_CONTEXT_RULES_SPECIFIC
    } else {
        RulesPosition::NO_CONTEXT_RULES_ANY
    };

    let state = construction_state[rules_bucket_index as usize];

    rules.insert(get_rule_insertion_index(state, position) as usize, rule);
    construction_state[rules_bucket_index as usize] = increase_insertion_index(state, position);
}

// Go: format/rulesmap.go:143 getRuleInsertionIndex
pub fn get_rule_insertion_index(mut index_bitmap: i32, mask_position: RulesPosition) -> i32 {
    let mut index = 0;
    let mut pos = 0;
    while pos <= mask_position.0 {
        index += index_bitmap & MASK;
        index_bitmap >>= MASK_BIT_SIZE;
        pos += MASK_BIT_SIZE;
    }
    index
}

// Go: format/rulesmap.go:152 increaseInsertionIndex
pub fn increase_insertion_index(index_bitmap: i32, mask_position: RulesPosition) -> i32 {
    let value = ((index_bitmap >> mask_position.0) & MASK) + 1;
    crate::go_assert!(
        (value & MASK) == value,
        "Adding more rules into the sub-bucket than allowed. Maximum allowed is 32 rules."
    );
    (index_bitmap & !(MASK << mask_position.0)) | (value << mask_position.0)
}

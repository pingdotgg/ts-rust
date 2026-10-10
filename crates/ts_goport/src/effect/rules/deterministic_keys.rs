// Go: internal/rules/deterministic_keys.go

use crate::effect::diag;
use crate::effect::etscore::*;
use crate::effect::keybuilder;
use crate::effect::rule::*;
use crate::effect::typeparser::*;
use crate::prelude::*;

/// DeterministicKeys ensures string key literals in Effect service/tag/error constructors
/// follow a deterministic, location-based naming convention.
pub static DETERMINISTIC_KEYS: Rule = Rule {
    name: "deterministicKeys",
    group: "style",
    description: "Enforces deterministic naming for service/tag/error identifiers based on class names",
    default_severity: Severity::Off,
    supported_effect: &["v3", "v4"],
    codes: &[377049],
    run: run_deterministic_keys,
};

fn run_deterministic_keys(ctx: &mut RuleContext<'_, '_>) -> Vec<Diagnostic> {
    let matches =
        analyze_deterministic_keys(ctx.tp, ctx.program, ctx.source_file, Some(ctx.options));
    let mut diags = Vec::with_capacity(matches.len());
    for m in &matches {
        diags.push(ctx.new_diagnostic(
            m.source_file,
            m.location,
            diag::This_key_does_not_match_the_deterministic_key_for_this_declaration_The_expected_key_is_0_effect_deterministicKeys,
            Vec::new(),
            vec![m.expected_key.clone()],
        ));
    }
    diags
}

/// DeterministicKeyMatch holds the AST nodes and computed key info needed by both
/// the diagnostic rule and the quick-fix for the deterministicKeys pattern.
#[derive(Clone, Debug)]
pub struct DeterministicKeyMatch {
    /// The source file of the match
    pub source_file: Node,
    /// The pre-computed error range for the key string literal
    pub location: TextRange,
    /// The key string literal node
    pub key_string_literal: Node,
    /// The actual key string found in the source
    pub actual_key: String,
    /// The expected key computed by keybuilder
    pub expected_key: String,
}

// Go: rules/deterministic_keys.go AnalyzeDeterministicKeys
/// AnalyzeDeterministicKeys finds all class declarations where the key string literal
/// doesn't match the expected deterministic key.
pub fn analyze_deterministic_keys(
    tp: &mut TypeParser<'_>,
    program: &'static GoProgram,
    sf: Node,
    effect_config: Option<&ResolvedEffectPluginOptions>,
) -> Vec<DeterministicKeyMatch> {
    let Some(effect_config) = effect_config else {
        return Vec::new();
    };

    let key_patterns = effect_config.get_key_patterns();
    let extended_key_detection = effect_config.extended_key_detection;

    let mut matches = Vec::new();

    let mut node_to_visit: Vec<Node> = Vec::new();
    sf.for_each_child(|child| {
        node_to_visit.push(child);
        false
    });

    while let Some(node) = node_to_visit.pop() {
        if node.kind() == SyntaxKind::ClassDeclaration && node.name().is_some() {
            if let Some(m) = check_deterministic_key_match(
                tp,
                program,
                sf,
                node,
                &key_patterns,
                extended_key_detection,
            ) {
                matches.push(m);
            }
        }

        node.for_each_child(|child| {
            node_to_visit.push(child);
            false
        });
    }

    matches
}

/// deterministicKeyMatch holds the matched key info from a class declaration.
// PORT: Go `deterministicKeyMatch`. Its Rust name would collide with the
// exported `DeterministicKeyMatch`, so it has an `Inner` suffix.
#[derive(Clone, Debug)]
pub struct DeterministicKeyMatchInner {
    pub class_name: Node,
    pub key_string_literal: Node,
    pub target: String,
}

// Go: rules/deterministic_keys.go checkDeterministicKeyMatch
fn check_deterministic_key_match(
    tp: &mut TypeParser<'_>,
    program: &'static GoProgram,
    sf: Node,
    class_node: Node,
    key_patterns: &[KeyPattern],
    extended_key_detection: bool,
) -> Option<DeterministicKeyMatch> {
    let m = match_class_pattern(tp, sf, class_node, extended_key_detection)?;
    if m.key_string_literal.is_nil() {
        return None;
    }

    // Get class name text
    let class_name_text = get_text_of_node(m.class_name);

    // Get package info
    let pkg_json = tp.package_json_for_source_file(sf)?;
    let (package_name, ok) = pkg_json.fields.name.get_value();
    if !ok || package_name.is_empty() {
        return None;
    }

    // Get package directory from source file metadata
    let package_directory = get_package_json_directory(program, sf);
    if package_directory.is_empty() {
        return None;
    }

    // Get source file name
    let source_file_name = source_file_file_name(sf);

    // Compute expected key
    let expected_key = keybuilder::create_string(
        source_file_name,
        &package_name,
        &package_directory,
        &class_name_text,
        &m.target,
        key_patterns,
    );
    if expected_key.is_empty() {
        return None;
    }

    // Get actual key
    let actual_key = m.key_string_literal.text().to_string();

    if actual_key == expected_key {
        return None;
    }

    Some(DeterministicKeyMatch {
        source_file: sf,
        location: get_error_range_for_node(sf, m.key_string_literal),
        key_string_literal: m.key_string_literal,
        actual_key,
        expected_key,
    })
}

// Go: rules/deterministic_keys.go matchClassPattern
/// matchClassPattern tries to match a class declaration against the supported patterns.
/// Priority: service targets first, then error targets, then custom.
fn match_class_pattern(
    tp: &mut TypeParser<'_>,
    sf: Node,
    class_node: Node,
    extended_key_detection: bool,
) -> Option<DeterministicKeyMatchInner> {
    // Service target: ExtendsEffectService → ExtendsContextTag → ExtendsEffectTag → ExtendsServiceMapService
    if let Some(result) = tp.extends_effect_v3_service(class_node) {
        return Some(DeterministicKeyMatchInner {
            class_name: result.class_name,
            key_string_literal: result.key_string_literal,
            target: "service".to_string(),
        });
    }
    if let Some(result) = tp.extends_context_tag(class_node) {
        return Some(DeterministicKeyMatchInner {
            class_name: result.class_name,
            key_string_literal: result.key_string_literal,
            target: "service".to_string(),
        });
    }
    if let Some(result) = tp.extends_effect_tag(class_node) {
        return Some(DeterministicKeyMatchInner {
            class_name: result.class_name,
            key_string_literal: result.key_string_literal,
            target: "service".to_string(),
        });
    }
    if let Some(result) = tp.extends_context_service(class_node) {
        return Some(DeterministicKeyMatchInner {
            class_name: result.class_name,
            key_string_literal: result.key_string_literal,
            target: "service".to_string(),
        });
    }

    // Error target: ExtendsDataTaggedError → ExtendsSchemaTaggedError
    if let Some(result) = tp.extends_data_tagged_error(class_node) {
        return Some(DeterministicKeyMatchInner {
            class_name: result.class_name,
            key_string_literal: result.key_string_literal,
            target: "error".to_string(),
        });
    }
    if let Some(result) = tp.extends_schema_tagged_error(class_node) {
        return Some(DeterministicKeyMatchInner {
            class_name: result.class_name,
            key_string_literal: result.key_string_literal,
            target: "error".to_string(),
        });
    }

    // Custom target (only if extendedKeyDetection is enabled)
    if extended_key_detection {
        if let Some(result) = match_custom_pattern(tp.checker, sf, class_node) {
            return Some(result);
        }
    }

    None
}

// Go: rules/deterministic_keys.go matchCustomPattern
/// matchCustomPattern checks heritage clause nodes for call expressions with string literal
/// arguments whose parameter declarations contain the @effect-identifier annotation.
fn match_custom_pattern(
    c: &mut Checker,
    _sf: Node,
    class_node: Node,
) -> Option<DeterministicKeyMatchInner> {
    if class_node.name().is_nil() {
        return None;
    }

    let heritage_clauses = class_node.heritage_clauses();
    if heritage_clauses.is_nil() {
        return None;
    }

    // BFS through heritage clause nodes
    let mut nodes_to_visit: std::collections::VecDeque<Node> =
        heritage_clauses.nodes().iter().collect();

    while let Some(current) = nodes_to_visit.pop_front() {
        if is_call_expression(current) {
            let call = current;
            let arguments = call.argument_list();
            if arguments.is_some() {
                for (i, arg) in arguments.nodes().iter().enumerate() {
                    if !is_string_literal(arg) {
                        continue;
                    }

                    let sig = c.get_resolved_signature_exported(current);
                    if sig.is_nil() {
                        continue;
                    }

                    let params = c.sig(sig).parameters.clone();
                    if i >= params.len() {
                        continue;
                    }

                    let param = params[i];
                    let declarations: Vec<Node> = c.sym(param).declarations.to_vec();
                    if declarations.is_empty() {
                        continue;
                    }

                    for decl in declarations {
                        let param_sf = get_source_file_of_node(decl);
                        if param_sf.is_nil() {
                            continue;
                        }
                        let param_text = source_file_text(param_sf);
                        let pos = decl.pos();
                        let end = decl.end();
                        if pos >= 0 && end >= pos && (end as usize) <= param_text.len() {
                            // PORT: Go slices the bytes; the text is lowered as Go
                            // `strings.ToLower` does (one rune at a time).
                            let decl_text = String::from_utf8_lossy(
                                &param_text.as_bytes()[pos as usize..end as usize],
                            );
                            let lowered: String = decl_text
                                .chars()
                                .map(crate::gostd::unicode::to_lower)
                                .collect();
                            if lowered.contains("@effect-identifier") {
                                return Some(DeterministicKeyMatchInner {
                                    class_name: class_node.name(),
                                    key_string_literal: arg,
                                    target: "custom".to_string(),
                                });
                            }
                        }
                    }
                }
            }
        }

        // Visit children
        current.for_each_child(|child| {
            nodes_to_visit.push_back(child);
            false
        });
    }

    None
}

// Go: rules/deterministic_keys.go getPackageJsonDirectory
/// getPackageJsonDirectory gets the package.json directory for a source file from its metadata.
pub(crate) fn get_package_json_directory(_program: &'static GoProgram, sf: Node) -> String {
    // PORT: Go asserts the program to a `GetSourceFileMetaData` provider.
    // The port's program always has it, and `get_source_file_meta_data`
    // reads the current program of the thread (as `PackageJsonForSourceFile`
    // does).
    let meta = get_source_file_meta_data(&source_file_info(sf).path);
    meta.package_json_directory
}

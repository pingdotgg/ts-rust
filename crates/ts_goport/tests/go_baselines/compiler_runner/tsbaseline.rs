//! Go: internal/testutil/tsbaseline: error_baseline.go, js_emit_baseline.go,
//! sourcemap_baseline.go, sourcemap_record_baseline.go,
//! module_resolution_baseline.go, contentmapper_baseline.go (tsgo#4712),
//! util.go and the `testing` wrappers of
//! type_symbol_baseline.go (`DoTypeAndSymbolBaseline`, `checkBaselines`,
//! `isTypeBaselineNodeReuseLine`). The walker itself is
//! `ts_goport::baseline::type_symbol`.
//!
//! PORT: Go `baseline.Run` reports through `t`. Here each `do_*` function
//! returns `Err` with the messages of a failed comparison or check.
//! `assert.Check` failures (the error baseline counts) are collected in
//! `checks` and fail the subtest after the comparison, as in Go.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::sync::Arc;

use ts_goport::api::to_rooted_path;
use ts_goport::baseline::type_symbol::{TestFile, generate_baseline, new_type_writer_walker};
use ts_goport::baseline::util::{is_default_library_file, remove_test_path_prefixes};
use ts_goport::contentmapper::Mapper;
use ts_goport::execute::tsc::compile::{Writer, write_str};
use ts_goport::execute::tsc::diagnostics::{
    FormattingOptions, write_error_summary_text, write_flattened_diagnostic_message, write_location,
};
use ts_goport::frontend::json::json_unmarshal;
use ts_goport::frontend::parser::{SourceFileParseOptions, parse_source_file};
use ts_goport::frontend::prelude::*;
use ts_goport::sourcemap::generator::RawSourceMap;

use super::go_regex;
use super::harness::{CompilationResult, HarnessOptions, TestConfiguration, compile_files_ex};
use crate::support::baseline::{self, NO_CONTENT, Options};
use crate::tsoptions::tsoptionstest::{
    capture_writer, format_diagnostics_with_color_and_context, write_format_diagnostics,
};

// ---------------------------------------------------------------------------
// util.go
// ---------------------------------------------------------------------------

// Go: util.go:19 libFolder, builtFolder
const LIB_FOLDER: &str = "built/local/";
const BUILT_FOLDER: &str = "/.ts";

// Go: util.go:56 isBuiltFile
fn is_built_file(file_path: &str) -> bool {
    file_path.starts_with(LIB_FOLDER)
        || file_path.starts_with(&ensure_trailing_directory_separator(BUILT_FOLDER))
}

// Go: util.go:60 isTsConfigFile
fn is_ts_config_file(path: &str) -> bool {
    // !!! fix to check for just prefixes/suffixes
    path.contains("tsconfig") && path.contains("json")
}

// Go: util.go:65 sanitizeTestFilePath
fn sanitize_test_file_path(name: &str) -> String {
    let path = go_regex::replace_test_path_characters(name);
    let path = normalize_slashes(&path);
    let path = go_regex::replace_dot_dot_slash(&path);
    // ts#64159 (util.go:69): `CaseInsensitive.Canonicalize(NormalizePath(path))`.
    // N used `ToPath(path, "", false)`, which also resolved a relative name
    // against "".
    let path = to_file_name_lower_case(&normalize_path(&path));
    path.strip_prefix('/').unwrap_or(&path).to_string()
}

// ---------------------------------------------------------------------------
// error_baseline.go
// ---------------------------------------------------------------------------

// Go: error_baseline.go:24 harnessNewLine
// IO
const HARNESS_NEW_LINE: &str = "\r\n";

// Go: error_baseline.go:26 formatOpts
fn format_opts() -> FormattingOptions {
    FormattingOptions {
        new_line: HARNESS_NEW_LINE.to_string(),
        ..FormattingOptions::default()
    }
}

// Go: error_baseline.go:35 DoErrorBaseline
pub fn do_error_baseline(
    baseline_path: &str,
    input_files: &[TestFile],
    errors: &[Diagnostic],
    pretty: bool,
    opts: &Options,
) -> Result<(), String> {
    let baseline_path = go_regex::replace_ts_extension(baseline_path, ".errors.txt");
    let mut checks = Vec::new();
    let error_baseline = if errors.is_empty() {
        NO_CONTENT.to_string()
    } else {
        get_error_baseline(&mut checks, input_files, errors, pretty)
    };
    let result = baseline::run(&baseline_path, &error_baseline, opts);
    let result = finish_checks(result, checks);
    // PORT: Go `t.Fatalf` after the comparison, so its message comes last.
    if errors.iter().any(|d| d.code() == -1) {
        let message = "Found diagnostic with code -1, which is used to log critical assertion violations in the baseline. Inspect and fix those failures.";
        return Err(match result {
            Ok(()) => message.to_string(),
            Err(previous) => format!("{previous}\n{message}"),
        });
    }
    result
}

/// Joins a comparison result with the Go `assert.Check` failures.
pub fn finish_checks(result: Result<(), String>, checks: Vec<String>) -> Result<(), String> {
    if checks.is_empty() {
        return result;
    }
    let mut messages = checks;
    if let Err(message) = result {
        messages.push(message);
    }
    Err(messages.join("\n"))
}

// Go: error_baseline.go:51 minimalDiagnosticsToString
fn minimal_diagnostics_to_string(diagnostics: &[Diagnostic], pretty: bool) -> String {
    let mut output = String::new();
    if pretty {
        format_diagnostics_with_color_and_context(&mut output, diagnostics, &format_opts());
    } else {
        write_format_diagnostics(&mut output, diagnostics, &format_opts());
    }
    output
}

// Go: error_baseline.go:61 GetErrorBaseline
pub fn get_error_baseline(
    checks: &mut Vec<String>,
    input_files: &[TestFile],
    diagnostics: &[Diagnostic],
    pretty: bool,
) -> String {
    let mut output_lines = iterate_error_baseline(checks, input_files, diagnostics, pretty);

    if pretty {
        let summary = capture_writer(|w| {
            write_error_summary_text(w, diagnostics, &format_opts());
        });
        let summary = remove_test_path_prefixes(&summary, false);
        output_lines.push(summary);
    }
    output_lines.concat()
}

/// Go `diagnosticwriter.FlattenDiagnosticMessage(d, newLine, locale.Default)`.
fn flatten_diagnostic_message(diagnostic: &Diagnostic, new_line: &str) -> String {
    capture_writer(|w| {
        write_flattened_diagnostic_message(w, diagnostic, new_line, &ts_goport::locale::DEFAULT);
    })
}

/// The Go `formatLocation` writer: `fmt.Fprint(output, text)`.
fn write_plain(output: &Writer, text: &str, _format_style: &str) {
    write_str(output, text);
}

// Go: error_baseline.go:276 formatLocation
fn format_location(file: Node, pos: i32, format_opts: &FormattingOptions) -> String {
    capture_writer(|w| write_location(w, file, pos, Some(format_opts), write_plain))
}

/// Go `diag.File().FileName()` for an `*ast.Diagnostic` with a file: the
/// file's own name (a supplemental mapper output keeps its own name). The
/// compiler runner, the content mapper baseline and the transpile runner read
/// it.
pub fn diagnostic_file_name(diagnostic: &Diagnostic) -> Option<String> {
    diagnostic
        .file
        .is_some()
        .then(|| source_file_file_name(diagnostic.file).to_string())
}

/// Go `diag.File().FileName()` for a wrapped `*diagnosticwriter.ASTDiagnostic`
/// with a file (the error baseline, ts#63936).
fn wrapped_diagnostic_file_name(diagnostic: &Diagnostic) -> Option<&'static str> {
    diagnostic
        .file
        .is_some()
        .then(|| ast_diagnostic_file_name(diagnostic.file))
}

// Go: diagnosticwriter/diagnosticwriter.go:64 ASTDiagnostic.File (ts#63936)
// The file name of `File()`: the canonical source file's name for a
// supplemental mapper output, else the file's own name.
// PORT: the port writes diagnostics without the wrapper, so this is the part
// of `File()` that `FileName()` reads.
fn ast_diagnostic_file_name(file: Node) -> &'static str {
    let canonical = ts_goport::ast::source_file_canonical_source_file(file);
    if canonical.is_some() {
        return source_file_file_name(canonical);
    }
    source_file_file_name(file)
}

/// The Go `newLine` closure of `iterateErrorBaseline`.
fn new_line(first_line: &mut bool) -> &'static str {
    if *first_line {
        *first_line = false;
        return "";
    }
    "\r\n"
}

/// Go `line[a:b]` of a port form line. Offsets are unit boundaries unless
/// a diagnostic starts inside a char; then the slice is empty.
fn slice(line: &str, a: usize, b: usize) -> &str {
    line.get(a..b).unwrap_or("")
}

// Go: error_baseline.go:78 iterateErrorBaseline
fn iterate_error_baseline(
    checks: &mut Vec<String>,
    input_files: &[TestFile],
    input_diagnostics: &[Diagnostic],
    pretty: bool,
) -> Vec<String> {
    let mut diagnostics: Vec<Diagnostic> = input_diagnostics.to_vec();
    diagnostics.sort_by(|a, b| compare_diagnostics(a, b).cmp(&0));

    let mut output_lines = String::new();
    // Count up all errors that were found in files other than lib.d.ts so we don't miss any
    let mut total_errors_reported_in_non_library_non_tsconfig_files: i64 = 0;
    let mut errors_reported: i64 = 0;

    let mut first_line = true;

    let mut result: Vec<String> = Vec::new();

    let output_error_text = |diag: &Diagnostic,
                             output_lines: &mut String,
                             first_line: &mut bool,
                             errors_reported: &mut i64,
                             total: &mut i64| {
        let message = flatten_diagnostic_message(diag, HARNESS_NEW_LINE);

        let mut err_lines: Vec<String> = Vec::new();
        for line in remove_test_path_prefixes(&message, false).split('\n') {
            let line = line.strip_suffix('\r').unwrap_or(line);
            if line.is_empty() {
                continue;
            }
            err_lines.push(format!(
                "!!! {} TS{}: {line}",
                diag.category.name(),
                diag.code
            ));
        }

        for info in &diag.related_information {
            let mut location = String::new();
            if info.file.is_some() {
                location = format!(" {}", format_location(info.file, info.pos, &format_opts()));
            }
            location = remove_test_path_prefixes(&location, false);
            if !location.is_empty() && is_default_library_file(ast_diagnostic_file_name(info.file))
            {
                location = go_regex::replace_diagnostics_location_pattern(&location);
            }
            err_lines.push(format!(
                "!!! related TS{}{location}: {}",
                info.code,
                flatten_diagnostic_message(info, HARNESS_NEW_LINE)
            ));
        }

        for e in &err_lines {
            output_lines.push_str(new_line(first_line));
            output_lines.push_str(e);
        }

        *errors_reported += 1;

        // do not count errors from lib.d.ts here, they are computed separately as numLibraryDiagnostics
        // if lib.d.ts is explicitly included in input files and there are some errors in it (i.e. because of duplicate identifiers)
        // then they will be added twice thus triggering 'total errors' assertion with condition
        // Similarly for tsconfig, which may be in the input files and contain errors.
        // 'totalErrorsReportedInNonLibraryNonTsconfigFiles + numLibraryDiagnostics + numTsconfigDiagnostics, diagnostics.length

        match wrapped_diagnostic_file_name(diag) {
            None => *total += 1,
            Some(name) => {
                if !is_default_library_file(name) && !is_ts_config_file(name) {
                    *total += 1;
                }
            }
        }
    };

    let top_diagnostics = minimal_diagnostics_to_string(&diagnostics, pretty);
    let top_diagnostics = remove_test_path_prefixes(&top_diagnostics, false);
    let top_diagnostics = go_regex::replace_diagnostics_location_prefix(&top_diagnostics);

    result.push(format!(
        "{top_diagnostics}{HARNESS_NEW_LINE}{HARNESS_NEW_LINE}"
    ));

    // Report global errors
    for error in &diagnostics {
        if error.file.is_nil() {
            output_error_text(
                error,
                &mut output_lines,
                &mut first_line,
                &mut errors_reported,
                &mut total_errors_reported_in_non_library_non_tsconfig_files,
            );
        }
    }

    result.push(std::mem::take(&mut output_lines));
    errors_reported = 0;

    // 'merge' the lines of each input file with any errors associated with it
    let dupe_case: BTreeMap<String, i32> = BTreeMap::new();
    for input_file in input_files {
        // Filter down to the errors in the file
        let unit_name = remove_test_path_prefixes(&input_file.unit_name, false);
        let file_errors: Vec<&Diagnostic> = diagnostics
            .iter()
            .filter(|e| {
                e.file.is_some()
                    && compare_paths(
                        &remove_test_path_prefixes(ast_diagnostic_file_name(e.file), false),
                        &unit_name,
                        &ComparePathsOptions::default(),
                    ) == 0
            })
            .collect();

        // Header
        output_lines.push_str(&format!(
            "{}==== {} ({} errors) ====",
            new_line(&mut first_line),
            unit_name,
            file_errors.len()
        ));

        // Make sure we emit something for every error
        let mut marked_error_count = 0usize;
        // For each line, emit the line followed by any error squiggles matching this line

        let line_starts = compute_ecma_line_starts(&input_file.content);
        let lines = go_regex::split_line_delimiter(&input_file.content);

        for (line_index, line) in lines.iter().enumerate() {
            let line: &str = line.strip_suffix('\r').unwrap_or(line);

            let this_line_start = line_starts[line_index] as i64;
            // On the last line of the file, fake the next line start number so that we handle errors on the last character of the file correctly
            let next_line_start = if line_index == lines.len() - 1 {
                input_file.content.len() as i64
            } else {
                line_starts[line_index + 1] as i64
            };
            // Emit this line from the original file
            output_lines.push_str(new_line(&mut first_line));
            output_lines.push_str("    ");
            output_lines.push_str(line);
            for err_diagnostic in &file_errors {
                // Does any error start or continue on to this line? Emit squiggles
                let err_start = err_diagnostic.pos() as i64;
                let end = err_start + err_diagnostic.len() as i64;
                if end >= this_line_start
                    && (err_start < next_line_start || line_index == lines.len() - 1)
                {
                    // How many characters from the start of this line the error starts at (could be positive or negative)
                    let relative_offset = err_start - this_line_start;
                    // How many characters of the error are on this line (might be longer than this line in reality)
                    let length = (end - err_start) - 0.max(this_line_start - err_start);
                    // Calculate the start of the squiggle
                    let squiggle_start = 0.max(relative_offset) as usize;
                    // TODO/REVIEW: this doesn't work quite right in the browser if a multi file test has files whose names are just the right length relative to one another
                    output_lines.push_str(new_line(&mut first_line));
                    output_lines.push_str("    ");
                    if squiggle_start > line.len() {
                        panic!(
                            "runtime error: slice bounds out of range [:{squiggle_start}] with length {}",
                            line.len()
                        );
                    }
                    output_lines.push_str(&go_regex::replace_non_whitespace(slice(
                        line,
                        0,
                        squiggle_start,
                    )));
                    // This was `new Array(count).join("~")`; which maps 0 to "", 1 to "", 2 to "~", 3 to "~~", etc.
                    let squiggle_end = (squiggle_start as i64)
                        .max((squiggle_start as i64 + length).min(line.len() as i64))
                        as usize;
                    output_lines.push_str(&"~".repeat(go_regex::go_rune_count(slice(
                        line,
                        squiggle_start,
                        squiggle_end,
                    ))));
                    // If the error ended here, or we're at the end of the file, emit its message
                    if line_index == lines.len() - 1 || next_line_start > end {
                        output_error_text(
                            err_diagnostic,
                            &mut output_lines,
                            &mut first_line,
                            &mut errors_reported,
                            &mut total_errors_reported_in_non_library_non_tsconfig_files,
                        );
                        marked_error_count += 1;
                    }
                }
            }
        }

        // Verify we didn't miss any errors in this file
        if marked_error_count != file_errors.len() {
            checks.push(format!(
                "assertion failed: {marked_error_count} (markedErrorCount int) != {} (len(fileErrors) int): count of errors in {}",
                file_errors.len(),
                input_file.unit_name
            ));
        }
        let is_dupe = dupe_case.contains_key(&sanitize_test_file_path(&input_file.unit_name));
        result.push(std::mem::take(&mut output_lines));
        if is_dupe {
            // Case-duplicated files on a case-insensitive build will have errors reported in both the dupe and the original
            // thanks to the canse-insensitive path comparison on the error file path - We only want to count those errors once
            // for the assert below, so we subtract them here.
            total_errors_reported_in_non_library_non_tsconfig_files -= errors_reported;
        }
        errors_reported = 0;
    }

    let num_library_diagnostics = diagnostics
        .iter()
        .filter(|d| {
            wrapped_diagnostic_file_name(d)
                .is_some_and(|name| is_default_library_file(name) || is_built_file(name))
        })
        .count() as i64;
    let num_tsconfig_diagnostics = diagnostics
        .iter()
        .filter(|d| wrapped_diagnostic_file_name(d).is_some_and(is_ts_config_file))
        .count() as i64;
    // Go: error_baseline.go:255 (tsgo#4712, ts#63936)
    // PORT: the diagnostics are Go `*diagnosticwriter.ASTDiagnostic`, so this
    // is the first branch: the file of the wrapped `ast.Diagnostic`.
    let num_content_mapper_supplemental_diagnostics = diagnostics
        .iter()
        .filter(|d| {
            d.file.is_some() && ts_goport::ast::source_file_is_content_mapper_supplemental(d.file)
        })
        .count() as i64;
    // Verify we didn't miss any errors in total
    let total = total_errors_reported_in_non_library_non_tsconfig_files
        + num_library_diagnostics
        + num_tsconfig_diagnostics
        + num_content_mapper_supplemental_diagnostics;
    if total != diagnostics.len() as i64 {
        checks.push(format!(
            "assertion failed: {total} (int) != {} (int): total number of errors",
            diagnostics.len()
        ));
    }

    result
}

// ---------------------------------------------------------------------------
// contentmapper_baseline.go (tsgo#4712)
// ---------------------------------------------------------------------------

// Go: contentmapper_baseline.go:19 contentMapperFormatOpts
fn content_mapper_format_opts() -> FormattingOptions {
    FormattingOptions {
        new_line: "\n".to_string(),
        ..FormattingOptions::default()
    }
}

// Go: contentmapper_baseline.go:27 DoContentMapperBaseline
// DoContentMapperBaseline writes a baseline for content-mapped files that shows the original source, the
// transformed source the compiler actually checks, and the file's diagnostics. Diagnostics are rendered
// with the standard diagnostic writer, which maps each one to the text it belongs to: mapper-produced
// diagnostics render against the original source, compiler diagnostics on mappable code render against the
// original source, and compiler diagnostics on synthesized code render against the transformed source. If
// the program has no content-mapped files, no baseline is written.
// PORT: Go takes the `compiler.ProgramLike`; here it is the compilation
// result that holds the program.
pub fn do_content_mapper_baseline(
    baseline_path: &str,
    program: &CompilationResult,
    diagnostics: &[Diagnostic],
    opts: &Options,
) -> Result<(), String> {
    let content = get_content_mapper_baseline(program, diagnostics);
    if content.is_empty() {
        return Ok(());
    }
    baseline::run(
        &go_regex::replace_ts_extension(baseline_path, ".contentmapper"),
        &content,
        opts,
    )
}

// Go: contentmapper_baseline.go:41 getContentMapperBaseline
fn get_content_mapper_baseline(program: &CompilationResult, diagnostics: &[Diagnostic]) -> String {
    let files = program.content_mapped_source_files();
    if files.is_empty() {
        return String::new();
    }
    let mapped: HashMap<String, Rc<Mapper>> = files
        .iter()
        .map(|(file, mapper)| (source_file_file_name(*file).to_string(), mapper.clone()))
        .collect();

    let mut b = String::new();
    for (file, mapper) in &files {
        // Go `%s` of `file.ScriptKind` and `%v` of the `[]string` extensions.
        let _ = writeln!(
            b,
            "//// [{}] (ScriptKind: {}, ContentMapper: [{}])",
            remove_test_path_prefixes(source_file_file_name(*file), false),
            source_file_info(*file).script_kind,
            mapper.definition.extensions.join(" ")
        );
        b.push_str("--- Original ---\n");
        b.push_str(&ensure_trailing_newline(
            &ts_goport::ast::source_file_original_text(*file),
        ));
        b.push_str("--- Transformed ---\n");
        b.push_str(&ensure_trailing_newline(&source_file_text(*file)));
        b.push('\n');
    }

    let file_diagnostics: Vec<Diagnostic> = diagnostics
        .iter()
        .filter(|d| diagnostic_file_name(d).is_some_and(|name| mapped.contains_key(&name)))
        .cloned()
        .collect();

    b.push_str("=== Diagnostics ===\n\n");
    if file_diagnostics.is_empty() {
        b.push_str(NO_CONTENT);
        b.push('\n');
        return b;
    }
    // Go `FormatDiagnosticsWithColorAndContext` of
    // `ToDiagnostics(WrapASTDiagnostics(fileDiagnostics))`.
    let mut rendered = String::new();
    format_diagnostics_with_color_and_context(
        &mut rendered,
        &file_diagnostics,
        &content_mapper_format_opts(),
    );
    b.push_str(&remove_test_path_prefixes(
        &go_regex::replace_ansi_escapes(&rendered),
        false,
    ));
    b
}

// Go: contentmapper_baseline.go:88 ensureTrailingNewline
fn ensure_trailing_newline(s: &str) -> String {
    if s.is_empty() || s.ends_with('\n') {
        return s.to_string();
    }
    format!("{s}\n")
}

// ---------------------------------------------------------------------------
// js_emit_baseline.go
// ---------------------------------------------------------------------------

// Go: js_emit_baseline.go:20 DoJSEmitBaseline
#[allow(clippy::too_many_arguments)]
pub fn do_js_emit_baseline(
    baseline_path: &str,
    header: &str,
    options: &CompilerOptions,
    result: &CompilationResult,
    ts_config_files: &[TestFile],
    to_be_compiled: &[TestFile],
    other_files: &[TestFile],
    harness_settings: &HarnessOptions,
    opts: &Options,
) -> Result<(), String> {
    let mut checks = Vec::new();
    if !options.no_emit.is_true()
        && !options.emit_declaration_only.is_true()
        && result.js.is_empty()
        && result.diagnostics.is_empty()
    {
        return Err(
            "Expected at least one js file to be emitted or at least one error to be created."
                .to_string(),
        );
    }

    // check js output
    let mut ts_code = String::new();
    let ts_sources: Vec<&TestFile> = other_files.iter().chain(to_be_compiled).collect();
    ts_code.push_str("//// [");
    ts_code.push_str(header);
    ts_code.push_str("] ////\r\n\r\n");

    for (i, file) in ts_sources.iter().enumerate() {
        ts_code.push_str("//// [");
        ts_code.push_str(&get_base_file_name(&file.unit_name));
        ts_code.push_str("]\r\n");
        ts_code.push_str(&file.content);
        if i < ts_sources.len() - 1 {
            ts_code.push_str("\r\n");
        }
    }

    let mut js_code = String::new();
    for file in result.js.values() {
        if !js_code.is_empty() && !js_code.ends_with('\n') {
            js_code.push_str("\r\n");
        }
        if result.diagnostics.is_empty() && file.unit_name.ends_with(EXTENSION_JSON) {
            // ts#64159 (js_emit_baseline.go:59): the name is rooted against the
            // result's current directory; the key is its case-sensitive form.
            let file_name = to_rooted_path(&file.unit_name, &result.current_directory);
            let file_parse_result = parse_source_file(
                &SourceFileParseOptions {
                    path: Path(file_name.clone()),
                    file_name,
                    ..SourceFileParseOptions::default()
                },
                &*Box::leak(file.content.clone().into_boxed_str()),
                ScriptKind::JSON,
            );
            if !file_parse_result.diagnostics.is_empty() {
                js_code.push_str(&get_error_baseline(
                    &mut checks,
                    std::slice::from_ref(file),
                    &file_parse_result.diagnostics,
                    false, /*pretty*/
                ));
                continue;
            }
        }
        js_code.push_str(&file_output(file, harness_settings));
    }

    if !result.dts.is_empty() {
        js_code.push_str("\r\n\r\n");
        for decl_file in result.dts.values() {
            js_code.push_str(&file_output(decl_file, harness_settings));
        }
    }

    let decl_file_context = prepare_declaration_compilation_context(
        to_be_compiled,
        other_files,
        result,
        harness_settings,
        options,
        "", /*currentDirectory*/
    );
    let decl_file_compilation_result =
        compile_declaration_files(decl_file_context, &result.symlinks);

    if let Some(decl_file_compilation_result) = &decl_file_compilation_result
        && !decl_file_compilation_result
            .decl_result
            .diagnostics
            .is_empty()
    {
        js_code.push_str("\r\n\r\n//// [DtsFileErrors]\r\n");
        js_code.push_str("\r\n\r\n");
        let files: Vec<TestFile> = ts_config_files
            .iter()
            .chain(&decl_file_compilation_result.decl_input_files)
            .chain(&decl_file_compilation_result.decl_other_files)
            .cloned()
            .collect();
        let decl_result = &decl_file_compilation_result.decl_result;
        let _scope = decl_result.enter();
        js_code.push_str(&get_error_baseline(
            &mut checks,
            &files,
            &decl_result.diagnostics,
            false, /*pretty*/
        ));
    }

    if !options.no_check.is_true() && !options.no_emit.is_true() {
        let mut test_config = TestConfiguration::new();
        test_config.insert("noCheck".to_string(), "true".to_string());
        let without_checking = result.repeat(&test_config);
        let mut compare_result_file_sets =
            |a: &IndexMap<String, TestFile>, b: &IndexMap<String, TestFile>| {
                for (key, doc) in a {
                    match b.get(key) {
                        None => {
                            js_code.push_str("\r\n\r\n!!!! File ");
                            js_code.push_str(&remove_test_path_prefixes(
                                &doc.unit_name,
                                false, /*retainTrailingDirectorySeparator*/
                            ));
                            js_code.push_str(
                                " missing from original emit, but present in noCheck emit\r\n",
                            );
                            js_code.push_str(&file_output(doc, harness_settings));
                        }
                        Some(original) if original.content != doc.content => {
                            js_code.push_str("\r\n\r\n!!!! File ");
                            js_code.push_str(&remove_test_path_prefixes(
                                &doc.unit_name,
                                false, /*retainTrailingDirectorySeparator*/
                            ));
                            js_code.push_str(" differs from original emit in noCheck emit\r\n");
                            let file_name = if harness_settings.full_emit_paths {
                                remove_test_path_prefixes(
                                    &doc.unit_name,
                                    false, /*retainTrailingDirectorySeparator*/
                                )
                            } else {
                                get_base_file_name(&doc.unit_name)
                            };
                            js_code.push_str("//// [");
                            js_code.push_str(&file_name);
                            js_code.push_str("]\r\n");
                            let expected = &original.content;
                            let actual = &doc.content;
                            js_code.push_str(&baseline::diff_text(
                                "Expected\tThe full check baseline",
                                "Actual\twith noCheck set",
                                expected,
                                actual,
                            ));
                        }
                        Some(_) => {}
                    }
                }
            };
        compare_result_file_sets(&without_checking.dts, &result.dts);
        compare_result_file_sets(&without_checking.js, &result.js);
    }

    let mut baseline_path = baseline_path.to_string();
    if file_extension_is_one_of(&baseline_path, &[EXTENSION_TS, EXTENSION_TSX]) {
        baseline_path = change_extension(&baseline_path, EXTENSION_JS);
    }

    let actual = if !js_code.is_empty() {
        format!("{ts_code}\r\n\r\n{js_code}")
    } else {
        NO_CONTENT.to_string()
    };

    let result = baseline::run(&baseline_path, &actual, opts);
    finish_checks(result, checks)
}

// Go: js_emit_baseline.go:150 fileOutput
fn file_output(file: &TestFile, settings: &HarnessOptions) -> String {
    let file_name = if settings.full_emit_paths {
        remove_test_path_prefixes(
            &file.unit_name,
            false, /*retainTrailingDirectorySeparator*/
        )
    } else {
        get_base_file_name(&file.unit_name)
    };
    format!("//// [{file_name}]\r\n{}", file.content)
}

// Go: js_emit_baseline.go:160 declarationCompilationContext
struct DeclarationCompilationContext {
    decl_input_files: Vec<TestFile>,
    decl_other_files: Vec<TestFile>,
    harness_settings: HarnessOptions,
    options: CompilerOptions,
    current_directory: String,
    config: Rc<ParsedCommandLine>,
}

// Go: js_emit_baseline.go:169 prepareDeclarationCompilationContext
fn prepare_declaration_compilation_context(
    input_files: &[TestFile],
    other_files: &[TestFile],
    result: &CompilationResult,
    harness_settings: &HarnessOptions,
    options: &CompilerOptions,
    // Current directory is needed for rwcRunner to be able to use currentDirectory defined in json file
    current_directory: &str,
) -> Option<DeclarationCompilationContext> {
    if options.declaration.is_true() && result.diagnostics.is_empty() {
        if options.emit_declaration_only.is_true() {
            if !result.js.is_empty() {
                panic!("Only declaration files should be generated when emitDeclarationOnly:true");
            }
            if result.dts.is_empty() && !options.no_emit.is_true() {
                panic!(
                    "Expected at least one declaration file to be emitted when emitDeclarationOnly:true and no errors were generated"
                );
            }
        } else if !result.has_content_mapped_source_files()
            && result.dts.len() != result.get_number_of_js_files(false /*includeJson*/)
        {
            panic!(
                "There were no errors and declFiles generated did not match number of js files generated"
            );
        }
    }

    let mut decl_input_files: Vec<TestFile> = Vec::new();
    let mut decl_other_files: Vec<TestFile> = Vec::new();

    let find_unit = |file_name: &str, units: &[TestFile]| -> bool {
        units.iter().any(|unit| unit.unit_name == file_name)
    };

    let find_result_code_file = |file_name: &str| -> Option<TestFile> {
        let Some(mut source_file_name) =
            result.source_file_name(&to_rooted_path(file_name, &result.current_directory))
        else {
            panic!("Program has no source file with name '{file_name}'");
        };
        // Is this file going to be emitted separately
        // ts#64159 (js_emit_baseline.go:212): the path relative to the
        // common source directory goes under `OutDir`; across roots (R4) the
        // file keeps its name. N cut the common source directory text out of
        // the name (`strings.Replace`).
        if !options.out_dir.is_empty()
            && let Some(relative_path) = relative_path_from_directory(
                &result.common_source_directory(),
                &source_file_name,
                result.use_case_sensitive_file_names,
            )
        {
            // Go `options.OutDir.ResolveRelativeFile(relativePath)`.
            source_file_name = if relative_path.is_empty() {
                options.out_dir.clone()
            } else {
                to_rooted_path(&relative_path, &options.out_dir)
            };
        }

        let d_ts_file_name = result.change_to_declaration_extension(&source_file_name);
        result.dts.get(&d_ts_file_name).cloned()
    };

    let add_dts_file = |file: &TestFile,
                        dts_files: &mut Vec<TestFile>,
                        decl_input_files: &[TestFile],
                        decl_other_files: &[TestFile]| {
        if is_declaration_file_name(&file.unit_name) || has_json_file_extension(&file.unit_name) {
            dts_files.push(file.clone());
        } else if let Some(content_mapper) = result
            .source_file_content_mapper(&to_rooted_path(&file.unit_name, &result.current_directory))
            && (has_ts_file_extension(&file.unit_name)
                || (has_js_file_extension(&file.unit_name) && options.get_allow_js())
                || !content_mapper.is_empty())
        {
            let decl_file = find_result_code_file(&file.unit_name);
            if let Some(decl_file) = decl_file
                && !find_unit(&decl_file.unit_name, decl_input_files)
                && !find_unit(&decl_file.unit_name, decl_other_files)
            {
                dts_files.push(TestFile {
                    unit_name: decl_file.unit_name.clone(),
                    content: decl_file
                        .content
                        .strip_prefix('\u{FEFF}')
                        .unwrap_or(&decl_file.content)
                        .to_string(),
                });
            }
        }
    };

    // if the .d.ts is non-empty, confirm it compiles correctly as well
    if options.declaration.is_true() && result.diagnostics.is_empty() && !result.dts.is_empty() {
        // ts#64159 (js_emit_baseline.go:243): a given current directory is
        // rooted against the harness one.
        let declaration_current_directory = if current_directory.is_empty() {
            harness_settings.current_directory.clone()
        } else {
            to_rooted_path(current_directory, &harness_settings.current_directory)
        };
        for file in input_files {
            let mut dts_files = std::mem::take(&mut decl_input_files);
            let current = dts_files.clone();
            add_dts_file(file, &mut dts_files, &current, &decl_other_files);
            decl_input_files = dts_files;
        }
        for file in other_files {
            let mut dts_files = std::mem::take(&mut decl_other_files);
            let current = dts_files.clone();
            add_dts_file(file, &mut dts_files, &decl_input_files, &current);
            decl_other_files = dts_files;
        }
        return Some(DeclarationCompilationContext {
            decl_input_files,
            decl_other_files,
            harness_settings: harness_settings.clone(),
            options: options.clone(),
            current_directory: declaration_current_directory,
            config: result.command_line.clone(),
        });
    }
    None
}

// Go: js_emit_baseline.go:265 declarationCompilationResult
struct DeclarationCompilationResult {
    decl_input_files: Vec<TestFile>,
    decl_other_files: Vec<TestFile>,
    decl_result: CompilationResult,
}

// Go: js_emit_baseline.go:271 compileDeclarationFiles
fn compile_declaration_files(
    context: Option<DeclarationCompilationContext>,
    symlinks: &BTreeMap<String, String>,
) -> Option<DeclarationCompilationResult> {
    let context = context?;
    let tsconfig =
        context
            .config
            .config_file
            .clone()
            .map(|config_file| super::harness::TsConfigPart {
                config_file: Some(config_file),
                // Go (js_emit_baseline.go:277) gives this command line no
                // base directory, so the compilation's base directory is
                // `context.currentDirectory` (ts#64159).
                base_directory: String::new(),
                errors: Vec::new(),
                content_mappers: context.config.content_mappers().to_vec(),
            });
    let decl_file_compilation_result = compile_files_ex(
        &context.decl_input_files,
        &context.decl_other_files,
        &context.harness_settings,
        &context.options,
        &context.current_directory,
        symlinks,
        tsconfig.as_ref(),
    );
    Some(DeclarationCompilationResult {
        decl_input_files: context.decl_input_files,
        decl_other_files: context.decl_other_files,
        decl_result: decl_file_compilation_result,
    })
}

// ---------------------------------------------------------------------------
// sourcemap_baseline.go
// ---------------------------------------------------------------------------

// Go: sourcemap_baseline.go:18 DoSourcemapBaseline
pub fn do_sourcemap_baseline(
    baseline_path: &str,
    _header: &str,
    options: &CompilerOptions,
    result: &CompilationResult,
    harness_settings: &HarnessOptions,
    opts: &Options,
) -> Result<(), String> {
    let decl_maps = options.get_are_declaration_maps_enabled();
    if options.inline_source_map.is_true() {
        if !result.maps.is_empty() && !decl_maps {
            return Err(
                "No sourcemap files should be generated if inlineSourceMaps was set.".into(),
            );
        }
        return Ok(());
    } else if options.source_map.is_true() || decl_maps {
        let mut expected_map_count = 0;
        if options.source_map.is_true() {
            expected_map_count += result.get_number_of_js_files(false /*includeJSON*/);
        }
        if decl_maps {
            expected_map_count += result.dts.len();
        }
        if result.maps.len() != expected_map_count {
            return Err("Number of sourcemap files should be same as js files.".into());
        }

        let source_map_code = if options.no_emit_on_error.is_true()
            && !result.diagnostics.is_empty()
            || result.maps.is_empty()
        {
            NO_CONTENT.to_string()
        } else {
            let mut source_map_code_builder = String::new();
            for source_map in result.maps.values() {
                if !source_map_code_builder.is_empty() {
                    source_map_code_builder.push_str("\r\n");
                }
                source_map_code_builder.push_str(&file_output(source_map, harness_settings));
                if !options.inline_source_map.is_true() {
                    source_map_code_builder
                        .push_str(&create_source_map_preview_link(source_map, result));
                }
            }
            source_map_code_builder
        };

        let mut baseline_path = baseline_path.to_string();
        if file_extension_is_one_of(&baseline_path, &[EXTENSION_TS, EXTENSION_TSX]) {
            baseline_path = change_extension(&baseline_path, &format!("{EXTENSION_JS}.map"));
        }

        return baseline::run(&baseline_path, &source_map_code, opts);
    }
    Ok(())
}

// Go: sourcemap_baseline.go:70 createSourceMapPreviewLink
fn create_source_map_preview_link(source_map: &TestFile, result: &CompilationResult) -> String {
    let mut sourcemap_json = RawSourceMap::default();
    if let Err(err) = json_unmarshal(
        &go_string_bytes(&source_map.content),
        &mut sourcemap_json,
        &[],
    ) {
        panic!("{err:?}");
    }

    let Some(output_js_file) = result
        .outputs()
        .iter()
        .find(|td| td.unit_name.ends_with(&sourcemap_json.file))
    else {
        return String::new();
    };

    // !!! Strada uses a fallible approach to associating inputs and outputs derived from a source map output. The
    // !!! commented logic below should be used after the Strada migration is complete:

    let source_tds: Vec<Option<TestFile>> = sourcemap_json
        .sources
        .iter()
        .map(|s| {
            let source_file = result
                .inputs()
                .iter()
                .find(|td| td.unit_name.ends_with(s.as_str()));
            if let Some(source_file) = source_file {
                // Go: `result.Program.GetSourceFile(...)` (ts#63936), with the
                // unit name rooted against the current directory (ts#64159,
                // sourcemap_baseline.go:99).
                let _scope = ts_goport::core::enter_program(Some(result.program));
                let program_source = ts_goport::program::get_source_file(&to_rooted_path(
                    &source_file.unit_name,
                    &result.current_directory,
                ));
                if program_source.is_some() {
                    return Some(TestFile {
                        unit_name: source_file.unit_name.clone(),
                        content: ts_goport::ast::source_file_original_text(program_source)
                            .to_string(),
                    });
                }
            }
            source_file.cloned()
        })
        .collect();
    if source_tds.iter().any(Option::is_none) {
        return String::new();
    }

    let mut hash = String::new();
    hash.push_str("\n//// https://sokra.github.io/source-map-visualization#base64,");
    hash.push_str(&base64_encode_chunk(&output_js_file.content));
    hash.push(',');
    hash.push_str(&base64_encode_chunk(&source_map.content));
    for td in source_tds.into_iter().flatten() {
        hash.push(',');
        hash.push_str(&base64_encode_chunk(&td.content));
    }
    hash.push('\n');
    hash
}

// Go: sourcemap_baseline.go:123 base64EncodeChunk
// PORT: Go `url.QueryUnescape(url.QueryEscape(s))` gives `s` back, so this
// encodes the Go bytes of `s`.
fn base64_encode_chunk(s: &str) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = go_string_bytes(s);
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[(n >> 6) as usize & 63] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[n as usize & 63] as char);
        } else {
            out.push('=');
        }
    }
    out
}

// ---------------------------------------------------------------------------
// sourcemap_record_baseline.go
// ---------------------------------------------------------------------------

// Go: sourcemap_record_baseline.go:12 DoSourcemapRecordBaseline
pub fn do_sourcemap_record_baseline(
    baseline_path: &str,
    _header: &str,
    options: &CompilerOptions,
    result: &CompilationResult,
    _harness_settings: &HarnessOptions,
    opts: &Options,
) -> Result<(), String> {
    let mut actual = NO_CONTENT.to_string();
    if options.source_map.is_true()
        || options.inline_source_map.is_true()
        || options.declaration_map.is_true()
    {
        let record = remove_test_path_prefixes(
            &result.get_source_map_record(),
            false, /*retainTrailingDirectorySeparator*/
        );
        if !(options.no_emit_on_error.is_true() && !result.diagnostics.is_empty())
            && !record.is_empty()
        {
            actual = record;
        }
    }

    let mut baseline_path = baseline_path.to_string();
    if file_extension_is_one_of(&baseline_path, &[EXTENSION_TS, EXTENSION_TSX]) {
        baseline_path = change_extension(&baseline_path, ".sourcemap.txt");
    }

    baseline::run(&baseline_path, &actual, opts)
}

// ---------------------------------------------------------------------------
// module_resolution_baseline.go
// ---------------------------------------------------------------------------

// Go: module_resolution_baseline.go:9 DoModuleResolutionBaseline
pub fn do_module_resolution_baseline(
    baseline_path: &str,
    trace: &str,
    opts: &Options,
) -> Result<(), String> {
    let baseline_path = go_regex::replace_ts_extension(baseline_path, ".trace.json");
    let error_baseline = if !trace.is_empty() {
        trace.to_string()
    } else {
        NO_CONTENT.to_string()
    };
    baseline::run(&baseline_path, &error_baseline, opts)
}

// ---------------------------------------------------------------------------
// type_symbol_baseline.go
// ---------------------------------------------------------------------------

/// The results of `DoTypeAndSymbolBaseline`: the `type` and the `symbol`
/// subtests.
pub struct TypeAndSymbolResults {
    pub types: Result<(), String>,
    pub symbols: Result<(), String>,
}

// Go: type_symbol_baseline.go:30 DoTypeAndSymbolBaseline
// PORT: the program is the current one (see `CompilationResult::enter`).
// A panic in one subtest is caught by `catch`, as Go `RecoverAndFail`.
pub fn do_type_and_symbol_baseline(
    baseline_path: &str,
    header: &str,
    all_files: &[TestFile],
    opts: &Options,
    has_error_baseline: bool,
    catch: impl Fn(&str, &mut dyn FnMut() -> Result<(), String>) -> Result<(), String>,
) -> TypeAndSymbolResults {
    // The full walker simulates the types that you would get from doing a full
    // compile.  The pull walker simulates the types you get when you just do
    // a type query for a random node (like how the LS would do it).  Most of the
    // time, these will be the same.  However, occasionally, they can be different.
    // Specifically, when the compiler internally depends on symbol IDs to order
    // things, then we may see different results because symbols can be created in a
    // different order with 'pull' operations, and thus can produce slightly differing
    // output.
    //
    // For example, with a full type check, we may see a type displayed as: number | string
    // But with a pull type check, we may see it as:                        string | number
    //
    // These types are equivalent, but depend on what order the compiler observed
    // certain parts of the program.

    let full_walker = std::cell::RefCell::new(new_type_writer_walker(has_error_baseline));

    let types = catch(
        &format!("Panic on creating type baseline for test {header}"),
        &mut || {
            // !!! Remove once the type baselines print node reuse lines
            let mut types_opts = opts.clone();
            types_opts.diff_fixup_old = Some(Arc::new(types_diff_fixup_old));

            check_baselines(
                baseline_path,
                all_files,
                &mut full_walker.borrow_mut(),
                header,
                &types_opts,
                false, /*isSymbolBaseline*/
            )
        },
    );
    let symbols = catch(
        &format!("Panic on creating symbol baseline for test {header}"),
        &mut || {
            check_baselines(
                baseline_path,
                all_files,
                &mut full_walker.borrow_mut(),
                header,
                opts,
                true, /*isSymbolBaseline*/
            )
        },
    );
    TypeAndSymbolResults { types, symbols }
}

/// The `DiffFixupOld` of the `type` subtest.
fn types_diff_fixup_old(s: &str) -> String {
    let mut sb = String::with_capacity(s.len());

    let mut perf_stats = false;
    for line in s.split('\n') {
        if is_type_baseline_node_reuse_line(line) {
            continue;
        }

        if !perf_stats && line.starts_with("=== Performance Stats ===") {
            perf_stats = true;
            continue;
        } else if perf_stats {
            if line.starts_with("=== ") {
                perf_stats = false;
            } else {
                continue;
            }
        }

        const RELATIVE_PREFIX_NEW: &str = "=== ";
        const RELATIVE_PREFIX_OLD: &str = "=== ./";
        let fixed;
        let line = if let Some(rest) = line.strip_prefix(RELATIVE_PREFIX_OLD) {
            fixed = format!("{RELATIVE_PREFIX_NEW}{rest}");
            fixed.as_str()
        } else {
            line
        };

        sb.push_str(line);
        sb.push('\n');
    }

    sb.truncate(sb.len().saturating_sub(1));
    sb
}

// Go: type_symbol_baseline.go:107 isTypeBaselineNodeReuseLine
fn is_type_baseline_node_reuse_line(line: &str) -> bool {
    let Some(line) = line.strip_prefix('>') else {
        return false;
    };
    // Go `line[1:]` panics on an empty rest. A slice inside a char starts
    // with a continuation byte, so the checks below fail on it.
    if line.is_empty() {
        panic!("runtime error: slice bounds out of range [1:0]");
    }
    let Some(rest) = line.get(1..) else {
        return false;
    };
    let line = rest.trim_start_matches(' ');
    let Some(line) = line.strip_prefix(':') else {
        return false;
    };

    line.chars().all(|c| matches!(c, ' ' | '^' | '\r'))
}

// Go: type_symbol_baseline.go:129 checkBaselines
fn check_baselines(
    baseline_path: &str,
    all_files: &[TestFile],
    full_walker: &mut ts_goport::baseline::type_symbol::TypeWriterWalker,
    header: &str,
    opts: &Options,
    is_symbol_baseline: bool,
) -> Result<(), String> {
    let full_extension = if is_symbol_baseline {
        ".symbols"
    } else {
        ".types"
    };
    let output_file_name = go_regex::replace_ts_extension(baseline_path, full_extension);
    let full_baseline = generate_baseline(all_files, full_walker, header, is_symbol_baseline);
    baseline::run(&output_file_name, &full_baseline, opts)
}

/// The Go `DiffFixupOld` of the `error` subtest (compiler_runner.go:350).
pub fn error_diff_fixup_old(old: &str) -> String {
    let mut sb = String::with_capacity(old.len());

    for line in old.split('\n') {
        const RELATIVE_PREFIX_NEW: &str = "==== ";
        const RELATIVE_PREFIX_OLD: &str = "==== ./";
        if let Some(rest) = line.strip_prefix(RELATIVE_PREFIX_OLD) {
            sb.push_str(RELATIVE_PREFIX_NEW);
            sb.push_str(rest);
        } else {
            sb.push_str(line);
        }
        sb.push('\n');
    }

    sb.truncate(sb.len().saturating_sub(1));
    sb
}

use crate::frontend::prelude::*;

// This file ports tsoptions/commandlineparser.go.
// PORT: Go `any` values are `CompilerOptionsValue`. Go
// `*collections.OrderedMap[string, any]` is `IndexMap<String,
// CompilerOptionsValue>`. Go nil `*ast.Node` and `*ast.SourceFile` are
// `Node::NIL`.

impl CommandLineParser {
    // Go: tsoptions/commandlineparser.go:16 (*commandLineParser).AlternateMode
    pub fn alternate_mode(&self) -> Option<&AlternateModeDiagnostics> {
        self.worker_diagnostics.did_you_mean.alternate_mode.as_ref()
    }

    // Go: tsoptions/commandlineparser.go:20 (*commandLineParser).OptionsDeclarations
    pub fn options_declarations(&self) -> &'static [&'static CommandLineOption] {
        self.worker_diagnostics.did_you_mean.option_declarations
    }

    // Go: tsoptions/commandlineparser.go:24 (*commandLineParser).UnknownOptionDiagnostic
    pub fn unknown_option_diagnostic(&self) -> &'static Message {
        self.worker_diagnostics
            .did_you_mean
            .unknown_option_diagnostic
    }

    // Go: tsoptions/commandlineparser.go:28 (*commandLineParser).UnknownDidYouMeanDiagnostic
    pub fn unknown_did_you_mean_diagnostic(&self) -> &'static Message {
        self.worker_diagnostics
            .did_you_mean
            .unknown_did_you_mean_diagnostic
    }
}

// Go: tsoptions/commandlineparser.go:32 commandLineParser
// PORT: the Go `fs vfs.FS` field is not stored. errors.rs has `impl
// CommandLineParser` with no lifetime, so the parser cannot hold the
// borrowed `&dyn Fs` that `ParseConfigHost::fs` returns. The file system is
// passed to `parse_strings` and `parse_response_file` instead. The value is
// the same for the whole parse, as in Go.
pub struct CommandLineParser {
    pub worker_diagnostics: &'static ParseCommandLineWorkerDiagnostics,
    pub options_map: NameMap,
    pub current_directory: String,
    pub options: IndexMap<String, CompilerOptionsValue>,
    pub file_names: Vec<String>,
    pub errors: Vec<Diagnostic>,
    pub response_file_stack: FxHashSet<Path>,
}

// Go: tsoptions/commandlineparser.go:43 ParseCommandLine
// PORT: the plan names the entry point `parse_command_line(args, fs)`; Go
// takes a `ParseConfigHost`, so this does too.
pub fn parse_command_line(
    command_line: &[String],
    host: &dyn ParseConfigHost,
) -> ParsedCommandLine {
    let fs = host.fs();
    let parser = parse_command_line_worker(
        &COMPILER_OPTIONS_DID_YOU_MEAN_DIAGNOSTICS,
        command_line,
        Some(&*fs),
        host.get_current_directory(),
    );
    let options = convert_to_options_with_absolute_paths(
        Some(parser.options.clone()),
        &COMMAND_LINE_COMPILER_OPTIONS_MAP,
        &host.get_current_directory(),
    )
    .unwrap_or_default();
    let mut compiler_options = CompilerOptions::default();
    convert_map_to_options(
        &options,
        CompilerOptionsParser {
            compiler_options: &mut compiler_options,
        },
    );
    let mut result = new_parsed_command_line(
        Rc::new(compiler_options),
        parser.file_names,
        None,
        ComparePathsOptions {
            use_case_sensitive_file_names: host.fs().use_case_sensitive_file_names(),
            current_directory: host.get_current_directory(),
        },
    );
    result.errors = parser.errors;
    result.raw = CompilerOptionsValue::Map(parser.options);
    result
}

// Go: tsoptions/commandlineparser.go:64 ParseBuildCommandLine
// PORT: ported with the other build mode types in
// `execute/build/command_line.rs` (`parse_build_command_line`).

// Go: tsoptions/commandlineparser.go:112 parseCommandLineWorker
// PORT: `fs` is passed down instead of stored (see `CommandLineParser`).
// Go nil `vfs.FS` is `None`.
pub fn parse_command_line_worker(
    parse_command_line_with_diagnostics: &'static ParseCommandLineWorkerDiagnostics,
    command_line: &[String],
    fs: Option<&dyn Fs>,
    current_directory: String,
) -> CommandLineParser {
    let mut parser = CommandLineParser {
        current_directory,
        worker_diagnostics: parse_command_line_with_diagnostics,
        file_names: Vec::new(),
        options: IndexMap::default(),
        errors: Vec::new(),
        options_map: NameMap::default(),
        response_file_stack: FxHashSet::default(),
    };
    parser.options_map = get_name_map_from_list(parser.options_declarations());
    parser.parse_strings(command_line, fs);
    parser
}

impl CommandLineParser {
    // Go: tsoptions/commandlineparser.go:131 (*commandLineParser).parseStrings
    pub fn parse_strings(&mut self, args: &[String], fs: Option<&dyn Fs>) {
        let mut i = 0usize;
        while i < args.len() {
            let s = &args[i];
            i += 1;
            if s.is_empty() {
                continue;
            }
            match s.as_bytes()[0] {
                b'@' => self.parse_response_file(&s[1..], fs),
                b'-' => {
                    let input_option_name = get_input_option_name(s);
                    let opt = self.options_map.get_option_declaration_from_name(
                        input_option_name,
                        true, /*allowShort*/
                    );
                    if let Some(opt) = opt {
                        i = self.parse_option_value(
                            args,
                            i,
                            opt,
                            self.worker_diagnostics.option_type_mismatch_diagnostic,
                        );
                    } else {
                        // ts#64457: the watch options are gone, so a name that
                        // is not an option of this parser is unknown (TS5023,
                        // or TS5072 for --build).
                        let err = self.create_unknown_option_error(
                            input_option_name,
                            s,
                            Node::NIL,
                            Node::NIL,
                        );
                        self.errors.push(err);
                    }
                }
                _ => self.file_names.push(s.clone()),
            }
        }
    }
}

// Go: tsoptions/commandlineparser.go:156 getInputOptionName
pub fn get_input_option_name(input: &str) -> &str {
    // removes at most two leading '-' from the input string
    let input = input.strip_prefix('-').unwrap_or(input);
    input.strip_prefix('-').unwrap_or(input)
}

impl CommandLineParser {
    // Go: tsoptions/commandlineparser.go:161 (*commandLineParser).parseResponseFile
    // PORT: Go `defer p.responseFileStack.Delete(path)` is a `remove` before
    // each return. Go calls `p.fs.UseCaseSensitiveFileNames()` on a nil `fs`
    // and panics; so does this.
    pub fn parse_response_file(&mut self, file_name: &str, fs: Option<&dyn Fs>) {
        let file_name = get_normalized_absolute_path(file_name, &self.current_directory);
        let path = to_path(
            &file_name,
            &self.current_directory,
            fs.expect("nil pointer dereference: fs")
                .use_case_sensitive_file_names(),
        );
        if self.response_file_stack.contains(&path) {
            return;
        }
        self.response_file_stack.insert(path.clone());

        let (file_contents, errors) = try_read_file(
            &file_name,
            &mut |file_name: &str| {
                let Some(fs) = fs else {
                    return (String::new(), false);
                };
                fs.read_file(file_name)
            },
            std::mem::take(&mut self.errors),
        );
        self.errors = errors;

        if file_contents.is_empty() {
            self.response_file_stack.remove(&path);
            return;
        }

        let mut args: Vec<String> = Vec::new();
        let text: Vec<char> = file_contents.chars().collect();
        let text_length = text.len();
        let mut pos = 0usize;
        while pos < text_length {
            while pos < text_length && text[pos] <= ' ' {
                pos += 1;
            }
            if pos >= text_length {
                break;
            }
            let start = pos;
            if text[pos] == '"' {
                pos += 1;
                while pos < text_length && text[pos] != '"' {
                    pos += 1;
                }
                if pos < text_length {
                    args.push(text[start + 1..pos].iter().collect());
                    pos += 1;
                } else {
                    self.errors.push(new_compiler_diagnostic(
                        diag::Unterminated_quoted_string_in_response_file_0,
                        args![file_name],
                    ));
                }
            } else {
                while pos < text_length && text[pos] > ' ' {
                    pos += 1;
                }
                args.push(text[start..pos].iter().collect());
            }
        }
        self.parse_strings(&args, fs);
        self.response_file_stack.remove(&path);
    }
}

// Go: tsoptions/commandlineparser.go:219 tryReadFile
pub fn try_read_file(
    file_name: &str,
    read_file: &mut dyn FnMut(&str) -> (String, bool),
    mut errors: Vec<Diagnostic>,
) -> (String, Vec<Diagnostic>) {
    // this function adds a compiler diagnostic if the file cannot be read
    let (mut text, e) = read_file(file_name);

    if !e {
        // !!! Divergence: the returned error will not give a useful message
        // errors = append(errors, ast.NewCompilerDiagnostic(diagnostics.Cannot_read_file_0_Colon_1, *e));
        text = String::new();
        errors.push(new_compiler_diagnostic(
            diag::Cannot_read_file_0,
            args![file_name],
        ));
    }
    (text, errors)
}

impl CommandLineParser {
    // Go: tsoptions/commandlineparser.go:232 (*commandLineParser).parseOptionValue
    // PORT: the Go parameter `diag` is `diag_message`, because `diag` is the
    // diagnostics module in Rust.
    pub fn parse_option_value(
        &mut self,
        args: &[String],
        mut i: usize,
        opt: &'static CommandLineOption,
        diag_message: &'static Message,
    ) -> usize {
        if opt.is_ts_config_only && i <= args.len() {
            let opt_value = if i < args.len() { args[i].as_str() } else { "" };
            if opt_value == "null" {
                self.options
                    .insert(opt.name.to_string(), CompilerOptionsValue::Nil);
                i += 1;
            } else if opt.kind == CommandLineOptionKind::BOOLEAN {
                if opt_value == "false" {
                    self.options
                        .insert(opt.name.to_string(), CompilerOptionsValue::Bool(false));
                    i += 1;
                } else {
                    if opt_value == "true" {
                        i += 1;
                    }
                    self.errors.push(new_compiler_diagnostic(
                        diag::Option_0_can_only_be_specified_in_tsconfig_json_file_or_set_to_false_or_null_on_command_line,
                        args![opt.name],
                    ));
                }
            } else {
                self.errors.push(new_compiler_diagnostic(
                    diag::Option_0_can_only_be_specified_in_tsconfig_json_file_or_set_to_null_on_command_line,
                    args![opt.name],
                ));
                if !opt_value.is_empty() && !opt_value.starts_with('-') {
                    i += 1;
                }
            }
        } else {
            // Check to see if no argument was provided (e.g. "--locale" is the last command-line argument).
            if i >= args.len() {
                if opt.kind != CommandLineOptionKind::BOOLEAN {
                    self.errors.push(new_compiler_diagnostic(
                        diag_message,
                        args![opt.name, get_compiler_option_value_type_string(opt)],
                    ));
                    if opt.kind == CommandLineOptionKind::LIST {
                        self.options.insert(
                            opt.name.to_string(),
                            CompilerOptionsValue::StringList(Vec::new()),
                        );
                    } else if opt.kind == CommandLineOptionKind::ENUM {
                        self.errors.push(create_diagnostic_for_invalid_enum_type(
                            opt,
                            Node::NIL,
                            Node::NIL,
                        ));
                    }
                } else {
                    self.options
                        .insert(opt.name.to_string(), CompilerOptionsValue::Bool(true));
                }
                return i;
            }
            if args[i] != "null" {
                match opt.kind {
                    CommandLineOptionKind::NUMBER => {
                        // !!! Make sure this parseInt matches JS parseInt
                        // PORT: Go `strconv.Atoi` parses a 64-bit int with an
                        // optional sign; Rust `i64` parsing accepts the same
                        // text, and `Int` holds the whole Go `int`.
                        match args[i].parse::<i64>() {
                            Ok(num) => {
                                if num >= i64::from(opt.min_value) {
                                    self.options.insert(
                                        opt.name.to_string(),
                                        CompilerOptionsValue::Int(num),
                                    );
                                } else {
                                    self.errors.push(new_compiler_diagnostic(
                                        diag::Option_0_requires_value_to_be_greater_than_1,
                                        args![opt.name, opt.min_value],
                                    ));
                                }
                            }
                            Err(_) => {
                                self.errors.push(new_compiler_diagnostic(
                                    diag_message,
                                    args![opt.name, "number"],
                                ));
                            }
                        }
                        i += 1;
                    }
                    CommandLineOptionKind::BOOLEAN => {
                        // boolean flag has optional value true, false, others
                        let opt_value = args[i].as_str();

                        // check next argument as boolean flag value
                        if opt_value == "false" {
                            self.options
                                .insert(opt.name.to_string(), CompilerOptionsValue::Bool(false));
                        } else {
                            self.options
                                .insert(opt.name.to_string(), CompilerOptionsValue::Bool(true));
                        }
                        // try to consume next argument as value for boolean flag; do not consume argument if it is not "true" or "false"
                        if opt_value == "false" || opt_value == "true" {
                            i += 1;
                        }
                    }
                    CommandLineOptionKind::STRING => {
                        let (val, err) = validate_json_option_value(
                            opt,
                            CompilerOptionsValue::String(args[i].clone()),
                            Node::NIL,
                            Node::NIL,
                        );
                        // PORT: Go tests `err == nil`. validateJsonOptionValue
                        // returns nil errors or a non-empty slice, so that is
                        // `is_empty`.
                        if err.is_empty() {
                            self.options.insert(opt.name.to_string(), val);
                        } else {
                            self.errors.extend(err);
                        }
                        i += 1;
                    }
                    CommandLineOptionKind::LIST => {
                        let (result, err) = self.parse_list_type_option(opt, &args[i]);
                        let consumed = result
                            .as_any_slice()
                            .is_some_and(|result| !result.is_empty())
                            || !err.is_empty();
                        self.options.insert(opt.name.to_string(), result);
                        self.errors.extend(err);
                        if consumed {
                            i += 1;
                        }
                    }
                    CommandLineOptionKind::LIST_OR_ELEMENT => {
                        // If not a primitive, the possible types are specified in what is effectively a map of options.
                        panic!("listOrElement not supported here");
                    }
                    _ => {
                        let (val, err) = convert_json_option_of_enum_type(
                            opt,
                            args[i].trim_matches(is_white_space_like),
                            Node::NIL,
                            Node::NIL,
                        );
                        self.options.insert(opt.name.to_string(), val);
                        self.errors.extend(err);
                        i += 1;
                    }
                }
            } else {
                self.options
                    .insert(opt.name.to_string(), CompilerOptionsValue::Nil);
                i += 1;
            }
        }
        i
    }

    // Go: tsoptions/commandlineparser.go:338 (*commandLineParser).parseListTypeOption
    pub fn parse_list_type_option(
        &self,
        opt: &'static CommandLineOption,
        value: &str,
    ) -> (CompilerOptionsValue, Vec<Diagnostic>) {
        parse_list_type_option(opt, value)
    }
}

// Go: tsoptions/commandlineparser.go:342 ParseListTypeOption
// PORT: Go returns `[]any`: a `List`, or a `NilList` when `core.MapFiltered`
// keeps no element (for example `--types ,`), so the option stays unset.
pub fn parse_list_type_option(
    opt: &'static CommandLineOption,
    value: &str,
) -> (CompilerOptionsValue, Vec<Diagnostic>) {
    let value = value.trim();
    let mut errors: Vec<Diagnostic> = Vec::new();
    if value.starts_with('-') {
        return (CompilerOptionsValue::List(Vec::new()), errors);
    }
    if opt.kind == CommandLineOptionKind::LIST_OR_ELEMENT && !value.contains(',') {
        let (val, err) = validate_json_option_value(
            opt,
            CompilerOptionsValue::String(value.to_string()),
            Node::NIL,
            Node::NIL,
        );
        if !err.is_empty() {
            return (CompilerOptionsValue::List(Vec::new()), err);
        }
        // PORT: Go `val.(string)` panics when the value is not a string.
        let CompilerOptionsValue::String(s) = val else {
            panic!("interface conversion: interface {{}} is not string");
        };
        return (
            CompilerOptionsValue::List(vec![CompilerOptionsValue::String(s)]),
            errors,
        );
    }
    if value.is_empty() {
        return (CompilerOptionsValue::List(Vec::new()), errors);
    }
    let values: Vec<&str> = value.split(',').collect();
    // PORT: Go `opt.Elements()` is non-nil for list options; a nil value
    // would panic on `.Kind`, as `expect` does here.
    let elements_opt = opt.elements().expect("list option has elements");
    match elements_opt.kind {
        CommandLineOptionKind::STRING => {
            let mut elements: Vec<CompilerOptionsValue> = Vec::new();
            for v in values {
                let (val, err) = validate_json_option_value(
                    elements_opt,
                    CompilerOptionsValue::String(v.to_string()),
                    Node::NIL,
                    Node::NIL,
                );
                if let CompilerOptionsValue::String(s) = &val
                    && err.is_empty()
                    && !s.is_empty()
                {
                    elements.push(val);
                    continue;
                }
                errors.extend(err);
            }
            (map_filtered_list(elements), errors)
        }
        CommandLineOptionKind::BOOLEAN
        | CommandLineOptionKind::OBJECT
        | CommandLineOptionKind::NUMBER => {
            // do nothing: only string and enum/object types currently allowed as list entries
            // 				!!! we don't actually have number list options, so I didn't implement number list parsing
            panic!("List of {} is not yet supported.", elements_opt.kind.0);
        }
        _ => {
            let mut result: Vec<CompilerOptionsValue> = Vec::new();
            for v in values {
                let (val, err) = convert_json_option_of_enum_type(
                    elements_opt,
                    v.trim_matches(is_white_space_like),
                    Node::NIL,
                    Node::NIL,
                );
                if let CompilerOptionsValue::String(s) = &val
                    && err.is_empty()
                    && !s.is_empty()
                {
                    result.push(val);
                    continue;
                }
                errors.extend(err);
            }
            (map_filtered_list(result), errors)
        }
    }
}

/// Go `core.MapFiltered` returns a nil slice when it keeps no element.
fn map_filtered_list(values: Vec<CompilerOptionsValue>) -> CompilerOptionsValue {
    if values.is_empty() {
        CompilerOptionsValue::NilList
    } else {
        CompilerOptionsValue::List(values)
    }
}

// Go: tsoptions/commandlineparser.go:387 convertJsonOptionOfEnumType
pub fn convert_json_option_of_enum_type(
    opt: &CommandLineOption,
    value: &str,
    value_expression: Node,
    source_file: Node,
) -> (CompilerOptionsValue, Vec<Diagnostic>) {
    if value.is_empty() {
        return (CompilerOptionsValue::Nil, Vec::new());
    }
    // PORT: Go `strings.ToLower` uses the simple (one rune) mapping. The
    // first rune of Rust `to_lowercase` is that mapping (U+0130 is the only
    // multi-rune case, and it starts with 'i').
    let key: String = value
        .chars()
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect();
    let Some(type_map) = opt.enum_map() else {
        return (CompilerOptionsValue::Nil, Vec::new());
    };
    if let Some(val) = type_map.get(&key) {
        return validate_json_option_value(opt, val.clone(), value_expression, source_file);
    }
    (
        CompilerOptionsValue::Nil,
        vec![create_diagnostic_for_invalid_enum_type(
            opt,
            source_file,
            value_expression,
        )],
    )
}

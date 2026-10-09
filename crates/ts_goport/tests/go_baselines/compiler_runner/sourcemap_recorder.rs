//! Go: internal/testutil/harnessutil/sourcemap_recorder.go, and
//! `CompilationResult.GetSourceMapRecord` (harnessutil.go:909).

use ts_goport::api::to_rooted_path;
use ts_goport::baseline::type_symbol::TestFile;
use ts_goport::frontend::json::append_json_quote;
use ts_goport::frontend::prelude::*;
use ts_goport::sourcemap::generator::RawSourceMap;
use ts_goport::sourcemap::lineinfo::create_ecma_line_info;
use ts_goport::sourcemap::util::try_get_source_mapping_url;
use ts_goport::sourcemap::{MISSING_NAME, Mapping, MappingsDecoder, decode_mappings};

use super::harness::CompilationResult;

// Go: sourcemap_recorder.go:16 writerAggregator
#[derive(Default)]
struct WriterAggregator {
    text: String,
}

impl WriterAggregator {
    fn write_string(&mut self, s: &str) {
        self.text.push_str(s);
    }

    // Go: sourcemap_recorder.go:24 WriteLine
    fn write_line(&mut self, s: &str) {
        self.text.push_str(s);
        self.text.push_str("\r\n");
    }
}

// Go: sourcemap_recorder.go:33 sourceMapSpanWithDecodeErrors
struct SourceMapSpanWithDecodeErrors {
    source_map_span: Mapping,
    decode_errors: Vec<String>,
}

// Go: sourcemap_recorder.go:38 decodedMapping
struct DecodedMapping {
    source_map_span: Mapping,
    error: Option<String>,
}

// Go: sourcemap_recorder.go:43 sourceMapDecoder
struct SourceMapDecoder {
    source_map_mappings: String,
    mappings: MappingsDecoder,
}

impl SourceMapDecoder {
    // Go: sourcemap_recorder.go:48 newSourceMapDecoder
    fn new(source_map: &RawSourceMap) -> SourceMapDecoder {
        SourceMapDecoder {
            source_map_mappings: source_map.mappings.clone(),
            mappings: decode_mappings(&source_map.mappings),
        }
    }

    // Go: sourcemap_recorder.go:55 decodeNextEncodedSourceMapSpan
    fn decode_next_encoded_source_map_span(&mut self) -> DecodedMapping {
        let (value, done) = self.mappings.next();
        if done {
            let error = self
                .mappings
                .error()
                .map(|err| err.error())
                .or_else(|| Some("No encoded entry found".to_string()));
            return DecodedMapping {
                error,
                source_map_span: self.mappings.state(),
            };
        }
        DecodedMapping {
            source_map_span: value.expect("a mapping when not done"),
            error: None,
        }
    }

    // Go: sourcemap_recorder.go:69 hasCompletedDecoding
    fn has_completed_decoding(&self) -> bool {
        self.mappings.pos() as usize == self.source_map_mappings.len()
    }

    // Go: sourcemap_recorder.go:73 getRemainingDecodeString
    fn get_remaining_decode_string(&self) -> &str {
        &self.source_map_mappings[self.mappings.pos() as usize..]
    }
}

// Go: sourcemap_recorder.go:77 sourceMapSpanWriter
struct SourceMapSpanWriter<'a> {
    source_map_recorder: &'a mut WriterAggregator,
    source_map_sources: Vec<String>,
    source_map_names: Vec<String>,
    js_file: TestFile,
    js_line_map: Vec<i32>,
    ts_code: String,
    ts_line_map: Vec<i32>,
    spans_on_single_line: Vec<SourceMapSpanWithDecodeErrors>,
    prev_written_source_pos: i32,
    next_js_line_to_write: i32,
    span_marker_continues: bool,
    source_map_decoder: SourceMapDecoder,
}

impl<'a> SourceMapSpanWriter<'a> {
    // Go: sourcemap_recorder.go:92 newSourceMapSpanWriter
    fn new(
        source_map_recorder: &'a mut WriterAggregator,
        source_map: &RawSourceMap,
        js_file: TestFile,
    ) -> SourceMapSpanWriter<'a> {
        let js_line_map = compute_ecma_line_starts(&js_file.content);
        source_map_recorder
            .write_line("===================================================================");
        source_map_recorder.write_line(&format!("JsFile: {}", source_map.file));
        source_map_recorder.write_line(&format!(
            "mapUrl: {}",
            try_get_source_mapping_url(Some(&create_ecma_line_info(
                &js_file.content,
                js_line_map.clone()
            )))
        ));
        source_map_recorder.write_line(&format!("sourceRoot: {}", source_map.source_root));
        source_map_recorder.write_line(&format!("sources: {}", source_map.sources.join(",")));
        if let Some(sources_content) = &source_map.sources_content
            && !sources_content.is_empty()
        {
            let mut content = String::from("[");
            for (i, item) in sources_content.iter().enumerate() {
                if i > 0 {
                    content.push(',');
                }
                match item {
                    Some(item) => append_json_quote(&mut content, item),
                    None => content.push_str("null"),
                }
            }
            content.push(']');
            source_map_recorder.write_line(&format!("sourcesContent: {content}"));
        }
        source_map_recorder
            .write_line("===================================================================");
        SourceMapSpanWriter {
            source_map_recorder,
            source_map_sources: source_map.sources.clone(),
            source_map_names: source_map.names.clone(),
            js_file,
            js_line_map,
            ts_code: String::new(),
            ts_line_map: Vec::new(),
            spans_on_single_line: Vec::new(),
            prev_written_source_pos: 0,
            next_js_line_to_write: 0,
            span_marker_continues: false,
            source_map_decoder: SourceMapDecoder::new(source_map),
        }
    }

    // Go: sourcemap_recorder.go:122 getSourceMapSpanString
    fn get_source_map_span_string(
        &self,
        map_entry: &Mapping,
        get_absent_name_index: bool,
    ) -> String {
        let mut map_string = format!(
            "Emitted({}, {})",
            map_entry.generated_line + 1,
            map_entry.generated_character + 1
        );
        if map_entry.is_source_mapping() {
            map_string.push_str(&format!(
                " Source({}, {}) + SourceIndex({})",
                map_entry.source_line + 1,
                map_entry.source_character + 1,
                map_entry.source_index
            ));
            if map_entry.name_index >= 0
                && (map_entry.name_index as usize) < self.source_map_names.len()
            {
                map_string.push_str(&format!(
                    " name ({})",
                    self.source_map_names[map_entry.name_index as usize]
                ));
            } else if map_entry.name_index != MISSING_NAME || get_absent_name_index {
                map_string.push_str(&format!(" nameIndex ({})", map_entry.name_index));
            }
        }
        map_string
    }

    // Go: sourcemap_recorder.go:139 recordSourceMapSpan
    fn record_source_map_span(&mut self, source_map_span: Mapping) {
        // verify the decoded span is same as the new span
        let decode_result = self
            .source_map_decoder
            .decode_next_encoded_source_map_span();
        let mut decode_errors = Vec::new();
        if decode_result.error.is_some() || !decode_result.source_map_span.equals(&source_map_span)
        {
            if let Some(error) = &decode_result.error {
                decode_errors.push(format!(
                    "!!^^ !!^^ There was decoding error in the sourcemap at this location: {error}"
                ));
            } else {
                decode_errors.push("!!^^ !!^^ The decoded span from sourcemap's mapping entry does not match what was encoded for this span:".to_string());
            }
            decode_errors.push(format!(
                "!!^^ !!^^ Decoded span from sourcemap's mappings entry: {} Span encoded by the emitter:{}",
                self.get_source_map_span_string(&decode_result.source_map_span, true),
                self.get_source_map_span_string(&source_map_span, true),
            ));
        }

        if !self.spans_on_single_line.is_empty()
            && self.spans_on_single_line[0].source_map_span.generated_line
                != source_map_span.generated_line
        {
            // On different line from the one that we have been recording till now,
            self.write_recorded_spans();
            self.spans_on_single_line.clear();
        }
        self.spans_on_single_line
            .push(SourceMapSpanWithDecodeErrors {
                source_map_span,
                decode_errors,
            });
    }

    // Go: sourcemap_recorder.go:170 recordNewSourceFileSpan
    fn record_new_source_file_span(
        &mut self,
        source_map_span: Mapping,
        new_source_file_code: &str,
    ) {
        let mut continues_line = false;
        if !self.spans_on_single_line.is_empty()
            && self.spans_on_single_line[0]
                .source_map_span
                .generated_character
                == source_map_span.generated_line
        {
            // !!! char == line seems like a bug in Strada?
            self.write_recorded_spans();
            self.spans_on_single_line.clear();
            self.next_js_line_to_write -= 1; // walk back one line to reprint the line
            continues_line = true;
        }

        self.record_source_map_span(source_map_span);

        if self.spans_on_single_line.len() != 1 {
            panic!("expected a single span");
        }

        self.source_map_recorder
            .write_line("-------------------------------------------------------------------");
        if continues_line {
            let line = format!(
                "emittedFile:{} ({}, {})",
                self.js_file.unit_name,
                source_map_span.generated_line + 1,
                source_map_span.generated_character + 1
            );
            self.source_map_recorder.write_line(&line);
        } else {
            let line = format!("emittedFile:{}", self.js_file.unit_name);
            self.source_map_recorder.write_line(&line);
        }
        let source_index = self.spans_on_single_line[0].source_map_span.source_index;
        let line = format!(
            "sourceFile:{}",
            self.source_map_sources[source_index as usize]
        );
        self.source_map_recorder.write_line(&line);
        self.source_map_recorder
            .write_line("-------------------------------------------------------------------");

        self.ts_line_map = compute_ecma_line_starts(new_source_file_code);
        self.ts_code = new_source_file_code.to_string();
        self.prev_written_source_pos = 0;
    }

    // Go: sourcemap_recorder.go:199 close
    fn close(&mut self) {
        // Write the lines pending on the single line
        self.write_recorded_spans();

        if !self.source_map_decoder.has_completed_decoding() {
            self.source_map_recorder.write_line(
                "!!!! **** There are more source map entries in the sourceMap's mapping than what was encoded",
            );
            let line = format!(
                "!!!! **** Remaining decoded string: {}",
                self.source_map_decoder.get_remaining_decode_string()
            );
            self.source_map_recorder.write_line(&line);
        }

        // write remaining js lines
        self.write_js_file_lines(self.js_line_map.len() as i32);
    }

    // Go: sourcemap_recorder.go:213 getTextOfLine
    fn get_text_of_line<'t>(line: i32, line_map: &[i32], code: &'t str) -> &'t str {
        let start_pos = line_map[line as usize] as usize;
        let end_pos = if ((line + 1) as usize) < line_map.len() {
            line_map[(line + 1) as usize] as usize
        } else {
            code.len()
        };
        let text = &code[start_pos..end_pos];
        if line == 0 {
            // Go: stringutil.RemoveByteOrderMark
            return text.strip_prefix('\u{FEFF}').unwrap_or(text);
        }
        // return line == 0 ? Utils.removeByteOrderMark(text) : text;
        text
    }

    // Go: sourcemap_recorder.go:228 writeJsFileLines
    fn write_js_file_lines(&mut self, end_js_line: i32) {
        while self.next_js_line_to_write < end_js_line {
            let text = Self::get_text_of_line(
                self.next_js_line_to_write,
                &self.js_line_map,
                &self.js_file.content,
            )
            .to_string();
            self.source_map_recorder.write_string(&format!(">>>{text}"));
            self.next_js_line_to_write += 1;
        }
    }

    // Go: sourcemap_recorder.go:234 writeRecordedSpans
    fn write_recorded_spans(&mut self) {
        let mut recorded_span_writer = RecordedSpanWriter {
            marker_ids: Vec::new(),
            prev_emitted_col: 0,
        };
        recorded_span_writer.write_recorded_spans(self);
    }
}

// Go: sourcemap_recorder.go:239 recordedSpanWriter
struct RecordedSpanWriter {
    marker_ids: Vec<String>,
    prev_emitted_col: i32,
}

impl RecordedSpanWriter {
    // Go: sourcemap_recorder.go:245 getMarkerId
    fn get_marker_id(&self, w: &SourceMapSpanWriter<'_>, marker_index: usize) -> String {
        if w.span_marker_continues {
            if marker_index != 0 {
                panic!("expected markerIndex to be 0");
            }
            return "1->".to_string();
        }
        let mut marker_id = (marker_index + 1).to_string();
        if marker_id.len() < 2 {
            marker_id.push(' ');
        }
        marker_id.push('>');
        marker_id
    }

    // Go: sourcemap_recorder.go:261 iterateSpans
    fn iterate_spans(
        &mut self,
        w: &mut SourceMapSpanWriter<'_>,
        f: fn(&mut RecordedSpanWriter, &mut SourceMapSpanWriter<'_>, usize),
    ) {
        self.prev_emitted_col = 0;
        for i in 0..w.spans_on_single_line.len() {
            f(self, w, i);
            self.prev_emitted_col = w.spans_on_single_line[i]
                .source_map_span
                .generated_character;
        }
    }

    // Go: sourcemap_recorder.go:269 writeSourceMapIndent
    fn write_source_map_indent(
        &self,
        w: &mut SourceMapSpanWriter<'_>,
        indent_length: i32,
        indent_prefix: &str,
    ) {
        w.source_map_recorder.write_string(indent_prefix);
        for _ in 0..indent_length.max(0) {
            w.source_map_recorder.write_string(" ");
        }
    }

    // Go: sourcemap_recorder.go:276 writeSourceMapMarker
    fn write_source_map_marker(&mut self, w: &mut SourceMapSpanWriter<'_>, index: usize) {
        let end_column = w.spans_on_single_line[index]
            .source_map_span
            .generated_character;
        self.write_source_map_marker_ex(w, index, end_column, false /*endContinues*/);
    }

    // Go: sourcemap_recorder.go:280 writeSourceMapMarkerEx
    fn write_source_map_marker_ex(
        &mut self,
        w: &mut SourceMapSpanWriter<'_>,
        index: usize,
        end_column: i32,
        end_continues: bool,
    ) {
        let marker_id = self.get_marker_id(w, index);
        self.marker_ids.push(marker_id.clone());
        self.write_source_map_indent(w, self.prev_emitted_col, &marker_id);
        for _ in self.prev_emitted_col..end_column {
            w.source_map_recorder.write_string("^");
        }
        if end_continues {
            w.source_map_recorder.write_string("->");
        }
        w.source_map_recorder.write_line("");
        w.span_marker_continues = end_continues;
    }

    // Go: sourcemap_recorder.go:295 writeSourceMapSourceText
    fn write_source_map_source_text(&mut self, w: &mut SourceMapSpanWriter<'_>, index: usize) {
        let current_span = &w.spans_on_single_line[index];
        // Convert UTF-16 character offset from the source map to a byte position.
        let source_pos = compute_position_of_line_and_utf16_character(
            &w.ts_line_map,
            current_span.source_map_span.source_line,
            current_span.source_map_span.source_character,
            &w.ts_code,
            true, /*allowEdits*/
        );
        let mut source_text = String::new();
        if w.prev_written_source_pos < source_pos {
            // Position that goes forward, get text
            source_text =
                w.ts_code[w.prev_written_source_pos as usize..source_pos as usize].to_string();
        }

        // If there are decode errors, write
        let decode_errors = current_span.decode_errors.clone();
        for decode_error in &decode_errors {
            let marker_id = self.marker_ids[index].clone();
            self.write_source_map_indent(w, self.prev_emitted_col, &marker_id);
            w.source_map_recorder.write_line(decode_error);
        }

        let ts_code_line_map = compute_ecma_line_starts(&source_text);
        for i in 0..ts_code_line_map.len() {
            if i == 0 {
                let marker_id = self.marker_ids[index].clone();
                self.write_source_map_indent(w, self.prev_emitted_col, &marker_id);
            } else {
                self.write_source_map_indent(w, self.prev_emitted_col, "  >");
            }
            let text =
                SourceMapSpanWriter::get_text_of_line(i as i32, &ts_code_line_map, &source_text);
            w.source_map_recorder.write_string(text);
            if i == ts_code_line_map.len() - 1 {
                w.source_map_recorder.write_line("");
            }
        }

        w.prev_written_source_pos = source_pos;
    }

    // Go: sourcemap_recorder.go:333 writeSpanDetails
    fn write_span_details(&mut self, w: &mut SourceMapSpanWriter<'_>, index: usize) {
        let details = w.get_source_map_span_string(
            &w.spans_on_single_line[index].source_map_span,
            false, /*getAbsentNameIndex*/
        );
        let line = format!("{}{details}", self.marker_ids[index]);
        w.source_map_recorder.write_line(&line);
    }

    // Go: sourcemap_recorder.go:337 writeRecordedSpans
    fn write_recorded_spans(&mut self, w: &mut SourceMapSpanWriter<'_>) {
        if !w.spans_on_single_line.is_empty() {
            let current_js_line = w.spans_on_single_line[0].source_map_span.generated_line;

            // Write js line
            w.write_js_file_lines(current_js_line + 1);

            // Emit markers
            self.iterate_spans(w, |sw, w, i| sw.write_source_map_marker(w, i));

            let js_file_text_len = SourceMapSpanWriter::get_text_of_line(
                current_js_line + 1,
                &w.js_line_map,
                &w.js_file.content,
            )
            .len() as i32; // TODO: Strada is wrong here, we should be looking at `currentJsLine`, not `currentJsLine+1`
            if self.prev_emitted_col < js_file_text_len - 1 {
                // There is remaining text on this line that will be part of next source span so write marker that continues
                let count = w.spans_on_single_line.len();
                self.write_source_map_marker_ex(
                    w,
                    count,
                    js_file_text_len - 1, /*endColumn*/
                    true,                 /*endContinues*/
                );
            }

            // Emit Source text
            self.iterate_spans(w, |sw, w, i| sw.write_source_map_source_text(w, i));

            // Emit column number etc
            self.iterate_spans(w, |sw, w, i| sw.write_span_details(w, i));

            w.source_map_recorder.write_line("---");
        }
    }
}

// Go: scanner/scanner.go:2755 ComputePositionOfLineAndUTF16Character
// PORT: the port's copy is private to `sourcemap::source_mapper`; this is
// the same Go function.
fn compute_position_of_line_and_utf16_character(
    line_starts: &[i32],
    line: i32,
    character: i32,
    text: &str,
    allow_edits: bool,
) -> i32 {
    let mut line = line;
    if line < 0 || line as usize >= line_starts.len() {
        if allow_edits {
            // Clamp line to nearest allowable value
            if line < 0 {
                line = 0;
            } else {
                line = line_starts.len() as i32 - 1;
            }
        } else {
            panic!(
                "Bad line number. Line: {}, lineStarts.length: {}.",
                line,
                line_starts.len()
            );
        }
    }

    let line_start = line_starts[line as usize];

    if character > 0 {
        // UTF-16 character offset: scan from line start counting UTF-16 code units.
        let mut line_end = text.len() as i32;
        if ((line + 1) as usize) < line_starts.len() {
            line_end = line_starts[(line + 1) as usize];
        }
        let mut utf16_count: i32 = 0;
        let mut pos = line_start;
        while pos < line_end {
            if utf16_count >= character {
                break;
            }
            let (unit, size) = go_unit_at(text, pos as usize);
            utf16_count += unit.go_utf16_len() as i32;
            pos += size as i32;
        }
        if !allow_edits {
            if pos == line_end && utf16_count < character {
                panic!("Bad UTF-16 character offset. Line: {line}, character: {character}.");
            }
            return pos;
        }
        return pos.min(text.len() as i32);
    }

    // Character is 0: line start position.
    let res = line_start;

    if allow_edits {
        return res.min(text.len() as i32);
    }
    res
}

impl CompilationResult {
    // Go: harnessutil.go:909 GetSourceMapRecord
    // ts#64159: the generated and input file names are rooted against the
    // result's current directory before the lookups (:920, :932).
    pub fn get_source_map_record(&self) -> String {
        if self.result.source_maps.is_empty() {
            return String::new();
        }

        let mut source_map_recorder = WriterAggregator::default();
        for source_map_data in &self.result.source_maps {
            let mut prev_source_file: Option<String> = None;

            let generated_file =
                to_rooted_path(&source_map_data.generated_file, &self.current_directory);
            let current_file = if is_declaration_file_name(&source_map_data.generated_file) {
                self.dts.get(&generated_file)
            } else {
                self.js.get(&generated_file)
            };
            // PORT: Go dereferences a nil file (a panic) when the generated
            // file was not recorded.
            let current_file = current_file
                .cloned()
                .expect("nil pointer dereference: generated file not recorded");

            let mut source_map_span_writer = SourceMapSpanWriter::new(
                &mut source_map_recorder,
                &source_map_data.source_map,
                current_file,
            );
            let mut mapper = decode_mappings(&source_map_data.source_map.mappings);
            for decoded_source_mapping in mapper.values() {
                if !decoded_source_mapping.is_source_mapping() {
                    source_map_span_writer.record_source_map_span(decoded_source_mapping);
                    continue;
                }
                let name = &source_map_data.input_source_file_names
                    [decoded_source_mapping.source_index as usize];
                // Go compares `*ast.SourceFile` pointers; the file name
                // identifies the file.
                let current_source_file =
                    self.source_file_name(&to_rooted_path(name, &self.current_directory));
                if current_source_file != prev_source_file {
                    if let Some(name) = &current_source_file {
                        let text = self
                            .source_file_original_text(name)
                            .expect("the program has the source file");
                        source_map_span_writer
                            .record_new_source_file_span(decoded_source_mapping, &text);
                    }
                    prev_source_file = current_source_file;
                } else {
                    source_map_span_writer.record_source_map_span(decoded_source_mapping);
                }
            }
            source_map_span_writer.close();
        }
        source_map_recorder.text
    }
}

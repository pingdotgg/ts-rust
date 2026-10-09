//! Go: internal/testutil/harnessutil/harnessutil.go, the parts that the tsc
//! runner uses: `FakeTSVersion` and `TracerForBaselining`.

use rustc_hash::FxHashMap;
use ts_goport::core::version;
use ts_goport::diagnostics::Message;
use ts_goport::diagnostics_loc::message_localize;
use ts_goport::execute::tsc::compile::{Writer, write_str};
use ts_goport::frontend::tspath::{ComparePathsOptions, Path, to_path};

// Go: harnessutil.go:41 FakeTSVersion
pub const FAKE_TS_VERSION: &str = "FakeTSVersion";

// Go: harnessutil.go:505 TracerForBaselining
// PORT: Go `builder *strings.Builder` is the shared output buffer of the
// test system (`Writer`). The methods that change the package.json cache
// take `&mut self`; the test system keeps the tracer in a `RefCell`.
pub struct TracerForBaselining {
    opts: ComparePathsOptions,
    package_json_cache: FxHashMap<Path, bool>,
    builder: Writer,
    /// The bytes of `builder`, for `string` (Go `builder.String()`).
    builder_bytes: Option<std::rc::Rc<std::cell::RefCell<Vec<u8>>>>,
}

impl TracerForBaselining {
    // Go: harnessutil.go:511 NewTracerForBaselining
    // PORT: `builder_bytes` is the buffer behind `builder` when the caller
    // has it, so `string` can read it back.
    pub fn new(
        opts: ComparePathsOptions,
        builder: Writer,
        builder_bytes: Option<std::rc::Rc<std::cell::RefCell<Vec<u8>>>>,
    ) -> TracerForBaselining {
        TracerForBaselining {
            opts,
            package_json_cache: FxHashMap::default(),
            builder,
            builder_bytes,
        }
    }

    // Go: harnessutil.go:519 Trace
    pub fn trace(&mut self, msg: &'static Message, args: Vec<String>) {
        let builder = self.builder.clone();
        let text = message_localize(msg, &ts_goport::locale::DEFAULT, &args);
        self.trace_with_writer(&builder, &text, true);
    }

    // Go: harnessutil.go:523 TraceWithWriter
    pub fn trace_with_writer(&mut self, w: &Writer, msg: &str, use_package_json_cache: bool) {
        let line = self.sanitize_trace(msg, use_package_json_cache);
        write_str(w, &format!("{line}\n"));
    }

    // Go: harnessutil.go:527 sanitizeTrace
    fn sanitize_trace(&mut self, msg: &str, use_package_json_cache: bool) -> String {
        // Version
        let quoted_version = format!("'{}'", version());
        let str = msg.replacen(&quoted_version, &format!("'{FAKE_TS_VERSION}'"), 1);
        if str != msg {
            return str;
        }
        // caching of fs in trace to be replaces with non caching version
        if let Some(str) = msg.strip_suffix("' does not exist according to earlier cached lookups.")
        {
            let file = str.strip_prefix("File '").unwrap_or(str);
            if use_package_json_cache {
                let file_path = self.to_path(file);
                if let std::collections::hash_map::Entry::Vacant(e) =
                    self.package_json_cache.entry(file_path)
                {
                    e.insert(false);
                } else {
                    return msg.to_string();
                }
            }
            return format!("File '{file}' does not exist.");
        }
        if let Some(str) = msg.strip_suffix("' exists according to earlier cached lookups.") {
            let file = str.strip_prefix("File '").unwrap_or(str);
            if use_package_json_cache {
                let file_path = self.to_path(file);
                if let std::collections::hash_map::Entry::Vacant(e) =
                    self.package_json_cache.entry(file_path)
                {
                    e.insert(true);
                } else {
                    return msg.to_string();
                }
            }
            return format!("Found 'package.json' at '{file}'.");
        }
        if use_package_json_cache {
            if let Some(str) = msg.strip_suffix("' does not exist.") {
                let file = str.strip_prefix("File '").unwrap_or(str);
                let file_path = self.to_path(file);
                if let std::collections::hash_map::Entry::Vacant(e) =
                    self.package_json_cache.entry(file_path)
                {
                    e.insert(false);
                    return msg.to_string();
                }
                return format!(
                    "File '{file}' does not exist according to earlier cached lookups."
                );
            }
            if let Some(str) = msg.strip_prefix("Found 'package.json' at '") {
                let file = str.strip_suffix("'.").unwrap_or(str);
                let file_path = self.to_path(file);
                if let std::collections::hash_map::Entry::Vacant(e) =
                    self.package_json_cache.entry(file_path)
                {
                    e.insert(true);
                    return msg.to_string();
                }
                return format!("File '{file}' exists according to earlier cached lookups.");
            }
        }
        msg.to_string()
    }

    /// Go `t.caseSensitivity.PathKey(tspath.ToRootedPath(file, t.currentDirectory))`
    /// (ts#64159, harnessutil.go:536): `to_path` of the name against the
    /// current directory gives the same key.
    fn to_path(&self, file: &str) -> Path {
        to_path(
            file,
            &self.opts.current_directory,
            self.opts.use_case_sensitive_file_names,
        )
    }

    // Go: harnessutil.go:582 String
    pub fn string(&self) -> String {
        self.builder_bytes
            .as_ref()
            .map(|bytes| String::from_utf8_lossy(&bytes.borrow()).into_owned())
            .unwrap_or_default()
    }

    // Go: harnessutil.go:586 Reset
    pub fn reset(&mut self) {
        self.package_json_cache = FxHashMap::default();
    }
}

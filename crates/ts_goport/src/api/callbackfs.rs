//! Port of internal/api/callbackfs.go.

use crate::api::prelude::*;

use crate::frontend::json::{
    JsonDecoder, JsonError, MarshalerTo, UnmarshalerFrom, json_unmarshal_decode,
};
use crate::frontend::json_ext::{
    AnyValue, JsonValue, marshal_field, unmarshal_int_as, unmarshal_root, unmarshal_struct_fields,
    write_object_end, write_object_start,
};
use crate::frontend::vfs::{Entries, FileInfo, FileMode, Fs, FsError};
use crate::gostd::{Context, GoError, errors};
use crate::ipc::Conn;
use std::time::{Duration, SystemTime};

// Go: callbackfs.go:24 callbackFS
// callbackFS wraps a base filesystem and delegates certain operations
// to the client via RPC callbacks. This allows the API client to provide
// a virtual filesystem (e.g., in-memory files for testing).
//
// The callbacks to enable are specified at construction time via the
// --callbacks CLI flag. The connection is set via SetConnection after
// the transport connection is established.
// PORT: the Go `caseSensitive *bool` is `Option<bool>` (ts#64447).
pub struct CallbackFS {
    base: Rc<dyn Fs>,
    enabled_callbacks: FxHashMap<String, bool>,
    realpath_identity: bool,
    fake_stat: bool,
    write_file_noop: bool,
    remove_file_noop: bool,
    error_callbacks: FxHashMap<String, bool>,
    case_sensitive: Option<bool>,

    // conn and ctx are set after connection is established
    // PORT: `RefCell` so SetConnection works through the shared `Rc`; nil
    // is `None`.
    conn: RefCell<Option<Rc<dyn Conn>>>,
    ctx: RefCell<Option<Context>>,
}

// Go: callbackfs.go:40
// Callback names that can be enabled
const CALLBACK_READ_FILE: &str = "readFile";
const CALLBACK_FILE_EXISTS: &str = "fileExists";
const CALLBACK_DIRECTORY_EXISTS: &str = "directoryExists";
const CALLBACK_GET_ACCESSIBLE_ENTRIES: &str = "getAccessibleEntries";
const CALLBACK_REALPATH: &str = "realpath";
// ts#64447
const CALLBACK_STAT: &str = "stat";
// tsgo#4699
const CALLBACK_WRITE_FILE: &str = "writeFile";
// ts#64158
const CALLBACK_REMOVE_FILE: &str = "removeFile";

// Go: callbackfs.go:51 isCallbackName
fn is_callback_name(name: &str) -> bool {
    matches!(
        name,
        CALLBACK_READ_FILE
            | CALLBACK_FILE_EXISTS
            | CALLBACK_DIRECTORY_EXISTS
            | CALLBACK_GET_ACCESSIBLE_ENTRIES
            | CALLBACK_REALPATH
            | CALLBACK_STAT
            | CALLBACK_WRITE_FILE
            | CALLBACK_REMOVE_FILE
    )
}

// Go: callbackfs.go:70 newCallbackFS
// newCallbackFS creates a new callbackFS wrapping the given base filesystem.
// The callbacks slice specifies which filesystem operations should be delegated
// to the client (e.g., "readFile", "fileExists").
// ts#64447: a "<name>:error" entry makes the operation panic, and
// "realpath:identity", "stat:fakeStat", "writeFile:noop" and
// "removeFile:noop" set the default of an operation without a callback.
// `case_sensitive` overrides the base file system's case sensitivity.
pub fn new_callback_fs(
    base: Rc<dyn Fs>,
    callbacks: &[String],
    case_sensitive: Option<bool>,
) -> Rc<CallbackFS> {
    let mut enabled: FxHashMap<String, bool> =
        FxHashMap::with_capacity_and_hasher(callbacks.len(), Default::default());
    let mut error_callbacks: FxHashMap<String, bool> = FxHashMap::default();
    for cb in callbacks {
        if let Some(name) = cb.strip_suffix(":error") {
            if !is_callback_name(name) {
                crate::core::go_panic(format!("unknown callback name: {name}"));
            }
            error_callbacks.insert(name.to_string(), true);
            continue;
        }
        if cb == "realpath:identity"
            || cb == "stat:fakeStat"
            || cb == "writeFile:noop"
            || cb == "removeFile:noop"
        {
            continue;
        }
        if !is_callback_name(cb) {
            crate::core::go_panic(format!("unknown callback name: {cb}"));
        }
        enabled.insert(cb.clone(), true);
    }
    let has = |name: &str| callbacks.iter().any(|cb| cb == name);
    Rc::new(CallbackFS {
        base,
        enabled_callbacks: enabled,
        realpath_identity: has("realpath:identity"),
        fake_stat: has("stat:fakeStat"),
        write_file_noop: has("writeFile:noop"),
        remove_file_noop: has("removeFile:noop"),
        error_callbacks,
        case_sensitive,
        conn: RefCell::new(None),
        ctx: RefCell::new(None),
    })
}

// Go: callbackfs.go:127 callbackResponse (ts#64447)
// Every callback answer is `{kind, value}`. `kind` is "value" (use
// `value`), "useOS" (ask the base file system), "missing", "identity",
// "fakeStat", "noop" or "error", as each operation allows.
#[derive(Default)]
struct CallbackResponse {
    kind: String,
    value: JsonValue,
}

impl UnmarshalerFrom for CallbackResponse {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        let is_object = unmarshal_struct_fields(dec, "api.callbackResponse", |name, dec| {
            match name {
                "kind" => json_unmarshal_decode(dec, &mut self.kind)?,
                "value" => json_unmarshal_decode(dec, &mut self.value)?,
                _ => return Ok(false),
            }
            Ok(true)
        })?;
        if !is_object {
            *self = CallbackResponse::default();
        }
        Ok(())
    }
}

// Go: callbackfs.go:132 decodeCallbackResponse (ts#64447)
fn decode_callback_response(name: &str, result: &[u8]) -> CallbackResponse {
    let mut response = CallbackResponse::default();
    if let Err(err) = unmarshal_root(result, &mut response) {
        panic_error(&errors::from_value(err));
    }
    if response.kind.is_empty() {
        crate::core::go_panic("filesystem callback response is missing a kind".to_string());
    }
    if response.kind == "error" {
        crate::core::go_panic(format!(
            "filesystem callback returned serverFS.error: {name}"
        ));
    }
    response
}

// Go: callbackfs.go:146 invalidCallbackResponse (ts#64447)
#[track_caller]
fn invalid_callback_response(name: &str, response: &CallbackResponse) -> ! {
    crate::core::go_panic(format!(
        "invalid {name} callback response kind: {}",
        response.kind
    ))
}

// Go `json.Unmarshal(response.Value, &v)`, panicking on an error.
fn unmarshal_value<T: UnmarshalerFrom>(value: &JsonValue, v: &mut T) {
    if let Err(err) = unmarshal_root(&value.0, v) {
        panic_error(&errors::from_value(err));
    }
}

// Go: the anonymous `struct { Files []string; Directories []string;
// Symlinks []string }` (with JSON tags) in GetAccessibleEntries. Its Go
// type string holds the tags. ts#64447 adds Symlinks: a nil slice (absent
// or null) is `None`.
#[derive(Default)]
struct RawEntries {
    files: Vec<String>,
    directories: Vec<String>,
    symlinks: Option<Vec<String>>,
}

impl UnmarshalerFrom for RawEntries {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        let is_object = unmarshal_struct_fields(
            dec,
            r#"struct { Files []string "json:\"files\""; Directories []string "json:\"directories\""; Symlinks []string "json:\"symlinks\"" }"#,
            |name, dec| {
                match name {
                    "files" => json_unmarshal_decode(dec, &mut self.files)?,
                    "directories" => json_unmarshal_decode(dec, &mut self.directories)?,
                    "symlinks" => json_unmarshal_decode(dec, &mut self.symlinks)?,
                    _ => return Ok(false),
                }
                Ok(true)
            },
        )?;
        if !is_object {
            *self = RawEntries::default();
        }
        Ok(())
    }
}

// Go: the anonymous `struct { Mode uint32; Size int64; MTime string }`
// (with JSON tags) in Stat (ts#64447).
#[derive(Default)]
struct RawStat {
    mode: u32,
    size: i64,
    mtime: String,
}

impl UnmarshalerFrom for RawStat {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        let is_object = unmarshal_struct_fields(
            dec,
            r#"struct { Mode uint32 "json:\"mode\""; Size int64 "json:\"size\""; MTime string "json:\"mtime\"" }"#,
            |name, dec| {
                match name {
                    "mode" => json_unmarshal_decode(dec, &mut self.mode)?,
                    "size" => self.size = unmarshal_int_as(dec, "int64")?,
                    "mtime" => json_unmarshal_decode(dec, &mut self.mtime)?,
                    _ => return Ok(false),
                }
                Ok(true)
            },
        )?;
        if !is_object {
            *self = RawStat::default();
        }
        Ok(())
    }
}

// Go: the anonymous `struct { Path string; Data string }` in WriteFile
// (tsgo#4699).
#[derive(Debug)]
struct WriteFilePayload {
    path: String,
    data: String,
}

impl MarshalerTo for WriteFilePayload {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        write_object_start(enc);
        let mut first = true;
        marshal_field(enc, &mut first, "path", &self.path)?;
        marshal_field(enc, &mut first, "data", &self.data)?;
        write_object_end(enc);
        Ok(())
    }
}

impl CallbackFS {
    // Go: callbackfs.go:104 SetConnection
    // SetConnection sets the RPC connection for callbacks.
    // This must be called after the transport connection is established
    // but before any filesystem operations that need callbacks.
    pub fn set_connection(&self, ctx: &Context, conn: Rc<dyn Conn>) {
        *self.ctx.borrow_mut() = Some(ctx.clone());
        *self.conn.borrow_mut() = Some(conn);
    }

    // Go: callbackfs.go:110 isEnabled
    // isEnabled returns true if the named callback is enabled.
    fn is_enabled(&self, name: &str) -> bool {
        self.enabled_callbacks.get(name).copied().unwrap_or(false)
    }

    // Go: callbackfs.go:115 call
    // call invokes a callback on the client and returns the result.
    // PORT: Go `arg any` is any value that marshals (`AnyValue`).
    fn call(&self, name: &str, arg: impl AnyValue) -> Result<Vec<u8>, GoError> {
        let conn = self.conn.borrow().clone();
        let Some(conn) = conn else {
            return Err(errors::new(format!(
                "CallbackFS: {name} called before connection set"
            )));
        };

        // PORT: Go passes the nil `fs.ctx` only when conn is nil too, which
        // returned above.
        let ctx = self
            .ctx
            .borrow()
            .clone()
            .expect("CallbackFS: ctx is set with conn");
        let result = conn.call(&ctx, name, Some(Box::new(arg)))?;
        Ok(result.0)
    }

    // Go `fs.call(name, arg)` then `decodeCallbackResponse`, panicking on a
    // call error (every read operation does this).
    fn call_decoded(&self, name: &str, path: &str) -> CallbackResponse {
        let result = match self.call(name, path.to_string()) {
            Ok(result) => result,
            Err(err) => panic_error(&err),
        };
        decode_callback_response(name, &result)
    }

    // Go: callbackfs.go:150 panicIfError (ts#64447)
    fn panic_if_error(&self, name: &str) {
        if self.error_callbacks.get(name).copied().unwrap_or(false) {
            crate::core::go_panic(format!(
                "filesystem operation configured with serverFS.error: {name}"
            ));
        }
    }

    // Go: callbackfs.go:372 fakeStatForPath (ts#64447)
    fn fake_stat_for_path(&self, path: &str) -> Option<FileInfo> {
        if self.directory_exists(path) {
            return Some(FileInfo {
                name: tspath::get_base_file_name(path),
                size: 0,
                mode: FileMode::DIR | FileMode(0o555),
                mod_time: None,
            });
        }
        if self.file_exists(path) {
            return Some(FileInfo {
                name: tspath::get_base_file_name(path),
                size: 0,
                mode: FileMode(0o444),
                mod_time: None,
            });
        }
        None
    }
}

// Go `panic(err)`. The runtime prints an error value by its text.
#[track_caller]
fn panic_error(err: &GoError) -> ! {
    crate::core::go_panic(err.error())
}

// Go: callbackfs.go:382 nodeFileModeToGoFileMode (ts#64447)
// The Node `fs.Stats.mode` bits as a Go `io/fs.FileMode`.
fn node_file_mode_to_go_file_mode(mode: u32) -> FileMode {
    let mut result = FileMode(mode & 0o777);
    if mode & 0o4000 != 0 {
        result |= FileMode::SETUID;
    }
    if mode & 0o2000 != 0 {
        result |= FileMode::SETGID;
    }
    if mode & 0o1000 != 0 {
        result |= FileMode::STICKY;
    }
    match mode & 0o170000 {
        0o010000 => result |= FileMode::NAMED_PIPE,
        0o020000 => result |= FileMode::DEVICE | FileMode::CHAR_DEVICE,
        0o040000 => result |= FileMode::DIR,
        0o060000 => result |= FileMode::DEVICE,
        0o100000 => {
            // Regular file.
        }
        0o120000 => result |= FileMode::SYMLINK,
        0o140000 => result |= FileMode::SOCKET,
        _ => result |= FileMode::IRREGULAR,
    }
    result
}

// Go `time.RFC3339Nano`.
const RFC3339_NANO: &str = "2006-01-02T15:04:05.999999999Z07:00";

// Go `time.Parse(time.RFC3339Nano, value)` (Go `time/format_rfc3339.go`
// parseRFC3339, the path that every RFC 3339 time takes): the date and
// time, an optional fraction of a second (up to 9 digits are kept) and "Z"
// or a "+hh:mm" or "-hh:mm" offset.
// PORT: Go tries its general layout parser after the RFC 3339 fast path
// fails. That parser accepts the same texts here, so its result is not
// ported; its error text names the first element that does not parse, and
// the port names the whole value and "2006".
fn parse_rfc3339_nano(value: &str) -> Result<SystemTime, GoError> {
    let parse_error = || {
        errors::new(format!(
            "parsing time {value:?} as {RFC3339_NANO:?}: cannot parse {value:?} as \"2006\""
        ))
    };
    let s = value.as_bytes();
    if s.len() < "2006-01-02T15:04:05".len() {
        return Err(parse_error());
    }
    let digits = |b: &[u8]| -> Option<i64> {
        b.iter().try_fold(0i64, |n, &c| {
            c.is_ascii_digit().then(|| n * 10 + i64::from(c - b'0'))
        })
    };
    let (Some(year), Some(month), Some(day), Some(hour), Some(min), Some(sec)) = (
        digits(&s[0..4]),
        digits(&s[5..7]),
        digits(&s[8..10]),
        digits(&s[11..13]),
        digits(&s[14..16]),
        digits(&s[17..19]),
    ) else {
        return Err(parse_error());
    };
    if !(s[4] == b'-' && s[7] == b'-' && s[10] == b'T' && s[13] == b':' && s[16] == b':') {
        return Err(parse_error());
    }
    let is_leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days_in_month = match month {
        2 if is_leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if !(1..=12).contains(&month)
        || !(1..=days_in_month).contains(&day)
        || hour > 23
        || min > 59
        || sec > 59
    {
        return Err(parse_error());
    }
    let mut rest = &s[19..];

    // Parse the fractional second.
    let mut nsec: u32 = 0;
    if rest.len() >= 2 && rest[0] == b'.' && rest[1].is_ascii_digit() {
        let n = 1 + rest[1..].iter().take_while(|c| c.is_ascii_digit()).count();
        for (i, &c) in rest[1..n].iter().take(9).enumerate() {
            nsec += u32::from(c - b'0') * 10u32.pow(8 - i as u32);
        }
        rest = &rest[n..];
    }

    // Parse the time zone.
    let mut offset_secs: i64 = 0;
    if rest != b"Z" {
        if rest.len() != "-07:00".len() {
            return Err(parse_error());
        }
        let (Some(hr), Some(mm)) = (digits(&rest[1..3]), digits(&rest[4..6])) else {
            return Err(parse_error());
        };
        if hr > 23 || mm > 59 || !(matches!(rest[0], b'-' | b'+') && rest[3] == b':') {
            return Err(parse_error());
        }
        offset_secs = (hr * 60 + mm) * 60;
        if rest[0] == b'-' {
            offset_secs = -offset_secs;
        }
    }

    // Days from 1970-01-01 (the civil-from-days algorithm in reverse).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let unix = days * 86_400 + hour * 3600 + min * 60 + sec - offset_secs;
    let time = if unix >= 0 {
        SystemTime::UNIX_EPOCH + Duration::new(unix as u64, nsec)
    } else {
        SystemTime::UNIX_EPOCH - Duration::from_secs(unix.unsigned_abs())
            + Duration::from_nanos(u64::from(nsec))
    };
    Ok(time)
}

impl Fs for CallbackFS {
    // Go: callbackfs.go:157 CaseSensitivity (ts#64159 renames
    // UseCaseSensitiveFileNames to CaseSensitivity; the port keeps the bool)
    // UseCaseSensitiveFileNames implements vfs.FS.
    fn use_case_sensitive_file_names(&self) -> bool {
        // ts#64447
        if let Some(case_sensitive) = self.case_sensitive {
            return case_sensitive;
        }
        self.base.use_case_sensitive_file_names()
    }

    // Go: callbackfs.go:168 ReadFile
    // ReadFile implements vfs.FS.
    fn read_file(&self, path: &str) -> (String, bool) {
        self.panic_if_error(CALLBACK_READ_FILE);
        if self.is_enabled(CALLBACK_READ_FILE) {
            let response = self.call_decoded(CALLBACK_READ_FILE, path);
            match response.kind.as_str() {
                "value" => {
                    let mut content = String::new();
                    unmarshal_value(&response.value, &mut content);
                    return (content, true);
                }
                "missing" => return (String::new(), false),
                "useOS" => return self.base.read_file(path),
                _ => invalid_callback_response(CALLBACK_READ_FILE, &response),
            }
        }
        self.base.read_file(path)
    }

    // Go: callbackfs.go:195 FileExists
    // FileExists implements vfs.FS.
    fn file_exists(&self, path: &str) -> bool {
        self.panic_if_error(CALLBACK_FILE_EXISTS);
        if self.is_enabled(CALLBACK_FILE_EXISTS) {
            let response = self.call_decoded(CALLBACK_FILE_EXISTS, path);
            match response.kind.as_str() {
                "value" => {
                    let mut exists = false;
                    unmarshal_value(&response.value, &mut exists);
                    return exists;
                }
                "useOS" => return self.base.file_exists(path),
                _ => invalid_callback_response(CALLBACK_FILE_EXISTS, &response),
            }
        }
        self.base.file_exists(path)
    }

    // Go: callbackfs.go:220 DirectoryExists
    // DirectoryExists implements vfs.FS.
    fn directory_exists(&self, path: &str) -> bool {
        self.panic_if_error(CALLBACK_DIRECTORY_EXISTS);
        if self.is_enabled(CALLBACK_DIRECTORY_EXISTS) {
            let response = self.call_decoded(CALLBACK_DIRECTORY_EXISTS, path);
            match response.kind.as_str() {
                "value" => {
                    let mut exists = false;
                    unmarshal_value(&response.value, &mut exists);
                    return exists;
                }
                "useOS" => return self.base.directory_exists(path),
                _ => invalid_callback_response(CALLBACK_DIRECTORY_EXISTS, &response),
            }
        }
        self.base.directory_exists(path)
    }

    // Go: callbackfs.go:245 GetAccessibleEntries
    // GetAccessibleEntries implements vfs.FS.
    fn get_accessible_entries(&self, path: &str) -> Entries {
        self.panic_if_error(CALLBACK_GET_ACCESSIBLE_ENTRIES);
        if self.is_enabled(CALLBACK_GET_ACCESSIBLE_ENTRIES) {
            let response = self.call_decoded(CALLBACK_GET_ACCESSIBLE_ENTRIES, path);
            match response.kind.as_str() {
                "value" => {
                    let mut raw_entries: Option<RawEntries> = None;
                    unmarshal_value(&response.value, &mut raw_entries);
                    // Go reads the fields of the nil pointer that `null`
                    // gives.
                    let raw_entries =
                        raw_entries.unwrap_or_else(|| crate::core::go_nil_dereference());
                    return Entries {
                        files: raw_entries.files,
                        directories: raw_entries.directories,
                        symlinks: raw_entries
                            .symlinks
                            .map(|names| names.into_iter().collect()),
                    };
                }
                "useOS" => return self.base.get_accessible_entries(path),
                _ => invalid_callback_response(CALLBACK_GET_ACCESSIBLE_ENTRIES, &response),
            }
        }
        self.base.get_accessible_entries(path)
    }

    // Go: callbackfs.go:284 Realpath
    // Realpath implements vfs.FS.
    fn realpath(&self, path: &str) -> String {
        self.panic_if_error(CALLBACK_REALPATH);
        if self.is_enabled(CALLBACK_REALPATH) {
            let response = self.call_decoded(CALLBACK_REALPATH, path);
            match response.kind.as_str() {
                "value" => {
                    let mut realpath = String::new();
                    unmarshal_value(&response.value, &mut realpath);
                    return realpath;
                }
                "identity" => return path.to_string(),
                "useOS" => return self.base.realpath(path),
                _ => invalid_callback_response(CALLBACK_REALPATH, &response),
            }
        }
        if self.realpath_identity {
            return path.to_string();
        }
        self.base.realpath(path)
    }

    // Go: callbackfs.go:415 WriteFile
    // WriteFile implements vfs.FS.
    // PORT: Go returns the callback error; it is `FsError::Other` with its
    // text.
    fn write_file(&self, path: &str, data: &str) -> Result<(), FsError> {
        self.panic_if_error(CALLBACK_WRITE_FILE);
        // tsgo#4699
        if self.is_enabled(CALLBACK_WRITE_FILE) {
            let payload = WriteFilePayload {
                path: path.to_string(),
                data: data.to_string(),
            };

            let result = match self.call(CALLBACK_WRITE_FILE, payload) {
                Ok(result) => result,
                Err(err) => return Err(FsError::Other(err.error())),
            };
            let response = decode_callback_response(CALLBACK_WRITE_FILE, &result);
            match response.kind.as_str() {
                "value" | "noop" => return Ok(()),
                "useOS" => return self.base.write_file(path, data),
                _ => invalid_callback_response(CALLBACK_WRITE_FILE, &response),
            }
        }
        if self.write_file_noop {
            return Ok(());
        }

        self.base.write_file(path, data)
    }

    // Go: callbackfs.go:445 AppendFile
    // AppendFile implements vfs.FS - always delegates to base (no callback support).
    fn append_file(&self, path: &str, data: &str) -> Result<(), FsError> {
        self.base.append_file(path, data)
    }

    // Go: callbackfs.go:450 Remove
    // Remove implements vfs.FS.
    // PORT: Go returns the callback error; it is `FsError::Other` with its
    // text (as in `write_file`).
    fn remove(&self, path: &str) -> Result<(), FsError> {
        self.panic_if_error(CALLBACK_REMOVE_FILE);
        // ts#64158
        if self.is_enabled(CALLBACK_REMOVE_FILE) {
            let result = match self.call(CALLBACK_REMOVE_FILE, path.to_string()) {
                Ok(result) => result,
                Err(err) => return Err(FsError::Other(err.error())),
            };
            let response = decode_callback_response(CALLBACK_REMOVE_FILE, &result);
            match response.kind.as_str() {
                "value" | "noop" => return Ok(()),
                "useOS" => return self.base.remove(path),
                _ => invalid_callback_response(CALLBACK_REMOVE_FILE, &response),
            }
        }
        if self.remove_file_noop {
            return Ok(());
        }
        self.base.remove(path)
    }

    // Go: callbackfs.go:474 Chtimes
    // Chtimes implements vfs.FS - always delegates to base (no callback support).
    fn chtimes(
        &self,
        path: &str,
        a_time: Option<SystemTime>,
        m_time: Option<SystemTime>,
    ) -> Result<(), FsError> {
        self.base.chtimes(path, a_time, m_time)
    }

    // Go: callbackfs.go:328 Stat
    // Stat implements vfs.FS.
    // ts#64447: the stat callback answers `{mode, size, mtime}` with a Node
    // mode and an RFC 3339 time.
    fn stat(&self, path: &str) -> Option<FileInfo> {
        self.panic_if_error(CALLBACK_STAT);
        if self.is_enabled(CALLBACK_STAT) {
            let response = self.call_decoded(CALLBACK_STAT, path);
            match response.kind.as_str() {
                "value" => {
                    let mut stat = RawStat::default();
                    unmarshal_value(&response.value, &mut stat);
                    let mod_time = match parse_rfc3339_nano(&stat.mtime) {
                        Ok(time) => time,
                        Err(err) => panic_error(&err),
                    };
                    return Some(FileInfo {
                        name: tspath::get_base_file_name(path),
                        size: stat.size,
                        mode: node_file_mode_to_go_file_mode(stat.mode),
                        mod_time: Some(mod_time),
                    });
                }
                "missing" => return None,
                "fakeStat" => return self.fake_stat_for_path(path),
                "useOS" => return self.base.stat(path),
                _ => invalid_callback_response(CALLBACK_STAT, &response),
            }
        }
        if self.fake_stat {
            return self.fake_stat_for_path(path);
        }
        self.base.stat(path)
    }

    // ts#64277: callbackFS.WalkDir is gone with vfs.FS.WalkDir (the program
    // lane removes `Fs::walk_dir`).
}

// Go: api/callbackfs_test.go (ts#64447)
// PORT: the Go tests use `vfstest.FromMap`; the library has no map file
// system, so `MapFs` holds files and their directories.
#[cfg(test)]
mod callbackfs_tests {
    use super::*;
    use crate::gostd::context;

    struct MapFs {
        files: RefCell<FxHashMap<String, String>>,
        case_sensitive: bool,
    }

    impl MapFs {
        fn new(files: &[(&str, &str)], case_sensitive: bool) -> Rc<MapFs> {
            Rc::new(MapFs {
                files: RefCell::new(
                    files
                        .iter()
                        .map(|(name, text)| (name.to_string(), text.to_string()))
                        .collect(),
                ),
                case_sensitive,
            })
        }
    }

    impl Fs for MapFs {
        fn use_case_sensitive_file_names(&self) -> bool {
            self.case_sensitive
        }
        fn file_exists(&self, path: &str) -> bool {
            self.files.borrow().contains_key(path)
        }
        fn read_file(&self, path: &str) -> (String, bool) {
            match self.files.borrow().get(path) {
                Some(text) => (text.clone(), true),
                None => (String::new(), false),
            }
        }
        fn write_file(&self, path: &str, data: &str) -> Result<(), FsError> {
            self.files
                .borrow_mut()
                .insert(path.to_string(), data.to_string());
            Ok(())
        }
        fn append_file(&self, _path: &str, _data: &str) -> Result<(), FsError> {
            unreachable!()
        }
        fn remove(&self, path: &str) -> Result<(), FsError> {
            self.files.borrow_mut().remove(path);
            Ok(())
        }
        fn chtimes(
            &self,
            _path: &str,
            _a_time: Option<SystemTime>,
            _m_time: Option<SystemTime>,
        ) -> Result<(), FsError> {
            unreachable!()
        }
        fn directory_exists(&self, path: &str) -> bool {
            let dir = tspath::ensure_trailing_directory_separator(path);
            self.files
                .borrow()
                .keys()
                .any(|name| name.starts_with(&dir))
        }
        fn get_accessible_entries(&self, _path: &str) -> Entries {
            Entries::default()
        }
        fn stat(&self, _path: &str) -> Option<FileInfo> {
            unreachable!()
        }
        fn realpath(&self, path: &str) -> String {
            path.to_string()
        }
    }

    // Go: callbackfs_test.go:16 callbackTestConn
    struct CallbackTestConn {
        responses: RefCell<FxHashMap<&'static str, &'static str>>,
    }

    impl CallbackTestConn {
        fn new(responses: &[(&'static str, &'static str)]) -> Rc<CallbackTestConn> {
            Rc::new(CallbackTestConn {
                responses: RefCell::new(responses.iter().copied().collect()),
            })
        }
        fn set(&self, method: &'static str, response: &'static str) {
            self.responses.borrow_mut().insert(method, response);
        }
    }

    impl Conn for CallbackTestConn {
        fn run(&self, _ctx: &Context) -> Result<(), GoError> {
            Ok(())
        }
        fn call(
            &self,
            _ctx: &Context,
            method: &str,
            _params: Option<Box<dyn AnyValue>>,
        ) -> Result<JsonValue, GoError> {
            let response = self.responses.borrow().get(method).copied();
            Ok(JsonValue(response.unwrap_or("").as_bytes().to_vec()))
        }
        fn notify(
            &self,
            _ctx: &Context,
            _method: &str,
            _params: Option<Box<dyn AnyValue>>,
        ) -> Result<(), GoError> {
            Ok(())
        }
    }

    fn callbacks(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    // Go: callbackfs_test.go:32 TestCallbackFSDefaults
    #[test]
    fn test_callback_fs_defaults() {
        let base = MapFs::new(&[("/file.ts", "content")], false);
        let fs = new_callback_fs(
            base,
            &callbacks(&["realpath:identity", "stat:fakeStat"]),
            Some(true),
        );

        assert!(
            fs.use_case_sensitive_file_names(),
            "expected configured case sensitivity"
        );
        assert_eq!(fs.realpath("/file.ts"), "/file.ts", "want identity");
        let info = fs.stat("/file.ts").expect("want inferred file");
        assert!(!info.is_dir() && info.size() == 0, "{info:?}");
        let info = fs.stat("/").expect("want inferred directory");
        assert!(info.is_dir(), "{info:?}");
        assert_eq!(fs.stat("/missing"), None);
    }

    // Go: callbackfs_test.go:58 TestCallbackFSStatAndEntries
    #[test]
    fn test_callback_fs_stat_and_entries() {
        let base = MapFs::new(&[], true);
        let fs = new_callback_fs(base, &callbacks(&["stat", "getAccessibleEntries"]), None);
        let conn = CallbackTestConn::new(&[
            (
                CALLBACK_STAT,
                r#"{"kind":"value","value":{"mode":33060,"size":12,"mtime":"2024-01-02T03:04:05.000Z"}}"#,
            ),
            (
                CALLBACK_GET_ACCESSIBLE_ENTRIES,
                r#"{"kind":"value","value":{"files":["link.ts"],"directories":["pkg"],"symlinks":["link.ts","pkg"]}}"#,
            ),
        ]);
        fs.set_connection(&context::background(), conn.clone());

        let info = fs.stat("/link.ts").expect("want callback file metadata");
        assert!(!info.is_dir() && info.size() == 12, "{info:?}");
        assert_eq!(
            info.mode(),
            FileMode(0o444),
            "want translated regular-file mode 0444"
        );
        // time.Date(2024, time.January, 2, 3, 4, 5, 0, time.UTC)
        let want_time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_704_164_645);
        assert_eq!(info.mod_time(), Some(want_time));
        conn.set(CALLBACK_STAT, r#"{"kind":"missing"}"#);
        assert_eq!(fs.stat("/missing.ts"), None);

        let entries = fs.get_accessible_entries("/");
        let symlinks = entries.symlinks.expect("expected symlink metadata");
        assert!(
            symlinks.contains("link.ts"),
            "expected file symlink metadata"
        );
        assert!(
            symlinks.contains("pkg"),
            "expected directory symlink metadata"
        );

        conn.set(
            CALLBACK_GET_ACCESSIBLE_ENTRIES,
            r#"{"kind":"value","value":{"files":[],"directories":["src"],"symlinks":[]}}"#,
        );
        let entries = fs.get_accessible_entries("/");
        assert!(
            entries.symlinks.is_some(),
            "explicitly empty symlink metadata was treated as unavailable"
        );
    }

    // Go: callbackfs_test.go:105 TestNodeFileModeToGoFileMode
    fn check_node_file_mode(node: u32, go_mode: FileMode) {
        let got = node_file_mode_to_go_file_mode(node);
        assert_eq!(
            got, go_mode,
            "nodeFileModeToGoFileMode({node:#o}) = {:#o}, want {:#o}",
            got.0, go_mode.0
        );
    }

    #[test]
    fn test_node_file_mode_to_go_file_mode_directory() {
        check_node_file_mode(0o040755, FileMode::DIR | FileMode(0o755));
    }

    #[test]
    fn test_node_file_mode_to_go_file_mode_regular() {
        check_node_file_mode(0o100644, FileMode(0o644));
    }

    #[test]
    fn test_node_file_mode_to_go_file_mode_symlink() {
        check_node_file_mode(0o120777, FileMode::SYMLINK | FileMode(0o777));
    }

    #[test]
    fn test_node_file_mode_to_go_file_mode_fifo() {
        check_node_file_mode(0o010600, FileMode::NAMED_PIPE | FileMode(0o600));
    }

    #[test]
    fn test_node_file_mode_to_go_file_mode_socket() {
        check_node_file_mode(0o140600, FileMode::SOCKET | FileMode(0o600));
    }

    #[test]
    fn test_node_file_mode_to_go_file_mode_setuid() {
        check_node_file_mode(0o104755, FileMode::SETUID | FileMode(0o755));
    }

    // Go: callbackfs_test.go:131 TestCallbackFSWriteFilePassthrough
    #[test]
    fn test_callback_fs_write_file_passthrough() {
        let base = MapFs::new(&[], true);
        let fs = new_callback_fs(base.clone(), &callbacks(&["writeFile"]), None);
        let conn = CallbackTestConn::new(&[(CALLBACK_WRITE_FILE, r#"{"kind":"useOS"}"#)]);
        fs.set_connection(&context::background(), conn.clone());

        fs.write_file("/use-os.ts", "content").unwrap();
        assert_eq!(
            base.read_file("/use-os.ts"),
            ("content".to_string(), true),
            "want OS filesystem content"
        );

        conn.set(CALLBACK_WRITE_FILE, r#"{"kind":"value"}"#);
        fs.write_file("/handled.ts", "content").unwrap();
        assert!(
            !base.read_file("/handled.ts").1,
            "handled callback write unexpectedly reached base filesystem"
        );

        conn.set(CALLBACK_WRITE_FILE, r#"{"kind":"noop"}"#);
        fs.write_file("/noop.ts", "content").unwrap();
        assert!(
            !base.read_file("/noop.ts").1,
            "noop callback write unexpectedly reached base filesystem"
        );
    }

    // Go: callbackfs_test.go:161 TestCallbackFSWriteFileNoop
    #[test]
    fn test_callback_fs_write_file_noop() {
        let base = MapFs::new(&[], true);
        let fs = new_callback_fs(base.clone(), &callbacks(&["writeFile:noop"]), None);

        fs.write_file("/ignored.ts", "content").unwrap();
        assert!(
            !base.read_file("/ignored.ts").1,
            "noop write unexpectedly reached base filesystem"
        );
    }

    // Go: callbackfs_test.go:175 TestCallbackFSPerCallFakeStat
    #[test]
    fn test_callback_fs_per_call_fake_stat() {
        let base = MapFs::new(&[], true);
        let fs = new_callback_fs(
            base,
            &callbacks(&["stat", "directoryExists", "fileExists"]),
            None,
        );
        fs.set_connection(
            &context::background(),
            CallbackTestConn::new(&[
                (CALLBACK_STAT, r#"{"kind":"fakeStat"}"#),
                (
                    CALLBACK_DIRECTORY_EXISTS,
                    r#"{"kind":"value","value":false}"#,
                ),
                (CALLBACK_FILE_EXISTS, r#"{"kind":"value","value":true}"#),
            ]),
        );

        let info = fs.stat("/virtual.ts").expect("want fake file stat");
        assert!(!info.is_dir(), "{info:?}");
    }

    // Go: callbackfs_test.go:194 TestCallbackFSPerCallIdentityRealpath
    #[test]
    fn test_callback_fs_per_call_identity_realpath() {
        let base = MapFs::new(&[], true);
        let fs = new_callback_fs(base, &callbacks(&["realpath"]), None);
        fs.set_connection(
            &context::background(),
            CallbackTestConn::new(&[(CALLBACK_REALPATH, r#"{"kind":"identity"}"#)]),
        );

        assert_eq!(fs.realpath("/virtual.ts"), "/virtual.ts", "want identity");
    }

    // Go: callbackfs_test.go:209 TestCallbackFSError
    #[test]
    fn test_callback_fs_error() {
        let names = [
            CALLBACK_READ_FILE,
            CALLBACK_FILE_EXISTS,
            CALLBACK_DIRECTORY_EXISTS,
            CALLBACK_GET_ACCESSIBLE_ENTRIES,
            CALLBACK_REALPATH,
            CALLBACK_STAT,
            CALLBACK_WRITE_FILE,
            CALLBACK_REMOVE_FILE,
        ];

        let error_callbacks: Vec<String> =
            names.iter().map(|name| format!("{name}:error")).collect();
        let base = MapFs::new(&[], true);
        let fs = new_callback_fs(base.clone(), &error_callbacks, None);
        for name in names {
            assert!(
                fs.error_callbacks.get(name).copied().unwrap_or(false),
                "{name} was not configured to panic"
            );
        }
        let message = crate::core::go_panic_text(|| {
            fs.read_file("/unexpected.ts");
        });
        assert!(
            message.contains("serverFS.error: readFile"),
            "panic = {message:?}"
        );

        let callback_fs = new_callback_fs(base, &callbacks(&["fileExists"]), None);
        callback_fs.set_connection(
            &context::background(),
            CallbackTestConn::new(&[(CALLBACK_FILE_EXISTS, r#"{"kind":"error"}"#)]),
        );
        let message = crate::core::go_panic_text(|| {
            callback_fs.file_exists("/unexpected.ts");
        });
        assert!(
            message.contains("serverFS.error: fileExists"),
            "panic = {message:?}"
        );
    }

    // Go: callbackfs_test.go:249 TestCallbackFSRemoveFileNoop
    #[test]
    fn test_callback_fs_remove_file_noop() {
        let base = MapFs::new(&[("/retained.ts", "content")], true);
        let fs = new_callback_fs(base.clone(), &callbacks(&["removeFile:noop"]), None);

        fs.remove("/retained.ts").unwrap();
        assert!(
            base.read_file("/retained.ts").1,
            "noop removal unexpectedly reached base filesystem"
        );
    }

    // PORT: no Go counterpart. Go `time.Parse(time.RFC3339Nano, ...)` for
    // the texts that a Node `Date.toISOString()` and an offset give.
    #[test]
    fn parse_rfc3339_nano_matches_go() {
        let at = |secs: u64, nanos: u32| SystemTime::UNIX_EPOCH + Duration::new(secs, nanos);
        assert_eq!(
            parse_rfc3339_nano("2024-01-02T03:04:05.000Z").unwrap(),
            at(1_704_164_645, 0)
        );
        assert_eq!(
            parse_rfc3339_nano("2024-01-02T03:04:05.123456789123Z").unwrap(),
            at(1_704_164_645, 123_456_789)
        );
        assert_eq!(
            parse_rfc3339_nano("2024-01-02T05:04:05+02:00").unwrap(),
            at(1_704_164_645, 0)
        );
        assert_eq!(
            parse_rfc3339_nano("1969-12-31T23:59:59.5Z").unwrap(),
            SystemTime::UNIX_EPOCH - Duration::from_millis(500)
        );
        assert!(parse_rfc3339_nano("2023-02-29T00:00:00Z").is_err());
        assert!(parse_rfc3339_nano("").is_err());
    }
}

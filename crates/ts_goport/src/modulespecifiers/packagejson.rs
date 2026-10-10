//! The parts of Go `internal/packagejson` that modulespecifiers uses.
//!
//! PORT: the full port in `src/frontend/packagejson.rs` is not thread-safe
//! (`Rc`), and module specifier generation runs on checker threads. This
//! file keeps the same data model (JSONValue, Expected, ExportsOrImports,
//! VersionPaths, InfoCacheEntry) as a thread-safe copy of a frontend value
//! (`PackageJson::of_frontend`), or with a small strict JSON parser:
//! - Duplicate object members are allowed and the last one wins, as with
//!   `json.AllowDuplicateNames(true)`. In objects kept as `JSONValue`, the
//!   member keeps the position of its first occurrence (OrderedMap.Set).
//! - Any syntax error makes the whole file unparseable (`parseable ==
//!   false`, empty fields), as a fatal Go v2 error does.

use crate::prelude::*;

use super::semver;
use std::sync::Arc;

// Go: packagejson/jsonvalue.go:10 JSONValueType
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum JSONValueType {
    #[default]
    NotPresent,
    Null,
    String,
    Number,
    Boolean,
    Array,
    Object,
}

impl JSONValueType {
    // Go: packagejson/jsonvalue.go:22 String
    pub fn string(self) -> &'static str {
        match self {
            JSONValueType::Null => "null",
            JSONValueType::String => "string",
            JSONValueType::Number => "number",
            JSONValueType::Boolean => "boolean",
            JSONValueType::Array => "array",
            JSONValueType::Object => "object",
            JSONValueType::NotPresent => "unknown(0)",
        }
    }
}

// Go: packagejson/jsonvalue.go:41 JSONValue
// PORT: Go keeps `Type` plus `Value any`. The Rust enum holds both.
// `ExportsOrImports` is the same tree (see below).
#[derive(Clone, Debug, Default, PartialEq)]
pub enum JSONValue {
    #[default]
    NotPresent,
    Null,
    String(String),
    Number(f64),
    Boolean(bool),
    Array(Vec<JSONValue>),
    Object(IndexMap<String, JSONValue>),
}

impl JSONValue {
    /// A copy of a frontend value (`PackageJson::of_frontend`).
    fn of_frontend(value: &crate::frontend::packagejson::JSONValue) -> Self {
        use crate::frontend::packagejson::{JSONValueType as Type, JsonAny};
        match &value.value {
            JsonAny::Nil if value.type_ == Type::NULL => JSONValue::Null,
            JsonAny::Nil => JSONValue::NotPresent,
            JsonAny::String(s) => JSONValue::String(s.clone()),
            JsonAny::Number(n) => JSONValue::Number(*n),
            JsonAny::Bool(b) => JSONValue::Boolean(*b),
            JsonAny::Array(a) => JSONValue::Array(a.iter().map(JSONValue::of_frontend).collect()),
            JsonAny::Object(o) => JSONValue::Object(
                o.iter()
                    .map(|(k, v)| (k.clone(), JSONValue::of_frontend(v)))
                    .collect(),
            ),
            JsonAny::ExportsArray(a) => JSONValue::Array(
                a.iter()
                    .map(|v| JSONValue::of_frontend(&v.json_value))
                    .collect(),
            ),
            JsonAny::ExportsObject(o) => JSONValue::Object(
                o.iter()
                    .map(|(k, v)| (k.clone(), JSONValue::of_frontend(&v.json_value)))
                    .collect(),
            ),
        }
    }

    pub fn type_(&self) -> JSONValueType {
        match self {
            JSONValue::NotPresent => JSONValueType::NotPresent,
            JSONValue::Null => JSONValueType::Null,
            JSONValue::String(_) => JSONValueType::String,
            JSONValue::Number(_) => JSONValueType::Number,
            JSONValue::Boolean(_) => JSONValueType::Boolean,
            JSONValue::Array(_) => JSONValueType::Array,
            JSONValue::Object(_) => JSONValueType::Object,
        }
    }

    // Go: packagejson/jsonvalue.go:46 IsPresent
    pub fn is_present(&self) -> bool {
        !matches!(self, JSONValue::NotPresent)
    }

    // Go: packagejson/jsonvalue.go:65 AsObject
    pub fn as_object(&self) -> &IndexMap<String, JSONValue> {
        match self {
            JSONValue::Object(o) => o,
            _ => panic!("expected object"),
        }
    }

    // Go: packagejson/jsonvalue.go:72 AsArray
    pub fn as_array(&self) -> &[JSONValue] {
        match self {
            JSONValue::Array(a) => a,
            _ => panic!("expected array"),
        }
    }

    // Go: packagejson/jsonvalue.go:79 AsString
    pub fn as_string(&self) -> &str {
        match self {
            JSONValue::String(s) => s,
            _ => panic!("expected string"),
        }
    }

    // Go: packagejson/exportsorimports.go:43 IsSubpaths
    pub fn is_subpaths(&self) -> bool {
        self.object_kind() == ObjectKind::Subpaths
    }

    // Go: packagejson/exportsorimports.go:48 IsImports
    pub fn is_imports(&self) -> bool {
        self.object_kind() == ObjectKind::Imports
    }

    // Go: packagejson/exportsorimports.go:53 IsConditions
    pub fn is_conditions(&self) -> bool {
        self.object_kind() == ObjectKind::Conditions
    }

    // Go: packagejson/exportsorimports.go:58 initObjectKind
    // PORT: Go caches the kind on a copy of the value, so it is computed on
    // every call there too.
    fn object_kind(&self) -> ObjectKind {
        let JSONValue::Object(obj) = self else {
            return ObjectKind::Unknown;
        };
        if !obj.is_empty() {
            let (mut seen_dot, mut seen_hash, mut seen_other) = (false, false, false);
            for k in obj.keys() {
                let b = k.as_bytes();
                if !b.is_empty() {
                    seen_dot = seen_dot || b[0] == b'.';
                    seen_hash = seen_hash || b[0] == b'#';
                    seen_other = seen_other || (b[0] != b'.' && b[0] != b'#');
                    if seen_other && (seen_dot || seen_hash) {
                        return ObjectKind::Invalid;
                    }
                }
            }
            if seen_dot {
                return ObjectKind::Subpaths;
            }
            if seen_hash {
                return ObjectKind::Imports;
            }
        }
        ObjectKind::Conditions
    }
}

// Go: packagejson/exportsorimports.go:8 objectKind
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ObjectKind {
    Unknown,
    Subpaths,
    Conditions,
    Imports,
    Invalid,
}

// Go: packagejson/exportsorimports.go:18 ExportsOrImports
// PORT: Go embeds JSONValue and adds a cached object kind. The kind is
// computed on demand, so the type is the same tree.
pub type ExportsOrImports = JSONValue;

// Go: packagejson/expected.go:9 Expected
#[derive(Clone, Debug, Default)]
pub struct Expected<T> {
    pub null: bool,
    pub valid: bool,
    pub value: T,
}

impl<T: Clone> Expected<T> {
    // Go: packagejson/expected.go:43 GetValue
    pub fn get_value(&self) -> (T, bool) {
        (self.value.clone(), self.valid)
    }
}

// Go: packagejson/packagejson.go:8 HeaderFields, PathFields, DependencyFields
#[derive(Clone, Debug, Default)]
pub struct Fields {
    pub name: Expected<String>,
    pub version: Expected<String>,
    pub type_: Expected<String>,
    pub main: Expected<String>,
    pub types: Expected<String>,
    pub typings: Expected<String>,
    pub types_versions: JSONValue,
    pub imports: ExportsOrImports,
    pub exports: ExportsOrImports,
    pub dependencies: Expected<IndexMap<String, String>>,
    pub dev_dependencies: Expected<IndexMap<String, String>>,
    pub peer_dependencies: Expected<IndexMap<String, String>>,
    pub optional_dependencies: Expected<IndexMap<String, String>>,
}

impl Fields {
    // Go: packagejson/packagejson.go:89 GetRuntimeDependencyNames
    // PORT: Go returns a Set with random iteration order. This keeps the
    // order of the maps (the package.json order, or the frontend map order
    // for `PackageJson::of_frontend`).
    pub fn get_runtime_dependency_names(&self) -> IndexSet<String> {
        let mut names = IndexSet::default();
        for name in self.dependencies.value.keys() {
            names.insert(name.clone());
        }
        for name in self.peer_dependencies.value.keys() {
            names.insert(name.clone());
        }
        for name in self.optional_dependencies.value.keys() {
            names.insert(name.clone());
        }
        names
    }
}

// Go: packagejson/packagejson.go:127 Parse
// PORT: `data` is the file's Go bytes. Go fails on invalid UTF-8 in a
// string. Each string value is in the port form (see
// `scanner_util::GO_STRING_MARKER`).
pub fn parse(data: &[u8]) -> Result<Fields, String> {
    let value = JsonParser { s: data, pos: 0 }.parse_document()?;
    let mut f = Fields::default();
    let JSONValue::Object(obj) = value else {
        return Err("package.json must be an object".to_string());
    };
    // PORT: `obj` already holds the last value of each duplicate member.
    for (key, v) in obj {
        match key.as_str() {
            "name" => f.name = expected_string(v),
            "version" => f.version = expected_string(v),
            "type" => f.type_ = expected_string(v),
            "main" => f.main = expected_string(v),
            "types" => f.types = expected_string(v),
            "typings" => f.typings = expected_string(v),
            "typesVersions" => f.types_versions = v,
            "imports" => f.imports = v,
            "exports" => f.exports = v,
            "dependencies" => f.dependencies = expected_string_map(v),
            "devDependencies" => f.dev_dependencies = expected_string_map(v),
            "peerDependencies" => f.peer_dependencies = expected_string_map(v),
            "optionalDependencies" => f.optional_dependencies = expected_string_map(v),
            _ => {}
        }
    }
    Ok(f)
}

// Go: packagejson/expected.go:16 UnmarshalJSON
fn expected_string(v: JSONValue) -> Expected<String> {
    match v {
        JSONValue::Null => Expected {
            null: true,
            valid: false,
            value: String::new(),
        },
        JSONValue::String(s) => Expected {
            null: false,
            valid: true,
            value: s,
        },
        _ => Expected::default(),
    }
}

// Go: packagejson/expected.go:16 UnmarshalJSON
// PORT: Go decodes the map up to the first non-string value and keeps that
// partial map with `Valid == false`. This keeps the entries before the
// first non-string value too.
fn expected_string_map(v: JSONValue) -> Expected<IndexMap<String, String>> {
    match v {
        JSONValue::Null => Expected {
            null: true,
            valid: false,
            value: IndexMap::default(),
        },
        JSONValue::Object(obj) => {
            let mut map = IndexMap::default();
            for (k, v) in obj {
                match v {
                    JSONValue::String(s) => {
                        map.insert(k, s);
                    }
                    _ => {
                        return Expected {
                            null: false,
                            valid: false,
                            value: map,
                        };
                    }
                }
            }
            Expected {
                null: false,
                valid: true,
                value: map,
            }
        }
        _ => Expected::default(),
    }
}

// Go: packagejson/cache.go:15 PackageJson
#[derive(Debug, Default)]
pub struct PackageJson {
    pub fields: Fields,
    pub parseable: bool,
    version_paths: std::sync::OnceLock<VersionPaths>,
}

impl PackageJson {
    pub fn new(fields: Fields, parseable: bool) -> PackageJson {
        PackageJson {
            fields,
            parseable,
            version_paths: std::sync::OnceLock::new(),
        }
    }

    /// A copy of `package_json`, a package.json that the frontend resolver
    /// read (Go has one type). Module specifier generation uses it on the
    /// loading thread (`modulespecifiers::host`).
    // PORT: the frontend dependency maps have no order, so the dependency
    // fields of a copy are in that map order, not the package.json order.
    pub(crate) fn of_frontend(package_json: &crate::frontend::packagejson::PackageJson) -> Self {
        use crate::frontend::packagejson::Expected as FrontendExpected;
        fn string(e: &FrontendExpected<String>) -> Expected<String> {
            Expected {
                null: e.null,
                valid: e.valid,
                value: e.value.clone(),
            }
        }
        fn string_map(
            e: &FrontendExpected<FxHashMap<String, String>>,
        ) -> Expected<IndexMap<String, String>> {
            Expected {
                null: e.null,
                valid: e.valid,
                value: e
                    .value
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            }
        }
        let f = &package_json.fields;
        let (header, paths, deps) = (&f.header_fields, &f.path_fields, &f.dependency_fields);
        let fields = Fields {
            name: string(&header.name),
            version: string(&header.version),
            type_: string(&header.type_),
            main: string(&paths.main),
            types: string(&paths.types),
            typings: string(&paths.typings),
            types_versions: JSONValue::of_frontend(&paths.types_versions),
            imports: JSONValue::of_frontend(&paths.imports.json_value),
            exports: JSONValue::of_frontend(&paths.exports.json_value),
            dependencies: string_map(&deps.dependencies),
            dev_dependencies: string_map(&deps.dev_dependencies),
            peer_dependencies: string_map(&deps.peer_dependencies),
            optional_dependencies: string_map(&deps.optional_dependencies),
        };
        PackageJson::new(fields, package_json.parseable)
    }

    // Go: packagejson/cache.go:28 GetVersionPaths
    // PORT: modulespecifiers passes a nil trace, so the traces are not kept.
    pub fn get_version_paths(&self) -> &VersionPaths {
        self.version_paths.get_or_init(|| {
            let mut result = VersionPaths::default();
            let JSONValue::Object(types_versions) = &self.fields.types_versions else {
                return result;
            };
            let type_script_version = semver::must_parse_version(TYPE_SCRIPT_VERSION);
            for (key, value) in types_versions {
                let (key_range, ok) = semver::try_parse_version_range(key);
                if !ok {
                    continue;
                }
                if key_range.test(&type_script_version) {
                    if let JSONValue::Object(paths) = value {
                        result = VersionPaths {
                            version: key.clone(),
                            paths_json: Some(paths.clone()),
                        };
                    }
                    return result;
                }
            }
            result
        })
    }
}

// Go: core/version.go:8 version
pub(crate) const TYPE_SCRIPT_VERSION: &str = crate::core::VERSION;

// Go: packagejson/cache.go:88 VersionPaths
#[derive(Clone, Debug, Default)]
pub struct VersionPaths {
    pub version: String,
    paths_json: Option<IndexMap<String, JSONValue>>,
}

impl VersionPaths {
    // Go: packagejson/cache.go:94 Exists
    pub fn exists(&self) -> bool {
        !self.version.is_empty() && self.paths_json.is_some()
    }

    // Go: packagejson/cache.go:98 GetPaths
    // PORT: Go caches the result; this builds it on each call.
    pub fn get_paths(&self) -> Option<IndexMap<String, Option<Vec<String>>>> {
        if !self.exists() {
            return None;
        }
        let paths_json = self.paths_json.as_ref()?;
        let mut paths = IndexMap::default();
        for (key, value) in paths_json {
            let JSONValue::Array(arr) = value else {
                continue;
            };
            let mut slice = vec![String::new(); arr.len()];
            for (i, path) in arr.iter().enumerate() {
                if let JSONValue::String(s) = path {
                    slice[i] = s.clone();
                }
            }
            paths.insert(key.clone(), Some(slice));
        }
        Some(paths)
    }
}

// Go: packagejson/cache.go:182 InfoCacheEntry
// PORT: Go `Contents *PackageJson` is an `Arc`, so the copy that
// `with_package_directory` makes shares it, as Go's shallow copy does.
#[derive(Debug, Default)]
pub struct InfoCacheEntry {
    pub package_directory: String,
    pub directory_exists: bool,
    pub contents: Option<Arc<PackageJson>>,
}

impl InfoCacheEntry {
    // Go: packagejson/cache.go:130 Exists
    pub fn exists(&self) -> bool {
        self.contents.is_some()
    }

    // Go: packagejson/cache.go:192 GetContents
    pub fn get_contents(&self) -> Option<&PackageJson> {
        self.contents.as_deref()
    }

    // Go: packagejson/cache.go:140 GetDirectory (at 673a5f17d713;
    // removed by ts#64159; the entry keeps a PackageDirectory, packagejson/cache.go:123)
    pub fn get_directory(&self) -> &str {
        &self.package_directory
    }

    // Go: packagejson/cache.go:208 WithPackageDirectory
    // WithPackageDirectory returns an entry whose PackageDirectory matches the
    // caller's value. The package.json info cache is keyed by the canonical
    // path of the package.json file, so a lookup with another spelling of
    // the directory (another case on a case-insensitive file system, or a
    // trailing separator) gets the entry of the first spelling.
    #[must_use]
    pub fn with_package_directory(self: &Arc<Self>, package_directory: &str) -> Arc<Self> {
        if self.package_directory == package_directory {
            return self.clone();
        }
        Arc::new(InfoCacheEntry {
            package_directory: package_directory.to_string(),
            directory_exists: self.directory_exists,
            contents: self.contents.clone(),
        })
    }
}

/// Strict RFC 8259 JSON parser for package.json text.
struct JsonParser<'a> {
    s: &'a [u8],
    pos: usize,
}

impl JsonParser<'_> {
    fn parse_document(mut self) -> Result<JSONValue, String> {
        // A UTF-8 byte order mark is not valid JSON, but the Go file reader
        // strips it before decoding.
        if self.s.starts_with(&[0xEF, 0xBB, 0xBF]) {
            self.pos = 3;
        }
        let v = self.parse_value()?;
        self.skip_ws();
        if self.pos != self.s.len() {
            return Err(self.err("unexpected data after top-level value"));
        }
        Ok(v)
    }

    fn err(&self, msg: &str) -> String {
        format!("{msg} at offset {}", self.pos)
    }

    fn skip_ws(&mut self) {
        while let Some(&c) = self.s.get(self.pos) {
            if c == b' ' || c == b'\t' || c == b'\n' || c == b'\r' {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn expect_literal(&mut self, lit: &str, v: JSONValue) -> Result<JSONValue, String> {
        if self.s[self.pos..].starts_with(lit.as_bytes()) {
            self.pos += lit.len();
            Ok(v)
        } else {
            Err(self.err("invalid literal"))
        }
    }

    fn parse_value(&mut self) -> Result<JSONValue, String> {
        self.skip_ws();
        match self.s.get(self.pos) {
            None => Err(self.err("unexpected end of input")),
            Some(b'{') => self.parse_object(),
            Some(b'[') => self.parse_array(),
            Some(b'"') => Ok(JSONValue::String(self.parse_string()?)),
            Some(b't') => self.expect_literal("true", JSONValue::Boolean(true)),
            Some(b'f') => self.expect_literal("false", JSONValue::Boolean(false)),
            Some(b'n') => self.expect_literal("null", JSONValue::Null),
            Some(b'-' | b'0'..=b'9') => self.parse_number(),
            Some(_) => Err(self.err("invalid character")),
        }
    }

    fn parse_object(&mut self) -> Result<JSONValue, String> {
        self.pos += 1;
        let mut obj: IndexMap<String, JSONValue> = IndexMap::default();
        self.skip_ws();
        if self.s.get(self.pos) == Some(&b'}') {
            self.pos += 1;
            return Ok(JSONValue::Object(obj));
        }
        loop {
            self.skip_ws();
            if self.s.get(self.pos) != Some(&b'"') {
                return Err(self.err("expected object name"));
            }
            let key = self.parse_string()?;
            self.skip_ws();
            if self.s.get(self.pos) != Some(&b':') {
                return Err(self.err("expected ':'"));
            }
            self.pos += 1;
            let v = self.parse_value()?;
            obj.insert(key, v);
            self.skip_ws();
            match self.s.get(self.pos) {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(JSONValue::Object(obj));
                }
                _ => return Err(self.err("expected ',' or '}'")),
            }
        }
    }

    fn parse_array(&mut self) -> Result<JSONValue, String> {
        self.pos += 1;
        let mut arr = Vec::new();
        self.skip_ws();
        if self.s.get(self.pos) == Some(&b']') {
            self.pos += 1;
            return Ok(JSONValue::Array(arr));
        }
        loop {
            arr.push(self.parse_value()?);
            self.skip_ws();
            match self.s.get(self.pos) {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    return Ok(JSONValue::Array(arr));
                }
                _ => return Err(self.err("expected ',' or ']'")),
            }
        }
    }

    fn parse_hex4(&mut self) -> Result<u32, String> {
        let hex = self
            .s
            .get(self.pos..self.pos + 4)
            .ok_or_else(|| self.err("short unicode escape"))?;
        let text = std::str::from_utf8(hex).map_err(|_| self.err("invalid unicode escape"))?;
        let v = u32::from_str_radix(text, 16).map_err(|_| self.err("invalid unicode escape"))?;
        self.pos += 4;
        Ok(v)
    }

    fn parse_string(&mut self) -> Result<String, String> {
        self.pos += 1;
        let mut out: Vec<u8> = Vec::new();
        loop {
            let Some(&c) = self.s.get(self.pos) else {
                return Err(self.err("unterminated string"));
            };
            match c {
                b'"' => {
                    self.pos += 1;
                    return String::from_utf8(out)
                        .map(crate::scanner_util::go_string_from_utf8)
                        .map_err(|_| self.err("invalid UTF-8"));
                }
                b'\\' => {
                    self.pos += 1;
                    let Some(&e) = self.s.get(self.pos) else {
                        return Err(self.err("unterminated escape"));
                    };
                    self.pos += 1;
                    let ch = match e {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            let hi = self.parse_hex4()?;
                            let code = if (0xD800..0xDC00).contains(&hi)
                                && self.s[self.pos..].starts_with(b"\\u")
                            {
                                let save = self.pos;
                                self.pos += 2;
                                let lo = self.parse_hex4()?;
                                if (0xDC00..0xE000).contains(&lo) {
                                    0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
                                } else {
                                    self.pos = save;
                                    0xFFFD
                                }
                            } else {
                                hi
                            };
                            char::from_u32(code).unwrap_or('\u{FFFD}')
                        }
                        _ => return Err(self.err("invalid escape")),
                    };
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                }
                0..=0x1F => return Err(self.err("control character in string")),
                _ => {
                    out.push(c);
                    self.pos += 1;
                }
            }
        }
    }

    fn parse_number(&mut self) -> Result<JSONValue, String> {
        let start = self.pos;
        if self.s.get(self.pos) == Some(&b'-') {
            self.pos += 1;
        }
        match self.s.get(self.pos) {
            Some(b'0') => self.pos += 1,
            Some(b'1'..=b'9') => {
                while matches!(self.s.get(self.pos), Some(b'0'..=b'9')) {
                    self.pos += 1;
                }
            }
            _ => return Err(self.err("invalid number")),
        }
        if self.s.get(self.pos) == Some(&b'.') {
            self.pos += 1;
            if !matches!(self.s.get(self.pos), Some(b'0'..=b'9')) {
                return Err(self.err("invalid number"));
            }
            while matches!(self.s.get(self.pos), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        if matches!(self.s.get(self.pos), Some(b'e' | b'E')) {
            self.pos += 1;
            if matches!(self.s.get(self.pos), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if !matches!(self.s.get(self.pos), Some(b'0'..=b'9')) {
                return Err(self.err("invalid number"));
            }
            while matches!(self.s.get(self.pos), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        let text = std::str::from_utf8(&self.s[start..self.pos])
            .map_err(|_| self.err("invalid number"))?;
        text.parse::<f64>()
            .map(JSONValue::Number)
            .map_err(|_| self.err("invalid number"))
    }
}

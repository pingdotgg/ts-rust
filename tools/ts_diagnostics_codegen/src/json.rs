//! A small JSON reader that replaces `serde_json` (lust has no proc macros).
//! [`parse`] reads text into a [`Value`] tree, and [`FromJson`] turns a value
//! into a Rust value with the serde rules that the catalog entries used.

use std::collections::BTreeMap;

/// Nesting limit, the same as `serde_json`. It keeps deep input off the stack.
const MAX_DEPTH: usize = 128;

/// A parsed JSON value. Objects keep their keys in source order.
pub enum Value {
    Null,
    Bool(bool),
    /// The number as written in the source.
    Number(String),
    String(String),
    #[allow(dead_code)] // The catalog has no array fields.
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
}

/// Parses one complete JSON text.
///
/// # Errors
///
/// Returns a message with the line and column of the first syntax error.
pub fn parse(text: &str) -> Result<Value, String> {
    let mut parser = Parser {
        text,
        bytes: text.as_bytes(),
        pos: 0,
        depth: 0,
    };
    let value = parser.value()?;
    parser.skip_whitespace();
    if parser.pos < text.len() {
        return Err(parser.error("trailing characters"));
    }
    Ok(value)
}

struct Parser<'a> {
    text: &'a str,
    bytes: &'a [u8],
    pos: usize,
    depth: usize,
}

impl Parser<'_> {
    fn error(&self, message: &str) -> String {
        let before = &self.bytes[..self.pos];
        let line = before.split(|&byte| byte == b'\n').count();
        let column = before
            .iter()
            .rev()
            .take_while(|&&byte| byte != b'\n')
            .count()
            + 1;
        format!("{message} at line {line} column {column}")
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    /// Steps over `byte` when it is next and reports whether it was.
    fn eat(&mut self, byte: u8) -> bool {
        let found = self.peek() == Some(byte);
        self.pos += usize::from(found);
        found
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn value(&mut self) -> Result<Value, String> {
        self.skip_whitespace();
        match self.peek() {
            Some(open @ (b'{' | b'[')) => {
                if self.depth == MAX_DEPTH {
                    return Err(self.error("recursion limit exceeded"));
                }
                self.depth += 1;
                let value = if open == b'{' {
                    self.list(b'}', Self::member).map(Value::Object)
                } else {
                    self.list(b']', Self::value).map(Value::Array)
                };
                self.depth -= 1;
                value
            }
            Some(b'"') => self.string().map(Value::String),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(b't') => self.literal("true", Value::Bool(true)),
            Some(b'f') => self.literal("false", Value::Bool(false)),
            Some(b'n') => self.literal("null", Value::Null),
            Some(_) => Err(self.error("expected value")),
            None => Err(self.error("EOF while parsing a value")),
        }
    }

    fn literal(&mut self, word: &str, value: Value) -> Result<Value, String> {
        if !self.bytes[self.pos..].starts_with(word.as_bytes()) {
            return Err(self.error("expected value"));
        }
        self.pos += word.len();
        Ok(value)
    }

    /// Reads the comma-separated items of an array or object, from its open
    /// byte through `close`. `item` reads one item.
    fn list<T>(
        &mut self,
        close: u8,
        mut item: impl FnMut(&mut Self) -> Result<T, String>,
    ) -> Result<Vec<T>, String> {
        let mut items = Vec::new();
        self.pos += 1;
        self.skip_whitespace();
        if self.eat(close) {
            return Ok(items);
        }
        loop {
            items.push(item(self)?);
            self.skip_whitespace();
            if self.eat(close) {
                return Ok(items);
            }
            if !self.eat(b',') {
                return Err(self.error(&format!("expected `,` or `{}`", char::from(close))));
            }
        }
    }

    /// Reads one `"key": value` member of an object.
    fn member(&mut self) -> Result<(String, Value), String> {
        self.skip_whitespace();
        if self.peek() != Some(b'"') {
            return Err(self.error("expected a string key"));
        }
        let key = self.string()?;
        self.skip_whitespace();
        if !self.eat(b':') {
            return Err(self.error("expected `:`"));
        }
        Ok((key, self.value()?))
    }

    fn string(&mut self) -> Result<String, String> {
        self.pos += 1;
        let mut output = String::new();
        loop {
            let start = self.pos;
            while self
                .peek()
                .is_some_and(|byte| byte != b'"' && byte != b'\\' && byte >= 0x20)
            {
                self.pos += 1;
            }
            // The run stops at an ASCII byte or the end, so both ends are char boundaries.
            output.push_str(&self.text[start..self.pos]);
            match self.peek() {
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(output);
                }
                Some(b'\\') => {
                    self.pos += 1;
                    output.push(self.escape()?);
                }
                Some(_) => return Err(self.error("control character in string")),
                None => return Err(self.error("EOF while parsing a string")),
            }
        }
    }

    /// Reads the escape after a `\`, joining a `\uXXXX` surrogate pair.
    fn escape(&mut self) -> Result<char, String> {
        let byte = self
            .peek()
            .ok_or_else(|| self.error("EOF while parsing a string"))?;
        self.pos += 1;
        Ok(match byte {
            b'"' | b'\\' | b'/' => char::from(byte),
            b'b' => '\u{8}',
            b'f' => '\u{c}',
            b'n' => '\n',
            b'r' => '\r',
            b't' => '\t',
            b'u' => {
                let first = self.hex4()?;
                let code = match first {
                    0xD800..=0xDBFF => {
                        if !(self.eat(b'\\') && self.eat(b'u')) {
                            return Err(self.error("lone leading surrogate in hex escape"));
                        }
                        let second = self.hex4()?;
                        if !(0xDC00..=0xDFFF).contains(&second) {
                            return Err(self.error("invalid surrogate pair in hex escape"));
                        }
                        0x10000 + ((first - 0xD800) << 10) + (second - 0xDC00)
                    }
                    0xDC00..=0xDFFF => {
                        return Err(self.error("lone trailing surrogate in hex escape"));
                    }
                    _ => first,
                };
                char::from_u32(code).ok_or_else(|| self.error("invalid hex escape"))?
            }
            _ => return Err(self.error("invalid escape")),
        })
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let digits = self.text.get(self.pos..self.pos + 4);
        let code = digits
            .filter(|digits| digits.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .and_then(|digits| u32::from_str_radix(digits, 16).ok())
            .ok_or_else(|| self.error("invalid hex escape"))?;
        self.pos += 4;
        Ok(code)
    }

    /// Reads a number with the JSON grammar: `-? (0 | [1-9][0-9]*) frac? exp?`.
    fn number(&mut self) -> Result<Value, String> {
        let start = self.pos;
        self.eat(b'-');
        if !self.eat(b'0') {
            self.digits()?;
        }
        if self.eat(b'.') {
            self.digits()?;
        }
        if self.eat(b'e') || self.eat(b'E') {
            if !self.eat(b'+') {
                self.eat(b'-');
            }
            self.digits()?;
        }
        Ok(Value::Number(self.text[start..self.pos].to_owned()))
    }

    fn digits(&mut self) -> Result<(), String> {
        let start = self.pos;
        while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
            self.pos += 1;
        }
        if self.pos == start {
            return Err(self.error("invalid number"));
        }
        Ok(())
    }
}

/// Converts a JSON value into a Rust value, like a serde `Deserialize` impl.
/// An error starts with the path inside the value (`.field`, `["key"]`) or `: `.
pub trait FromJson: Sized {
    fn from_json(value: &Value) -> Result<Self, String>;
}

/// The error of a value that has the wrong JSON type.
fn expected<T>(what: &str) -> Result<T, String> {
    Err(format!(": expected {what}"))
}

impl Value {
    /// Returns the fields of an object that is read into a struct.
    pub fn fields(&self) -> Result<Fields<'_>, String> {
        match self {
            Self::Object(fields) => Ok(Fields(fields)),
            _ => expected("an object"),
        }
    }
}

/// The fields of a JSON object. Unknown fields are ignored, as in serde.
pub struct Fields<'a>(&'a [(String, Value)]);

impl Fields<'_> {
    fn read<T: FromJson>(
        &self,
        key: &str,
        missing: impl FnOnce() -> Option<T>,
    ) -> Result<T, String> {
        let mut found = self
            .0
            .iter()
            .filter(|(name, _)| name == key)
            .map(|(_, value)| value);
        match (found.next(), found.next()) {
            (Some(_), Some(_)) => Err(format!(".{key}: duplicate field")),
            (Some(value), None) => T::from_json(value).map_err(|error| format!(".{key}{error}")),
            (None, _) => missing().ok_or_else(|| format!(".{key}: missing field")),
        }
    }

    /// Reads a field that must be present.
    pub fn required<T: FromJson>(&self, key: &str) -> Result<T, String> {
        self.read(key, || None)
    }

    /// Reads a field that may be missing and then is `T::default()`: a
    /// `#[serde(default)]` field, or an `Option` field (serde gives `None`).
    pub fn optional<T: FromJson + Default>(&self, key: &str) -> Result<T, String> {
        self.read(key, || Some(T::default()))
    }
}

impl FromJson for String {
    fn from_json(value: &Value) -> Result<Self, String> {
        match value {
            Value::String(text) => Ok(text.clone()),
            _ => expected("a string"),
        }
    }
}

impl FromJson for bool {
    fn from_json(value: &Value) -> Result<Self, String> {
        match value {
            Value::Bool(flag) => Ok(*flag),
            _ => expected("a boolean"),
        }
    }
}

impl FromJson for u32 {
    fn from_json(value: &Value) -> Result<Self, String> {
        match value {
            Value::Number(text) => text.parse().or_else(|_| expected("a u32")),
            _ => expected("a u32"),
        }
    }
}

/// Later duplicate keys replace earlier ones, as in serde.
impl<T: FromJson> FromJson for BTreeMap<String, T> {
    fn from_json(value: &Value) -> Result<Self, String> {
        let Value::Object(fields) = value else {
            return expected("an object");
        };
        let entry = |(key, item): &(String, Value)| {
            let item = T::from_json(item).map_err(|error| format!("[{key:?}]{error}"))?;
            Ok((key.clone(), item))
        };
        fields.iter().map(entry).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{FromJson, Value, parse};

    #[test]
    fn reads_escapes_surrogate_pairs_and_numbers() {
        let value = parse(r#" ["a\"\\\/\b\f\n\r\t", "\u00e9\ud83d\ude00ü", 6385] "#).unwrap();
        let Value::Array(items) = value else {
            panic!("expected an array");
        };
        assert_eq!(
            String::from_json(&items[0]).unwrap(),
            "a\"\\/\u{8}\u{c}\n\r\t"
        );
        assert_eq!(String::from_json(&items[1]).unwrap(), "é😀ü");
        assert_eq!(u32::from_json(&items[2]).unwrap(), 6385);
        for number in ["-1", "1.5", "1e3", "4294967296"] {
            assert!(u32::from_json(&parse(number).unwrap()).is_err(), "{number}");
        }
    }

    #[test]
    fn rejects_bad_input_with_a_position() {
        let deep = "[".repeat(200);
        for (text, error) in [
            ("", "EOF while parsing a value at line 1 column 1"),
            ("{\"a\": 1,}", "expected a string key at line 1 column 9"),
            ("{\"a\" 1}", "expected `:` at line 1 column 6"),
            ("[\n  tru]", "expected value at line 2 column 3"),
            ("01", "trailing characters at line 1 column 2"),
            ("[1e]", "invalid number at line 1 column 4"),
            ("\"\\x\"", "invalid escape at line 1 column 4"),
            (
                "\"\\ud800\\u0041\"",
                "invalid surrogate pair in hex escape at line 1 column 14",
            ),
            (
                "\"\\udc00\"",
                "lone trailing surrogate in hex escape at line 1 column 8",
            ),
            ("\"a\tb\"", "control character in string at line 1 column 3"),
            (&deep, "recursion limit exceeded at line 1 column 129"),
        ] {
            assert_eq!(parse(text).err().as_deref(), Some(error), "{text:?}");
        }
    }
}

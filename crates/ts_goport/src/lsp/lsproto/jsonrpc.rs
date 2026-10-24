//! Port of internal/lsp/lsproto/jsonrpc.go.
//!
//! PORT: Go `any` fields (`Message.msg`, `RequestMessage.Params`,
//! `ResponseMessage.Result`) are `Option<Box<dyn AnyValue>>` (`None` is
//! nil). An inbound message keeps its params as the raw `JsonValue`; the
//! handler decodes them (`lsp.rs` `unmarshal_params`). Go `UnmarshalJSON`
//! methods are kept as `unmarshal_json`, which returns the `GoError` chain
//! (`errors::is(&err, &from_value(ErrorCode::X))` works on it). The
//! `UnmarshalerFrom` impls call them and keep only the error text, so
//! callers that test the error code call `unmarshal_json`.

use crate::lsp::lsproto::prelude::*;

// Go: `fmt.Errorf("%w: %w", code, err)`.
pub(crate) fn wrap_error_code(code: ErrorCode, err: GoError) -> GoError {
    let code = gostd::errors::from_value(code);
    gostd::errors::errorf(
        format!("{}: {}", code.error(), err.error()),
        vec![code, err],
    )
}

// Go: jsonrpc.go:12 NewID
// NewID creates an ID from an IntegerOrString value.
// This wrapper exists because lsproto has its own IntegerOrString type.
// PORT: Go returns `*jsonrpc.ID`; it is never nil, so the Rust result is
// the value.
pub fn new_id(raw_value: &IntegerOrString) -> crate::jsonrpc::ID {
    if let Some(s) = &raw_value.string {
        return crate::jsonrpc::new_id_string(s);
    }
    crate::jsonrpc::new_id_int(
        raw_value
            .integer
            // Go loads `*rawValue.Integer`: addr=0x0 (jsonrpc.go:16).
            .unwrap_or_else(|| crate::core::go_nil_dereference_at(0x0)),
    )
}

// Go: jsonrpc.go:19 Message
#[derive(Debug, Default)]
pub struct Message {
    pub kind: crate::jsonrpc::MessageKind,
    msg: Option<Box<dyn AnyValue>>,
}

/// The Go runtime panic of a failed type assertion `m.msg.(want)`. The
/// message holds nil, a `*RequestMessage` or a `*ResponseMessage`.
#[cold]
#[track_caller]
fn interface_conversion_panic(msg: Option<&dyn AnyValue>, want: &str) -> ! {
    let have = match msg {
        None => "nil",
        Some(m) if m.downcast_ref::<RequestMessage>().is_some() => "*lsproto.RequestMessage",
        Some(_) => "*lsproto.ResponseMessage",
    };
    crate::core::go_panic(format!(
        "interface conversion: interface {{}} is {have}, not {want}"
    ))
}

impl Message {
    // Go: jsonrpc.go:24 AsRequest
    pub fn as_request(&self) -> &RequestMessage {
        let msg = self.msg.as_deref();
        msg.and_then(|m| m.downcast_ref::<RequestMessage>())
            .unwrap_or_else(|| interface_conversion_panic(msg, "*lsproto.RequestMessage"))
    }

    // Go: jsonrpc.go:28 AsResponse
    pub fn as_response(&self) -> &ResponseMessage {
        let msg = self.msg.as_deref();
        msg.and_then(|m| m.downcast_ref::<ResponseMessage>())
            .unwrap_or_else(|| interface_conversion_panic(msg, "*lsproto.ResponseMessage"))
    }

    // PORT: Go shares the `*RequestMessage` pointer after `AsRequest`; Rust
    // moves the message out. It panics where `AsRequest` panics.
    pub fn into_request(self) -> RequestMessage {
        self.as_request();
        let msg: Box<dyn std::any::Any> = self.msg.expect("as_request checked it");
        *msg.downcast::<RequestMessage>()
            .expect("as_request checked it")
    }

    // PORT: Go shares the `*ResponseMessage` pointer after `AsResponse`;
    // Rust moves the message out. It panics where `AsResponse` panics.
    pub fn into_response(self) -> ResponseMessage {
        self.as_response();
        let msg: Box<dyn std::any::Any> = self.msg.expect("as_response checked it");
        *msg.downcast::<ResponseMessage>()
            .expect("as_response checked it")
    }

    // Go: jsonrpc.go:32 UnmarshalJSON
    pub fn unmarshal_json(&mut self, data: &[u8]) -> Result<(), GoError> {
        let mut raw = RawMessage::default();
        // Go `json.Unmarshal(data, &raw)`, with the v2 error texts.
        if let Err(err) = json_ext::unmarshal_root(data, &mut raw) {
            return Err(wrap_error_code(
                ErrorCode::INVALID_REQUEST,
                gostd::errors::from_value(err),
            ));
        }
        if raw.id.is_some() && raw.method.0.is_empty() {
            self.kind = crate::jsonrpc::MessageKind::RESPONSE;
            self.msg = Some(Box::new(ResponseMessage {
                id: raw.id,
                // Go stores the json.Value in `any`: never a nil `any`, even
                // when the member is missing.
                result: Some(Box::new(raw.result)),
                error: raw.error,
                ..ResponseMessage::default()
            }));
            return Ok(());
        }

        let mut params: Option<Box<dyn AnyValue>> = None;
        if !raw.params.0.is_empty() {
            params = Some(Box::new(raw.params));
        }

        if raw.id.is_none() {
            self.kind = crate::jsonrpc::MessageKind::NOTIFICATION;
        } else {
            self.kind = crate::jsonrpc::MessageKind::REQUEST;
        }

        self.msg = Some(Box::new(RequestMessage {
            id: raw.id,
            method: raw.method,
            params,
            ..RequestMessage::default()
        }));

        Ok(())
    }

    // Go: jsonrpc.go:76 MarshalJSON
    // PORT: Go returns bytes. The text is in the port form (see
    // `scanner_util::GO_STRING_MARKER`), so its Go bytes are returned.
    pub fn marshal_json(&self) -> Result<Vec<u8>, GoError> {
        json_marshal(&self.msg, &[])
            .map(|text| crate::scanner_util::go_string_bytes(&text).into_owned())
            .map_err(gostd::errors::from_value)
    }
}

// Go v2 calls MarshalJSON and writes its output with WriteValue.
impl MarshalerTo for Message {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        let data = self
            .marshal_json()
            .map_err(|e| JsonError { message: e.error() })?;
        write_value(enc, &data)
    }
}

// Go v2 calls UnmarshalJSON with the raw value.
impl UnmarshalerFrom for Message {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        let data = dec.read_value()?;
        self.unmarshal_json(data)
            .map_err(|e| JsonError { message: e.error() })
    }
}

// Go: jsonrpc.go:34 the anonymous `raw` struct of Message.UnmarshalJSON.
#[derive(Default)]
struct RawMessage {
    jsonrpc: crate::jsonrpc::JSONRPCVersion,
    method: Method,
    id: Option<crate::jsonrpc::ID>,
    params: JsonValue,
    // We don't have a method in the response, so we have no idea what to decode.
    // Store the raw text and let the caller decode it.
    result: JsonValue,
    error: Option<crate::jsonrpc::ResponseError>,
}

// Go v2 default struct arshaler for the anonymous struct.
impl UnmarshalerFrom for RawMessage {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        let is_object = unmarshal_struct_fields(dec, "struct", |name, dec| {
            match name {
                "jsonrpc" => json_unmarshal_decode(dec, &mut self.jsonrpc)?,
                "method" => json_unmarshal_decode(dec, &mut self.method)?,
                "id" => json_unmarshal_decode(dec, &mut self.id)?,
                "params" => json_unmarshal_decode(dec, &mut self.params)?,
                "result" => json_unmarshal_decode(dec, &mut self.result)?,
                "error" => json_unmarshal_decode(dec, &mut self.error)?,
                _ => return Ok(false),
            }
            Ok(true)
        })?;
        if !is_object {
            *self = RawMessage::default();
        }
        Ok(())
    }
}

// Go: jsonrpc.go:80 RequestMessage
#[derive(Debug, Default)]
pub struct RequestMessage {
    pub jsonrpc: crate::jsonrpc::JSONRPCVersion,
    pub id: Option<crate::jsonrpc::ID>,
    pub method: Method,
    pub params: Option<Box<dyn AnyValue>>,
}

impl RequestMessage {
    // Go: jsonrpc.go:91 Message
    // PORT: Go wraps the pointer; Rust moves the request into the message.
    pub fn message(self) -> Message {
        let mut kind = crate::jsonrpc::MessageKind::REQUEST;
        if self.id.is_none() {
            kind = crate::jsonrpc::MessageKind::NOTIFICATION;
        }
        Message {
            kind,
            msg: Some(Box::new(self)),
        }
    }

    // Go: jsonrpc.go:102 UnmarshalJSON
    pub fn unmarshal_json(&mut self, data: &[u8]) -> Result<(), GoError> {
        let mut raw = RawRequestMessage::default();
        // Go `json.Unmarshal(data, &raw)`, with the v2 error texts.
        if let Err(err) = json_ext::unmarshal_root(data, &mut raw) {
            return Err(wrap_error_code(
                ErrorCode::INVALID_REQUEST,
                gostd::errors::from_value(err),
            ));
        }

        self.id = raw.id;
        self.method = raw.method;
        if !raw.params.0.is_empty() {
            self.params = Some(Box::new(raw.params));
        }

        Ok(())
    }
}

// Go v2 default struct arshaler (no MarshalJSON method).
impl MarshalerTo for RequestMessage {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        write_object_start(enc);
        let mut first = true;
        marshal_field(enc, &mut first, "jsonrpc", &self.jsonrpc)?;
        marshal_opt_field(enc, &mut first, "id", &self.id)?;
        marshal_field(enc, &mut first, "method", &self.method)?;
        marshal_opt_field(enc, &mut first, "params", &self.params)?;
        write_object_end(enc);
        Ok(())
    }
}

// Go v2 calls UnmarshalJSON with the raw value.
impl UnmarshalerFrom for RequestMessage {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        let data = dec.read_value()?;
        self.unmarshal_json(data)
            .map_err(|e| JsonError { message: e.error() })
    }
}

// Go: jsonrpc.go:103 the anonymous `raw` struct of RequestMessage.UnmarshalJSON.
#[derive(Default)]
struct RawRequestMessage {
    jsonrpc: crate::jsonrpc::JSONRPCVersion,
    id: Option<crate::jsonrpc::ID>,
    method: Method,
    params: JsonValue,
}

// Go v2 default struct arshaler for the anonymous struct.
impl UnmarshalerFrom for RawRequestMessage {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        let is_object = unmarshal_struct_fields(dec, "struct", |name, dec| {
            match name {
                "jsonrpc" => json_unmarshal_decode(dec, &mut self.jsonrpc)?,
                "id" => json_unmarshal_decode(dec, &mut self.id)?,
                "method" => json_unmarshal_decode(dec, &mut self.method)?,
                "params" => json_unmarshal_decode(dec, &mut self.params)?,
                _ => return Ok(false),
            }
            Ok(true)
        })?;
        if !is_object {
            *self = RawRequestMessage::default();
        }
        Ok(())
    }
}

// Go: jsonrpc.go:118 ResponseMessage
// PORT: `id` has no omitzero in Go, so a nil id writes `null`.
#[derive(Debug, Default)]
pub struct ResponseMessage {
    pub jsonrpc: crate::jsonrpc::JSONRPCVersion,
    pub id: Option<crate::jsonrpc::ID>,
    pub result: Option<Box<dyn AnyValue>>,
    pub error: Option<crate::jsonrpc::ResponseError>,
}

impl ResponseMessage {
    // Go: jsonrpc.go:132 Message
    // PORT: Go wraps the pointer; Rust moves the response into the message.
    pub fn message(self) -> Message {
        Message {
            kind: crate::jsonrpc::MessageKind::RESPONSE,
            msg: Some(Box::new(self)),
        }
    }
}

// Go v2 default struct arshaler.
impl MarshalerTo for ResponseMessage {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        write_object_start(enc);
        let mut first = true;
        marshal_field(enc, &mut first, "jsonrpc", &self.jsonrpc)?;
        marshal_field(enc, &mut first, "id", &self.id)?;
        marshal_opt_field(enc, &mut first, "result", &self.result)?;
        marshal_opt_field(enc, &mut first, "error", &self.error)?;
        write_object_end(enc);
        Ok(())
    }
}

// Go v2 default struct arshaler.
impl UnmarshalerFrom for ResponseMessage {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        let is_object = unmarshal_struct_fields(dec, "lsproto.ResponseMessage", |name, dec| {
            match name {
                "jsonrpc" => json_unmarshal_decode(dec, &mut self.jsonrpc)?,
                "id" => json_unmarshal_decode(dec, &mut self.id)?,
                "result" => unmarshal_any_interface(dec, &mut self.result)?,
                "error" => json_unmarshal_decode(dec, &mut self.error)?,
                _ => return Ok(false),
            }
            Ok(true)
        })?;
        if !is_object {
            *self = ResponseMessage::default();
        }
        Ok(())
    }
}

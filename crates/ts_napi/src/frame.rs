//! The frames of the sync API protocol (`ts_goport::api::protocol_msgpack`,
//! TS7 `src/api/syncChannel.ts`): a `MessagePack` array of three items, the
//! message type, the method and the payload, the last two as `bin`.

use std::collections::VecDeque;

use crate::session::FramePipe;

pub const REQUEST: u8 = 1;
pub const CALL_RESPONSE: u8 = 2;
pub const CALL_ERROR: u8 = 3;
pub const RESPONSE: u8 = 4;
pub const ERROR: u8 = 5;
pub const CALL: u8 = 6;

const FIXARRAY3: u8 = 0x93;
const UINT8: u8 = 0xcc;
const BIN8: u8 = 0xc4;
const BIN16: u8 = 0xc5;
const BIN32: u8 = 0xc6;

/// One frame read from the session.
pub struct Frame {
    pub kind: u8,
    pub method: Vec<u8>,
    pub payload: Vec<u8>,
}

/// The bytes of a frame. A payload of 4 GiB or more does not fit a `bin32`.
pub fn encode(kind: u8, method: &[u8], payload: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(2 + 5 + method.len() + 5 + payload.len());
    out.push(FIXARRAY3);
    out.push(kind);
    put_bin(&mut out, method)?;
    put_bin(&mut out, payload)?;
    Ok(out)
}

fn put_bin(out: &mut Vec<u8>, data: &[u8]) -> Result<(), String> {
    if let Ok(len) = u8::try_from(data.len()) {
        out.extend_from_slice(&[BIN8, len]);
    } else if let Ok(len) = u16::try_from(data.len()) {
        out.push(BIN16);
        out.extend_from_slice(&len.to_be_bytes());
    } else if let Ok(len) = u32::try_from(data.len()) {
        out.push(BIN32);
        out.extend_from_slice(&len.to_be_bytes());
    } else {
        return Err(format!("payload too large: {} bytes", data.len()));
    }
    out.extend_from_slice(data);
    Ok(())
}

/// Reads frames from the chunks of a pipe. A `bin` that fills one chunk
/// exactly (the server writes a payload of 4 KiB or more in one write) is
/// taken without a copy.
#[derive(Default)]
pub struct FrameReader {
    chunks: VecDeque<Vec<u8>>,
    /// The read position in the first chunk.
    pos: usize,
}

/// Why a frame could not be read.
pub enum ReadError {
    /// The pipe closed, with its reason.
    Closed(String),
    /// The bytes are not a frame.
    Invalid(String),
}

impl FrameReader {
    pub fn read(&mut self, pipe: &FramePipe) -> Result<Frame, ReadError> {
        let marker = self.byte(pipe)?;
        if marker != FIXARRAY3 {
            return Err(ReadError::Invalid(format!(
                "expected fixed 3-element array (0x93), received: 0x{marker:02x}"
            )));
        }
        let kind = match self.byte(pipe)? {
            byte @ 0..=0x7f => byte,
            UINT8 => self.byte(pipe)?,
            byte => {
                return Err(ReadError::Invalid(format!(
                    "expected positive fixint or uint8 marker, received: 0x{byte:02x}"
                )));
            }
        };
        let method = self.bin(pipe)?;
        let payload = self.bin(pipe)?;
        Ok(Frame {
            kind,
            method,
            payload,
        })
    }

    fn bin(&mut self, pipe: &FramePipe) -> Result<Vec<u8>, ReadError> {
        let size = match self.byte(pipe)? {
            BIN8 => usize::from(self.byte(pipe)?),
            BIN16 => {
                let mut size = [0u8; 2];
                self.fill(pipe, &mut size)?;
                usize::from(u16::from_be_bytes(size))
            }
            BIN32 => {
                let mut size = [0u8; 4];
                self.fill(pipe, &mut size)?;
                usize::try_from(u32::from_be_bytes(size))
                    .map_err(|_| ReadError::Invalid("bin32 size overflows usize".to_string()))?
            }
            marker => {
                return Err(ReadError::Invalid(format!(
                    "expected binary data (0xc4-0xc6), received: 0x{marker:02x}"
                )));
            }
        };
        if size == 0 {
            return Ok(Vec::new());
        }
        self.ensure(pipe)?;
        if self.pos == 0 && self.chunks.front().is_some_and(|chunk| chunk.len() == size) {
            return Ok(self.chunks.pop_front().unwrap_or_default());
        }
        let mut out = vec![0u8; size];
        self.fill(pipe, &mut out)?;
        Ok(out)
    }

    fn byte(&mut self, pipe: &FramePipe) -> Result<u8, ReadError> {
        let mut byte = [0u8; 1];
        self.fill(pipe, &mut byte)?;
        Ok(byte[0])
    }

    /// Makes sure a chunk with unread bytes is first, after a wait.
    fn ensure(&mut self, pipe: &FramePipe) -> Result<(), ReadError> {
        while self
            .chunks
            .front()
            .is_none_or(|chunk| self.pos >= chunk.len())
        {
            if self.chunks.pop_front().is_some() {
                self.pos = 0;
                continue;
            }
            let chunk = pipe.pop().map_err(ReadError::Closed)?;
            self.chunks.push_back(chunk);
            self.pos = 0;
        }
        Ok(())
    }

    fn fill(&mut self, pipe: &FramePipe, out: &mut [u8]) -> Result<(), ReadError> {
        let mut done = 0;
        while done < out.len() {
            self.ensure(pipe)?;
            let Some(chunk) = self.chunks.front() else {
                continue;
            };
            let n = (chunk.len() - self.pos).min(out.len() - done);
            out[done..done + n].copy_from_slice(&chunk[self.pos..self.pos + n]);
            self.pos += n;
            done += n;
        }
        Ok(())
    }
}

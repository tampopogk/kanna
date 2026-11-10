use serde_json::Value;
use std::io::{self, Read, Write};

pub const MAX_HEADER: usize = 64 << 10;

pub const MAX_PAYLOAD: u64 = 4 << 30;

pub struct Frame {
    pub header: Value,
    pub size: u64,
}

impl Frame {
    pub fn op(&self) -> &str {
        self.header["op"].as_str().unwrap_or_default()
    }

    pub fn str(&self, key: &str) -> Option<&str> {
        self.header[key].as_str()
    }
}

pub fn bad(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

pub fn write_header(out: &mut (impl Write + ?Sized), header: &Value, size: u64) -> io::Result<()> {
    let mut header = header.clone();
    if let Some(object) = header.as_object_mut() {
        object.insert("size".into(), size.into());
    }
    let encoded = serde_json::to_vec(&header).map_err(|e| bad(e.to_string()))?;
    if encoded.len() > MAX_HEADER {
        return Err(bad("frame header too large"));
    }
    out.write_all(&(encoded.len() as u32).to_be_bytes())?;
    out.write_all(&encoded)
}

pub fn send(out: &mut (impl Write + ?Sized), header: &Value, payload: &[u8]) -> io::Result<()> {
    let mut buffer = Vec::with_capacity(payload.len() + 256);
    write_header(&mut buffer, header, payload.len() as u64)?;
    buffer.extend_from_slice(payload);
    out.write_all(&buffer)?;
    out.flush()
}

pub fn send_reader(out: &mut (impl Write + ?Sized), header: &Value, size: u64, from: &mut (impl Read + ?Sized)) -> io::Result<()> {
    write_header(out, header, size)?;
    let copied = io::copy(&mut from.take(size), out)?;
    if copied != size {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "file shrank while it was sent"));
    }
    out.flush()
}

pub fn read_header(input: &mut (impl Read + ?Sized), max_payload: u64) -> io::Result<Frame> {
    let mut length = [0u8; 4];
    input.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length) as usize;
    if length > MAX_HEADER {
        return Err(bad("frame header too large"));
    }
    let mut encoded = vec![0u8; length];
    input.read_exact(&mut encoded)?;
    let header: Value = serde_json::from_slice(&encoded).map_err(|e| bad(format!("frame header isn't JSON: {e}")))?;
    if !header.is_object() {
        return Err(bad("frame header isn't an object"));
    }
    let size = match &header["size"] {
        Value::Null => 0,
        size => size.as_u64().ok_or_else(|| bad("frame size isn't a number"))?,
    };
    if size > max_payload {
        return Err(bad(format!("frame payload of {size} bytes is over the {max_payload} allowed")));
    }
    Ok(Frame { header, size })
}

pub fn read_payload(input: &mut (impl Read + ?Sized), frame: &Frame) -> io::Result<Vec<u8>> {
    let mut payload = vec![0u8; frame.size as usize];
    input.read_exact(&mut payload)?;
    Ok(payload)
}

pub fn read(input: &mut (impl Read + ?Sized), max_payload: u64) -> io::Result<(Frame, Vec<u8>)> {
    let frame = read_header(input, max_payload)?;
    let payload = read_payload(input, &frame)?;
    Ok((frame, payload))
}

pub fn skip(input: &mut (impl Read + ?Sized), frame: &Frame) -> io::Result<()> {
    let skipped = io::copy(&mut input.take(frame.size), &mut io::sink())?;
    if skipped != frame.size {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "frame cut short"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_frame_carries_its_header_and_payload() {
        let mut wire = Vec::new();
        send(&mut wire, &json!({ "op": "out", "fd": 1 }), b"hello").unwrap();
        send(&mut wire, &json!({ "op": "done" }), b"").unwrap();
        let mut input = &wire[..];
        let (frame, payload) = read(&mut input, 10).unwrap();
        assert_eq!((frame.op(), frame.header["fd"].as_u64(), payload.as_slice()), ("out", Some(1), &b"hello"[..]));
        let (frame, payload) = read(&mut input, 10).unwrap();
        assert_eq!((frame.op(), payload.len()), ("done", 0));
    }

    #[test]
    fn oversized_frames_are_refused() {
        let mut wire = Vec::new();
        send(&mut wire, &json!({ "op": "blob" }), &[0u8; 64]).unwrap();
        assert!(read(&mut &wire[..], 63).is_err());
        let mut huge = (MAX_HEADER as u32 + 1).to_be_bytes().to_vec();
        huge.extend(vec![b' '; MAX_HEADER + 1]);
        assert!(read(&mut &huge[..], 10).is_err());
    }
}

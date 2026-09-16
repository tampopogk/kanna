//! Kanna secure channel (KSC): the mutually authenticated, end-to-end
//! encrypted layer that wraps every Kanna Stream Protocol (KSP) frame between
//! a paired phone and a desktop, over the LAN (`ws://`) and over the cloud
//! relay tunnel alike.
//!
//! The construction is deliberately *not* new cryptography. It is the Noise
//! Protocol Framework's published `IK` handshake
//! (`Noise_IK_25519_ChaChaPoly_SHA256`) as implemented by the `snow` crate,
//! plus a thin, fully specified framing so that KSP frames of any size ride
//! on Noise transport messages, whose maximum size is 65535 bytes:
//!
//! - **Prologue** `kanna-ksc/1\0<desktop_id>`: binds the protocol version
//!   and the target desktop into the handshake transcript. A phone that
//!   dialled the wrong desktop, or an intermediary that swapped the target,
//!   fails the handshake instead of silently talking to someone else.
//! - **Message 1** (phone → desktop) carries the phone's static key
//!   *encrypted* under the desktop's static key (so a LAN sniffer learns no
//!   device identity) and an encrypted JSON `InitiatorHello`.
//! - **Message 2** (desktop → phone) carries an encrypted JSON
//!   `ResponderHello`. Both sides contribute ephemerals: recorded traffic
//!   stays confidential after a later compromise of either static key.
//! - **Transport**: each Noise transport message's plaintext begins with one
//!   *kind* byte — `1` data chunk with more to follow, `2` final data chunk,
//!   `3` authenticated close — followed by the chunk. Per-direction nonces
//!   are implicit and strictly sequential, so a replayed, reordered,
//!   dropped-then-continued or truncated message fails authentication and
//!   ends the session. The authenticated close is what lets a receiver tell
//!   "the peer meant to hang up" from "someone cut the connection" — it
//!   promises nothing about availability, only about intent.
//! - **Wire text**: a frame on the WebSocket is the ASCII prefix `ksc1:`
//!   followed by standard base64 of one or more length-prefixed
//!   (`u16` big-endian) Noise messages. Text framing keeps the relay, React
//!   Native and Node WebSocket paths identical; the relay never parses it
//!   and cannot, because nothing after the prefix is plaintext.
//!
//! Anything that touches key material or nonces lives in `snow`; this crate
//! only decides *what* goes into a message and *when* a session ends.

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use std::sync::Arc;

/// Version of the KSC wire protocol. Bound into the prologue, so two peers
/// on different versions cannot complete a handshake.
pub const PROTOCOL_VERSION: u16 = 1;
/// Every KSC frame on the wire starts with this. A legacy KSP client's first
/// frame is JSON (`{`), so the prefix is also how a server tells a sealed
/// session from a plaintext one on the same WebSocket endpoint.
pub const WIRE_PREFIX: &str = "ksc1:";
const NOISE_PATTERN: &str = "Noise_IK_25519_ChaChaPoly_SHA256";
const PROLOGUE_PREFIX: &[u8] = b"kanna-ksc/1\0";
const SAS_DOMAIN: &[u8] = b"kanna-ksc/sas/1\0";
/// Noise's hard limit on one transport or handshake message.
pub const MAX_NOISE_MESSAGE_LEN: usize = 65_535;
const AEAD_TAG_LEN: usize = 16;
const CHUNK_KIND_LEN: usize = 1;
/// The most payload bytes one Noise transport message can carry after the
/// kind byte and the AEAD tag.
pub const MAX_CHUNK_PAYLOAD_LEN: usize = MAX_NOISE_MESSAGE_LEN - AEAD_TAG_LEN - CHUNK_KIND_LEN;
/// Default bound on one reassembled logical message. Large enough for the
/// biggest KSP frame the protocol allows (a 32 MiB visual-companion bundle,
/// with headroom), small enough that a peer cannot exhaust memory with an
/// endless `more follows` sequence.
pub const DEFAULT_MAX_MESSAGE_LEN: usize = 128 * 1024 * 1024;
/// Handshake payloads are small JSON documents; anything larger is refused
/// before it is parsed.
const MAX_HELLO_LEN: usize = 4096;
const CLOSE_REASON_MAX_LEN: usize = 1024;

const KIND_DATA_MORE: u8 = 1;
const KIND_DATA_FINAL: u8 = 2;
const KIND_CLOSE: u8 = 3;

/// Why a KSC operation failed. Every variant is terminal for the session it
/// happened on: there is no recovery path other than a fresh handshake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The Noise layer refused the message (bad MAC, wrong key, wrong
    /// prologue, malformed handshake). Details are deliberately coarse.
    Noise(String),
    /// The wire text was not a KSC frame or was structurally invalid.
    Wire(&'static str),
    /// A handshake payload did not parse or declared something unsupported.
    Hello(&'static str),
    /// A logical message would exceed the configured reassembly bound.
    MessageTooLarge { limit: usize },
    /// The peer sent an authenticated close; nothing more may be read.
    Closed,
    /// The nonce space for this direction is exhausted; reconnect.
    NonceExhausted,
    /// A key was not 32 bytes / not valid base64url.
    InvalidKey,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Noise(detail) => write!(
                formatter,
                "secure channel handshake/transport failure: {detail}"
            ),
            Self::Wire(detail) => write!(formatter, "secure channel wire frame invalid: {detail}"),
            Self::Hello(detail) => write!(formatter, "secure channel hello invalid: {detail}"),
            Self::MessageTooLarge { limit } => {
                write!(formatter, "secure channel message exceeds {limit} bytes")
            }
            Self::Closed => formatter.write_str("secure channel closed by peer"),
            Self::NonceExhausted => formatter.write_str("secure channel nonce space exhausted"),
            Self::InvalidKey => {
                formatter.write_str("secure channel key must be 32 base64url bytes")
            }
        }
    }
}

impl std::error::Error for Error {}

impl From<snow::Error> for Error {
    fn from(error: snow::Error) -> Self {
        Self::Noise(format!("{error:?}"))
    }
}

/// An X25519 static identity: the desktop's *channel identity* or a phone's
/// *device identity*. Distinct from every other key in the system (the
/// Ed25519 push identity, the LAN TLS CA, the task-transfer X25519 key): a
/// key used in one protocol is never reused in another.
#[derive(Clone)]
pub struct Keypair {
    private: [u8; 32],
    public: [u8; 32],
}

impl fmt::Debug for Keypair {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Keypair")
            .field("public", &encode_key(&self.public))
            .finish_non_exhaustive()
    }
}

impl Keypair {
    /// A fresh identity from the operating-system CSPRNG (`snow`'s default
    /// resolver uses `getrandom`).
    pub fn generate() -> Result<Self, Error> {
        let generated = builder()?.generate_keypair()?;
        let private: [u8; 32] = generated
            .private
            .as_slice()
            .try_into()
            .map_err(|_| Error::InvalidKey)?;
        Self::from_private(private)
    }

    /// Rebuilds an identity from its persisted private key.
    pub fn from_private(private: [u8; 32]) -> Result<Self, Error> {
        let public = x25519_public_from_private(&private);
        Ok(Self { private, public })
    }

    pub fn public_key(&self) -> &[u8; 32] {
        &self.public
    }

    pub fn private_key(&self) -> &[u8; 32] {
        &self.private
    }

    pub fn encoded_public_key(&self) -> String {
        encode_key(&self.public)
    }
}

/// X25519 public-key derivation, used only to *report* a keypair's public
/// half (for the pairing store, the QR payload and the status endpoint). The
/// handshake never calls it: `snow` derives keys internally through the same
/// dalek arithmetic and the same scalar clamping, so the advertised key is
/// byte-for-byte the one the handshake presents (asserted in tests).
fn x25519_public_from_private(private: &[u8; 32]) -> [u8; 32] {
    let secret = x25519_dalek::StaticSecret::from(*private);
    x25519_dalek::PublicKey::from(&secret).to_bytes()
}

/// Unpadded base64url of a 32-byte key, the encoding every Kanna surface
/// (pairing store, QR payload, status endpoint) uses for channel keys.
pub fn encode_key(key: &[u8; 32]) -> String {
    URL_SAFE_NO_PAD.encode(key)
}

pub fn decode_key(encoded: &str) -> Result<[u8; 32], Error> {
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded.trim())
        .map_err(|_| Error::InvalidKey)?;
    bytes.as_slice().try_into().map_err(|_| Error::InvalidKey)
}

/// What the phone tells the desktop inside message 1 (encrypted).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitiatorHello {
    pub version: u16,
    /// `session` for an already-paired device; `pairing` when the phone is
    /// about to claim a pairing code and its static key is not yet trusted.
    pub intent: HelloIntent,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HelloIntent {
    Session,
    Pairing,
}

/// What the desktop tells the phone inside message 2 (encrypted).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponderHello {
    pub version: u16,
    pub desktop_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
}

fn builder() -> Result<snow::Builder<'static>, Error> {
    let params = NOISE_PATTERN
        .parse()
        .map_err(|error: snow::Error| Error::Noise(format!("{error:?}")))?;
    Ok(snow::Builder::new(params))
}

fn prologue(desktop_id: &str) -> Vec<u8> {
    let mut prologue = PROLOGUE_PREFIX.to_vec();
    prologue.extend_from_slice(desktop_id.as_bytes());
    prologue
}

/// Whether a WebSocket text frame is a KSC frame at all.
pub fn is_wire_frame(text: &str) -> bool {
    text.starts_with(WIRE_PREFIX)
}

fn encode_wire(records: &[Vec<u8>]) -> String {
    let mut bytes = Vec::with_capacity(records.iter().map(|record| record.len() + 2).sum());
    for record in records {
        let len = record.len() as u16;
        bytes.extend_from_slice(&len.to_be_bytes());
        bytes.extend_from_slice(record);
    }
    format!("{WIRE_PREFIX}{}", STANDARD.encode(bytes))
}

fn decode_wire(text: &str) -> Result<Vec<Vec<u8>>, Error> {
    let encoded = text
        .strip_prefix(WIRE_PREFIX)
        .ok_or(Error::Wire("missing ksc1 prefix"))?;
    let bytes = STANDARD
        .decode(encoded)
        .map_err(|_| Error::Wire("frame is not base64"))?;
    let mut records = Vec::new();
    let mut offset = 0;
    while offset < bytes.len() {
        let Some(header) = bytes.get(offset..offset + 2) else {
            return Err(Error::Wire("truncated record length"));
        };
        let len = u16::from_be_bytes([header[0], header[1]]) as usize;
        offset += 2;
        let Some(record) = bytes.get(offset..offset + len) else {
            return Err(Error::Wire("truncated record"));
        };
        if len < AEAD_TAG_LEN {
            return Err(Error::Wire("record shorter than an AEAD tag"));
        }
        records.push(record.to_vec());
        offset += len;
    }
    if records.is_empty() {
        return Err(Error::Wire("frame carries no records"));
    }
    Ok(records)
}

/// A six-digit short authentication string derived from the handshake hash.
/// Both peers compute it from their own transcript; a person comparing the
/// two on both screens is comparing the exact pair of static keys and
/// ephemerals this session negotiated, so a substituted key on either side
/// shows up as a mismatch. Only the *typed-code* pairing path needs it; a
/// QR-anchored pairing already pinned the desktop key visually.
pub fn sas_code(handshake_hash: &[u8; 32]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(SAS_DOMAIN);
    hasher.update(handshake_hash);
    let digest = hasher.finalize();
    let value = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]) % 1_000_000;
    format!("{value:06}")
}

/// Responder side of the handshake between reading message 1 and writing
/// message 2. The server reads the initiator's static key and hello here,
/// decides what this session may do (known device, pairing-only, refuse) and
/// only then answers.
pub struct PendingResponder {
    handshake: snow::HandshakeState,
    hello: InitiatorHello,
    remote_static: [u8; 32],
    max_message_len: usize,
}

impl PendingResponder {
    pub fn read_hello(local: &Keypair, desktop_id: &str, wire: &str) -> Result<Self, Error> {
        Self::read_hello_with(local, desktop_id, wire, None, DEFAULT_MAX_MESSAGE_LEN)
    }

    /// `fixed_ephemeral` exists for deterministic test vectors only.
    pub fn read_hello_with(
        local: &Keypair,
        desktop_id: &str,
        wire: &str,
        fixed_ephemeral: Option<&[u8; 32]>,
        max_message_len: usize,
    ) -> Result<Self, Error> {
        let records = decode_wire(wire)?;
        if records.len() != 1 {
            return Err(Error::Wire("handshake frame must carry exactly one record"));
        }
        let prologue = prologue(desktop_id);
        let mut builder = builder()?
            .local_private_key(local.private_key())?
            .prologue(&prologue)?;
        if let Some(ephemeral) = fixed_ephemeral {
            builder = builder.fixed_ephemeral_key_for_testing_only(ephemeral);
        }
        let mut handshake = builder.build_responder()?;
        let mut payload = vec![0_u8; MAX_NOISE_MESSAGE_LEN];
        let payload_len = handshake.read_message(&records[0], &mut payload)?;
        let hello = parse_initiator_hello(&payload[..payload_len])?;
        let remote_static: [u8; 32] = handshake
            .get_remote_static()
            .ok_or(Error::Noise("initiator static missing".into()))?
            .try_into()
            .map_err(|_| Error::InvalidKey)?;
        Ok(Self {
            handshake,
            hello,
            remote_static,
            max_message_len,
        })
    }

    pub fn initiator_hello(&self) -> &InitiatorHello {
        &self.hello
    }

    /// The phone's static key, authenticated by the handshake: message 1
    /// proved possession of the matching private key (the `ss` DH) under
    /// this desktop's own static key.
    pub fn remote_static(&self) -> &[u8; 32] {
        &self.remote_static
    }

    /// Writes message 2 and completes the handshake.
    pub fn accept(mut self, hello: &ResponderHello) -> Result<(String, Channel), Error> {
        let payload = serde_json::to_vec(hello).map_err(|_| Error::Hello("unserializable"))?;
        if payload.len() > MAX_HELLO_LEN {
            return Err(Error::Hello("too large"));
        }
        let mut message = vec![0_u8; MAX_NOISE_MESSAGE_LEN];
        let len = self.handshake.write_message(&payload, &mut message)?;
        message.truncate(len);
        let wire = encode_wire(&[message]);
        let channel = Channel::from_handshake(self.handshake, self.max_message_len)?;
        Ok((wire, channel))
    }
}

/// Initiator side of the handshake between writing message 1 and reading
/// message 2.
pub struct PendingInitiator {
    handshake: snow::HandshakeState,
    max_message_len: usize,
}

impl PendingInitiator {
    pub fn start(
        local: &Keypair,
        remote_static: &[u8; 32],
        desktop_id: &str,
        hello: &InitiatorHello,
    ) -> Result<(Self, String), Error> {
        Self::start_with(
            local,
            remote_static,
            desktop_id,
            hello,
            None,
            DEFAULT_MAX_MESSAGE_LEN,
        )
    }

    /// `fixed_ephemeral` exists for deterministic test vectors only.
    pub fn start_with(
        local: &Keypair,
        remote_static: &[u8; 32],
        desktop_id: &str,
        hello: &InitiatorHello,
        fixed_ephemeral: Option<&[u8; 32]>,
        max_message_len: usize,
    ) -> Result<(Self, String), Error> {
        let prologue = prologue(desktop_id);
        let mut builder = builder()?
            .local_private_key(local.private_key())?
            .remote_public_key(remote_static)?
            .prologue(&prologue)?;
        if let Some(ephemeral) = fixed_ephemeral {
            builder = builder.fixed_ephemeral_key_for_testing_only(ephemeral);
        }
        let mut handshake = builder.build_initiator()?;
        let payload = serde_json::to_vec(hello).map_err(|_| Error::Hello("unserializable"))?;
        if payload.len() > MAX_HELLO_LEN {
            return Err(Error::Hello("too large"));
        }
        let mut message = vec![0_u8; MAX_NOISE_MESSAGE_LEN];
        let len = handshake.write_message(&payload, &mut message)?;
        message.truncate(len);
        Ok((
            Self {
                handshake,
                max_message_len,
            },
            encode_wire(&[message]),
        ))
    }

    pub fn finish(mut self, wire: &str) -> Result<(Channel, ResponderHello), Error> {
        let records = decode_wire(wire)?;
        if records.len() != 1 {
            return Err(Error::Wire("handshake frame must carry exactly one record"));
        }
        let mut payload = vec![0_u8; MAX_NOISE_MESSAGE_LEN];
        let payload_len = self.handshake.read_message(&records[0], &mut payload)?;
        let hello = parse_responder_hello(&payload[..payload_len])?;
        let channel = Channel::from_handshake(self.handshake, self.max_message_len)?;
        Ok((channel, hello))
    }
}

fn parse_initiator_hello(payload: &[u8]) -> Result<InitiatorHello, Error> {
    if payload.len() > MAX_HELLO_LEN {
        return Err(Error::Hello("too large"));
    }
    let hello: InitiatorHello =
        serde_json::from_slice(payload).map_err(|_| Error::Hello("malformed initiator hello"))?;
    if hello.version != PROTOCOL_VERSION {
        return Err(Error::Hello("unsupported initiator version"));
    }
    if let Some(device_id) = hello.device_id.as_deref() {
        if device_id.trim().is_empty() || device_id.len() > 256 {
            return Err(Error::Hello("invalid device id"));
        }
    }
    Ok(hello)
}

fn parse_responder_hello(payload: &[u8]) -> Result<ResponderHello, Error> {
    if payload.len() > MAX_HELLO_LEN {
        return Err(Error::Hello("too large"));
    }
    let hello: ResponderHello =
        serde_json::from_slice(payload).map_err(|_| Error::Hello("malformed responder hello"))?;
    if hello.version != PROTOCOL_VERSION {
        return Err(Error::Hello("unsupported responder version"));
    }
    Ok(hello)
}

/// An established session. Split it to hand the halves to independent
/// reader and writer tasks; each half owns its own nonce counter.
pub struct Channel {
    handshake_hash: [u8; 32],
    sender: Sender,
    receiver: Receiver,
}

impl Channel {
    fn from_handshake(
        handshake: snow::HandshakeState,
        max_message_len: usize,
    ) -> Result<Self, Error> {
        let handshake_hash: [u8; 32] = handshake
            .get_handshake_hash()
            .try_into()
            .map_err(|_| Error::Noise("handshake hash length".into()))?;
        let transport = Arc::new(handshake.into_stateless_transport_mode()?);
        Ok(Self {
            handshake_hash,
            sender: Sender {
                transport: Arc::clone(&transport),
                nonce: 0,
                closed: false,
            },
            receiver: Receiver {
                transport,
                nonce: 0,
                partial: Vec::new(),
                max_message_len,
                closed: false,
            },
        })
    }

    /// Noise's handshake hash: a transcript digest both peers hold. It is
    /// what the SAS is derived from and what a pending pairing confirmation
    /// is keyed by, so a confirmation can never apply to a different
    /// session than the one the person looked at.
    pub fn handshake_hash(&self) -> &[u8; 32] {
        &self.handshake_hash
    }

    pub fn sas_code(&self) -> String {
        sas_code(&self.handshake_hash)
    }

    pub fn split(self) -> (Sender, Receiver) {
        (self.sender, self.receiver)
    }
}

/// Sending half of a session.
pub struct Sender {
    transport: Arc<snow::StatelessTransportState>,
    nonce: u64,
    closed: bool,
}

impl Sender {
    fn next_nonce(&mut self) -> Result<u64, Error> {
        if self.closed {
            return Err(Error::Closed);
        }
        // Noise reserves u64::MAX as the "cannot encrypt" sentinel.
        if self.nonce >= u64::MAX - 1 {
            return Err(Error::NonceExhausted);
        }
        let nonce = self.nonce;
        self.nonce += 1;
        Ok(nonce)
    }

    fn seal_record(&mut self, kind: u8, payload: &[u8]) -> Result<Vec<u8>, Error> {
        let nonce = self.next_nonce()?;
        let mut plaintext = Vec::with_capacity(payload.len() + 1);
        plaintext.push(kind);
        plaintext.extend_from_slice(payload);
        let mut message = vec![0_u8; plaintext.len() + AEAD_TAG_LEN];
        let len = self
            .transport
            .write_message(nonce, &plaintext, &mut message)?;
        message.truncate(len);
        Ok(message)
    }

    /// One logical message (a KSP frame) as one wire frame, chunked into as
    /// many Noise messages as it needs.
    pub fn seal(&mut self, data: &[u8]) -> Result<String, Error> {
        let mut records = Vec::new();
        if data.is_empty() {
            records.push(self.seal_record(KIND_DATA_FINAL, &[])?);
            return Ok(encode_wire(&records));
        }
        let mut chunks = data.chunks(MAX_CHUNK_PAYLOAD_LEN).peekable();
        while let Some(chunk) = chunks.next() {
            let kind = if chunks.peek().is_some() {
                KIND_DATA_MORE
            } else {
                KIND_DATA_FINAL
            };
            records.push(self.seal_record(kind, chunk)?);
        }
        Ok(encode_wire(&records))
    }

    /// An authenticated close: the peer learns this side *chose* to stop.
    /// After it, this sender refuses further frames.
    pub fn seal_close(&mut self, reason: &str) -> Result<String, Error> {
        let reason = truncate_utf8(reason, CLOSE_REASON_MAX_LEN);
        let record = self.seal_record(KIND_CLOSE, reason.as_bytes())?;
        self.closed = true;
        Ok(encode_wire(&[record]))
    }
}

fn truncate_utf8(text: &str, max_len: usize) -> &str {
    if text.len() <= max_len {
        return text;
    }
    let mut end = max_len;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// What one received wire frame produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Received {
    /// A complete logical message.
    Message(Vec<u8>),
    /// The peer's authenticated close, with its reason.
    Closed(String),
}

/// Receiving half of a session.
pub struct Receiver {
    transport: Arc<snow::StatelessTransportState>,
    nonce: u64,
    partial: Vec<u8>,
    max_message_len: usize,
    closed: bool,
}

impl Receiver {
    /// Decrypts one wire frame. A frame may complete zero, one or several
    /// logical messages; a `Closed` item is always last. Any error is fatal
    /// for the session: the receiver refuses everything afterwards.
    pub fn open(&mut self, wire: &str) -> Result<Vec<Received>, Error> {
        if self.closed {
            return Err(Error::Closed);
        }
        let result = self.open_inner(wire);
        if result.is_err() {
            self.closed = true;
            self.partial.clear();
        }
        result
    }

    fn open_inner(&mut self, wire: &str) -> Result<Vec<Received>, Error> {
        let records = decode_wire(wire)?;
        let mut received = Vec::new();
        for record in records {
            if self.nonce >= u64::MAX - 1 {
                return Err(Error::NonceExhausted);
            }
            let mut plaintext = vec![0_u8; record.len()];
            let len = self
                .transport
                .read_message(self.nonce, &record, &mut plaintext)?;
            self.nonce += 1;
            let plaintext = &plaintext[..len];
            let Some((&kind, payload)) = plaintext.split_first() else {
                return Err(Error::Wire("empty record plaintext"));
            };
            match kind {
                KIND_DATA_MORE | KIND_DATA_FINAL => {
                    if self.partial.len() + payload.len() > self.max_message_len {
                        return Err(Error::MessageTooLarge {
                            limit: self.max_message_len,
                        });
                    }
                    self.partial.extend_from_slice(payload);
                    if kind == KIND_DATA_FINAL {
                        received.push(Received::Message(std::mem::take(&mut self.partial)));
                    }
                }
                KIND_CLOSE => {
                    let reason = String::from_utf8_lossy(payload).into_owned();
                    self.closed = true;
                    self.partial.clear();
                    received.push(Received::Closed(reason));
                    return Ok(received);
                }
                _ => return Err(Error::Wire("unknown record kind")),
            }
        }
        Ok(received)
    }

    /// Whether the peer has sent its authenticated close (or the session
    /// failed): nothing more will be accepted.
    pub fn is_closed(&self) -> bool {
        self.closed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hello_session(device: &str) -> InitiatorHello {
        InitiatorHello {
            version: PROTOCOL_VERSION,
            intent: HelloIntent::Session,
            device_id: Some(device.to_string()),
            capabilities: vec!["ksp".into()],
        }
    }

    fn responder_hello() -> ResponderHello {
        ResponderHello {
            version: PROTOCOL_VERSION,
            desktop_id: "DESKTOP-1".into(),
            capabilities: vec![],
        }
    }

    fn establish() -> (Channel, Channel) {
        let phone = Keypair::generate().unwrap();
        let desktop = Keypair::generate().unwrap();
        let (pending, m1) = PendingInitiator::start(
            &phone,
            desktop.public_key(),
            "DESKTOP-1",
            &hello_session("phone-1"),
        )
        .unwrap();
        let responder = PendingResponder::read_hello(&desktop, "DESKTOP-1", &m1).unwrap();
        assert_eq!(responder.remote_static(), phone.public_key());
        assert_eq!(responder.initiator_hello(), &hello_session("phone-1"));
        let (m2, desktop_channel) = responder.accept(&responder_hello()).unwrap();
        let (phone_channel, hello) = pending.finish(&m2).unwrap();
        assert_eq!(hello, responder_hello());
        assert_eq!(
            phone_channel.handshake_hash(),
            desktop_channel.handshake_hash()
        );
        assert_eq!(phone_channel.sas_code(), desktop_channel.sas_code());
        assert_eq!(phone_channel.sas_code().len(), 6);
        (phone_channel, desktop_channel)
    }

    #[test]
    fn keypair_public_key_matches_what_the_handshake_presents() {
        let phone = Keypair::generate().unwrap();
        let rebuilt = Keypair::from_private(*phone.private_key()).unwrap();
        assert_eq!(rebuilt.public_key(), phone.public_key());
        let desktop = Keypair::generate().unwrap();
        let (_, m1) =
            PendingInitiator::start(&phone, desktop.public_key(), "D", &hello_session("p"))
                .unwrap();
        let responder = PendingResponder::read_hello(&desktop, "D", &m1).unwrap();
        assert_eq!(responder.remote_static(), phone.public_key());
        let decoded = decode_key(&phone.encoded_public_key()).unwrap();
        assert_eq!(&decoded, phone.public_key());
    }

    #[test]
    fn round_trip_both_directions_including_empty_and_chunked_messages() {
        let (phone, desktop) = establish();
        let (mut phone_tx, mut phone_rx) = phone.split();
        let (mut desktop_tx, mut desktop_rx) = desktop.split();

        let big = vec![0xAB_u8; MAX_CHUNK_PAYLOAD_LEN * 3 + 17];
        for payload in [b"{\"type\":\"auth\"}".as_slice(), b"", big.as_slice()] {
            let wire = phone_tx.seal(payload).unwrap();
            assert!(is_wire_frame(&wire));
            assert!(!wire.contains("auth"));
            let received = desktop_rx.open(&wire).unwrap();
            assert_eq!(received, vec![Received::Message(payload.to_vec())]);
        }
        let wire = desktop_tx.seal(b"{\"type\":\"auth_ok\"}").unwrap();
        assert_eq!(
            phone_rx.open(&wire).unwrap(),
            vec![Received::Message(b"{\"type\":\"auth_ok\"}".to_vec())]
        );
    }

    #[test]
    fn a_replayed_frame_is_refused_and_the_session_is_dead_afterwards() {
        let (phone, desktop) = establish();
        let (mut phone_tx, _) = phone.split();
        let (_, mut desktop_rx) = desktop.split();
        let wire = phone_tx.seal(b"first").unwrap();
        desktop_rx.open(&wire).unwrap();
        assert!(matches!(desktop_rx.open(&wire), Err(Error::Noise(_))));
        assert!(desktop_rx.is_closed());
        let later = phone_tx.seal(b"second").unwrap();
        assert_eq!(desktop_rx.open(&later), Err(Error::Closed));
    }

    #[test]
    fn a_reordered_or_dropped_frame_is_refused() {
        let (phone, desktop) = establish();
        let (mut phone_tx, _) = phone.split();
        let (_, mut desktop_rx) = desktop.split();
        let _first = phone_tx.seal(b"first").unwrap();
        let second = phone_tx.seal(b"second").unwrap();
        assert!(matches!(desktop_rx.open(&second), Err(Error::Noise(_))));
    }

    #[test]
    fn a_tampered_or_truncated_frame_is_refused() {
        let (phone, desktop) = establish();
        let (mut phone_tx, _) = phone.split();
        let (_, mut desktop_rx) = desktop.split();
        let wire = phone_tx.seal(b"payload").unwrap();
        let mut bytes = STANDARD
            .decode(wire.strip_prefix(WIRE_PREFIX).unwrap())
            .unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0x01;
        let tampered = format!("{WIRE_PREFIX}{}", STANDARD.encode(&bytes));
        assert!(matches!(desktop_rx.open(&tampered), Err(Error::Noise(_))));

        let (phone, desktop) = establish();
        let (mut phone_tx, _) = phone.split();
        let (_, mut desktop_rx) = desktop.split();
        let wire = phone_tx.seal(b"payload").unwrap();
        let truncated = &wire[..wire.len() - 8];
        assert!(desktop_rx.open(truncated).is_err());
        assert!(desktop_rx.is_closed());
    }

    #[test]
    fn non_ksc_text_is_rejected_before_any_crypto() {
        let (phone, _) = establish();
        let (_, mut phone_rx) = phone.split();
        assert_eq!(
            phone_rx.open("{\"type\":\"auth_ok\"}"),
            Err(Error::Wire("missing ksc1 prefix"))
        );
        assert!(!is_wire_frame("{\"type\":\"auth\"}"));
    }

    #[test]
    fn handshake_against_the_wrong_desktop_key_fails() {
        let phone = Keypair::generate().unwrap();
        let desktop = Keypair::generate().unwrap();
        let impostor = Keypair::generate().unwrap();
        let (_, m1) = PendingInitiator::start(
            &phone,
            desktop.public_key(),
            "DESKTOP-1",
            &hello_session("phone-1"),
        )
        .unwrap();
        // An active relay that swapped the desktop key cannot read message 1.
        assert!(matches!(
            PendingResponder::read_hello(&impostor, "DESKTOP-1", &m1),
            Err(Error::Noise(_))
        ));
    }

    #[test]
    fn an_impostor_responder_cannot_complete_message_two() {
        let phone = Keypair::generate().unwrap();
        let desktop = Keypair::generate().unwrap();
        let (pending, _m1) = PendingInitiator::start(
            &phone,
            desktop.public_key(),
            "DESKTOP-1",
            &hello_session("phone-1"),
        )
        .unwrap();
        // An impostor that knows the phone's public key but not the
        // desktop's private key cannot produce a message 2 the phone
        // accepts: the only handshake frame it can mint is its own message
        // 1 against the phone's key, which is not a valid message 2.
        let impostor = Keypair::generate().unwrap();
        let (_, forged_m1) = PendingInitiator::start(
            &impostor,
            phone.public_key(),
            "DESKTOP-1",
            &hello_session("x"),
        )
        .unwrap();
        // Any frame that is not the genuine desktop's message 2 fails.
        assert!(pending.finish(&forged_m1).is_err());
    }

    #[test]
    fn a_prologue_mismatch_fails_the_handshake() {
        let phone = Keypair::generate().unwrap();
        let desktop = Keypair::generate().unwrap();
        let (_, m1) = PendingInitiator::start(
            &phone,
            desktop.public_key(),
            "DESKTOP-1",
            &hello_session("phone-1"),
        )
        .unwrap();
        assert!(matches!(
            PendingResponder::read_hello(&desktop, "DESKTOP-2", &m1),
            Err(Error::Noise(_))
        ));
    }

    #[test]
    fn an_unsupported_hello_version_is_refused_after_authentication() {
        let phone = Keypair::generate().unwrap();
        let desktop = Keypair::generate().unwrap();
        let mut hello = hello_session("phone-1");
        hello.version = 2;
        let (_, m1) =
            PendingInitiator::start(&phone, desktop.public_key(), "DESKTOP-1", &hello).unwrap();
        assert_eq!(
            PendingResponder::read_hello(&desktop, "DESKTOP-1", &m1).err(),
            Some(Error::Hello("unsupported initiator version"))
        );
    }

    #[test]
    fn authenticated_close_is_distinguishable_and_final() {
        let (phone, desktop) = establish();
        let (mut phone_tx, _) = phone.split();
        let (_, mut desktop_rx) = desktop.split();
        let wire = phone_tx.seal_close("phone backgrounded").unwrap();
        assert_eq!(
            desktop_rx.open(&wire).unwrap(),
            vec![Received::Closed("phone backgrounded".into())]
        );
        assert!(desktop_rx.is_closed());
        assert_eq!(phone_tx.seal(b"more"), Err(Error::Closed));
    }

    #[test]
    fn oversized_reassembly_is_refused() {
        let phone = Keypair::generate().unwrap();
        let desktop = Keypair::generate().unwrap();
        let (pending, m1) = PendingInitiator::start_with(
            &phone,
            desktop.public_key(),
            "D",
            &hello_session("p"),
            None,
            1024,
        )
        .unwrap();
        let responder = PendingResponder::read_hello_with(&desktop, "D", &m1, None, 1024).unwrap();
        let (m2, desktop_channel) = responder.accept(&responder_hello()).unwrap();
        let (phone_channel, _) = pending.finish(&m2).unwrap();
        let (mut phone_tx, _) = phone_channel.split();
        let (_, mut desktop_rx) = desktop_channel.split();
        let wire = phone_tx.seal(&vec![1_u8; 2048]).unwrap();
        assert_eq!(
            desktop_rx.open(&wire),
            Err(Error::MessageTooLarge { limit: 1024 })
        );
    }

    #[test]
    fn sas_code_is_six_digits_and_bound_to_the_transcript() {
        let (a, _) = establish();
        let (b, _) = establish();
        assert_ne!(a.handshake_hash(), b.handshake_hash());
        assert!(a.sas_code().chars().all(|c| c.is_ascii_digit()));
        assert_eq!(sas_code(a.handshake_hash()), a.sas_code());
    }

    #[test]
    fn invalid_keys_are_refused() {
        assert_eq!(decode_key("not base64!"), Err(Error::InvalidKey));
        assert_eq!(
            decode_key(&URL_SAFE_NO_PAD.encode([1_u8; 31])),
            Err(Error::InvalidKey)
        );
    }
}

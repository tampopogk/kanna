//! Two vector suites keep the Rust and TypeScript implementations honest
//! against each other and against the Noise specification:
//!
//! 1. `noise-ik-25519-chachapoly-sha256.json` — the published cacophony
//!    vector for exactly the pattern/cipher/hash triple KSC uses, driven
//!    through `snow` with fixed ephemerals. This proves the parameters this
//!    crate names resolve to the standard construction. The TypeScript
//!    package runs the same file through its own Noise core.
//! 2. `ksc-cross-language.json` — generated *by this file* from `snow` with
//!    fixed static and ephemeral keys, covering the KSC prologue, hello
//!    payloads, wire framing, chunking and authenticated close. The
//!    TypeScript package must reproduce every byte as initiator and as
//!    responder. Regenerate with `KSC_WRITE_VECTORS=1 cargo test -p
//!    kanna-secure-channel --test vectors`; the default run asserts the
//!    committed file is still what this crate produces, so drift on either
//!    side fails a test rather than a phone.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use kanna_secure_channel::{
    HelloIntent, InitiatorHello, Keypair, PendingInitiator, PendingResponder, Received,
    ResponderHello, MAX_CHUNK_PAYLOAD_LEN, PROTOCOL_VERSION,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

fn vectors_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/secure-channel-vectors")
        .canonicalize()
        .expect("vectors directory")
}

#[derive(Deserialize)]
struct OfficialFile {
    vectors: Vec<OfficialVector>,
}

#[derive(Deserialize)]
struct OfficialVector {
    protocol_name: String,
    init_prologue: String,
    init_static: String,
    init_ephemeral: String,
    init_remote_static: String,
    resp_prologue: String,
    resp_static: String,
    resp_ephemeral: String,
    handshake_hash: String,
    messages: Vec<OfficialMessage>,
}

#[derive(Deserialize)]
struct OfficialMessage {
    payload: String,
    ciphertext: String,
}

#[test]
fn snow_reproduces_the_published_ik_25519_chachapoly_sha256_vector() {
    let raw = std::fs::read_to_string(vectors_dir().join("noise-ik-25519-chachapoly-sha256.json"))
        .expect("official vector file");
    let file: OfficialFile = serde_json::from_str(&raw).expect("parse official vectors");
    assert!(!file.vectors.is_empty());
    for vector in file.vectors {
        assert_eq!(vector.protocol_name, "Noise_IK_25519_ChaChaPoly_SHA256");
        let params: snow::params::NoiseParams = vector.protocol_name.parse().unwrap();
        let init_static = hex::decode(&vector.init_static).unwrap();
        let init_ephemeral = hex::decode(&vector.init_ephemeral).unwrap();
        let init_remote_static = hex::decode(&vector.init_remote_static).unwrap();
        let init_prologue = hex::decode(&vector.init_prologue).unwrap();
        let resp_static = hex::decode(&vector.resp_static).unwrap();
        let resp_ephemeral = hex::decode(&vector.resp_ephemeral).unwrap();
        let resp_prologue = hex::decode(&vector.resp_prologue).unwrap();

        let mut initiator = snow::Builder::new(params.clone())
            .local_private_key(&init_static)
            .unwrap()
            .remote_public_key(&init_remote_static)
            .unwrap()
            .prologue(&init_prologue)
            .unwrap()
            .fixed_ephemeral_key_for_testing_only(&init_ephemeral)
            .build_initiator()
            .unwrap();
        let mut responder = snow::Builder::new(params)
            .local_private_key(&resp_static)
            .unwrap()
            .prologue(&resp_prologue)
            .unwrap()
            .fixed_ephemeral_key_for_testing_only(&resp_ephemeral)
            .build_responder()
            .unwrap();

        let mut buffer = vec![0_u8; 65_535];
        let mut scratch = vec![0_u8; 65_535];
        let messages = &vector.messages;
        // Message 0: initiator -> responder.
        let payload = hex::decode(&messages[0].payload).unwrap();
        let len = initiator.write_message(&payload, &mut buffer).unwrap();
        assert_eq!(hex::encode(&buffer[..len]), messages[0].ciphertext);
        let read = responder
            .read_message(&buffer[..len], &mut scratch)
            .unwrap();
        assert_eq!(&scratch[..read], payload.as_slice());
        // Message 1: responder -> initiator.
        let payload = hex::decode(&messages[1].payload).unwrap();
        let len = responder.write_message(&payload, &mut buffer).unwrap();
        assert_eq!(hex::encode(&buffer[..len]), messages[1].ciphertext);
        let read = initiator
            .read_message(&buffer[..len], &mut scratch)
            .unwrap();
        assert_eq!(&scratch[..read], payload.as_slice());
        assert_eq!(
            hex::encode(initiator.get_handshake_hash()),
            vector.handshake_hash
        );
        assert_eq!(
            hex::encode(responder.get_handshake_hash()),
            vector.handshake_hash
        );

        let mut initiator = initiator.into_transport_mode().unwrap();
        let mut responder = responder.into_transport_mode().unwrap();
        for (index, message) in messages.iter().enumerate().skip(2) {
            let payload = hex::decode(&message.payload).unwrap();
            let (writer, reader) = if index % 2 == 0 {
                (&mut initiator, &mut responder)
            } else {
                (&mut responder, &mut initiator)
            };
            let len = writer.write_message(&payload, &mut buffer).unwrap();
            assert_eq!(
                hex::encode(&buffer[..len]),
                message.ciphertext,
                "message {index}"
            );
            let read = reader.read_message(&buffer[..len], &mut scratch).unwrap();
            assert_eq!(&scratch[..read], payload.as_slice());
        }
    }
}

#[derive(Serialize, Deserialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
struct CrossLanguageVectors {
    description: String,
    desktop_id: String,
    initiator_static_private: String,
    initiator_static_public: String,
    initiator_ephemeral: String,
    responder_static_private: String,
    responder_static_public: String,
    responder_ephemeral: String,
    initiator_hello: InitiatorHello,
    responder_hello: ResponderHello,
    message1: String,
    message2: String,
    handshake_hash: String,
    sas: String,
    /// In order: every frame the initiator sends after the handshake.
    initiator_to_responder: Vec<Frame>,
    /// In order: every frame the responder sends after the handshake.
    responder_to_initiator: Vec<Frame>,
}

#[derive(Serialize, Deserialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
struct Frame {
    /// `data` frames carry `plaintextBase64`; `close` frames carry `reason`.
    kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    plaintext_base64: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    wire: String,
}

fn clamped(seed: u8) -> [u8; 32] {
    let mut key = [0_u8; 32];
    for (index, byte) in key.iter_mut().enumerate() {
        *byte = seed.wrapping_mul(31).wrapping_add(index as u8 * 7);
    }
    key[0] &= 248;
    key[31] &= 127;
    key[31] |= 64;
    key
}

fn generate_cross_language_vectors() -> CrossLanguageVectors {
    let desktop_id = "DESKTOP-VECTOR-1".to_string();
    let initiator = Keypair::from_private(clamped(11)).unwrap();
    let responder = Keypair::from_private(clamped(23)).unwrap();
    let initiator_ephemeral = clamped(37);
    let responder_ephemeral = clamped(53);
    let initiator_hello = InitiatorHello {
        version: PROTOCOL_VERSION,
        intent: HelloIntent::Session,
        device_id: Some("mobile-vector-device".into()),
        capabilities: vec!["ksp".into()],
    };
    let responder_hello = ResponderHello {
        version: PROTOCOL_VERSION,
        desktop_id: desktop_id.clone(),
        capabilities: vec!["ksp".into()],
    };
    let (pending, message1) = PendingInitiator::start_with(
        &initiator,
        responder.public_key(),
        &desktop_id,
        &initiator_hello,
        Some(&initiator_ephemeral),
        1024 * 1024,
    )
    .unwrap();
    let responder_pending = PendingResponder::read_hello_with(
        &responder,
        &desktop_id,
        &message1,
        Some(&responder_ephemeral),
        1024 * 1024,
    )
    .unwrap();
    assert_eq!(responder_pending.remote_static(), initiator.public_key());
    let (message2, responder_channel) = responder_pending.accept(&responder_hello).unwrap();
    let (initiator_channel, hello) = pending.finish(&message2).unwrap();
    assert_eq!(hello, responder_hello);
    let handshake_hash = hex::encode(initiator_channel.handshake_hash());
    let sas = initiator_channel.sas_code();

    let (mut initiator_tx, mut initiator_rx) = initiator_channel.split();
    let (mut responder_tx, mut responder_rx) = responder_channel.split();

    let initiator_plaintexts: Vec<Vec<u8>> = vec![
        br#"{"type":"auth","capabilities":["term_input_boundary"]}"#.to_vec(),
        Vec::new(),
        (0..(MAX_CHUNK_PAYLOAD_LEN * 2 + 1234))
            .map(|index| (index % 251) as u8)
            .collect(),
        "unicode ✓ payload".as_bytes().to_vec(),
    ];
    let responder_plaintexts: Vec<Vec<u8>> = vec![
        br#"{"type":"auth_ok","stream_kinds":["agent","terminal"]}"#.to_vec(),
        (0..(MAX_CHUNK_PAYLOAD_LEN + 1))
            .map(|index| (index % 253) as u8)
            .collect(),
    ];

    let mut initiator_to_responder = Vec::new();
    for plaintext in &initiator_plaintexts {
        let wire = initiator_tx.seal(plaintext).unwrap();
        assert_eq!(
            responder_rx.open(&wire).unwrap(),
            vec![Received::Message(plaintext.clone())]
        );
        initiator_to_responder.push(Frame {
            kind: "data".into(),
            plaintext_base64: Some(STANDARD.encode(plaintext)),
            reason: None,
            wire,
        });
    }
    let close_wire = initiator_tx.seal_close("vector close").unwrap();
    assert_eq!(
        responder_rx.open(&close_wire).unwrap(),
        vec![Received::Closed("vector close".into())]
    );
    initiator_to_responder.push(Frame {
        kind: "close".into(),
        plaintext_base64: None,
        reason: Some("vector close".into()),
        wire: close_wire,
    });

    let mut responder_to_initiator = Vec::new();
    for plaintext in &responder_plaintexts {
        let wire = responder_tx.seal(plaintext).unwrap();
        assert_eq!(
            initiator_rx.open(&wire).unwrap(),
            vec![Received::Message(plaintext.clone())]
        );
        responder_to_initiator.push(Frame {
            kind: "data".into(),
            plaintext_base64: Some(STANDARD.encode(plaintext)),
            reason: None,
            wire,
        });
    }
    let close_wire = responder_tx.seal_close("").unwrap();
    assert_eq!(
        initiator_rx.open(&close_wire).unwrap(),
        vec![Received::Closed(String::new())]
    );
    responder_to_initiator.push(Frame {
        kind: "close".into(),
        plaintext_base64: None,
        reason: Some(String::new()),
        wire: close_wire,
    });

    CrossLanguageVectors {
        description: "Generated by crates/kanna-secure-channel/tests/vectors.rs from snow with fixed keys; packages/secure-channel must reproduce every wire frame as initiator and as responder.".into(),
        desktop_id,
        initiator_static_private: hex::encode(initiator.private_key()),
        initiator_static_public: hex::encode(initiator.public_key()),
        initiator_ephemeral: hex::encode(initiator_ephemeral),
        responder_static_private: hex::encode(responder.private_key()),
        responder_static_public: hex::encode(responder.public_key()),
        responder_ephemeral: hex::encode(responder_ephemeral),
        initiator_hello,
        responder_hello,
        message1,
        message2,
        handshake_hash,
        sas,
        initiator_to_responder,
        responder_to_initiator,
    }
}

#[test]
fn the_committed_cross_language_vectors_are_what_this_crate_produces() {
    let path = vectors_dir().join("ksc-cross-language.json");
    let generated = generate_cross_language_vectors();
    if std::env::var("KSC_WRITE_VECTORS").is_ok_and(|value| value == "1") {
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&generated).unwrap() + "\n",
        )
        .expect("write vectors");
        return;
    }
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "{} missing ({error}); run KSC_WRITE_VECTORS=1 cargo test -p kanna-secure-channel --test vectors",
            path.display()
        )
    });
    let committed: CrossLanguageVectors = serde_json::from_str(&raw).expect("parse vectors");
    assert_eq!(committed, generated, "regenerate with KSC_WRITE_VECTORS=1");
}

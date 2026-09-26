//! Live, authenticated transfer compatibility and terminal refusal delivery.
//! Discovery and product versions are advisory; both running implementations
//! must answer before a reservation or pull is created.
use super::events::RuntimeError;
use super::external_peers::{find_peer, TransferTransport};
use super::listener::{authenticate_peer_request, ensure_authenticated_argument};
use super::state::{ListenerContext, TransferRuntime};
use crate::crypto::{open_json, parse_public_key, seal_json};
use crate::protocol::{PeerRegistryEntry, PeerRequest, PeerResponse};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub(super) use kanna_runtime_defaults::TRANSFER_PROTOCOL_CONTRACT as CONTRACT;
const UPGRADE: &str = "incompatible-transfer-version: upgrade Kanna on both machines (server and transfer sidecar) before retrying";

pub(super) fn require_contract(value: &Value) -> Result<(), RuntimeError> {
    if value.get("transfer_protocol").and_then(Value::as_str) == Some(CONTRACT) {
        Ok(())
    } else {
        Err(RuntimeError::Protocol(UPGRADE.into()))
    }
}

/// This queries the live local server, not its package metadata or an inherited
/// environment flag. The response is small and the entire round trip bounded.
async fn server_request(port: Option<u16>, body: &Value) -> Result<Value, RuntimeError> {
    let port = port.ok_or_else(|| RuntimeError::Protocol(UPGRADE.into()))?;
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).await?;
        let body = serde_json::to_string(body)?;
        let request = format!("POST /v1/transfers/protocol HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        stream.write_all(request.as_bytes()).await?;
        let mut response = Vec::new();
        stream.take(65537).read_to_end(&mut response).await?;
        if response.len() > 65536 {
            return Err(RuntimeError::Protocol("transfer protocol server response too large".into()));
        }
        let response = String::from_utf8(response).map_err(|_| RuntimeError::Protocol("invalid transfer protocol response".into()))?;
        let status = response.lines().next().and_then(|line| line.split_whitespace().nth(1)).unwrap_or("");
        let body = response.split_once("\r\n\r\n").map(|(_, body)| body).unwrap_or("");
        match status {
            "200" => Ok(serde_json::from_str(body)?),
            "404" | "405" => Err(RuntimeError::Protocol(UPGRADE.into())),
            _ => Err(RuntimeError::Protocol(format!("transfer protocol server HTTP {status}: {body}"))),
        }
    }).await.map_err(|_| RuntimeError::Protocol("transfer protocol server connection timed out after 15s".into()))?
}

pub(super) async fn local_capabilities(
    port: Option<u16>,
    standalone_test_server: bool,
) -> Result<Value, RuntimeError> {
    // Standalone runtime unit fixtures have no server process. Process tests
    // and all shipped sidecars always query the real HTTP boundary.
    if standalone_test_server && port.is_none() {
        return Ok(json!({ "transfer_protocol": CONTRACT }));
    }
    let reply = server_request(port, &json!({ "operation": "capabilities" })).await?;
    require_contract(&reply)?;
    Ok(reply)
}

impl TransferRuntime {
    async fn transfer_protocol_request(
        &self,
        peer: &PeerRegistryEntry,
        arguments: Value,
    ) -> Result<Value, RuntimeError> {
        let request_id = self.next_request_id("transfer-protocol");
        let mut arguments = arguments;
        arguments["requester_peer_id"] = json!(self.config.peer_id);
        arguments["reserved_target_peer_id"] = json!(peer.peer_id);
        arguments["transfer_protocol"] = json!(CONTRACT);
        let sealed_payload = self
            .seal_authenticated_peer_request(peer, "transfer_protocol", &request_id, arguments)
            .await?;
        match self
            .send_peer_request(
                peer,
                PeerRequest::TransferProtocol {
                    request_id: request_id.clone(),
                    requester_peer_id: self.config.peer_id.clone(),
                    sealed_payload,
                },
            )
            .await?
        {
            PeerResponse::TransferProtocol {
                request_id: response_id,
                sealed_payload,
            } if response_id == request_id => {
                let reply = open_json(
                    &self.identity,
                    &parse_public_key(&peer.public_key)?,
                    &sealed_payload,
                )?;
                ensure_authenticated_argument(&reply, "request_id", &request_id)?;
                require_contract(&reply)?;
                Ok(reply)
            }
            // Only a positive unsupported-request answer is incompatibility.
            // Connection loss, timeout and backpressure remain transient.
            PeerResponse::Error { message, .. }
                if message.contains("unknown variant `transfer_protocol`") =>
            {
                Err(RuntimeError::Protocol(UPGRADE.into()))
            }
            PeerResponse::Error { message, .. } => Err(RuntimeError::Protocol(message)),
            _ => Err(RuntimeError::Protocol(
                "invalid transfer protocol response".into(),
            )),
        }
    }

    /// Both servers must speak the transfer contract. Returns the peer
    /// server's capabilities reply, so a source can refuse a task the peer
    /// cannot carry before anything is reserved there.
    pub(super) async fn negotiate_transfer_protocol(
        &self,
        peer: &PeerRegistryEntry,
    ) -> Result<Value, RuntimeError> {
        local_capabilities(
            self.config.kanna_server_port,
            self.config.standalone_test_server,
        )
        .await?;
        let mut reply = self
            .transfer_protocol_request(peer, json!({ "operation": "capabilities" }))
            .await?;
        if let Some(reply) = reply.as_object_mut() {
            reply.remove("request_id");
        }
        Ok(reply)
    }

    pub async fn notify_transfer_refused(
        &self,
        transfer_id: &str,
        source_peer_id: &str,
        source_task_id: &str,
        reason: &str,
    ) -> Result<(), RuntimeError> {
        let peer = self
            .incoming_source_peer(transfer_id, source_peer_id)
            .await?;
        self.transfer_protocol_request(&peer, json!({
            "operation": "refused", "transfer_id": transfer_id,
            "source_task_id": source_task_id, "reason": super::pull::truncate_refusal_reason(reason.into()),
        })).await?;
        Ok(())
    }
}

pub(super) async fn handle(
    context: &ListenerContext,
    request_id: &str,
    requester_peer_id: &str,
    sealed_payload: &str,
) -> Result<String, RuntimeError> {
    let mut request = authenticate_peer_request(
        context,
        requester_peer_id,
        Some(sealed_payload),
        "transfer_protocol",
        request_id,
    )
    .await?;
    ensure_authenticated_argument(
        &request,
        "requester_peer_id",
        &requester_peer_id.to_string(),
    )?;
    ensure_authenticated_argument(&request, "reserved_target_peer_id", &context.self_peer_id)?;
    require_contract(&request)?;
    let mut reply = match request.get("operation").and_then(Value::as_str) {
        Some("capabilities") => {
            local_capabilities(context.kanna_server_port, context.standalone_test_server).await?
        }
        Some("refused") => {
            // The server validates the durable transfer's target and task even
            // after the reservation is gone. Its committed decision is the ACK;
            // lost replies can safely retry without relying on an event queue.
            if let Some(reason) = request.get("reason").and_then(Value::as_str) {
                request["reason"] = json!(super::pull::truncate_refusal_reason(reason.into()));
            }
            let reply = server_request(context.kanna_server_port, &request).await?;
            require_contract(&reply)?;
            let transfer_id = request
                .get("transfer_id")
                .and_then(Value::as_str)
                .ok_or_else(|| RuntimeError::Protocol("refusal missing transfer id".into()))?;
            // A lost server ACK leaves the reservation available for cleanup.
            // Once removed, replayed/delayed ACKs cannot acquire a newer pull's
            // identity. Compare under the pull lock in case fresh work arrived
            // between removing this reservation and acquiring that lock.
            if let Some(reservation) = context.outgoing_transfers.lock().await.remove(transfer_id) {
                if let Some(pull_request_id) = reservation.pull_request_id {
                    let key = (reservation.target_peer_id, reservation.source_task_id);
                    let mut pending = context.pending_task_pull_requests.lock().await;
                    if pending
                        .get(&key)
                        .is_some_and(|pull| pull.request_id == pull_request_id)
                    {
                        pending.remove(&key);
                    }
                }
            }
            context
                .replay_store
                .remove_reservation_checked(transfer_id)?;
            let paths = super::utils::take_transfer_artifacts(
                &mut *context.transfer_artifacts.lock().await,
                transfer_id,
            );
            super::utils::remove_owned_artifact_paths(paths).await;
            reply
        }
        _ => {
            return Err(RuntimeError::Protocol(
                "unknown transfer protocol operation".into(),
            ))
        }
    };
    reply["request_id"] = json!(request_id);
    let peer = find_peer(
        &context.discovery,
        &context.external_peers,
        &context.self_peer_id,
        requester_peer_id,
        TransferTransport::Auto,
    )
    .await?;
    Ok(seal_json(
        &super::utils::load_or_create_identity(&context.registry_root, &context.self_peer_id)?,
        &parse_public_key(&peer.public_key)?,
        &reply,
    )?)
}

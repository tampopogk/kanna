use super::events::RuntimeError;
#[cfg(not(target_os = "macos"))]
use super::utils::CURRENT_PROTOCOL_VERSION;
#[cfg(not(target_os = "macos"))]
use crate::discovery::{encode_txt_record, hostname_for_peer, SERVICE_TYPE};
use crate::protocol::PeerRegistryEntry;
use crate::registry::PeerRegistry;
#[cfg(any(test, not(target_os = "macos")))]
use mdns_sd::ServiceEvent;
#[cfg(not(target_os = "macos"))]
use mdns_sd::{ServiceDaemon, ServiceInfo};

#[cfg(target_os = "macos")]
mod macos;
use std::collections::HashMap;
use std::sync::Arc;
#[cfg(any(test, not(target_os = "macos")))]
use tokio::sync::Mutex;
#[cfg(not(target_os = "macos"))]
use tokio::task::JoinHandle;

#[derive(Clone)]
pub(super) enum PeerDiscovery {
    Registry(PeerRegistry),
    Mdns(Arc<MdnsDiscovery>),
    #[cfg(test)]
    MdnsFixture(Arc<Mutex<MdnsState>>),
}

#[derive(Debug, Default)]
pub(super) struct MdnsState {
    observations: HashMap<String, PeerRegistryEntry>,
}

#[cfg(target_os = "macos")]
pub(super) use macos::MdnsDiscovery;

#[cfg(not(target_os = "macos"))]
pub(super) struct MdnsDiscovery {
    daemon: ServiceDaemon,
    state: Arc<Mutex<MdnsState>>,
    browse_task: JoinHandle<()>,
    service_fullname: String,
}

impl PeerDiscovery {
    pub(super) async fn list_peers(
        &self,
        self_peer_id: &str,
    ) -> Result<Vec<PeerRegistryEntry>, RuntimeError> {
        match self {
            Self::Registry(registry) => Ok(registry.list_peers(self_peer_id)?),
            Self::Mdns(discovery) => discovery.list_peers(self_peer_id).await,
            #[cfg(test)]
            Self::MdnsFixture(state) => Ok(state.lock().await.list_peers(self_peer_id)),
        }
    }

    pub(super) fn shutdown(&self) {
        if let Self::Mdns(discovery) = self {
            discovery.shutdown();
        }
    }
}

#[cfg(not(target_os = "macos"))]
impl MdnsDiscovery {
    pub(super) async fn spawn(
        peer_id: &str,
        display_name: &str,
        public_key: &str,
        listen_port: u16,
    ) -> Result<Self, RuntimeError> {
        let daemon =
            ServiceDaemon::new().map_err(|error| RuntimeError::Discovery(error.to_string()))?;
        let txt = encode_txt_record(
            peer_id,
            display_name,
            public_key,
            CURRENT_PROTOCOL_VERSION,
            true,
        )
        .map_err(|error| RuntimeError::InvalidConfig(error.to_string()))?;
        let properties = txt
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect::<Vec<_>>();
        let hostname = hostname_for_peer(peer_id)
            .map_err(|error| RuntimeError::InvalidConfig(error.to_string()))?;
        let service_info = ServiceInfo::new(
            SERVICE_TYPE,
            peer_id,
            &hostname,
            "",
            listen_port,
            &properties[..],
        )
        .map_err(|error| RuntimeError::Discovery(error.to_string()))?
        .enable_addr_auto();
        let service_fullname = service_info.get_fullname().to_string();
        daemon
            .register(service_info)
            .map_err(|error| RuntimeError::Discovery(error.to_string()))?;

        let receiver = daemon
            .browse(SERVICE_TYPE)
            .map_err(|error| RuntimeError::Discovery(error.to_string()))?;
        let state = Arc::new(Mutex::new(MdnsState::default()));
        let browse_state = Arc::clone(&state);
        let browse_task = tokio::spawn(async move {
            while let Ok(event) = receiver.recv_async().await {
                handle_mdns_event(&browse_state, event).await;
            }
        });

        Ok(Self {
            daemon,
            state,
            browse_task,
            service_fullname,
        })
    }

    async fn list_peers(&self, self_peer_id: &str) -> Result<Vec<PeerRegistryEntry>, RuntimeError> {
        let state = self.state.lock().await;
        Ok(state.list_peers(self_peer_id))
    }

    fn shutdown(&self) {
        self.browse_task.abort();
        let _ = self.daemon.unregister(&self.service_fullname);
        let _ = self.daemon.shutdown();
    }
}

impl MdnsState {
    fn list_peers(&self, self_peer_id: &str) -> Vec<PeerRegistryEntry> {
        // A service can resolve on several interfaces. Removing one observation
        // must not withdraw a still-live route from another interface.
        let mut peers = self
            .observations
            .values()
            .filter(|peer| peer.peer_id != self_peer_id)
            .cloned()
            .collect::<Vec<_>>();
        peers.sort_by(|left, right| {
            left.peer_id
                .cmp(&right.peer_id)
                .then_with(|| endpoint_rank(&left.endpoint).cmp(&endpoint_rank(&right.endpoint)))
                .then_with(|| left.endpoint.cmp(&right.endpoint))
        });
        peers.dedup_by(|left, right| left.peer_id == right.peer_id);
        peers
    }
}

fn endpoint_rank(endpoint: &str) -> u8 {
    match endpoint.parse::<std::net::SocketAddr>() {
        Ok(address) if address.ip().is_loopback() => 4,
        Ok(std::net::SocketAddr::V4(address)) if !address.ip().is_link_local() => 0,
        Ok(std::net::SocketAddr::V6(address)) if !address.ip().is_unicast_link_local() => 1,
        Ok(std::net::SocketAddr::V6(_)) => 2,
        _ => 3,
    }
}

#[cfg(any(test, not(target_os = "macos")))]
pub(super) async fn handle_mdns_event(state: &Arc<Mutex<MdnsState>>, event: ServiceEvent) {
    match event {
        ServiceEvent::ServiceResolved(service) => {
            let peer = match crate::discovery::resolved_service_to_peer_entry(&service) {
                Ok(peer) => peer,
                Err(_) => return,
            };

            let mut state = state.lock().await;
            state
                .observations
                .insert(service.get_fullname().to_owned(), peer);
        }
        ServiceEvent::ServiceRemoved(_, fullname) => {
            state.lock().await.observations.remove(&fullname);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::{encode_txt_record, SERVICE_TYPE};
    use mdns_sd::ServiceInfo;

    #[tokio::test]
    async fn service_removal_retains_other_observations_and_prefers_a_routable_address() {
        let state = Arc::new(Mutex::new(MdnsState::default()));
        let txt = encode_txt_record("peer-multi", "Multi", "public-key", 5, true).unwrap();
        let txt = txt.into_iter().collect::<Vec<_>>();
        let mut fullnames = Vec::new();
        for (name, address) in [
            ("loopback", "127.0.0.1"),
            ("link-local", "169.254.1.2"),
            ("lan", "192.168.1.2"),
        ] {
            let service =
                ServiceInfo::new(SERVICE_TYPE, name, "multi.local.", address, 4455, &txt[..])
                    .unwrap()
                    .as_resolved_service();
            fullnames.push(service.get_fullname().to_owned());
            handle_mdns_event(&state, ServiceEvent::ServiceResolved(Box::new(service))).await;
        }
        assert_eq!(
            state.lock().await.list_peers("self")[0].endpoint,
            "192.168.1.2:4455"
        );
        handle_mdns_event(
            &state,
            ServiceEvent::ServiceRemoved(SERVICE_TYPE.into(), fullnames[0].clone()),
        )
        .await;
        assert_eq!(
            state.lock().await.list_peers("self")[0].endpoint,
            "192.168.1.2:4455"
        );
        for fullname in fullnames.into_iter().skip(1) {
            handle_mdns_event(
                &state,
                ServiceEvent::ServiceRemoved(SERVICE_TYPE.into(), fullname),
            )
            .await;
        }
        assert!(state.lock().await.list_peers("self").is_empty());
    }
}

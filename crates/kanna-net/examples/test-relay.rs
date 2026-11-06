use std::sync::Arc;

#[derive(Debug)]
struct Token(String);

impl iroh_relay::server::AccessControl for Token {
    async fn on_connect(&self, request: &iroh_relay::server::ClientRequest) -> iroh_relay::server::Access {
        match request.auth_token() {
            Some(token) if token == self.0 => iroh_relay::server::Access::Allow,
            _ => iroh_relay::server::Access::Deny { reason: None },
        }
    }
}

#[tokio::main]
async fn main() {
    let token = std::env::args().nth(1).expect("usage: test-relay TOKEN");
    let mut relay = iroh_relay::server::RelayConfig::new(std::net::SocketAddr::from(([127, 0, 0, 1], 0)));
    relay.access = Arc::new(Token(token));
    let mut config = iroh_relay::server::ServerConfig::default();
    config.relay = Some(relay);
    let server = iroh_relay::server::Server::spawn(config).await.expect("the relay didn't start");
    println!("http://{}/", server.http_addr().expect("no address"));
    std::future::pending::<()>().await;
}

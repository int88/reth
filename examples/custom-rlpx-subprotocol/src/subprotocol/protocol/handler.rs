use super::event::ProtocolEvent;
use crate::subprotocol::connection::handler::CustomRlpxConnectionHandler;
use reth_network::protocol::ProtocolHandler;
use reth_network_api::PeerId;
use std::net::SocketAddr;
use tokio::sync::mpsc;

/// Protocol state is an helper struct to store the protocol events.
/// Protocol state是一个helper结构用来存储protocol events
#[derive(Clone, Debug)]
pub(crate) struct ProtocolState {
    pub(crate) events: mpsc::UnboundedSender<ProtocolEvent>,
}

/// The protocol handler takes care of incoming and outgoing connections.
/// protocol handler处理Incoming和outcoming的连接
#[derive(Debug)]
pub(crate) struct CustomRlpxProtoHandler {
    pub state: ProtocolState,
}

impl ProtocolHandler for CustomRlpxProtoHandler {
    type ConnectionHandler = CustomRlpxConnectionHandler;

    fn on_incoming(&self, _socket_addr: SocketAddr) -> Option<Self::ConnectionHandler> {
        Some(CustomRlpxConnectionHandler { state: self.state.clone() })
    }

    fn on_outgoing(
        &self,
        _socket_addr: SocketAddr,
        _peer_id: PeerId,
    ) -> Option<Self::ConnectionHandler> {
        Some(CustomRlpxConnectionHandler { state: self.state.clone() })
    }
}

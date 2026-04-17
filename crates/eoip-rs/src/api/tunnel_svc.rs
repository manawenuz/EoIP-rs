//! TunnelService gRPC implementation.

use std::sync::Arc;

use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tonic::{Request, Response, Status};

use eoip_api::*;

use crate::tunnel::registry::TunnelRegistry;

pub struct TunnelServiceImpl {
    registry: Arc<TunnelRegistry>,
    event_tx: broadcast::Sender<TunnelEvent>,
}

impl TunnelServiceImpl {
    pub fn new(registry: Arc<TunnelRegistry>) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        Self { registry, event_tx }
    }

    fn tunnel_to_proto(
        key: &eoip_proto::DemuxKey,
        handle: &crate::tunnel::handle::TunnelHandle,
    ) -> Tunnel {
        let state = match handle.state.load() {
            crate::tunnel::lifecycle::TunnelState::Initializing => TunnelState::Initializing,
            crate::tunnel::lifecycle::TunnelState::Configured => TunnelState::Configured,
            crate::tunnel::lifecycle::TunnelState::Active => TunnelState::Active,
            crate::tunnel::lifecycle::TunnelState::Stale => TunnelState::Stale,
            crate::tunnel::lifecycle::TunnelState::TearingDown => TunnelState::TearingDown,
            crate::tunnel::lifecycle::TunnelState::Destroyed => TunnelState::Destroyed,
        };

        Tunnel {
            tunnel_id: handle.config.tunnel_id as u32,
            local_addr: handle.config.local.to_string(),
            remote_addr: key.peer_addr.to_string(),
            iface_name: handle.config.effective_iface_name(),
            mtu: handle.config.mtu as u32,
            enabled: handle.config.enabled,
            state: state.into(),
        }
    }
}

#[tonic::async_trait]
impl tunnel_service_server::TunnelService for TunnelServiceImpl {
    async fn create_tunnel(
        &self,
        _request: Request<CreateTunnelRequest>,
    ) -> Result<Response<CreateTunnelResponse>, Status> {
        // Dynamic tunnel creation requires helper communication.
        // For now, return unimplemented — tunnels are created from config.
        Err(Status::unimplemented(
            "dynamic tunnel creation not yet supported; use config file",
        ))
    }

    async fn delete_tunnel(
        &self,
        request: Request<DeleteTunnelRequest>,
    ) -> Result<Response<DeleteTunnelResponse>, Status> {
        let tid = request.into_inner().tunnel_id as u16;
        let removed = self.registry.find_by_tunnel_id(tid);
        if removed.is_empty() {
            return Err(Status::not_found(format!("tunnel {tid} not found")));
        }
        for (key, _) in &removed {
            self.registry.remove(key);
        }
        tracing::info!(tunnel_id = tid, "tunnel deleted via gRPC");
        Ok(Response::new(DeleteTunnelResponse {}))
    }

    async fn get_tunnel(
        &self,
        request: Request<GetTunnelRequest>,
    ) -> Result<Response<GetTunnelResponse>, Status> {
        let tid = request.into_inner().tunnel_id as u16;
        let entries = self.registry.find_by_tunnel_id(tid);
        let (key, handle) = entries
            .first()
            .ok_or_else(|| Status::not_found(format!("tunnel {tid} not found")))?;

        Ok(Response::new(GetTunnelResponse {
            tunnel: Some(Self::tunnel_to_proto(key, handle)),
        }))
    }

    async fn list_tunnels(
        &self,
        _request: Request<ListTunnelsRequest>,
    ) -> Result<Response<ListTunnelsResponse>, Status> {
        let tunnels: Vec<Tunnel> = self
            .registry
            .iter()
            .map(|(key, handle)| Self::tunnel_to_proto(&key, &handle))
            .collect();

        Ok(Response::new(ListTunnelsResponse { tunnels }))
    }

    type WatchTunnelsStream =
        std::pin::Pin<Box<dyn tokio_stream::Stream<Item = Result<TunnelEvent, Status>> + Send>>;

    async fn watch_tunnels(
        &self,
        _request: Request<WatchTunnelsRequest>,
    ) -> Result<Response<Self::WatchTunnelsStream>, Status> {
        let rx = self.event_tx.subscribe();
        let stream = BroadcastStream::new(rx).filter_map(|result| match result {
            Ok(event) => Some(Ok(event)),
            Err(_) => None, // lagged — skip
        });

        Ok(Response::new(Box::pin(stream)))
    }
}

// tokio_stream re-export for filter_map
use tokio_stream::StreamExt;

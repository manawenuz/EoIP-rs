//! TunnelService gRPC implementation.

use std::net::IpAddr;
use std::sync::Arc;

use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tonic::{Request, Response, Status};

use eoip_api::*;

use crate::config::TunnelConfig;
use crate::tunnel::manager::TunnelManager;
use crate::tunnel::registry::TunnelRegistry;

pub struct TunnelServiceImpl {
    registry: Arc<TunnelRegistry>,
    manager: Arc<TunnelManager>,
    event_tx: broadcast::Sender<TunnelEvent>,
}

impl TunnelServiceImpl {
    pub fn new(registry: Arc<TunnelRegistry>, manager: Arc<TunnelManager>) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        Self { registry, manager, event_tx }
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
            keepalive_interval_secs: handle.config.keepalive_interval_secs as u32,
            keepalive_timeout_secs: handle.config.keepalive_timeout_secs as u32,
        }
    }
}

#[tonic::async_trait]
impl tunnel_service_server::TunnelService for TunnelServiceImpl {
    async fn create_tunnel(
        &self,
        request: Request<CreateTunnelRequest>,
    ) -> Result<Response<CreateTunnelResponse>, Status> {
        let req = request.into_inner();

        let local: IpAddr = req.local_addr.parse()
            .map_err(|_| Status::invalid_argument(format!("invalid local_addr: {}", req.local_addr)))?;
        let remote: IpAddr = req.remote_addr.parse()
            .map_err(|_| Status::invalid_argument(format!("invalid remote_addr: {}", req.remote_addr)))?;

        let config = TunnelConfig {
            tunnel_id: req.tunnel_id as u16,
            local,
            remote,
            iface_name: if req.iface_name.is_empty() { None } else { Some(req.iface_name) },
            mtu: if req.mtu == 0 { 1458 } else { req.mtu as u16 },
            enabled: true,
            keepalive_interval_secs: 10,
            keepalive_timeout_secs: 100,
        };

        self.manager.create_tunnel(config).await
            .map_err(|e| Status::internal(e))?;

        // Return the created tunnel
        let tid = req.tunnel_id as u16;
        let entries = self.registry.find_by_tunnel_id(tid);
        let (key, handle) = entries.first()
            .ok_or_else(|| Status::internal("tunnel created but not found in registry"))?;

        Ok(Response::new(CreateTunnelResponse {
            tunnel: Some(Self::tunnel_to_proto(key, handle)),
        }))
    }

    async fn delete_tunnel(
        &self,
        request: Request<DeleteTunnelRequest>,
    ) -> Result<Response<DeleteTunnelResponse>, Status> {
        let tid = request.into_inner().tunnel_id as u16;
        self.manager.destroy_tunnel(tid)
            .map_err(|e| Status::not_found(e))?;
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

    async fn update_tunnel(
        &self,
        request: Request<UpdateTunnelRequest>,
    ) -> Result<Response<UpdateTunnelResponse>, Status> {
        let req = request.into_inner();
        let tid = req.tunnel_id as u16;
        let entries = self.registry.find_by_tunnel_id(tid);
        let (key, handle) = entries
            .first()
            .ok_or_else(|| Status::not_found(format!("tunnel {tid} not found")))?;

        if let Some(enabled) = req.enabled {
            tracing::info!(tunnel_id = tid, enabled, "update_tunnel requested");
        }

        let tunnel = Self::tunnel_to_proto(key, handle);
        Ok(Response::new(UpdateTunnelResponse {
            tunnel: Some(tunnel),
        }))
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
            Err(_) => None,
        });

        Ok(Response::new(Box::pin(stream)))
    }
}

use tokio_stream::StreamExt;

//! StatsService gRPC implementation.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use tonic::{Request, Response, Status};

use eoip_api::*;

use crate::tunnel::registry::TunnelRegistry;

pub struct StatsServiceImpl {
    registry: Arc<TunnelRegistry>,
}

impl StatsServiceImpl {
    pub fn new(registry: Arc<TunnelRegistry>) -> Self {
        Self { registry }
    }
}

#[tonic::async_trait]
impl stats_service_server::StatsService for StatsServiceImpl {
    async fn get_stats(
        &self,
        request: Request<GetStatsRequest>,
    ) -> Result<Response<GetStatsResponse>, Status> {
        let tid = request.into_inner().tunnel_id as u16;
        let entries = self.registry.find_by_tunnel_id(tid);
        let (_key, handle) = entries
            .first()
            .ok_or_else(|| Status::not_found(format!("tunnel {tid} not found")))?;

        let snap = handle.stats.snapshot();
        Ok(Response::new(GetStatsResponse {
            stats: Some(TunnelStats {
                tunnel_id: tid as u32,
                tx_packets: snap.tx_packets,
                tx_bytes: snap.tx_bytes,
                rx_packets: snap.rx_packets,
                rx_bytes: snap.rx_bytes,
                tx_errors: snap.tx_errors,
                rx_errors: snap.rx_errors,
                last_rx_timestamp_ms: snap.last_rx_timestamp,
                last_tx_timestamp_ms: snap.last_tx_timestamp,
            }),
        }))
    }

    async fn get_global_stats(
        &self,
        _request: Request<GetGlobalStatsRequest>,
    ) -> Result<Response<GetGlobalStatsResponse>, Status> {
        let mut active = 0u32;
        let mut stale = 0u32;
        let mut total_tx_packets = 0u64;
        let mut total_rx_packets = 0u64;
        let mut total_tx_bytes = 0u64;
        let mut total_rx_bytes = 0u64;

        for (_key, handle) in self.registry.iter() {
            match handle.state.load() {
                crate::tunnel::lifecycle::TunnelState::Active => active += 1,
                crate::tunnel::lifecycle::TunnelState::Stale => stale += 1,
                _ => {}
            }
            total_tx_packets += handle.stats.tx_packets.load(Ordering::Relaxed);
            total_rx_packets += handle.stats.rx_packets.load(Ordering::Relaxed);
            total_tx_bytes += handle.stats.tx_bytes.load(Ordering::Relaxed);
            total_rx_bytes += handle.stats.rx_bytes.load(Ordering::Relaxed);
        }

        Ok(Response::new(GetGlobalStatsResponse {
            stats: Some(GlobalStats {
                active_tunnels: active,
                stale_tunnels: stale,
                total_tx_packets,
                total_rx_packets,
                total_tx_bytes,
                total_rx_bytes,
            }),
        }))
    }
}

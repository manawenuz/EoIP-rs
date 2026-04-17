//! gRPC management API server.

pub mod tunnel_svc;
pub mod stats_svc;
pub mod health_svc;

use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use tonic::transport::Server;

use crate::config::ApiConfig;
use crate::tunnel::registry::TunnelRegistry;

/// Start the gRPC API server.
pub async fn start_grpc_server(
    registry: Arc<TunnelRegistry>,
    config: &ApiConfig,
    shutdown: CancellationToken,
) -> Result<(), tonic::transport::Error> {
    let addr = config.listen.parse().expect("invalid API listen address");

    let tunnel_svc = tunnel_svc::TunnelServiceImpl::new(Arc::clone(&registry));
    let stats_svc = stats_svc::StatsServiceImpl::new(Arc::clone(&registry));
    let health_svc = health_svc::HealthServiceImpl::new();

    tracing::info!(%addr, "gRPC server starting");

    Server::builder()
        .add_service(eoip_api::tunnel_service_server::TunnelServiceServer::new(
            tunnel_svc,
        ))
        .add_service(eoip_api::stats_service_server::StatsServiceServer::new(
            stats_svc,
        ))
        .add_service(eoip_api::health_service_server::HealthServiceServer::new(
            health_svc,
        ))
        .serve_with_shutdown(addr, async move {
            shutdown.cancelled().await;
            tracing::info!("gRPC server shutting down");
        })
        .await
}

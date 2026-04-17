//! gRPC client wrapper — connects to the EoIP-rs daemon.

use tonic::transport::Channel;

use eoip_api::health_service_client::HealthServiceClient;
use eoip_api::stats_service_client::StatsServiceClient;
use eoip_api::tunnel_service_client::TunnelServiceClient;

pub struct GrpcClient {
    pub tunnels: TunnelServiceClient<Channel>,
    pub stats: StatsServiceClient<Channel>,
    pub health: HealthServiceClient<Channel>,
}

impl GrpcClient {
    pub async fn connect(address: &str) -> Result<Self, tonic::transport::Error> {
        let channel = Channel::from_shared(address.to_string())
            .expect("invalid address")
            .connect()
            .await?;

        Ok(Self {
            tunnels: TunnelServiceClient::new(channel.clone()),
            stats: StatsServiceClient::new(channel.clone()),
            health: HealthServiceClient::new(channel),
        })
    }
}

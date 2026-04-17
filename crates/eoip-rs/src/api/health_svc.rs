//! HealthService gRPC implementation.

use tonic::{Request, Response, Status};

use eoip_api::*;
use eoip_api::health_check_response::ServingStatus;

#[derive(Default)]
pub struct HealthServiceImpl {
    // TODO: Add helper connection status tracking
}

impl HealthServiceImpl {
    pub fn new() -> Self {
        Self::default()
    }
}

#[tonic::async_trait]
impl health_service_server::HealthService for HealthServiceImpl {
    async fn check(
        &self,
        _request: Request<HealthCheckRequest>,
    ) -> Result<Response<HealthCheckResponse>, Status> {
        Ok(Response::new(HealthCheckResponse {
            status: ServingStatus::Serving.into(),
        }))
    }
}

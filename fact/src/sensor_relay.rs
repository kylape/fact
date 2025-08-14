use anyhow::{Context, Result};
use log::{debug, info, warn};
use tokio::sync::mpsc;
use tonic::{
    metadata::MetadataValue,
    service::{interceptor::InterceptedService, Interceptor},
    transport::{Channel, ClientTlsConfig},
};
use std::str::FromStr;

use crate::{
    certs::Certs,
    vsock::VmMessage,
};

use fact_api::{
    sensor::{
        virtual_machine_index_report_service_client::VirtualMachineIndexReportServiceClient,
        UpsertVirtualMachineIndexReportRequest,
    },
    virtualmachine::v1::IndexReport,
};

#[derive(Debug, Clone)]
struct UserAgentInterceptor {}

impl Interceptor for UserAgentInterceptor {
    fn call(
        &mut self,
        mut request: tonic::Request<()>,
    ) -> Result<tonic::Request<()>, tonic::Status> {
        request.metadata_mut().insert(
            "user-agent",
            MetadataValue::from_str("StackRox Fact VM Relay").unwrap(),
        );
        Ok(request)
    }
}

/// Relay for forwarding VM data to sensor
pub struct SensorRelay {
    endpoint: String,
    certs: Option<Certs>,
    client: Option<VirtualMachineIndexReportServiceClient<InterceptedService<Channel, UserAgentInterceptor>>>,
}

impl SensorRelay {
    /// Create a new sensor relay
    pub fn new(endpoint: String, certs: Option<Certs>) -> Self {
        SensorRelay {
            endpoint,
            certs,
            client: None,
        }
    }
    
    /// Start the sensor relay service
    pub async fn start(
        &mut self,
        mut vm_rx: mpsc::Receiver<VmMessage>,
        mut shutdown: tokio::sync::broadcast::Receiver<()>,
    ) -> Result<()> {
        info!("Starting sensor relay to {}", self.endpoint);
        
        // Connect to sensor
        self.connect().await?;
        
        // Main relay loop
        loop {
            tokio::select! {
                _ = shutdown.recv() => {
                    info!("Sensor relay shutting down");
                    break;
                }
                msg = vm_rx.recv() => {
                    match msg {
                        Some(vm_msg) => {
                            if let Err(e) = self.forward_vm_message(vm_msg).await {
                                warn!("Failed to forward VM message: {}", e);
                                // Try to reconnect on error
                                if let Err(e) = self.connect().await {
                                    warn!("Failed to reconnect to sensor: {}", e);
                                }
                            }
                        }
                        None => {
                            debug!("VM message channel closed");
                            break;
                        }
                    }
                }
            }
        }
        
        Ok(())
    }
    
    /// Connect to the sensor
    async fn connect(&mut self) -> Result<()> {
        info!("Connecting to sensor at {}", self.endpoint);
        
        let mut channel = Channel::from_shared(self.endpoint.clone())?;
        
        // Configure TLS if certificates are provided
        if let Some(certs) = &self.certs {
            let tls = ClientTlsConfig::new()
                .domain_name("sensor.stackrox.svc")
                .ca_certificate(certs.ca.clone())
                .identity(certs.identity.clone());
            channel = channel.tls_config(tls)?;
        }
        
        let channel = channel.connect().await
            .context("Failed to connect to sensor")?;
        
        let client = VirtualMachineIndexReportServiceClient::with_interceptor(
            channel,
            UserAgentInterceptor {},
        );
        
        self.client = Some(client);
        info!("Connected to sensor successfully");
        Ok(())
    }
    
    /// Forward a VM message to the sensor
    async fn forward_vm_message(&mut self, vm_msg: VmMessage) -> Result<()> {
        debug!("Forwarding VM message from {}: {} bytes", vm_msg.vm_id, vm_msg.data.len());
        
        let client = self.client.as_mut()
            .context("Sensor client not connected")?;
        
        // Deserialize the IndexReport data from protobuf
        let index_report = prost::Message::decode(vm_msg.data.as_slice())
            .context("Failed to decode IndexReport protobuf data")?;
        
        // Create the upsert request
        let request = UpsertVirtualMachineIndexReportRequest {
            index_report: Some(index_report),
        };
        
        // Send to sensor
        client.upsert_virtual_machine_index_report(request).await
            .context("Failed to send IndexReport to sensor")?;
        
        debug!("Successfully forwarded VM message from {}", vm_msg.vm_id);
        Ok(())
    }
    
    /// Send a test IndexReport message (for development/testing)
    pub async fn send_test_vm(&mut self, vm_id: String) -> Result<()> {
        let client = self.client.as_mut()
            .context("Sensor client not connected")?;
        
        let index_report = IndexReport {
            vsock_cid: vm_id.clone(),
            index_v4: Some(fact_api::scanner::v4::IndexReport {
                success: true,
                contents: Some(fact_api::scanner::v4::Contents {
                    packages: vec![], // Empty packages for test
                    ..Default::default()
                }),
                ..Default::default()
            }),
        };
        
        let request = UpsertVirtualMachineIndexReportRequest {
            index_report: Some(index_report),
        };
        
        client.upsert_virtual_machine_index_report(request).await
            .context("Failed to send test IndexReport to sensor")?;
        
        info!("Sent test IndexReport message for {}", vm_id);
        Ok(())
    }
}

impl Drop for SensorRelay {
    fn drop(&mut self) {
        info!("Sensor relay to {} shutting down", self.endpoint);
    }
}
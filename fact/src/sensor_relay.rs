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
    client::{
        sensor::{
            virtual_machine_service_client::VirtualMachineServiceClient, 
            UpsertVirtualMachineRequest,
        },
        storage::{VirtualMachine, VirtualMachineScan},
    },
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
    client: Option<VirtualMachineServiceClient<InterceptedService<Channel, UserAgentInterceptor>>>,
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
        
        let client = VirtualMachineServiceClient::with_interceptor(
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
        
        // Deserialize the VM data from protobuf
        let vm_data = prost::Message::decode(vm_msg.data.as_slice())
            .context("Failed to decode VM protobuf data")?;
        
        // Create the upsert request
        let request = UpsertVirtualMachineRequest {
            virtual_machine: Some(vm_data),
        };
        
        // Send to sensor
        client.upsert_virtual_machine(request).await
            .context("Failed to send VM data to sensor")?;
        
        debug!("Successfully forwarded VM message from {}", vm_msg.vm_id);
        Ok(())
    }
    
    /// Send a test VM message (for development/testing)
    pub async fn send_test_vm(&mut self, vm_id: String) -> Result<()> {
        let client = self.client.as_mut()
            .context("Sensor client not connected")?;
        
        let vm = VirtualMachine {
            id: vm_id.clone(),
            scan: Some(VirtualMachineScan {
                components: vec![], // Empty scan for test
                ..Default::default()
            }),
            ..Default::default()
        };
        
        let request = UpsertVirtualMachineRequest {
            virtual_machine: Some(vm),
        };
        
        client.upsert_virtual_machine(request).await
            .context("Failed to send test VM to sensor")?;
        
        info!("Sent test VM message for {}", vm_id);
        Ok(())
    }
}

impl Drop for SensorRelay {
    fn drop(&mut self) {
        info!("Sensor relay to {} shutting down", self.endpoint);
    }
}
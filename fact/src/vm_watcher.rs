use anyhow::{Context, Result};
use log::{debug, info, warn};
use serde_json::Value;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// Represents a virtual machine discovered in Kubernetes
#[derive(Debug, Clone)]
pub struct VirtualMachine {
    pub name: String,
    pub namespace: String,
    pub uid: String,
    pub cid: Option<u32>, // VSOCK Context ID if available
}

/// VM watcher that monitors Kubernetes for virtual machine objects
pub struct VmWatcher {
    vms: HashMap<String, VirtualMachine>,
    vm_tx: mpsc::Sender<VirtualMachine>,
}

impl VmWatcher {
    /// Create a new VM watcher
    pub fn new() -> (Self, mpsc::Receiver<VirtualMachine>) {
        let (vm_tx, vm_rx) = mpsc::channel(100);
        
        let watcher = VmWatcher {
            vms: HashMap::new(),
            vm_tx,
        };
        
        (watcher, vm_rx)
    }
    
    /// Start watching for VMs
    pub async fn start(&mut self, mut shutdown: tokio::sync::broadcast::Receiver<()>) -> Result<()> {
        info!("Starting VM watcher");
        
        // In a real implementation, this would use kube-rs or similar to watch Kubernetes resources
        // For now, we'll simulate VM discovery
        tokio::spawn(async move {
            // Simulate periodic VM discovery
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
            
            loop {
                tokio::select! {
                    _ = shutdown.recv() => {
                        info!("VM watcher shutting down");
                        break;
                    }
                    _ = interval.tick() => {
                        // In real implementation, this would query Kubernetes API
                        debug!("Scanning for VMs...");
                        // For now, just log that we're watching
                    }
                }
            }
        });
        
        Ok(())
    }
    
    /// Handle a VM event (add/update/delete)
    pub async fn handle_vm_event(&mut self, event_type: &str, vm_data: Value) -> Result<()> {
        match event_type {
            "ADDED" | "MODIFIED" => {
                if let Some(vm) = self.parse_vm_object(vm_data)? {
                    let vm_key = format!("{}/{}", vm.namespace, vm.name);
                    
                    if !self.vms.contains_key(&vm_key) {
                        info!("Discovered new VM: {}", vm_key);
                    }
                    
                    self.vms.insert(vm_key, vm.clone());
                    
                    if let Err(e) = self.vm_tx.send(vm).await {
                        warn!("Failed to send VM event: {}", e);
                    }
                }
            }
            "DELETED" => {
                if let Some(metadata) = vm_data.get("metadata") {
                    if let (Some(name), Some(namespace)) = (
                        metadata.get("name").and_then(|v| v.as_str()),
                        metadata.get("namespace").and_then(|v| v.as_str()),
                    ) {
                        let vm_key = format!("{}/{}", namespace, name);
                        if self.vms.remove(&vm_key).is_some() {
                            info!("VM removed: {}", vm_key);
                        }
                    }
                }
            }
            _ => {
                debug!("Unknown VM event type: {}", event_type);
            }
        }
        
        Ok(())
    }
    
    /// Parse a Kubernetes VM object into our VirtualMachine struct
    fn parse_vm_object(&self, vm_data: Value) -> Result<Option<VirtualMachine>> {
        let metadata = vm_data.get("metadata")
            .context("VM object missing metadata")?;
        
        let name = metadata.get("name")
            .and_then(|v| v.as_str())
            .context("VM object missing name")?;
        
        let namespace = metadata.get("namespace")
            .and_then(|v| v.as_str())
            .context("VM object missing namespace")?;
        
        let uid = metadata.get("uid")
            .and_then(|v| v.as_str())
            .context("VM object missing uid")?;
        
        // Try to extract VSOCK context ID from annotations or spec
        let cid = self.extract_vsock_cid(&vm_data);
        
        Ok(Some(VirtualMachine {
            name: name.to_string(),
            namespace: namespace.to_string(),
            uid: uid.to_string(),
            cid,
        }))
    }
    
    /// Extract VSOCK context ID from VM specification
    fn extract_vsock_cid(&self, vm_data: &Value) -> Option<u32> {
        // Check annotations first
        if let Some(annotations) = vm_data.get("metadata")
            .and_then(|m| m.get("annotations"))
            .and_then(|a| a.as_object())
        {
            if let Some(cid_str) = annotations.get("vsock.stackrox.io/cid")
                .and_then(|v| v.as_str())
            {
                if let Ok(cid) = cid_str.parse::<u32>() {
                    return Some(cid);
                }
            }
        }
        
        // Check spec for VSOCK configuration
        if let Some(spec) = vm_data.get("spec") {
            // This would depend on the specific VM CRD being used
            // For KubeVirt VMs, it might be in spec.template.spec.domain.devices
            if let Some(devices) = spec.get("template")
                .and_then(|t| t.get("spec"))
                .and_then(|s| s.get("domain"))
                .and_then(|d| d.get("devices"))
            {
                // Look for VSOCK device configuration
                if let Some(vsock) = devices.get("vsock") {
                    if let Some(cid) = vsock.get("contextId").and_then(|v| v.as_u64()) {
                        return Some(cid as u32);
                    }
                }
            }
        }
        
        None
    }
    
    /// Get a VM by its identifier
    pub fn get_vm(&self, vm_key: &str) -> Option<&VirtualMachine> {
        self.vms.get(vm_key)
    }
    
    /// Get all currently tracked VMs
    pub fn get_all_vms(&self) -> Vec<&VirtualMachine> {
        self.vms.values().collect()
    }
    
    /// Get VM count
    pub fn vm_count(&self) -> usize {
        self.vms.len()
    }
}
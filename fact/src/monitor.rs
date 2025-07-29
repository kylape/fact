use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::mpsc;

/// Common event types that monitors can produce
#[derive(Debug, Clone)]
pub enum MonitorEvent {
    FileActivity(crate::event::Event),
    PackageUpdate(crate::client::storage::VirtualMachine),
}

/// Trait that all monitors must implement
#[async_trait]
pub trait Monitor: Send + Sync {
    /// Unique identifier for this monitor type
    fn name(&self) -> &'static str;
    
    /// Human-readable description of what this monitor does
    fn description(&self) -> &'static str;
    
    /// Check if this monitor can run on the current system
    async fn can_run(&self) -> Result<bool>;
    
    /// Start the monitor and begin producing events
    async fn start(&mut self, event_sender: mpsc::Sender<MonitorEvent>) -> Result<()>;
    
    /// Stop the monitor gracefully
    async fn stop(&mut self) -> Result<()>;
    
    /// Get the current status of the monitor
    fn is_running(&self) -> bool;
}

/// Registry for managing monitor plugins
pub struct MonitorRegistry {
    monitors: Vec<Box<dyn Monitor>>,
}

impl MonitorRegistry {
    pub fn new() -> Self {
        Self {
            monitors: Vec::new(),
        }
    }
    
    /// Add a monitor to the registry
    pub fn register<M: Monitor + 'static>(&mut self, monitor: M) {
        self.monitors.push(Box::new(monitor));
    }
    
    /// Get all registered monitors
    pub fn monitors(&self) -> &[Box<dyn Monitor>] {
        &self.monitors
    }
    
    /// Get a monitor by name
    pub fn get_monitor(&self, name: &str) -> Option<&Box<dyn Monitor>> {
        self.monitors.iter().find(|m| m.name() == name)
    }
    
    /// Get a mutable monitor by name
    pub fn get_monitor_mut(&mut self, name: &str) -> Option<&mut Box<dyn Monitor>> {
        self.monitors.iter_mut().find(|m| m.name() == name)
    }
    
    /// List all available monitor names
    pub fn list_monitors(&self) -> Vec<&'static str> {
        self.monitors.iter().map(|m| m.name()).collect()
    }
}

impl Default for MonitorRegistry {
    fn default() -> Self {
        Self::new()
    }
}
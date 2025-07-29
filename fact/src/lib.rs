use std::time::Duration;

use anyhow::Result;
use config::FactConfig;
use log::{info, warn};
use monitor::{Monitor, MonitorEvent, MonitorRegistry};
use monitors::{
    file_monitor::{FileMonitor, FileMonitorConfig},
    package_monitor::{PackageMonitor, PackageMonitorConfig},
};
use tokio::{signal, sync::mpsc};

mod bpf;
pub mod certs;
pub mod client;
pub mod config;
mod event;
mod host_info;
pub mod monitor;
pub mod monitors;
mod vm_agent;
mod vsock;

pub async fn run(config: FactConfig) -> Result<()> {
    let config = config.with_defaults();
    
    // Create monitor registry and register available monitors
    let mut registry = MonitorRegistry::new();
    
    // Register file monitor if enabled
    if config.enable_file_monitor {
        let file_config = FileMonitorConfig {
            paths: config.paths.clone(),
        };
        let file_monitor = FileMonitor::new(file_config);
        registry.register(file_monitor);
    }
    
    // Register package monitor if enabled  
    if config.enable_package_monitor {
        let certs = if let Some(certs_path) = &config.certs {
            Some(certs_path.clone().try_into()?)
        } else {
            None
        };
        
        let package_config = PackageMonitorConfig {
            rpmdb: config.rpmdb.clone(),
            interval: Duration::from_secs(config.interval),
            url: if !config.skip_http { config.url.clone() } else { None },
            certs,
            use_vsock: config.use_vsock,
            skip_http: config.skip_http,
        };
        let package_monitor = PackageMonitor::new(package_config);
        registry.register(package_monitor);
    }
    
    // Start the monitoring system
    run_monitors(registry).await
}

async fn run_monitors(mut registry: MonitorRegistry) -> Result<()> {
    let (event_tx, mut event_rx) = mpsc::channel(1000);
    
    info!("Starting fact monitoring system");
    
    // Check which monitors can run and start them
    let mut started_monitors = Vec::new();
    for monitor in registry.monitors.iter_mut() {
        match monitor.can_run().await {
            Ok(true) => {
                info!("Starting monitor: {}", monitor.name());
                match monitor.start(event_tx.clone()).await {
                    Ok(()) => {
                        started_monitors.push(monitor.name());
                        info!("Monitor {} started successfully", monitor.name());
                    }
                    Err(e) => {
                        warn!("Failed to start monitor {}: {}", monitor.name(), e);
                    }
                }
            }
            Ok(false) => {
                info!("Monitor {} cannot run on this system", monitor.name());
            }
            Err(e) => {
                warn!("Error checking if monitor {} can run: {}", monitor.name(), e);
            }
        }
    }
    
    if started_monitors.is_empty() {
        warn!("No monitors were started!");
        return Ok(());
    }
    
    info!("Started {} monitor(s): {:?}", started_monitors.len(), started_monitors);
    
    // Create the gRPC client for file events if needed
    let mut client = if let Some(url) = std::env::var("FACT_URL").ok() {
        let certs_path = std::env::var("FACT_CERTS").ok().map(std::path::PathBuf::from);
        match client::Client::start(&url, certs_path) {
            Ok(client) => Some(client),
            Err(e) => {
                warn!("Failed to create gRPC client: {}", e);
                None
            }
        }
    } else {
        None
    };
    
    // Event processing loop
    let event_processing = tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            match event {
                MonitorEvent::FileActivity(file_event) => {
                    println!("{file_event:?}");
                    if let Some(client) = client.as_mut() {
                        if let Err(e) = client.send(file_event).await {
                            warn!("Failed to send file event: {}", e);
                        }
                    }
                }
                MonitorEvent::PackageUpdate(vm_data) => {
                    info!(
                        "Package update: {} components", 
                        vm_data.scan.as_ref().map(|s| s.components.len()).unwrap_or(0)
                    );
                }
            }
        }
    });
    
    // Wait for shutdown signal
    let ctrl_c = signal::ctrl_c();
    info!("Monitoring system running. Press Ctrl-C to exit...");
    ctrl_c.await?;
    info!("Shutdown signal received, stopping monitors...");
    
    // Stop all monitors
    for monitor in registry.monitors.iter_mut() {
        if monitor.is_running() {
            if let Err(e) = monitor.stop().await {
                warn!("Error stopping monitor {}: {}", monitor.name(), e);
            } else {
                info!("Monitor {} stopped", monitor.name());
            }
        }
    }
    
    // Cancel event processing
    event_processing.abort();
    
    info!("Fact monitoring system stopped");
    Ok(())
}

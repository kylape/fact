use std::{
    env, fs::read_to_string, path::PathBuf, process::Command, str::FromStr, sync::LazyLock,
    time::Duration,
};

use anyhow::{Context, Result};
use async_trait::async_trait;
use crossbeam::{
    channel::{bounded, tick},
    select,
};
use log::{debug, info};
use prost::Message;
use tokio::sync::mpsc;
use tonic::{
    metadata::MetadataValue,
    service::{interceptor::InterceptedService, Interceptor},
    transport::{Channel, ClientTlsConfig},
};

use crate::{
    certs::Certs,
    client::{
        sensor::{
            virtual_machine_service_client::VirtualMachineServiceClient, UpsertVirtualMachineRequest,
        },
        storage::{EmbeddedImageScanComponent, VirtualMachine, VirtualMachineScan},
    },
    monitor::{Monitor, MonitorEvent},
    vsock::VsockClient,
};

static HOST_MOUNT: LazyLock<PathBuf> =
    LazyLock::new(|| env::var("FACT_HOST_MOUNT").unwrap_or_default().into());

static HOSTNAME: LazyLock<String> = LazyLock::new(|| {
    let hostname_paths = ["/etc/hostname", "/proc/sys/kernel/hostname"];
    for p in hostname_paths {
        let p = HOST_MOUNT.join(p);
        if p.exists() {
            return read_to_string(p).unwrap().trim().to_string();
        }
    }
    "no-hostname".to_string()
});

/// Configuration for the package monitor
#[derive(Debug, Clone)]
pub struct PackageMonitorConfig {
    pub rpmdb: String,
    pub interval: Duration,
    pub url: Option<String>,
    pub certs: Option<Certs>,
    pub use_vsock: bool,
    pub skip_http: bool,
}

#[derive(Debug, Clone)]
struct UserAgentInterceptor {}

impl Interceptor for UserAgentInterceptor {
    fn call(
        &mut self,
        mut request: tonic::Request<()>,
    ) -> Result<tonic::Request<()>, tonic::Status> {
        request.metadata_mut().insert(
            "user-agent",
            MetadataValue::from_str("Rox Admission Controller").unwrap(),
        );
        Ok(request)
    }
}

/// Package scanning monitor that periodically scans the RPM database
pub struct PackageMonitor {
    config: PackageMonitorConfig,
    running: bool,
    cmd: Command,
    user_agent: UserAgentInterceptor,
}

impl PackageMonitor {
    pub fn new(config: PackageMonitorConfig) -> Self {
        let mut cmd = Command::new("rpm");
        cmd.args([
            "--dbpath",
            &config.rpmdb,
            "-qa",
            "--qf",
            "%{NAME}|%{VERSION}|%{RELEASE}|%{ARCH}\n",
        ]);

        Self {
            config,
            running: false,
            cmd,
            user_agent: UserAgentInterceptor {},
        }
    }

    async fn scan_packages(&mut self) -> Result<Vec<EmbeddedImageScanComponent>> {
        info!("PackageMonitor collecting package information...");
        let pkgs = self.cmd.output().context("Failed to run rpm command")?;
        let stdout =
            std::str::from_utf8(pkgs.stdout.as_slice()).context("Failed to parse rpm output")?;

        info!("PackageMonitor parsing...");
        let pkgs = stdout
            .lines()
            .map(str::parse::<EmbeddedImageScanComponent>)
            .collect::<Result<Vec<_>, _>>()
            .context("Failed to parse package information")?;
        
        debug!("PackageMonitor found {} packages", pkgs.len());
        Ok(pkgs)
    }

    async fn create_client(
        &self,
        url: String,
    ) -> Result<
        VirtualMachineServiceClient<InterceptedService<Channel, UserAgentInterceptor>>,
    > {
        let mut channel = Channel::from_shared(url)?;

        if let Some(certs) = &self.config.certs {
            let tls = ClientTlsConfig::new()
                .domain_name("sensor.stackrox.svc")
                .ca_certificate(certs.ca.clone())
                .identity(certs.identity.clone());
            channel = channel.tls_config(tls)?;
        }

        let channel = channel.connect().await?;
        let client =
            VirtualMachineServiceClient::with_interceptor(channel, self.user_agent.clone());
        Ok(client)
    }

    async fn send_grpc(&self, url: String, pkgs: Vec<EmbeddedImageScanComponent>) -> Result<()> {
        let mut client = self.create_client(url).await?;
        let scan = VirtualMachineScan {
            components: pkgs,
            ..Default::default()
        };
        let vm = VirtualMachine {
            id: HOSTNAME.to_string(),
            scan: Some(scan),
            ..Default::default()
        };
        let request = UpsertVirtualMachineRequest {
            virtual_machine: Some(vm),
        };

        client.upsert_virtual_machine(request).await?;
        Ok(())
    }

    async fn send_vsock(&self, pkgs: Vec<EmbeddedImageScanComponent>) -> Result<()> {
        if !VsockClient::is_available() {
            return Err(anyhow::anyhow!("VSOCK is not available on this system"));
        }

        let mut client = VsockClient::connect()
            .context("Failed to connect to VSOCK endpoint")?;

        let scan = VirtualMachineScan {
            components: pkgs,
            ..Default::default()
        };
        let vm = VirtualMachine {
            id: HOSTNAME.to_string(),
            scan: Some(scan),
            ..Default::default()
        };

        let data = vm.encode_to_vec();
        client.send_data(&data)
            .context("Failed to send VM data via VSOCK")?;

        info!("PackageMonitor sent {} packages via VSOCK", 
              vm.scan.as_ref().map(|s| s.components.len()).unwrap_or(0));
        Ok(())
    }

    async fn process_packages(&mut self, event_sender: &mpsc::Sender<MonitorEvent>) -> Result<()> {
        let pkgs = self.scan_packages().await?;
        
        // Create VM data for event
        let scan = VirtualMachineScan {
            components: pkgs.clone(),
            ..Default::default()
        };
        let vm = VirtualMachine {
            id: HOSTNAME.to_string(),
            scan: Some(scan),
            ..Default::default()
        };

        // Send event to event processor
        if let Err(e) = event_sender.send(MonitorEvent::PackageUpdate(vm)).await {
            debug!("PackageMonitor event channel closed: {}", e);
            return Ok(());
        }

        // Send to external services if configured
        if self.config.use_vsock {
            if let Err(e) = self.send_vsock(pkgs).await {
                debug!("PackageMonitor VSOCK send failed: {}", e);
            }
        } else if let Some(url) = &self.config.url {
            if let Err(e) = self.send_grpc(url.clone(), pkgs).await {
                debug!("PackageMonitor gRPC send failed: {}", e);
            }
        }

        Ok(())
    }
}

#[async_trait]
impl Monitor for PackageMonitor {
    fn name(&self) -> &'static str {
        "package_monitor"
    }

    fn description(&self) -> &'static str {
        "Periodically scans RPM database for installed packages"
    }

    async fn can_run(&self) -> Result<bool> {
        // Check if rpm command exists
        match which::which("rpm") {
            Ok(_) => {},
            Err(_) => {
                debug!("PackageMonitor requires 'rpm' command to be available");
                return Ok(false);
            }
        }

        // Check if RPM database exists
        let rpmdb_path = PathBuf::from(&self.config.rpmdb);
        if !rpmdb_path.exists() {
            debug!("PackageMonitor RPM database not found at: {}", self.config.rpmdb);
            return Ok(false);
        }

        // Check VSOCK availability if requested
        if self.config.use_vsock && !VsockClient::is_available() {
            debug!("PackageMonitor VSOCK requested but not available");
            return Ok(false);
        }

        Ok(true)
    }

    async fn start(&mut self, event_sender: mpsc::Sender<MonitorEvent>) -> Result<()> {
        if self.running {
            return Ok(());
        }

        info!("Starting PackageMonitor with interval: {:?}", self.config.interval);

        if self.config.use_vsock {
            info!("PackageMonitor using VSOCK communication");
        } else if self.config.url.is_some() {
            info!("PackageMonitor using gRPC communication");
        } else {
            info!("PackageMonitor in local-only mode");
        }

        self.running = true;

        let (shutdown_tx, shutdown_rx) = bounded(1);
        let ticks = tick(self.config.interval);
        
        // Register signal handler
        let shutdown_tx_clone = shutdown_tx.clone();
        tokio::spawn(async move {
            tokio::signal::ctrl_c().await.ok();
            let _ = shutdown_tx_clone.send(());
        });

        // Run once immediately
        if let Err(e) = self.process_packages(&event_sender).await {
            debug!("PackageMonitor initial scan failed: {}", e);
        }

        // Main monitoring loop
        let event_sender_clone = event_sender.clone();
        let mut monitor_clone = self.clone();
        tokio::spawn(async move {
            loop {
                select! {
                    recv(ticks) -> _ => {
                        if let Err(e) = monitor_clone.process_packages(&event_sender_clone).await {
                            debug!("PackageMonitor scan failed: {}", e);
                        }
                    }
                    recv(shutdown_rx) -> _ => {
                        info!("PackageMonitor shutting down...");
                        break;
                    }
                }
            }
        });

        info!("PackageMonitor started successfully");
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if !self.running {
            return Ok(());
        }

        info!("Stopping PackageMonitor");
        self.running = false;
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.running
    }
}

// Make PackageMonitor cloneable for the async task
impl Clone for PackageMonitor {
    fn clone(&self) -> Self {
        let mut cmd = Command::new("rpm");
        cmd.args([
            "--dbpath",
            &self.config.rpmdb,
            "-qa",
            "--qf",
            "%{NAME}|%{VERSION}|%{RELEASE}|%{ARCH}\n",
        ]);

        Self {
            config: self.config.clone(),
            running: self.running,
            cmd,
            user_agent: UserAgentInterceptor {},
        }
    }
}
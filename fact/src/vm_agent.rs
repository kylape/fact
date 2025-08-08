use std::{env, fs::read_to_string, path::PathBuf, process::Command, str::FromStr, sync::LazyLock};

use anyhow::Context;
use crate::certs::Certs;
use crate::config::FactConfig;
use fact_api::{
    sensor::{
        virtual_machine_service_client::VirtualMachineServiceClient, UpsertVirtualMachineRequest,
    },
    storage::{EmbeddedImageScanComponent, VirtualMachine, VirtualMachineScan},
};
use tokio::{
    sync::mpsc,
    time::{interval, Duration},
    select,
};
use log::{debug, info};
use prost::Message;
use tonic::{
    metadata::MetadataValue,
    service::{interceptor::InterceptedService, Interceptor},
    transport::{Channel, ClientTlsConfig},
};
use crate::vsock::VsockClient;

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

struct VmAgent {
    url: Option<String>,
    cmd: Command,
    certs: Option<Certs>,
    user_agent: UserAgentInterceptor,
    use_vsock: bool,
}

impl VmAgent {
    async fn run(&mut self) -> anyhow::Result<()> {
        info!("Collecting package information...");
        let pkgs = self.cmd.output().context("Failed to run rpm command")?;
        let stdout =
            std::str::from_utf8(pkgs.stdout.as_slice()).context("Failed to parse rpm output")?;

        info!("Parsing...");
        let pkgs = stdout
            .lines()
            .filter_map(|line| {
                let parts: Vec<&str> = line.split('|').collect();
                if parts.len() >= 4 {
                    Some(EmbeddedImageScanComponent {
                        name: parts[0].to_string(),
                        version: format!("{}-{}", parts[1], parts[2]), // Combine version and release
                        architecture: parts[3].to_string(),
                        source: 0, // SourceType::Os
                        license: None,
                        vulns: vec![],
                        priority: 0,
                        location: String::new(),
                        risk_score: 0.0,
                        fixed_by: String::new(),
                        executables: vec![],
                        has_layer_index: None,
                        set_top_cvss: None,
                    })
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        debug!("{pkgs:?}");

        info!("Sending updates...");

        if self.use_vsock {
            self.send_vsock(pkgs).await?;
        } else if let Some(url) = &self.url {
            self.send_grpc(url.to_string(), pkgs).await?;
        }
        Ok(())
    }

    async fn create_client(
        &self,
        url: String,
    ) -> anyhow::Result<
        VirtualMachineServiceClient<InterceptedService<Channel, UserAgentInterceptor>>,
    > {
        let mut channel = Channel::from_shared(url)?;

        if let Some(certs) = &self.certs {
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

    async fn send_grpc(&self, url: String, pkgs: Vec<EmbeddedImageScanComponent>) -> anyhow::Result<()> {
        let mut client = self.create_client(url).await?;
        let scan = VirtualMachineScan {
            components: pkgs,
            ..Default::default()
        };
        let vm = VirtualMachine {
            id: HOSTNAME.to_string(),
            name: HOSTNAME.to_string(),
            scan: Some(scan),
            ..Default::default()
        };
        let request = UpsertVirtualMachineRequest {
            virtual_machine: Some(vm),
        };

        client.upsert_virtual_machine(request).await?;
        Ok(())
    }

    async fn send_vsock(&self, pkgs: Vec<EmbeddedImageScanComponent>) -> anyhow::Result<()> {
        if !VsockClient::is_available() {
            return Err(anyhow::anyhow!("VSOCK is not available on this system"));
        }

        let mut client = VsockClient::connect()
            .context("Failed to connect to VSOCK endpoint")?;

        // Create package data message
        let scan = VirtualMachineScan {
            components: pkgs,
            ..Default::default()
        };
        let vm = VirtualMachine {
            id: HOSTNAME.to_string(),
            scan: Some(scan),
            ..Default::default()
        };

        // Serialize the VM data to protobuf bytes
        let data = vm.encode_to_vec();

        // Send the protobuf data
        client.send_data(&data)
            .context("Failed to send VM data via VSOCK")?;

        info!("Successfully sent {} packages via VSOCK", vm.scan.as_ref().map(|s| s.components.len()).unwrap_or(0));
        Ok(())
    }
}

impl TryFrom<&FactConfig> for VmAgent {
    type Error = anyhow::Error;

    fn try_from(cfg: &FactConfig) -> Result<Self, Self::Error> {
        let url = if !cfg.skip_http { cfg.url.clone() } else { None };
        let mut cmd = Command::new("rpm");
        cmd.args([
            "--dbpath",
            &cfg.rpmdb,
            "-qa",
            "--qf",
            "%{NAME}|%{VERSION}|%{RELEASE}|%{ARCH}\n",
        ]);
        let certs = if let Some(path) = &cfg.certs {
            Some(path.clone().try_into()?)
        } else {
            None
        };

        Ok(VmAgent {
            url,
            cmd,
            certs,
            user_agent: UserAgentInterceptor {},
            use_vsock: cfg.use_vsock,
        })
    }
}

pub async fn run_vm_agent(config: &FactConfig) -> anyhow::Result<()> {
    let (tx, mut rx) = mpsc::channel::<()>(1);
    let mut ticks = interval(Duration::from_secs(config.interval));
    let mut vm_agent: VmAgent = config.try_into()?;

    // Check VSOCK availability if requested
    if vm_agent.use_vsock {
        if !VsockClient::is_available() {
            return Err(anyhow::anyhow!(
                "VSOCK support requested but not available on this system. \
                Ensure the VM has autoattachVSOCK: true configured."
            ));
        }
        info!("Using VSOCK communication mode");
    } else if vm_agent.url.is_some() {
        info!("Using gRPC communication mode");
    } else {
        info!("No communication method configured");
    }

    ctrlc::set_handler(move || {
        let _ = tx.try_send(());
    })
    .context("Failed setting signal handler")?;

    // Run once before going into the loop
    vm_agent.run().await?;

    loop {
        select! {
            _ = ticks.tick() => {
                vm_agent.run().await?;
            }
            _ = rx.recv() => {
                info!("Shutting down...");
                break;
            }
        }
    }

    Ok(())
}

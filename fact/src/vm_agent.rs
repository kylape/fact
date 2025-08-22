use std::{env, fs::read_to_string, path::PathBuf, process::Command, str::FromStr, sync::LazyLock, collections::HashMap};

use anyhow::Context;
use crate::certs::Certs;
use crate::config::FactConfig;
use fact_api::{
    sensor::{
        virtual_machine_index_report_service_client::VirtualMachineIndexReportServiceClient,
        UpsertVirtualMachineIndexReportRequest,
    },
    virtualmachine::v1::IndexReport,
    scanner::v4::{Contents, Package, Distribution},
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

static SYSTEM_CPE: LazyLock<String> = LazyLock::new(|| {
    let cpe_path = HOST_MOUNT.join("/etc/system-release-cpe");
    if cpe_path.exists() {
        read_to_string(cpe_path).unwrap_or_default().trim().to_string()
    } else {
        String::new()
    }
});

static DISTRIBUTION: LazyLock<Distribution> = LazyLock::new(|| {
    create_distribution()
});

fn create_distribution() -> Distribution {
    let os_release_path = HOST_MOUNT.join("/etc/os-release");
    let mut fields = HashMap::new();
    
    if os_release_path.exists() {
        if let Ok(content) = read_to_string(&os_release_path) {
            for line in content.lines() {
                if let Some((key, value)) = line.split_once('=') {
                    let value = value.trim_matches('"');
                    fields.insert(key.to_string(), value.to_string());
                }
            }
        }
    }
    
    // Get system architecture
    let arch = std::process::Command::new("uname")
        .arg("-m")
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    
    let id = fields.get("ID").cloned().unwrap_or_else(|| "unknown".to_string());
    let version = fields.get("VERSION_ID").cloned().unwrap_or_else(|| "unknown".to_string());
    
    Distribution {
        id: format!("{}-{}", id, version),
        did: id.clone(),
        name: fields.get("NAME").cloned().unwrap_or_else(|| "Unknown".to_string()),
        version: version.clone(),
        version_code_name: fields.get("VERSION_CODENAME").cloned().unwrap_or_default(),
        version_id: version,
        arch,
        cpe: fields.get("CPE_NAME").cloned().unwrap_or_default(),
        pretty_name: fields.get("PRETTY_NAME").cloned().unwrap_or_else(|| "Unknown".to_string()),
    }
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
                    let version = format!("{}-{}", parts[1], parts[2]); // Combine version and release
                    Some(Package {
                        id: format!("{}-{}", parts[0], version),
                        name: parts[0].to_string(),
                        version,
                        arch: parts[3].to_string(),
                        cpe: SYSTEM_CPE.clone(),
                        ..Default::default()
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
        VirtualMachineIndexReportServiceClient<InterceptedService<Channel, UserAgentInterceptor>>,
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
            VirtualMachineIndexReportServiceClient::with_interceptor(channel, self.user_agent.clone());
        Ok(client)
    }

    async fn send_grpc(&self, url: String, pkgs: Vec<Package>) -> anyhow::Result<()> {
        let mut client = self.create_client(url).await?;

        let contents = Contents {
            packages: pkgs,
            distributions: vec![DISTRIBUTION.clone()],
            ..Default::default()
        };

        let index_v4 = fact_api::scanner::v4::IndexReport {
            hash_id: format!("/v4/vm/{}", HOSTNAME.as_str()),
            success: true,
            contents: Some(contents),
            ..Default::default()
        };

        let index_report = IndexReport {
            vsock_cid: HOSTNAME.to_string(), // Use hostname as identifier for now
            index_v4: Some(index_v4),
        };

        println!("Full IndexReport content (gRPC): {:#?}", index_report);

        let request = UpsertVirtualMachineIndexReportRequest {
            index_report: Some(index_report),
        };

        client.upsert_virtual_machine_index_report(request).await?;
        Ok(())
    }

    async fn send_vsock(&self, pkgs: Vec<Package>) -> anyhow::Result<()> {
        if !VsockClient::is_available() {
            return Err(anyhow::anyhow!("VSOCK is not available on this system"));
        }

        let mut client = VsockClient::connect()
            .context("Failed to connect to VSOCK endpoint")?;

        let contents = Contents {
            packages: pkgs,
            distributions: vec![DISTRIBUTION.clone()],
            ..Default::default()
        };

        let index_v4 = fact_api::scanner::v4::IndexReport {
            success: true,
            contents: Some(contents),
            ..Default::default()
        };

        let index_report = IndexReport {
            vsock_cid: HOSTNAME.to_string(),
            index_v4: Some(index_v4),
        };

        println!("Full IndexReport content (VSOCK): {:#?}", index_report);

        // Serialize the IndexReport to protobuf bytes
        let data = index_report.encode_to_vec();

        // Send the protobuf data
        client.send_data(&data)
            .context("Failed to send VM data via VSOCK")?;

        info!("Successfully sent {} packages via VSOCK", index_report.index_v4.as_ref().and_then(|i| i.contents.as_ref()).map(|c| c.packages.len()).unwrap_or(0));
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

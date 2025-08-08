use std::path::PathBuf;

use clap::{Parser, ValueEnum};

#[derive(Debug, Clone, ValueEnum)]
pub enum AgentMode {
    /// File monitoring mode (eBPF, for Kubernetes nodes)
    FileMonitor,
    /// VM agent mode (package scanning, for virtual machines)
    VmAgent,
    /// VSOCK listener mode (server for VM connections)
    VsockListener,
    /// Hybrid mode (VM agent + VSOCK listener)
    Hybrid,
}

#[derive(Debug, Clone, Parser)]
#[clap(version, about)]
pub struct FactConfig {
    /// Agent mode
    #[arg(long, env = "FACT_MODE", default_value = "file-monitor")]
    pub mode: AgentMode,

    /// List of paths to be monitored (file-monitor mode only)
    #[clap(short, long, num_args = 0..16, value_delimiter = ':')]
    pub paths: Vec<PathBuf>,

    /// URL to forward the packages to
    #[arg(long, env = "FACT_URL")]
    pub url: Option<String>,

    /// Directory holding the mTLS certificates and keys
    #[arg(short, long, env = "FACT_CERTS")]
    pub certs: Option<PathBuf>,

    /// Skip sending packages over HTTP (vm-agent mode)
    #[arg(long, env = "FACT_SKIP_HTTP")]
    pub skip_http: bool,

    /// Use VSOCK instead of HTTP/gRPC for communication (vm-agent mode)
    #[arg(long, env = "FACT_USE_VSOCK")]
    pub use_vsock: bool,

    /// Path to the rpmdb (vm-agent mode)
    #[arg(long, env = "FACT_RPMDB", default_value = "/var/lib/rpm")]
    pub rpmdb: String,

    /// Interval between package scanning in seconds (vm-agent mode)
    #[arg(long, env = "FACT_INTERVAL", default_value_t = 3600)]
    pub interval: u64,

    /// VSOCK port to listen on (vsock-listener/hybrid mode)
    #[arg(long, env = "FACT_VSOCK_PORT", default_value_t = 818)]
    pub vsock_port: u32,

    /// Sensor endpoint for relaying VM data (vsock-listener/hybrid mode)
    #[arg(long, env = "FACT_SENSOR_ENDPOINT", default_value = "sensor:443")]
    pub sensor_endpoint: String,

    /// Enable VSOCK server functionality (hybrid mode)
    #[arg(long, env = "FACT_ENABLE_VSOCK_SERVER")]
    pub enable_vsock_server: bool,

    /// Enable VM agent functionality (hybrid mode)
    #[arg(long, env = "FACT_ENABLE_VM_AGENT")]
    pub enable_vm_agent: bool,
}

use std::path::PathBuf;

use clap::Parser;

#[derive(Debug, Parser)]
#[clap(version, about)]
pub struct FactConfig {
    /// Enable file monitoring (requires root for eBPF)
    #[arg(long, env = "FACT_ENABLE_FILE_MONITOR")]
    pub enable_file_monitor: bool,

    /// Enable package monitoring  
    #[arg(long, env = "FACT_ENABLE_PACKAGE_MONITOR")]
    pub enable_package_monitor: bool,

    /// List of paths to be monitored by file monitor
    #[clap(short, long, num_args = 0..16, value_delimiter = ':')]
    pub paths: Vec<PathBuf>,

    /// URL to forward events to
    #[arg(env = "FACT_URL")]
    pub url: Option<String>,

    /// Directory holding the mTLS certificates and keys
    #[arg(short, long, env = "FACT_CERTS")]
    pub certs: Option<PathBuf>,

    /// Skip sending data over HTTP
    #[arg(long, env = "FACT_SKIP_HTTP")]
    pub skip_http: bool,

    /// Use VSOCK instead of HTTP/gRPC for communication
    #[arg(long, env = "FACT_USE_VSOCK")]
    pub use_vsock: bool,

    /// Path to the rpmdb for package monitoring
    #[arg(long, env = "FACT_RPMDB", default_value = "/var/lib/rpm")]
    pub rpmdb: String,

    /// Interval between package scanning in seconds
    #[arg(long, env = "FACT_INTERVAL", default_value_t = 3600)]
    pub interval: u64,
}

impl FactConfig {
    /// Check if any monitors are enabled, if not enable reasonable defaults
    pub fn with_defaults(mut self) -> Self {
        if !self.enable_file_monitor && !self.enable_package_monitor {
            // Default behavior: enable file monitor if paths are provided, otherwise package monitor
            if !self.paths.is_empty() {
                self.enable_file_monitor = true;
            } else {
                self.enable_package_monitor = true;
            }
        }
        self
    }
}

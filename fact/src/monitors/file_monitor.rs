use std::path::PathBuf;

use anyhow::Result;
use async_trait::async_trait;
use aya::{
    maps::{Array, MapData, RingBuf},
    programs::Lsm,
    Btf,
};
use log::{debug, info};
use tokio::{io::unix::AsyncFd, sync::mpsc, task::yield_now};

use crate::{
    bpf::bindings::{event_t, path_cfg_t},
    event::Event,
    monitor::{Monitor, MonitorEvent},
};

/// Configuration for the file monitor
#[derive(Debug, Clone)]
pub struct FileMonitorConfig {
    pub paths: Vec<PathBuf>,
}

/// eBPF-based file access monitor
pub struct FileMonitor {
    config: FileMonitorConfig,
    running: bool,
}

impl FileMonitor {
    pub fn new(config: FileMonitorConfig) -> Self {
        Self {
            config,
            running: false,
        }
    }
}

#[async_trait]
impl Monitor for FileMonitor {
    fn name(&self) -> &'static str {
        "file_monitor"
    }

    fn description(&self) -> &'static str {
        "Monitors file access patterns using eBPF LSM hooks"
    }

    async fn can_run(&self) -> Result<bool> {
        // Check if we have root privileges (required for eBPF)
        if unsafe { libc::geteuid() } != 0 {
            debug!("FileMonitor requires root privileges for eBPF operations");
            return Ok(false);
        }

        // Check if we have paths to monitor
        if self.config.paths.is_empty() {
            debug!("FileMonitor has no paths configured to monitor");
            return Ok(false);
        }

        // Try to load BTF to see if eBPF is supported
        match Btf::from_sys_fs() {
            Ok(_) => Ok(true),
            Err(e) => {
                debug!("FileMonitor cannot load BTF, eBPF not supported: {}", e);
                Ok(false)
            }
        }
    }

    async fn start(&mut self, event_sender: mpsc::Sender<MonitorEvent>) -> Result<()> {
        if self.running {
            return Ok(());
        }

        info!("Starting FileMonitor with {} paths", self.config.paths.len());

        // Bump the memlock rlimit for older kernels
        let rlim = libc::rlimit {
            rlim_cur: libc::RLIM_INFINITY,
            rlim_max: libc::RLIM_INFINITY,
        };
        let ret = unsafe { libc::setrlimit(libc::RLIMIT_MEMLOCK, &rlim) };
        if ret != 0 {
            debug!("remove limit on locked memory failed, ret is: {ret}");
        }

        // Load the eBPF program
        let mut bpf = aya::EbpfLoader::new()
            .set_global("paths_len", &(self.config.paths.len() as u32), true)
            .load(aya::include_bytes_aligned!(concat!(
                env!("OUT_DIR"),
                "/main.o"
            )))?;

        // Setup the ring buffer for events
        let ringbuf = bpf.take_map("rb").unwrap();
        let ringbuf = RingBuf::try_from(ringbuf)?;
        let mut async_fd = AsyncFd::new(ringbuf)?;

        // Setup the map with the paths to be monitored
        let paths_map = bpf.take_map("paths_map").unwrap();
        let mut paths_map: Array<MapData, path_cfg_t> = Array::try_from(paths_map)?;
        let mut path_cfg = path_cfg_t::new();
        for (i, p) in self.config.paths.iter().enumerate() {
            info!("FileMonitor monitoring: {p:?}");
            path_cfg.set(p.to_str().unwrap());
            paths_map.set(i as u32, path_cfg, 0)?;
        }

        // Load and attach the eBPF program
        let btf = Btf::from_sys_fs()?;
        let program: &mut Lsm = bpf.program_mut("trace_file_open").unwrap().try_into()?;
        program.load("file_open", &btf)?;
        program.attach()?;

        self.running = true;

        // Spawn the event processing task
        tokio::spawn(async move {
            loop {
                let mut guard = async_fd.readable_mut().await.unwrap();
                let ringbuf = guard.get_inner_mut();
                while let Some(event) = ringbuf.next() {
                    let event: &event_t = unsafe { &*(event.as_ptr() as *const _) };
                    match Event::try_from(event) {
                        Ok(event) => {
                            debug!("FileMonitor event: {event:?}");
                            if event_sender
                                .send(MonitorEvent::FileActivity(event))
                                .await
                                .is_err()
                            {
                                debug!("FileMonitor event channel closed, stopping");
                                break;
                            }
                        }
                        Err(e) => {
                            debug!("Failed to parse eBPF event: {}", e);
                        }
                    }
                }
                guard.clear_ready();
                yield_now().await;
            }
        });

        info!("FileMonitor started successfully");
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if !self.running {
            return Ok(());
        }

        info!("Stopping FileMonitor");
        self.running = false;
        // Note: In a real implementation, we'd need to handle cleanup of eBPF resources
        // For now, the program will be cleaned up when the process exits
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.running
    }
}
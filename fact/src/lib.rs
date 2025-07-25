use aya::{
    maps::{Array, MapData, RingBuf},
    programs::Lsm,
    Btf,
};
use client::Client;
use config::{AgentMode, FactConfig};
use event::Event;
use log::{debug, info, warn};
use tokio::{io::unix::AsyncFd, signal, task::yield_now};

mod bpf;
mod certs;
mod client;
pub mod config;
mod event;
mod host_info;
mod sensor_relay;
mod vm_agent;
mod vm_watcher;
mod vsock;

use bpf::bindings::{event_t, path_cfg_t};

pub async fn run(config: FactConfig) -> anyhow::Result<()> {
    match config.mode {
        AgentMode::FileMonitor => run_file_monitor(config).await,
        AgentMode::VmAgent => vm_agent::run_vm_agent(&config).await,
        AgentMode::VsockListener => run_vsock_listener(config).await,
        AgentMode::Hybrid => run_hybrid_mode(config).await,
    }
}

async fn run_file_monitor(config: FactConfig) -> anyhow::Result<()> {
    // Bump the memlock rlimit. This is needed for older kernels that don't use the
    // new memcg based accounting, see https://lwn.net/Articles/837122/
    let rlim = libc::rlimit {
        rlim_cur: libc::RLIM_INFINITY,
        rlim_max: libc::RLIM_INFINITY,
    };
    let ret = unsafe { libc::setrlimit(libc::RLIMIT_MEMLOCK, &rlim) };
    if ret != 0 {
        debug!("remove limit on locked memory failed, ret is: {ret}");
    }

    // Include the BPF object as raw bytes at compile-time and load it
    // at runtime.
    let mut bpf = aya::EbpfLoader::new()
        .set_global("paths_len", &(config.paths.len() as u32), true)
        .load(aya::include_bytes_aligned!(concat!(
            env!("OUT_DIR"),
            "/main.o"
        )))?;

    // Setup the ring buffer for events.
    let ringbuf = bpf.take_map("rb").unwrap();
    let ringbuf = RingBuf::try_from(ringbuf)?;
    let mut async_fd = AsyncFd::new(ringbuf)?;

    // Setup the map with the paths to be monitored
    let paths_map = bpf.take_map("paths_map").unwrap();
    let mut paths_map: Array<MapData, path_cfg_t> = Array::try_from(paths_map)?;
    let mut path_cfg = path_cfg_t::new();
    for (i, p) in config.paths.iter().enumerate() {
        info!("Monitoring: {p:?}");
        path_cfg.set(p.to_str().unwrap());
        paths_map.set(i as u32, path_cfg, 0)?;
    }

    // Load the programs
    let btf = Btf::from_sys_fs()?;
    let program: &mut Lsm = bpf.program_mut("trace_file_open").unwrap().try_into()?;
    program.load("file_open", &btf)?;
    program.attach()?;

    // Create the gRPC client
    let mut client = if let Some(url) = config.url.as_ref() {
        Some(Client::start(url, config.certs)?)
    } else {
        None
    };

    // Gather events from the ring buffer and print them out.
    tokio::spawn(async move {
        loop {
            let mut guard = async_fd.readable_mut().await.unwrap();
            let ringbuf = guard.get_inner_mut();
            while let Some(event) = ringbuf.next() {
                let event: &event_t = unsafe { &*(event.as_ptr() as *const _) };
                let event: Event = event.try_into().unwrap();

                println!("{event:?}");
                if let Some(client) = client.as_mut() {
                    let _ = client.send(event).await;
                }
            }
            guard.clear_ready();
            yield_now().await;
        }
    });

    let ctrl_c = signal::ctrl_c();
    info!("Waiting for Ctrl-C...");
    ctrl_c.await?;
    info!("Exiting...");

    Ok(())
}

async fn run_vsock_listener(config: FactConfig) -> anyhow::Result<()> {
    use sensor_relay::SensorRelay;
    use vm_watcher::VmWatcher;
    use vsock::{VsockServer, VmMessage};
    use certs::Certs;
    
    info!("Starting VSOCK listener mode on port {}", config.vsock_port);
    
    // Set up shutdown signal
    let (shutdown_tx, _) = tokio::sync::broadcast::channel(1);
    let shutdown_rx1 = shutdown_tx.subscribe();
    let shutdown_rx2 = shutdown_tx.subscribe();
    let shutdown_rx3 = shutdown_tx.subscribe();
    
    // Set up signal handling
    tokio::spawn({
        let shutdown_tx = shutdown_tx.clone();
        async move {
            let _ = signal::ctrl_c().await;
            info!("Received shutdown signal");
            let _ = shutdown_tx.send(());
        }
    });
    
    // Load certificates if provided
    let certs = if let Some(cert_path) = &config.certs {
        Some(Certs::try_from(cert_path.clone())?)
    } else {
        None
    };
    
    // Start VM watcher
    let (mut vm_watcher, _vm_rx) = VmWatcher::new();
    tokio::spawn(async move {
        if let Err(e) = vm_watcher.start(shutdown_rx1).await {
            warn!("VM watcher error: {}", e);
        }
    });
    
    // Create VSOCK server
    let vsock_server = VsockServer::bind(config.vsock_port)?;
    
    // Start sensor relay
    let mut sensor_relay = SensorRelay::new(config.sensor_endpoint.clone(), certs);
    let (vm_msg_tx, vm_msg_rx) = tokio::sync::mpsc::channel::<VmMessage>(100);
    
    tokio::spawn(async move {
        if let Err(e) = sensor_relay.start(vm_msg_rx, shutdown_rx2).await {
            warn!("Sensor relay error: {}", e);
        }
    });
    
    // Start VSOCK server
    tokio::spawn(async move {
        if let Err(e) = vsock_server.serve(shutdown_rx3).await {
            warn!("VSOCK server error: {}", e);
        }
    });
    
    // Wait for shutdown
    let _ = shutdown_tx.subscribe().recv().await;
    info!("VSOCK listener shutting down");
    
    Ok(())
}

async fn run_hybrid_mode(config: FactConfig) -> anyhow::Result<()> {
    use sensor_relay::SensorRelay;
    use vm_watcher::VmWatcher;
    use vsock::{VsockServer, VmMessage};
    use certs::Certs;
    
    info!("Starting hybrid mode (VM agent + VSOCK listener)");
    
    // Set up shutdown signal
    let (shutdown_tx, _) = tokio::sync::broadcast::channel(1);
    let shutdown_rx1 = shutdown_tx.subscribe();
    let shutdown_rx2 = shutdown_tx.subscribe();
    let shutdown_rx3 = shutdown_tx.subscribe();
    let shutdown_rx4 = shutdown_tx.subscribe();
    
    // Set up signal handling
    tokio::spawn({
        let shutdown_tx = shutdown_tx.clone();
        async move {
            let _ = signal::ctrl_c().await;
            info!("Received shutdown signal");
            let _ = shutdown_tx.send(());
        }
    });
    
    // Load certificates if provided
    let certs = if let Some(cert_path) = &config.certs {
        Some(Certs::try_from(cert_path.clone())?)
    } else {
        None
    };
    
    // Start VM agent if enabled
    if config.enable_vm_agent {
        info!("Starting VM agent functionality");
        let vm_config = config.clone();
        tokio::spawn(async move {
            if let Err(e) = vm_agent::run_vm_agent(&vm_config).await {
                warn!("VM agent error: {}", e);
            }
        });
    }
    
    // Start VSOCK listener if enabled
    if config.enable_vsock_server {
        info!("Starting VSOCK listener functionality on port {}", config.vsock_port);
        
        // Start VM watcher
        let (mut vm_watcher, _vm_rx) = VmWatcher::new();
        tokio::spawn(async move {
            if let Err(e) = vm_watcher.start(shutdown_rx1).await {
                warn!("VM watcher error: {}", e);
            }
        });
        
        // Create VSOCK server
        let vsock_server = VsockServer::bind(config.vsock_port)?;
        
        // Start sensor relay
        let mut sensor_relay = SensorRelay::new(config.sensor_endpoint.clone(), certs);
        let (vm_msg_tx, vm_msg_rx) = tokio::sync::mpsc::channel::<VmMessage>(100);
        
        tokio::spawn(async move {
            if let Err(e) = sensor_relay.start(vm_msg_rx, shutdown_rx2).await {
                warn!("Sensor relay error: {}", e);
            }
        });
        
        // Start VSOCK server
        tokio::spawn(async move {
            if let Err(e) = vsock_server.serve(shutdown_rx3).await {
                warn!("VSOCK server error: {}", e);
            }
        });
    }
    
    // If neither mode is explicitly enabled, enable both by default
    if !config.enable_vm_agent && !config.enable_vsock_server {
        info!("No specific mode enabled, starting both VM agent and VSOCK listener");
        
        // Start VM agent
        let vm_config = config.clone();
        tokio::spawn(async move {
            if let Err(e) = vm_agent::run_vm_agent(&vm_config).await {
                warn!("VM agent error: {}", e);
            }
        });
        
        // Start VM watcher
        let (mut vm_watcher, _vm_rx) = VmWatcher::new();
        tokio::spawn(async move {
            if let Err(e) = vm_watcher.start(shutdown_rx4).await {
                warn!("VM watcher error: {}", e);
            }
        });
        
        // Create VSOCK server
        let vsock_server = VsockServer::bind(config.vsock_port)?;
        
        // Start sensor relay
        let certs = if let Some(cert_path) = &config.certs {
            Some(Certs::try_from(cert_path.clone())?)
        } else {
            None
        };
        let mut sensor_relay = SensorRelay::new(config.sensor_endpoint.clone(), certs);
        let (vm_msg_tx, vm_msg_rx) = tokio::sync::mpsc::channel::<VmMessage>(100);
        
        tokio::spawn(async move {
            if let Err(e) = sensor_relay.start(vm_msg_rx, shutdown_tx.subscribe()).await {
                warn!("Sensor relay error: {}", e);
            }
        });
        
        // Start VSOCK server
        tokio::spawn(async move {
            if let Err(e) = vsock_server.serve(shutdown_tx.subscribe()).await {
                warn!("VSOCK server error: {}", e);
            }
        });
    }
    
    // Wait for shutdown
    let _ = shutdown_tx.subscribe().recv().await;
    info!("Hybrid mode shutting down");
    
    Ok(())
}

# VSOCK Listener Integration into Fact Binary

## Overview

This document describes the integration of the standalone `vsock-listener` functionality into the existing `fact` Rust binary. This consolidation reduces the collector daemonset container count and creates a unified VM data collection service.

## Background

### Problem Statement

The StackRox collector daemonset was growing in complexity with multiple containers:
- **collector**: Runtime data collection (C++)
- **compliance**: Node compliance scanning (Go)  
- **vsock-listener**: VM VSOCK server for virtual machine connections (Go)
- **fact**: VM package scanning and file monitoring (Rust)

This resulted in 4 containers per node, increasing resource overhead and operational complexity.

### Solution Approach

Rather than combining `vsock-listener` with the existing `compliance` container (Go), we identified that integrating it with the `fact` binary (Rust) was architecturally superior because:

1. **VM-centric alignment**: Both `fact` and `vsock-listener` serve virtual machine use cases
2. **Shared VSOCK infrastructure**: `fact` already had VSOCK client capabilities  
3. **Modern runtime**: Rust's async capabilities and memory efficiency
4. **Future roadmap**: Natural convergence point for VM data collection features

## Architecture

### High-Level Design

```
┌─────────────────────────────────────────────────────────────┐
│                    Fact Binary (Rust)                      │
├─────────────────────────────────────────────────────────────┤
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐        │
│  │ File Monitor│  │  VM Agent   │  │VSOCK Listener│        │
│  │   (eBPF)    │  │(RPM Scanner)│  │  (Server)   │        │
│  └─────────────┘  └─────────────┘  └─────────────┘        │
├─────────────────────────────────────────────────────────────┤
│                 Shared Infrastructure                       │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐        │
│  │ VM Watcher  │  │Sensor Relay │  │ VSOCK Client│        │
│  │(Kubernetes) │  │   (gRPC)    │  │             │        │
│  └─────────────┘  └─────────────┘  └─────────────┘        │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
                    ┌─────────────────┐
                    │      Sensor     │
                    │    (gRPC/TLS)   │
                    └─────────────────┘
```

### Component Responsibilities

1. **VSOCK Server**: Listens for VM connections on port 818
2. **VM Watcher**: Monitors Kubernetes for VirtualMachine resources
3. **Sensor Relay**: Forwards VM data to sensor via gRPC with mTLS
4. **VM Agent**: Scans VM packages (existing functionality)
5. **File Monitor**: eBPF-based file monitoring (existing functionality)

## Implementation Details

### 1. Configuration Extensions

**File**: `src/config.rs`

Added new agent modes and configuration options:

```rust
#[derive(Debug, Clone, ValueEnum)]
pub enum AgentMode {
    FileMonitor,           // Existing: eBPF file monitoring
    VmAgent,              // Existing: VM package scanning  
    VsockListener,        // New: VSOCK server only
    Hybrid,               // New: Combined VM agent + VSOCK listener
}

// New configuration fields
pub struct FactConfig {
    // ... existing fields ...
    
    /// VSOCK port to listen on
    pub vsock_port: u32,
    
    /// Sensor endpoint for relaying VM data
    pub sensor_endpoint: String,
    
    /// Enable VSOCK server functionality (hybrid mode)
    pub enable_vsock_server: bool,
    
    /// Enable VM agent functionality (hybrid mode)
    pub enable_vm_agent: bool,
}
```

### 2. VSOCK Server Implementation

**File**: `src/vsock.rs`

Extended existing VSOCK client with server capabilities:

```rust
pub struct VsockServer {
    port: u32,
    listener_fd: OwnedFd,
}

impl VsockServer {
    pub fn bind(port: u32) -> Result<Self>
    pub async fn serve(&self, shutdown: Receiver<()>) -> Result<()>
    async fn accept_connection(&self, vm_tx: Sender<VmMessage>) -> Result<()>
    async fn handle_client(client_fd: OwnedFd, vm_id: String, vm_tx: Sender<VmMessage>) -> Result<()>
}
```

**Key Features**:
- Binds to VMADDR_CID_ANY on specified port
- Accepts multiple concurrent VM connections
- Implements the same protocol as original vsock-listener:
  - 4-byte length header
  - Variable-length protobuf data
  - 4-byte acknowledgment response
- Async I/O with proper error handling and graceful shutdown

### 3. VM Watcher

**File**: `src/vm_watcher.rs`

Kubernetes integration for VM resource discovery:

```rust
pub struct VmWatcher {
    vms: HashMap<String, VirtualMachine>,
    vm_tx: mpsc::Sender<VirtualMachine>,
}

pub struct VirtualMachine {
    pub name: String,
    pub namespace: String, 
    pub uid: String,
    pub cid: Option<u32>, // VSOCK Context ID
}
```

**Capabilities**:
- Monitors VirtualMachine CRDs (KubeVirt, etc.)
- Extracts VSOCK context IDs from VM specs/annotations
- Handles VM lifecycle events (add/update/delete)
- Provides VM registry for VSOCK connection mapping

### 4. Sensor Relay

**File**: `src/sensor_relay.rs`

gRPC client for forwarding VM data to sensor:

```rust
pub struct SensorRelay {
    endpoint: String,
    certs: Option<Certs>,
    client: Option<VirtualMachineServiceClient<InterceptedService<Channel, UserAgentInterceptor>>>,
}
```

**Features**:
- mTLS connection to sensor using StackRox certificates
- Automatic reconnection on failures
- Protobuf message forwarding
- User-Agent header injection for identification

### 5. Mode Orchestration  

**File**: `src/lib.rs`

Added new run modes with proper service orchestration:

```rust
pub async fn run(config: FactConfig) -> anyhow::Result<()> {
    match config.mode {
        AgentMode::FileMonitor => run_file_monitor(config).await,
        AgentMode::VmAgent => vm_agent::run_vm_agent(&config).await,
        AgentMode::VsockListener => run_vsock_listener(config).await,  // New
        AgentMode::Hybrid => run_hybrid_mode(config).await,            // New
    }
}
```

**Service Coordination**:
- Shared shutdown signaling across all services
- Concurrent service execution with tokio::spawn
- Proper error handling and graceful degradation
- Resource sharing (certificates, configuration)

## Usage

### Command Line Examples

```bash
# VSOCK listener mode (replaces standalone vsock-listener)
fact --mode vsock-listener --vsock-port 818 --sensor-endpoint sensor:443 --certs /var/run/secrets/stackrox.io/certs

# Hybrid mode with both functionalities
fact --mode hybrid --enable-vsock-server --enable-vm-agent --vsock-port 818 --sensor-endpoint sensor:443

# VM agent with VSOCK communication to host
fact --mode vm-agent --use-vsock --interval 3600

# Traditional file monitor mode (unchanged)
fact --mode file-monitor --paths /etc:/var/log
```

### Environment Variables

```bash
FACT_MODE=hybrid
FACT_VSOCK_PORT=818
FACT_SENSOR_ENDPOINT=sensor:443
FACT_ENABLE_VSOCK_SERVER=true
FACT_ENABLE_VM_AGENT=true
FACT_CERTS=/var/run/secrets/stackrox.io/certs
```

## Deployment Integration

### Collector DaemonSet Changes

**Before** (4 containers):
```yaml
containers:
- name: collector
- name: compliance  
- name: vsock-listener
- name: fact
```

**After** (3 containers):
```yaml
containers:
- name: collector
- name: compliance
- name: fact  # Now includes vsock-listener functionality
```

### Container Configuration

```yaml
- name: fact
  image: stackrox/fact:latest
  command: ["/usr/local/bin/fact"]
  args: 
    - "--mode=hybrid"
    - "--enable-vsock-server"
    - "--enable-vm-agent"
  env:
    - name: FACT_SENSOR_ENDPOINT
      value: "sensor:443"
    - name: FACT_VSOCK_PORT  
      value: "818"
    - name: FACT_CERTS
      value: "/run/secrets/stackrox.io/certs"
  securityContext:
    privileged: true
    readOnlyRootFilesystem: true
  volumeMounts:
    - name: vhost-vsock
      mountPath: /dev/vhost-vsock
    - name: certs
      mountPath: /run/secrets/stackrox.io/certs
      readOnly: true
```

## Benefits

### Resource Efficiency
- **Memory**: Shared Rust runtime vs separate Go processes
- **CPU**: Single process vs multiple processes with IPC overhead  
- **Network**: Shared gRPC connections and certificate handling
- **Storage**: Single binary vs multiple container images

### Operational Simplicity
- **Fewer containers**: 25% reduction in container count per node
- **Unified configuration**: Single binary with consistent CLI/env vars
- **Simplified monitoring**: One process to monitor vs multiple
- **Easier debugging**: Single log stream with unified context

### Architectural Alignment
- **VM-centric design**: All VM functionality in one place
- **Shared infrastructure**: VSOCK, certificates, sensor connections
- **Future extensibility**: Natural place for new VM features
- **Technology consistency**: Modern async Rust throughout

## Protocol Compatibility

The implementation maintains full compatibility with the original vsock-listener protocol:

### Message Format
```
┌─────────────┬─────────────────────────┬─────────────┐
│   Header    │      Protobuf Data      │     ACK     │
│  (4 bytes)  │    (variable length)    │  (4 bytes)  │
├─────────────┼─────────────────────────┼─────────────┤
│ Data Length │ VirtualMachine Message  │ Status Code │
│ (LE u32)    │     (Protobuf)          │ (LE u32)    │
└─────────────┴─────────────────────────┴─────────────┘
```

### Port Usage
- **VSOCK Port 818**: Same as original vsock-listener
- **Sensor gRPC**: Uses existing sensor service endpoints
- **mTLS**: Compatible with existing StackRox certificate infrastructure

## Testing

### Unit Testing
```bash
cd fact/
cargo test
```

### Integration Testing
```bash
# Start in hybrid mode
cargo run -- --mode hybrid --enable-vsock-server --enable-vm-agent

# Test VSOCK connectivity from VM
echo "test data" | nc-vsock 2 818

# Verify sensor relay
curl -k https://sensor:443/v1/virtualmachines
```

### Development Environment
```bash
# Install dependencies
sudo dnf install -y clang libbpf-devel protobuf-compiler protobuf-devel rustup
rustup toolchain install stable

# Build with eBPF support
cargo build --release

# Run with sudo for VSOCK access
sudo -E cargo run --release --config 'target."cfg(all())".runner="sudo -E"'
```

## Future Enhancements

### Planned Features
1. **Kubernetes Integration**: Native kube-rs client for VM watching
2. **Metrics**: Prometheus metrics for VSOCK connections and throughput
3. **Health Checks**: Readiness/liveness endpoints for Kubernetes
4. **Configuration Reload**: Dynamic configuration updates without restart

### Performance Optimizations
1. **Zero-copy VSOCK**: Direct memory mapping for large payloads
2. **Connection pooling**: Reuse gRPC connections across VM sessions
3. **Batched forwarding**: Aggregate small messages for efficiency
4. **Adaptive buffering**: Dynamic buffer sizing based on load

### Security Enhancements
1. **VM authentication**: Verify VM identity before accepting connections
2. **Rate limiting**: Protect against DoS attacks from VMs
3. **Audit logging**: Comprehensive logging of VM interactions
4. **Secure defaults**: Hardened configuration options

## Migration Guide

### From Standalone vsock-listener

1. **Remove vsock-listener container** from collector daemonset
2. **Update fact container** configuration to use hybrid mode
3. **Verify VSOCK device** mounting (/dev/vhost-vsock)
4. **Check certificate paths** match between containers
5. **Test VM connectivity** after deployment

### Configuration Mapping

| vsock-listener | fact equivalent |
|---|---|
| `VSOCK_PORT=818` | `FACT_VSOCK_PORT=818` |
| `SENSOR_ENDPOINT=sensor:443` | `FACT_SENSOR_ENDPOINT=sensor:443` |
| `/run/secrets/stackrox.io/certs` | `FACT_CERTS=/run/secrets/stackrox.io/certs` |

## Troubleshooting

### Common Issues

**VSOCK Permission Denied**
```bash
# Ensure privileged security context
securityContext:
  privileged: true
```

**Certificate Errors**
```bash
# Verify certificate mount and permissions
ls -la /run/secrets/stackrox.io/certs/
```

**Port Conflicts**
```bash
# Check VSOCK port availability
ss -l | grep vsock
```

### Debugging Commands

```bash
# Check fact logs
kubectl logs -l app=collector -c fact

# Test VSOCK from VM
nc-vsock -l 818  # Listen mode
nc-vsock 2 818   # Connect to host

# Verify sensor connectivity  
curl -k --cert client.pem --key client-key.pem https://sensor:443/v1/ping
```

## Conclusion

The integration of vsock-listener functionality into the fact binary represents a significant architectural improvement for StackRox's VM security capabilities. By consolidating VM-related functionality into a single, efficient Rust binary, we achieve better resource utilization, simplified operations, and a foundation for future VM security enhancements.

The implementation maintains full backward compatibility while providing new capabilities for flexible deployment scenarios. The hybrid mode allows for gradual migration and testing, while the dedicated VSOCK listener mode provides a direct replacement for the original standalone service.
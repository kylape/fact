# FACT Project Documentation

## Overview
FACT (File Activity Collection Tool) is a Rust-based security monitoring system that tracks file system activity and virtual machine information. It consists of multiple components working together to collect, process, and relay security data.

## Architecture

### Core Components
- **fact-ebpf/**: eBPF programs for kernel-level file system monitoring
- **fact-api/**: Protobuf API definitions and generated code
- **fact/**: Main application with multiple operational modes
- **third_party/stackrox/**: StackRox protocol buffer definitions

### Operational Modes
1. **FileMonitor**: Monitors file system activity using eBPF
2. **VmAgent**: Collects virtual machine package information via RPM queries
3. **VsockListener**: Listens for VM communications over VSOCK
4. **Hybrid**: Combines VM agent and VSOCK listener functionality

## Build Requirements

### System Dependencies
- **Rust toolchain** (specified in rust-toolchain.toml)
- **protoc** (Protocol Buffer compiler) with system includes
- **clang** (for eBPF compilation, optional - will skip if not available)

### Protobuf Setup
The project requires protoc with standard Google protobuf includes installed at `/usr/local/include/google/protobuf/`. The build system uses:
- System protobuf includes (NOT bundled in repo)
- Custom StackRox protobuf definitions from `third_party/stackrox/proto/`

### Key Build Commands
```bash
# Standard build
cargo build

# Run specific mode
cargo run -- --mode file-monitor
cargo run -- --mode vm-agent --interval 300
cargo run -- --mode vsock-listener --vsock-port 1024
cargo run -- --mode hybrid
```

## Development Notes

### Recent Major Changes
1. **Crossbeam → Tokio Migration**: Replaced crossbeam channels/select with tokio equivalents for async compatibility
2. **Protobuf Integration**: Added comprehensive StackRox protobuf support for FileActivity, ProcessSignal, and VM data
3. **Build System Fixes**: Resolved numerous compilation issues including:
   - Missing protobuf includes and dependencies
   - eBPF bindings generation (with fallback stubs when clang unavailable)
   - Nix crate API compatibility updates
   - CStr pointer type mismatches

### Code Structure
- **VM Agent** (`vm_agent.rs`): Collects RPM package data, converts to protobuf format
- **VSOCK Communication** (`vsock.rs`): Handles VM-to-host communication
- **Sensor Relay** (`sensor_relay.rs`): Forwards data to StackRox sensor
- **Event Processing** (`event.rs`): Converts eBPF events to protobuf messages
- **BPF Integration** (`bpf.rs`): eBPF program loading and data structures

### Testing & Debugging
```bash
# Build with warnings (useful for development)
cargo build 2>&1 | head -50

# Check specific functionality
cargo check --bin fact

# Run with logging
RUST_LOG=debug cargo run -- --mode vm-agent
```

### Common Issues & Solutions

#### Build Failures
- **"protoc failed: google/protobuf/timestamp.proto: File not found"**
  → Install protoc with includes: system protobuf development packages required
  
- **"Clang not found, skipping eBPF compilation"** 
  → Install clang for full eBPF functionality, or continue with stubs for development

- **Send trait errors with crossbeam**
  → Already fixed - project now uses tokio primitives throughout

#### Runtime Issues
- **VSOCK connection failures**: Ensure VM has `autoattachVSOCK: true` configured
- **Permission denied for eBPF**: Run with appropriate capabilities or as root
- **RPM query failures**: Ensure rpm is available and functional in the environment

## Configuration

### Environment Variables
- `FACT_HOST_MOUNT`: Path to host filesystem mount point (for containerized deployments)

### Command Line Options
- `--mode`: Operation mode (file-monitor, vm-agent, vsock-listener, hybrid)
- `--interval`: Collection interval in seconds (vm-agent mode)
- `--vsock-port`: VSOCK port for communication (default varies by mode)
- `--sensor-endpoint`: StackRox sensor gRPC endpoint
- `--certs`: Path to TLS certificates directory

## Integration

### StackRox Integration
The project integrates with StackRox for security data collection:
- Uses StackRox protobuf definitions from `third_party/stackrox/`
- Sends FileActivity and ProcessSignal data to sensor endpoints
- Supports both gRPC and VSOCK communication methods

### Protobuf APIs
Key message types:
- `FileActivity`: File system events with process context
- `ProcessSignal`: Process lifecycle and metadata
- `VirtualMachine`: VM inventory and package information
- `EmbeddedImageScanComponent`: Package/component vulnerability data

## Maintenance

### Dependency Updates
- **Tokio**: Async runtime - keep updated for performance and security
- **Tonic/Prost**: gRPC/protobuf stack - coordinate with StackRox proto versions
- **Nix**: System interface - may require API compatibility updates
- **Aya**: eBPF framework - updates may require BPF program changes

### Ignored Files
```gitignore
# Build artifacts
target/
debug/

# Temporary protoc downloads
include/
protoc-*.zip
readme.txt

# Python development
__pycache__/
.venv/
```

### Code Quality
- Run `cargo clippy` for linting
- Use `cargo fmt` for consistent formatting  
- Monitor `cargo audit` for security advisories
- Address compiler warnings promptly (currently building with warnings only)

## Troubleshooting

### Build Environment
1. Ensure protoc is properly installed with includes
2. Check Rust toolchain matches rust-toolchain.toml
3. Verify all git submodules are properly initialized
4. For eBPF development, ensure clang and kernel headers are available

### Runtime Debugging
1. Enable debug logging: `RUST_LOG=debug`
2. Check eBPF program loading in dmesg
3. Verify VSOCK connectivity with VM guests
4. Monitor gRPC connection status to StackRox sensor

This project represents a comprehensive security monitoring solution with complex build requirements but robust functionality once properly configured.
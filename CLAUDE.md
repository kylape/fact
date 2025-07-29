# fact - File and Container Tracking System

## Project Overview

`fact` is a modular security monitoring tool written in Rust that supports multiple monitoring capabilities:

1. **File Monitor**: Uses eBPF to monitor file access patterns (requires root)
2. **Package Monitor**: Scans RPM packages and reports vulnerabilities periodically

The project uses a plugin-style monitor architecture where multiple monitors can run simultaneously, each contributing events to a unified processing system. Communication with upstream services is handled via gRPC or VSOCK.

## Architecture

### Core Components
- **fact/** - Main binary with modular monitor system
- **fact-api/** - gRPC API definitions and client libraries  
- **fact-ebpf/** - eBPF C programs for system monitoring
- **mock-server/** - Python test server for development

### Monitor System
- **monitor.rs** - Core Monitor trait and plugin registry
- **monitors/file_monitor.rs** - eBPF-based file access monitoring
- **monitors/package_monitor.rs** - RPM package scanning
- **monitors/mod.rs** - Monitor module exports

New monitors can be added by implementing the `Monitor` trait and registering them in the main system.

## Development Environment

### Prerequisites
- Rust stable toolchain
- clang and libbpf-devel (for eBPF compilation)
- protobuf-compiler and protobuf-devel (for gRPC)

### Build Commands
```bash
# Standard build
cargo build

# Release build with sudo privileges (required for eBPF)
cargo run --release --config 'target."cfg(all())".runner="sudo -E"'
```

### Testing
```bash
# Run mock server for testing
make mock-server

# Test file monitoring
sudo -E cargo run -- --paths /tmp:/var/log
```

## Code Conventions

### Rust Style
- Uses rustfmt with custom configuration:
  - `group_imports = "StdExternalCrate"`
  - `imports_granularity = "Crate"`
  - `reorder_imports = true`
- Follow standard Rust naming conventions
- Use `anyhow::Result<()>` for error handling
- Prefer `tokio` async/await patterns

### Project Structure
- eBPF code in C (fact-ebpf/)
- Rust bindings generated via build.rs
- Configuration via clap with environment variable support
- Logging via env_logger with FACT_LOGLEVEL

### Key Dependencies
- **aya**: eBPF framework for Rust
- **tokio**: Async runtime
- **tonic**: gRPC client/server
- **clap**: CLI argument parsing
- **prost**: Protocol buffer support

## Configuration

### Environment Variables
- `FACT_ENABLE_FILE_MONITOR`: Enable file monitoring (boolean)
- `FACT_ENABLE_PACKAGE_MONITOR`: Enable package monitoring (boolean)
- `FACT_URL`: Upstream service URL
- `FACT_CERTS`: mTLS certificate directory
- `FACT_LOGLEVEL`: Log level (default: info)
- `FACT_SKIP_HTTP`: Skip HTTP communication
- `FACT_USE_VSOCK`: Use VSOCK communication
- `FACT_RPMDB`: RPM database path (default: /var/lib/rpm)
- `FACT_INTERVAL`: Package scan interval in seconds (default: 3600)

### Usage Examples
```bash
# Enable file monitoring only (requires root for eBPF)
sudo -E cargo run -- --enable-file-monitor --paths /etc:/var/log --url https://api.example.com

# Enable package monitoring only
cargo run -- --enable-package-monitor --interval 1800 --use-vsock

# Enable both monitors simultaneously
sudo -E cargo run -- --enable-file-monitor --enable-package-monitor --paths /tmp --url https://api.example.com

# Default behavior with paths enables file monitor, without paths enables package monitor
sudo -E cargo run -- --paths /etc:/var/log  # Enables file monitoring
cargo run --                                 # Enables package monitoring
```

## Security Considerations

This is a security monitoring tool that:
- Requires root privileges for eBPF operations
- Handles sensitive file system events
- Communicates over mTLS with upstream services
- Scans package databases for vulnerabilities

Always review changes carefully, especially in eBPF code and certificate handling.

## Development Notes

- eBPF programs are compiled at build time via build.rs
- The project uses a workspace structure with multiple crates
- Build artifacts include generated Rust bindings from C headers
- Mock server is provided for local testing without real infrastructure
- Monitor system uses async trait objects and tokio channels for event processing

## Adding New Monitors

To add a new monitor:

1. Create a new file in `src/monitors/` (e.g., `network_monitor.rs`)
2. Implement the `Monitor` trait with required methods:
   - `name()`: Unique identifier
   - `description()`: Human-readable description  
   - `can_run()`: Check if monitor can run on current system
   - `start()`: Begin monitoring and send events via channel
   - `stop()`: Clean shutdown
   - `is_running()`: Current status
3. Register the monitor in `lib.rs` within the `run()` function
4. Add any required configuration to `FactConfig`
5. Update CLAUDE.md documentation

Example monitor template available in existing monitors.

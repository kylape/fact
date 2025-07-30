use std::os::unix::io::{AsRawFd, RawFd, OwnedFd, FromRawFd};
use std::io;
use anyhow::{Context, Result};
use log::{debug, info, warn};
use nix::sys::socket::{
    accept, bind, connect, listen, socket, AddressFamily, SockFlag, SockType, VsockAddr, Backlog,
};
use tokio::sync::mpsc;

const VMADDR_CID_HOST: u32 = 2; // Host context ID
const VMADDR_CID_ANY: u32 = 0xFFFFFFFF; // Any context ID (for server binding)

#[derive(Debug)]
pub struct VmMessage {
    pub vm_id: String,
    pub data: Vec<u8>,
}

pub struct VsockClient {
    fd: OwnedFd,
}

impl VsockClient {
    /// Create a new VSOCK client connection to the host
    pub fn connect() -> Result<Self> {
        Self::connect_to_port(818)
    }

    /// Create a new VSOCK client connection to a specific port
    pub fn connect_to_port(port: u32) -> Result<Self> {
        info!("Connecting to host via VSOCK on port {}", port);
        
        // Create VSOCK socket
        let fd = socket(
            AddressFamily::Vsock,
            SockType::Stream,
            SockFlag::empty(),
            None,
        )
        .context("Failed to create VSOCK socket")?;
        
        // Connect to host
        let addr = VsockAddr::new(VMADDR_CID_HOST, port);
        connect(fd.as_raw_fd(), &addr).context("Failed to connect to VSOCK host")?;
        
        info!("Successfully connected to host via VSOCK");
        Ok(VsockClient { fd })
    }
    
    /// Send data with protocol header
    pub fn send_data(&mut self, data: &[u8]) -> Result<()> {
        debug!("Sending VSOCK message: len={}", data.len());
        
        // Create message header (4 bytes: length only)
        let header = (data.len() as u32).to_le_bytes();
        
        // Send header
        self.write_all(&header)
            .context("Failed to send message header")?;
        
        // Send data
        self.write_all(data)
            .context("Failed to send message data")?;
        
        // Read acknowledgment (4 bytes)
        let mut ack = [0u8; 4];
        self.read_exact(&mut ack)
            .context("Failed to read acknowledgment")?;
        
        let ack_code = u32::from_le_bytes(ack);
        if ack_code != 0 {
            return Err(anyhow::anyhow!("Server returned error code: {}", ack_code));
        }
        
        debug!("Message sent successfully and acknowledged");
        Ok(())
    }
    
    /// Write all bytes to the socket
    fn write_all(&mut self, mut buf: &[u8]) -> io::Result<()> {
        while !buf.is_empty() {
            match nix::unistd::write(&self.fd, buf) {
                Ok(0) => return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "failed to write whole buffer"
                )),
                Ok(n) => buf = &buf[n..],
                Err(nix::errno::Errno::EINTR) => {}
                Err(e) => return Err(io::Error::from(e)),
            }
        }
        Ok(())
    }
    
    /// Read exact number of bytes from the socket
    fn read_exact(&mut self, mut buf: &mut [u8]) -> io::Result<()> {
        while !buf.is_empty() {
            match nix::unistd::read(self.fd.as_raw_fd(), buf) {
                Ok(0) => return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "failed to fill whole buffer"
                )),
                Ok(n) => {
                    let tmp = buf;
                    buf = &mut tmp[n..];
                }
                Err(nix::errno::Errno::EINTR) => {}
                Err(e) => return Err(io::Error::from(e)),
            }
        }
        Ok(())
    }
    
    /// Check if VSOCK is available on this system
    pub fn is_available() -> bool {
        // Try to create a VSOCK socket to test availability
        match socket(
            AddressFamily::Vsock,
            SockType::Stream,
            SockFlag::empty(),
            None,
        ) {
            Ok(_fd) => {
                // fd will be automatically closed when dropped
                true
            }
            Err(_) => false,
        }
    }
}

impl Drop for VsockClient {
    fn drop(&mut self) {
        // OwnedFd automatically closes the file descriptor when dropped
    }
}

impl AsRawFd for VsockClient {
    fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}

pub struct VsockServer {
    port: u32,
    listener_fd: OwnedFd,
}

impl VsockServer {
    /// Create a new VSOCK server listening on the specified port
    pub fn bind(port: u32) -> Result<Self> {
        info!("Creating VSOCK server on port {}", port);
        
        // Create VSOCK socket
        let fd = socket(
            AddressFamily::Vsock,
            SockType::Stream,
            SockFlag::empty(),
            None,
        )
        .context("Failed to create VSOCK server socket")?;
        
        // Bind to any context ID on the specified port
        let addr = VsockAddr::new(VMADDR_CID_ANY, port);
        bind(fd.as_raw_fd(), &addr).context("Failed to bind VSOCK server")?;
        
        // Start listening for connections
        listen(&fd, Backlog::new(128).unwrap()).context("Failed to listen on VSOCK socket")?;
        
        info!("VSOCK server listening on port {}", port);
        Ok(VsockServer {
            port,
            listener_fd: fd,
        })
    }
    
    /// Accept incoming connections and handle them
    pub async fn serve(&self, mut shutdown: tokio::sync::broadcast::Receiver<()>) -> Result<()> {
        let (vm_tx, mut vm_rx) = mpsc::channel::<VmMessage>(100);
        
        // Spawn task to handle VM messages
        let sensor_tx = vm_tx.clone();
        tokio::spawn(async move {
            while let Some(msg) = vm_rx.recv().await {
                debug!("Received VM message from {}: {} bytes", msg.vm_id, msg.data.len());
                // TODO: Forward to sensor relay
            }
        });
        
        // Main server loop
        loop {
            tokio::select! {
                _ = shutdown.recv() => {
                    info!("VSOCK server shutting down");
                    break;
                }
                _ = self.accept_connection(sensor_tx.clone()) => {
                    // Connection handled
                }
            }
        }
        
        Ok(())
    }
    
    /// Accept a single connection and handle it
    async fn accept_connection(&self, vm_tx: mpsc::Sender<VmMessage>) -> Result<()> {
        // Accept connection (blocking, but we're in a select loop)
        let client_fd = match accept(self.listener_fd.as_raw_fd()) {
            Ok(fd) => fd,
            Err(e) => {
                warn!("Failed to accept VSOCK connection: {}", e);
                return Ok(());
            }
        };
        
        let client_fd = unsafe { OwnedFd::from_raw_fd(client_fd) };
        let vm_id = format!("vm-{}", client_fd.as_raw_fd());
        
        info!("Accepted VSOCK connection from {}", vm_id);
        
        // Spawn task to handle this client
        tokio::spawn(async move {
            if let Err(e) = Self::handle_client(client_fd, vm_id.clone(), vm_tx).await {
                warn!("Error handling client {}: {}", vm_id, e);
            }
        });
        
        Ok(())
    }
    
    /// Handle a single client connection
    async fn handle_client(
        mut client_fd: OwnedFd,
        vm_id: String,
        vm_tx: mpsc::Sender<VmMessage>,
    ) -> Result<()> {
        let mut buffer = vec![0u8; 4096];
        
        loop {
            // Read message header (4 bytes: length)
            let mut header = [0u8; 4];
            match Self::read_exact_fd(&client_fd, &mut header).await {
                Ok(()) => {},
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                    debug!("Client {} disconnected", vm_id);
                    break;
                }
                Err(e) => {
                    warn!("Error reading header from {}: {}", vm_id, e);
                    break;
                }
            }
            
            let msg_len = u32::from_le_bytes(header) as usize;
            if msg_len > buffer.len() {
                buffer.resize(msg_len, 0);
            }
            
            // Read message data
            match Self::read_exact_fd(&client_fd, &mut buffer[..msg_len]).await {
                Ok(()) => {},
                Err(e) => {
                    warn!("Error reading data from {}: {}", vm_id, e);
                    break;
                }
            }
            
            // Send acknowledgment (0 = success)
            let ack = 0u32.to_le_bytes();
            if let Err(e) = Self::write_all_fd(&client_fd, &ack).await {
                warn!("Error sending ack to {}: {}", vm_id, e);
                break;
            }
            
            // Forward message to sensor relay
            let msg = VmMessage {
                vm_id: vm_id.clone(),
                data: buffer[..msg_len].to_vec(),
            };
            
            if let Err(e) = vm_tx.send(msg).await {
                warn!("Error forwarding message from {}: {}", vm_id, e);
                break;
            }
        }
        
        info!("Client {} connection closed", vm_id);
        Ok(())
    }
    
    /// Read exact number of bytes from file descriptor
    async fn read_exact_fd(fd: &OwnedFd, mut buf: &mut [u8]) -> io::Result<()> {
        while !buf.is_empty() {
            match nix::unistd::read(fd.as_raw_fd(), buf) {
                Ok(0) => return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "failed to fill whole buffer"
                )),
                Ok(n) => {
                    let tmp = buf;
                    buf = &mut tmp[n..];
                }
                Err(nix::errno::Errno::EINTR) => {}
                Err(nix::errno::Errno::EAGAIN) | Err(nix::errno::Errno::EWOULDBLOCK) => {
                    // Would block, yield and try again
                    tokio::task::yield_now().await;
                }
                Err(e) => return Err(io::Error::from(e)),
            }
        }
        Ok(())
    }
    
    /// Write all bytes to file descriptor
    async fn write_all_fd(fd: &OwnedFd, mut buf: &[u8]) -> io::Result<()> {
        while !buf.is_empty() {
            match nix::unistd::write(fd, buf) {
                Ok(0) => return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "failed to write whole buffer"
                )),
                Ok(n) => buf = &buf[n..],
                Err(nix::errno::Errno::EINTR) => {}
                Err(nix::errno::Errno::EAGAIN) | Err(nix::errno::Errno::EWOULDBLOCK) => {
                    // Would block, yield and try again
                    tokio::task::yield_now().await;
                }
                Err(e) => return Err(io::Error::from(e)),
            }
        }
        Ok(())
    }
}

impl Drop for VsockServer {
    fn drop(&mut self) {
        info!("VSOCK server on port {} shutting down", self.port);
    }
}
pub mod storage {
    tonic::include_proto!("storage");
}

pub mod sensor {
    tonic::include_proto!("sensor");
}

// Expose commonly used types at the root level for backward compatibility
pub use sensor::*;
pub use storage::*;

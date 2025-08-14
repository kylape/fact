pub mod storage {
    tonic::include_proto!("storage");
}

pub mod sensor {
    tonic::include_proto!("sensor");
}

pub mod virtualmachine {
    pub mod v1 {
        tonic::include_proto!("virtualmachine.v1");
    }
}

pub mod scanner {
    pub mod v4 {
        tonic::include_proto!("scanner.v4");
    }
}

// Expose commonly used types at the root level for backward compatibility
pub use sensor::*;
pub use storage::*;

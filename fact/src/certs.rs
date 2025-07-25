use std::{fs::read_to_string, path::PathBuf};

use tonic::transport::{Certificate, Identity};

pub struct Certs {
    pub ca: Certificate,
    pub identity: Identity,
}

impl TryFrom<PathBuf> for Certs {
    type Error = anyhow::Error;

    fn try_from(path: PathBuf) -> Result<Self, Self::Error> {
        let ca = read_to_string(path.join("ca.pem"))?;
        let ca = Certificate::from_pem(ca);
        let cert = read_to_string(path.join("cert.pem"))?;
        let key = read_to_string(path.join("key.pem"))?;
        let identity = Identity::from_pem(cert, key);

        Ok(Self { ca, identity })
    }
}

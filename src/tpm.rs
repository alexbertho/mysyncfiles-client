use serde::{Deserialize, Serialize};
/// Opaque TPM-wrapped private material is unusable on another TPM. It is never
/// an exported software private key. EK certificates are only used at enrollment.
#[derive(Clone, Deserialize, Serialize)]
pub struct Identity {
    pub device: String,
    pub public: String,
    pub private: String,
    pub ek_kind: String,
    #[serde(default = "default_tcti")]
    pub tcti: String,
}

pub fn default_tcti() -> String {
    "device:/dev/tpmrm0".into()
}
impl Identity {
    pub fn fingerprint(&self) -> anyhow::Result<String> {
        crate::auth_protocol::fingerprint(&self.public)
    }
}
#[cfg(feature = "tpm")]
mod hardware;
#[cfg(feature = "tpm")]
pub use hardware::*;
#[cfg(not(feature = "tpm"))]
mod disabled;
#[cfg(not(feature = "tpm"))]
pub use disabled::*;

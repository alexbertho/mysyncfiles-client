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
    let device = resource_manager(std::path::Path::new("/dev"));
    format!("device:{}", device.display())
}

fn resource_manager(directory: &std::path::Path) -> std::path::PathBuf {
    // Prefer tpmrm0 when present; never fall back from a permission error to a
    // different TPM, or silently select a software/simulator transport.
    let mut devices: Vec<_> = std::fs::read_dir(directory)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let number = name.to_str()?.strip_prefix("tpmrm")?.parse::<u32>().ok()?;
            Some((number, entry.path()))
        })
        .collect();
    devices.sort_by_key(|(number, _)| *number);
    devices
        .into_iter()
        .next()
        .map(|(_, path)| path)
        .unwrap_or_else(|| directory.join("tpmrm0"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn detects_resource_managers_without_selecting_raw_or_software_tpms() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        assert_eq!(
            super::resource_manager(dir.path()),
            dir.path().join("tpmrm0")
        );
        for name in ["tpm0", "tpmrm-invalid", "tpmrm10", "tpmrm2"] {
            std::fs::write(dir.path().join(name), b"")?;
        }
        assert_eq!(
            super::resource_manager(dir.path()),
            dir.path().join("tpmrm2")
        );
        std::fs::write(dir.path().join("tpmrm0"), b"")?;
        assert_eq!(
            super::resource_manager(dir.path()),
            dir.path().join("tpmrm0")
        );
        Ok(())
    }
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

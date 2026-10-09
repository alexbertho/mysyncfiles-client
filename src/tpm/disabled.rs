//! Fast builds contain no TPM emulation or alternate authentication path.
use super::Identity;
use crate::auth_protocol::{Claims, EnrollChallenge};
use anyhow::{Result, bail};
use std::path::Path;
fn unavailable<T>() -> Result<T> {
    bail!("TPM support is not compiled; build mysync with --features tpm")
}
pub struct RequestSigner {
    device: String,
}
impl RequestSigner {
    pub fn new(identity: Identity) -> Result<Self> {
        Ok(Self {
            device: identity.device,
        })
    }
    pub fn device(&self) -> &str {
        &self.device
    }
    pub async fn proof(&self, _: Claims) -> Result<String> {
        unavailable()
    }
}
pub fn read_ek_certificate(_: &Path) -> Result<Vec<u8>> {
    unavailable()
}
pub fn certificate_kind(_: &[u8]) -> Result<&'static str> {
    unavailable()
}
pub fn doctor(_: &str) -> Result<&'static str> {
    unavailable()
}
pub fn doctor_with_certificate(_: &str, _: Option<&[u8]>) -> Result<&'static str> {
    unavailable()
}
impl Identity {
    pub fn create(_: &str, _: &str) -> Result<(Self, Vec<u8>)> {
        unavailable()
    }
    pub fn create_with_certificate(_: &str, _: &str, _: Option<&[u8]>) -> Result<(Self, Vec<u8>)> {
        unavailable()
    }
    pub fn ek_intermediates(&self) -> Result<Vec<Vec<u8>>> {
        unavailable()
    }
    pub fn ek_certificate(&self) -> Result<Vec<u8>> {
        unavailable()
    }
    pub fn ek_certificate_from(&self, _: &[u8]) -> Result<Vec<u8>> {
        unavailable()
    }
    pub fn activate(&self, _: &EnrollChallenge) -> Result<String> {
        unavailable()
    }
    pub fn proof(&self, _: Claims) -> Result<String> {
        unavailable()
    }
}

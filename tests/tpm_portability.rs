#![cfg(feature = "tpm")]
//! Isolated simulator coverage. Every mutating command uses this test's TCTI;
//! no command discovers or opens a host TPM.
use anyhow::{Context as _, Result, ensure};
use mysyncfiles_client::{
    auth_protocol::{Claims, Proof},
    tpm::{self, Identity},
};
use openssl::{
    asn1::Asn1Time,
    hash::MessageDigest,
    pkey::PKey,
    x509::{X509, X509NameBuilder},
};
use std::{
    io::Write,
    process::{Child, Command, Stdio},
    time::Duration,
};
use tss_esapi::{
    abstraction::{AsymmetricAlgorithmSelection, ek},
    interface_types::{ecc::EccCurve, key_bits::RsaKeyBits},
    traits::Marshall,
};

struct Simulator {
    child: Child,
    dir: tempfile::TempDir,
    tcti: String,
}
impl Drop for Simulator {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Simulator {
    fn new() -> Result<Self> {
        let dir = tempfile::tempdir()?;
        let (port, first, second) = loop {
            let first = std::net::TcpListener::bind("127.0.0.1:0")?;
            let port = first.local_addr()?.port();
            if port == 65535 {
                continue;
            }
            if let Ok(second) = std::net::TcpListener::bind(("127.0.0.1", port + 1)) {
                break (port, first, second);
            }
        };
        drop((first, second));
        let child = Command::new("swtpm")
            .args(["socket", "--tpm2", "--tpmstate"])
            .arg(format!("dir={}", dir.path().display()))
            .arg("--server")
            // A child from another parallel test may inherit a TCTI socket.
            // Close each completed connection on the simulator side as well.
            .arg(format!(
                "type=tcp,bindaddr=127.0.0.1,port={port},disconnect"
            ))
            .arg("--ctrl")
            .arg(format!("type=tcp,bindaddr=127.0.0.1,port={}", port + 1))
            .args(["--flags", "not-need-init,startup-clear"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let sim = Self {
            child,
            dir,
            tcti: format!("swtpm:host=127.0.0.1,port={port}"),
        };
        for _ in 0..100 {
            if sim.tool("tpm2_getcap", &["properties-fixed"]).is_ok() {
                return Ok(sim);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        anyhow::bail!("swtpm did not start")
    }
    fn tool(&self, tool: &str, args: &[&str]) -> Result<()> {
        self.tool_with_input(tool, args, &[])
    }
    fn tool_with_input(&self, tool: &str, args: &[&str], input: &[u8]) -> Result<()> {
        let mut child = Command::new(tool)
            .args(["-T", &self.tcti])
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        child
            .stdin
            .take()
            .context("TPM tool stdin missing")?
            .write_all(input)?;
        let output = child.wait_with_output()?;
        ensure!(
            output.status.success(),
            "{tool}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }
    fn write_nv(&self, index: &str, bytes: &[u8], attrs: &str) -> Result<()> {
        let file = self.dir.path().join("nv.bin");
        std::fs::write(&file, bytes)?;
        self.tool(
            "tpm2_nvdefine",
            &[
                index,
                "-C",
                "o",
                "-s",
                &bytes.len().to_string(),
                "-a",
                attrs,
            ],
        )?;
        self.tool(
            "tpm2_nvwrite",
            &[index, "-C", "o", "-i", file.to_str().unwrap()],
        )
    }
    fn certificate(&self, kind: &str, custom: bool, owner_only: bool) -> Result<Vec<u8>> {
        let nonce = b"manufacturer nonce";
        if custom {
            let alg = if kind == "rsa" {
                AsymmetricAlgorithmSelection::Rsa(RsaKeyBits::Rsa2048)
            } else {
                AsymmetricAlgorithmSelection::Ecc(EccCurve::NistP256)
            };
            let template = ek::create_ek_public_from_default_template_2(alg, None)?;
            let (template_index, nonce_index) = if kind == "rsa" {
                ("0x01c00004", "0x01c00003")
            } else {
                ("0x01c0000c", "0x01c0000b")
            };
            self.write_nv(
                template_index,
                &template.marshall()?,
                "ownerread|ownerwrite|authread",
            )?;
            self.write_nv(nonce_index, nonce, "ownerread|ownerwrite|authread")?;
        }
        let ctx = self.dir.path().join("ek.ctx");
        let pem = self.dir.path().join("ek.pem");
        if custom {
            // tpm2_createek -t in tpm2-tools 5.4 copies the nonce pointer,
            // not its bytes. Create this test-only EK with the explicit TCG
            // template and padded nonce, independently of the client's reader.
            let mut unique = vec![0; if kind == "rsa" { 256 } else { 64 }];
            unique[..nonce.len()].copy_from_slice(nonce);
            self.tool_with_input(
                "tpm2_createprimary",
                &[
                    "-C",
                    "e",
                    "-g",
                    "sha256",
                    "-G",
                    if kind == "rsa" {
                        "rsa2048:aes128cfb"
                    } else {
                        "ecc256:aes128cfb"
                    },
                    "-a",
                    "fixedtpm|fixedparent|sensitivedataorigin|adminwithpolicy|restricted|decrypt",
                    "-L",
                    "837197674484b3f81a90cc8d46a5d724fd52d76e06520b64f2a1da1b331469aa",
                    "-u",
                    "-",
                    "-c",
                    ctx.to_str().unwrap(),
                ],
                &unique,
            )?;
        } else {
            self.tool("tpm2_createek", &["-G", kind, "-c", ctx.to_str().unwrap()])?;
        }
        self.tool(
            "tpm2_readpublic",
            &[
                "-c",
                ctx.to_str().unwrap(),
                "-f",
                "pem",
                "-o",
                pem.to_str().unwrap(),
            ],
        )?;
        let key = PKey::public_key_from_pem(&std::fs::read(pem)?)?;
        let ca = PKey::from_rsa(openssl::rsa::Rsa::generate(2048)?)?;
        let mut name = X509NameBuilder::new()?;
        name.append_entry_by_text("CN", "Isolated TPM test issuer")?;
        let name = name.build();
        let mut cert = X509::builder()?;
        cert.set_version(2)?;
        cert.set_subject_name(&name)?;
        cert.set_issuer_name(&name)?;
        let serial = openssl::bn::BigNum::from_u32(1)?.to_asn1_integer()?;
        cert.set_serial_number(&serial)?;
        cert.set_pubkey(&key)?;
        let start = Asn1Time::days_from_now(0)?;
        let end = Asn1Time::days_from_now(2)?;
        cert.set_not_before(&start)?;
        cert.set_not_after(&end)?;
        cert.sign(&ca, MessageDigest::sha256())?;
        let der = cert.build().to_der()?;
        self.write_nv(
            if kind == "rsa" {
                "0x01c00002"
            } else {
                "0x01c0000a"
            },
            &der,
            if owner_only {
                "ownerread|ownerwrite"
            } else {
                "ownerread|ownerwrite|authread"
            },
        )?;
        // This simulator belongs exclusively to this test.
        self.tool("tpm2_flushcontext", &["-t"])?;
        Ok(der)
    }
    fn prove_after_reload(&self, kind: &str) -> Result<()> {
        let (identity, _) = Identity::create(&self.tcti, kind)?;
        let restored: Identity = serde_json::from_slice(&serde_json::to_vec(&identity)?)?;
        let claims = Claims::new(
            "test-device",
            "GET",
            "https://sync.example.test/v1/manifest",
            b"",
            "",
        )?;
        let proof = Proof::decode(&restored.proof(claims)?)?;
        proof.verify(
            &restored.public,
            "GET",
            "https://sync.example.test/v1/manifest",
            b"",
            "",
        )?;
        Ok(())
    }
}

#[test]
#[ignore = "requires swtpm and tpm2-tools; run make test-tpm"]
fn manufacturer_templates_and_nonces_support_rsa_and_ecc() -> Result<()> {
    for kind in ["rsa", "ecc"] {
        let sim = Simulator::new()?;
        sim.certificate(kind, true, false)?;
        ensure!(tpm::doctor(&sim.tcti)? == kind);
        sim.prove_after_reload(kind)?;
    }
    Ok(())
}

#[test]
#[ignore = "requires swtpm and tpm2-tools; run make test-tpm"]
fn certificates_and_chain_are_readable_with_provisioned_owner_auth() -> Result<()> {
    let sim = Simulator::new()?;
    let der = sim.certificate("rsa", false, false)?;
    sim.write_nv("0x01c00100", &der, "ownerread|ownerwrite|authread")?;
    sim.tool("tpm2_changeauth", &["-c", "o", "isolated-test-owner"])?;
    ensure!(tpm::doctor(&sim.tcti)? == "rsa");
    let (identity, _) = Identity::create(&sim.tcti, "rsa")?;
    ensure!(identity.ek_intermediates()? == vec![der]);
    sim.prove_after_reload("rsa")
}

#[test]
#[ignore = "requires swtpm and tpm2-tools; run make test-tpm"]
fn owner_read_only_certificate_and_wrong_certificate_fail_closed() -> Result<()> {
    let sim = Simulator::new()?;
    sim.certificate("rsa", false, true)?;
    ensure!(tpm::doctor(&sim.tcti)? == "rsa");
    let other = Simulator::new()?;
    let wrong = other.certificate("rsa", false, false)?;
    ensure!(tpm::doctor_with_certificate(&sim.tcti, Some(&wrong)).is_err());
    sim.tool("tpm2_changeauth", &["-c", "o", "isolated-test-owner"])?;
    ensure!(tpm::doctor(&sim.tcti).is_err());
    Ok(())
}

#[test]
#[ignore = "requires swtpm and tpm2-tools; run make test-tpm"]
fn invalid_manufacturer_template_is_not_silently_ignored() -> Result<()> {
    let sim = Simulator::new()?;
    sim.certificate("rsa", false, false)?;
    sim.write_nv(
        "0x01c00004",
        b"invalid template",
        "ownerread|ownerwrite|authread",
    )?;
    let error = tpm::doctor(&sim.tcti).unwrap_err();
    ensure!(
        error
            .to_string()
            .contains("invalid manufacturer EK template")
    );
    Ok(())
}

#[test]
#[ignore = "requires swtpm and tpm2-tools; run make test-tpm"]
fn doctor_accepts_an_explicit_transport() -> Result<()> {
    let sim = Simulator::new()?;
    sim.certificate("ecc", false, false)?;
    let output = Command::new(env!("CARGO_BIN_EXE_mysync"))
        .args(["doctor", "--tcti", &sim.tcti])
        .output()?;
    ensure!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

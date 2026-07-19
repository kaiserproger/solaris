use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use bytes::Buf;
use clap::Parser;
use mc_protocol::packets::Packet;
use mc_protocol::packets::configuration::{
    AcknowledgeFinishConfiguration, ClientboundKnownPacks, FinishConfiguration, RegistryData,
    ServerboundKnownPacks, UpdateTags,
};
use mc_protocol::{PROTOCOL_VERSION, TARGET_RELEASE};
use mc_test_harness::client::Client;
use mc_test_harness::parity::VanillaServerProcess;

#[derive(Debug, Parser)]
#[command(
    name = "registry-data-extract",
    about = "Capture exact full RegistryData payloads from a local vanilla server"
)]
struct Cli {
    /// Mojang server bundle jar for the exact Solaris target release.
    #[arg(long, default_value = ".analysis/server.jar")]
    jar: PathBuf,

    /// Existing vanilla sidecar populated by tools/extract-vanilla-data.sh.
    #[arg(long, default_value = "data/vanilla")]
    out: PathBuf,

    /// Maximum time to wait for the vanilla process to announce readiness.
    #[arg(long, default_value_t = 90)]
    startup_timeout_seconds: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let jar = fs::canonicalize(&cli.jar)
        .with_context(|| format!("resolve vanilla jar at {}", cli.jar.display()))?;
    let out = fs::canonicalize(&cli.out)
        .with_context(|| format!("resolve vanilla sidecar at {}", cli.out.display()))?;
    let expected_data = mc_data::load(&out)
        .with_context(|| format!("load vanilla sidecar at {}", out.display()))?;
    let expected = expected_registry_entries(&expected_data);

    let staging = tempfile::Builder::new()
        .prefix(".registry-network-nbt-")
        .tempdir_in(&out)
        .with_context(|| format!("create staging directory in {}", out.display()))?;
    let vanilla_work = tempfile::tempdir().context("create vanilla work directory")?;
    let vanilla = VanillaServerProcess::launch(
        &jar,
        vanilla_work.path(),
        Duration::from_secs(cli.startup_timeout_seconds),
    )?;

    let captured = capture_registry_payloads(vanilla.addr(), staging.path()).await?;
    ensure!(
        captured == expected,
        "captured RegistryData index differs from extracted JSON index:\nexpected={expected:#?}\ncaptured={captured:#?}"
    );

    let staged_payloads = staging.path().join(mc_data::NETWORK_REGISTRY_PAYLOAD_DIR);
    let final_payloads = out.join(mc_data::NETWORK_REGISTRY_PAYLOAD_DIR);
    if final_payloads.exists() {
        fs::remove_dir_all(&final_payloads)
            .with_context(|| format!("remove {}", final_payloads.display()))?;
    }
    fs::rename(&staged_payloads, &final_payloads).with_context(|| {
        format!(
            "install captured payloads from {} to {}",
            staged_payloads.display(),
            final_payloads.display()
        )
    })?;

    let installed = mc_data::load(&out).context("validate installed registry payloads")?;
    ensure!(
        installed.has_full_registry_payloads(),
        "installed RegistryData payload index is incomplete"
    );

    let entry_count = captured.values().map(BTreeSet::len).sum::<usize>();
    println!(
        "captured exact RegistryData fallback for {TARGET_RELEASE} protocol {PROTOCOL_VERSION}: {} registries, {entry_count} entries",
        captured.len()
    );
    vanilla.stop()?;
    Ok(())
}

fn expected_registry_entries(data: &mc_data::VanillaData) -> BTreeMap<String, BTreeSet<String>> {
    data.registries()
        .map(|registry| {
            (
                registry.id.to_string(),
                registry.entries.iter().map(ToString::to_string).collect(),
            )
        })
        .collect()
}

async fn capture_registry_payloads(
    addr: std::net::SocketAddr,
    staging_root: &Path,
) -> Result<BTreeMap<String, BTreeSet<String>>> {
    let mut client = Client::connect(addr).await?;
    let _ = client.drive_login(addr, "RegistryCapture").await?;

    loop {
        let frame = client.read_frame().await?;
        if frame.id == ClientboundKnownPacks::ID {
            let mut body = frame.body;
            let _ = ClientboundKnownPacks::decode(&mut body)?;
            ensure!(!body.has_remaining(), "Known Packs has trailing bytes");
            break;
        }
    }
    client
        .write_packet(&ServerboundKnownPacks { packs: Vec::new() })
        .await?;

    let mut captured = BTreeMap::new();
    loop {
        let frame = client.read_frame().await?;
        if frame.id == RegistryData::ID {
            let mut body = frame.body;
            let registry = RegistryData::decode(&mut body)?;
            ensure!(!body.has_remaining(), "RegistryData has trailing bytes");

            let mut entries = BTreeSet::new();
            for entry in registry.entries {
                let payload = entry.nbt_payload.ok_or_else(|| {
                    anyhow::anyhow!(
                        "vanilla omitted payload for {} in {} after Known Packs was declined",
                        entry.name,
                        registry.registry_id
                    )
                })?;
                ensure!(
                    entries.insert(entry.name.to_string()),
                    "duplicate entry {} in {}",
                    entry.name,
                    registry.registry_id
                );
                let path = mc_data::network_registry_payload_path(
                    staging_root,
                    &registry.registry_id,
                    &entry.name,
                );
                let parent = path.parent().ok_or_else(|| {
                    anyhow::anyhow!("captured payload path has no parent: {}", path.display())
                })?;
                fs::create_dir_all(parent)
                    .with_context(|| format!("create {}", parent.display()))?;
                fs::write(&path, payload.as_ref())
                    .with_context(|| format!("write {}", path.display()))?;
            }
            if captured
                .insert(registry.registry_id.to_string(), entries)
                .is_some()
            {
                bail!("duplicate RegistryData packet for {}", registry.registry_id);
            }
            continue;
        }
        if frame.id == UpdateTags::ID {
            let mut body = frame.body;
            let _ = UpdateTags::decode(&mut body)?;
            ensure!(!body.has_remaining(), "Update Tags has trailing bytes");
            continue;
        }
        if frame.id == FinishConfiguration::ID {
            let mut body = frame.body;
            let _ = FinishConfiguration::decode(&mut body)?;
            ensure!(
                !body.has_remaining(),
                "Finish Configuration has trailing bytes"
            );
            client.write_packet(&AcknowledgeFinishConfiguration).await?;
            return Ok(captured);
        }
        bail!(
            "unexpected configuration frame id=0x{:02X} body_len={}",
            frame.id,
            frame.body.len()
        );
    }
}

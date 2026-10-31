//! Tests for [`crate::configuration`]: pack advertisement, known-packs
//! negotiation, registry/tag/brand/finish ordering, and ignore tolerance.

use mc_protocol::TARGET_RELEASE;
use mc_protocol::packets::Packet as _;
use mc_protocol::packets::configuration::{
    AcknowledgeFinishConfiguration, ClientboundKnownPacks, FinishConfiguration,
    ServerboundClientInformation, ServerboundKnownPacks,
};
use mc_protocol::packets::{ChatVisibility, ClientInformation, MainHand, ParticleStatus};
use tokio::net::TcpStream;

use crate::connection::Connection;

async fn loopback_pair() -> (Connection, Connection) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let client = TcpStream::connect(addr).await.unwrap();
    let (server, _) = listener.accept().await.unwrap();
    (Connection::new(client), Connection::new(server))
}

fn information(view_distance: i8) -> ClientInformation {
    information_locale("en_us", view_distance)
}

fn information_locale(language: &str, view_distance: i8) -> ClientInformation {
    ClientInformation {
        language: language.to_owned(),
        view_distance,
        chat_visibility: ChatVisibility::Full,
        chat_colors: true,
        model_customisation: 0,
        main_hand: MainHand::Right,
        text_filtering_enabled: false,
        allows_listing: true,
        particle_status: ParticleStatus::All,
    }
}

fn test_data() -> mc_data::VanillaData {
    mc_data::testing::stub()
}

fn test_tags() -> mc_data::tags::TagsData {
    mc_data::tags::TagsData::default()
}

#[tokio::test]
async fn vanilla_configuration_completes_with_only_native_optional_bundle() {
    let (mut client, mut server) = loopback_pair().await;
    let data = test_data();
    let registry_count = data.registry_count();
    let native_optional = crate::server::loader_manifest(&[]).unwrap();
    let server_task = tokio::spawn(async move {
        let tags = test_tags();
        super::handle_configuration(
            &mut server,
            &data,
            &tags,
            native_optional.as_ref(),
            false,
            &Default::default(),
        )
        .await
        .unwrap()
    });

    // UpdateEnabledFeatures.
    let _ = client
        .read_frame_in(mc_protocol::State::Configuration)
        .await
        .unwrap();
    read_channel_register(&mut client).await;
    // The optional-only manifest is probed before known-pack negotiation.
    let probe = client
        .read_frame_in(mc_protocol::State::Configuration)
        .await
        .unwrap();
    assert_eq!(
        probe.id,
        mc_protocol::packets::configuration::ClientboundCustomPayload::ID
    );
    // ClientboundKnownPacks: must advertise minecraft:core:<release>.
    let frame = client
        .read_frame_in(mc_protocol::State::Configuration)
        .await
        .unwrap();
    assert_eq!(frame.id, ClientboundKnownPacks::ID);
    let mut body = frame.body.clone();
    let packs = ClientboundKnownPacks::decode(&mut body).expect("known packs decode");
    assert!(
        packs.packs.iter().any(|p| {
            p.namespace == "minecraft" && p.id == "core" && p.version == TARGET_RELEASE
        })
    );

    // Tolerated packet before KnownPacks: client information.
    client
        .send_packet(&ServerboundClientInformation {
            information: information(6),
        })
        .await
        .unwrap();
    client
        .send_packet(&ServerboundKnownPacks {
            packs: packs.packs.clone(),
        })
        .await
        .unwrap();

    // Drain RegistryData + UpdateTags + Brand + FinishConfiguration.
    let mut saw_finish = false;
    for _ in 0..(registry_count + 8) {
        let frame = client
            .read_frame_in(mc_protocol::State::Configuration)
            .await
            .unwrap();
        if frame.id == FinishConfiguration::ID {
            saw_finish = true;
            break;
        }
    }
    assert!(saw_finish, "server must send FinishConfiguration");

    // Tolerated packet while waiting for ack, then ack.
    client
        .send_packet(&ServerboundClientInformation {
            information: information_locale("ru_ru", 9),
        })
        .await
        .unwrap();
    client
        .send_packet(&AcknowledgeFinishConfiguration)
        .await
        .unwrap();

    let outcome = server_task.await.unwrap();
    assert_eq!(outcome.client_view_distance, 9);
    // The latest `Client Information` also carries the locale for dialogs.
    assert_eq!(outcome.client_language, "ru_ru");
}

#[test]
fn client_preferences_default_to_english_and_unknown_packets_change_nothing() {
    let mut prefs = super::ClientPrefs::default();
    assert_eq!(prefs.language, "");
    assert_eq!(prefs.view_distance, 8);
    assert!(
        super::note_ignored(&mut prefs, 0x7fff, bytes::Bytes::new()).unwrap(),
        "unknown packets are tolerated"
    );
    assert_eq!(prefs.language, "");
}

#[tokio::test]
async fn missing_pack_without_sidecar_payloads_fails() {
    let (mut client, mut server) = loopback_pair().await;
    // `testing::stub()` has no sidecar payloads, so a client that reports
    // no packs must be rejected.
    let data = test_data();
    assert!(!data.has_full_registry_payloads());
    let server_task = tokio::spawn(async move {
        let tags = test_tags();
        super::handle_configuration(&mut server, &data, &tags, None, false, &Default::default())
            .await
    });
    // Skip features + known packs advertisements.
    for _ in 0..2 {
        let _ = client
            .read_frame_in(mc_protocol::State::Configuration)
            .await
            .unwrap();
    }
    client
        .send_packet(&ServerboundKnownPacks { packs: Vec::new() })
        .await
        .unwrap();
    let result = server_task.await.unwrap();
    assert!(result.is_err());
}

/// One verified optional/required bundle (an items-only index) as the discovery
/// path would hand it to the Loader manifest.
fn items_bundle(required: bool) -> mc_loader::LoaderManifest {
    use std::io::Write as _;
    let index = br#"{"schema":2,"items":[],"assets":[]}"#;
    let mut archive = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    archive
        .start_file(
            "solaris-client.json",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
    archive.write_all(index).unwrap();
    let bytes = archive.finish().unwrap().into_inner();
    let sha256 = format!("{:x}", <sha2::Sha256 as sha2::Digest>::digest(&bytes));
    let bundle = mc_script::ClientBundle::new(
        "chisel",
        "chisel-content",
        "1",
        "client/chisel-content.zip",
        sha256,
        bytes.len() as u64,
        "client/chisel-content.zip".into(),
        std::sync::Arc::from(bytes),
        vec![mc_script::ClientLoader::Native],
        vec![mc_script::ClientContentKind::Items],
        vec![mc_script::ClientPermission::RegisterItems],
    )
    .with_required(required);
    assert_eq!(bundle.required(), required);
    mc_loader::LoaderManifest::from_script_bundles(&[bundle]).unwrap()
}

/// Reads server frames until FinishConfiguration, returning every custom
/// payload channel the server sent.
async fn channels_until_finish(client: &mut Connection) -> Vec<String> {
    let mut channels = Vec::new();
    loop {
        let frame = client
            .read_frame_in(mc_protocol::State::Configuration)
            .await
            .unwrap();
        if frame.id == FinishConfiguration::ID {
            return channels;
        }
        if frame.id == mc_protocol::packets::configuration::ClientboundCustomPayload::ID {
            let mut body = frame.body.clone();
            if let mc_protocol::packets::CustomPayload::Unknown { channel, .. } =
                mc_protocol::packets::CustomPayload::decode(&mut body).unwrap()
            {
                channels.push(channel.to_string());
            }
        }
    }
}

/// Reads the `minecraft:register` frame that declares every serverbound Loader
/// channel: NeoForge clients send a custom payload only on a declared channel.
async fn read_channel_register(client: &mut Connection) {
    let register = client
        .read_frame_in(mc_protocol::State::Configuration)
        .await
        .unwrap();
    match mc_protocol::packets::CustomPayload::decode(&mut register.body.clone()).unwrap() {
        mc_protocol::packets::CustomPayload::Unknown { channel, payload } => {
            assert_eq!(channel.to_string(), "minecraft:register");
            let declared: Vec<&[u8]> = payload.split(|byte| *byte == 0).collect();
            for serverbound in [
                "solaris:loader/hello",
                "solaris:loader/request",
                "solaris:loader/ack",
                "solaris:loader/view_action",
            ] {
                assert!(
                    declared.contains(&serverbound.as_bytes()),
                    "{serverbound} is declared: {:?}",
                    String::from_utf8_lossy(&payload)
                );
            }
        }
        other => panic!("expected minecraft:register, got {other:?}"),
    }
}

#[tokio::test]
async fn vanilla_client_completes_configuration_with_only_optional_bundles() {
    let (mut client, mut server) = loopback_pair().await;
    let data = test_data();
    let manifest = items_bundle(false);
    let server_task = tokio::spawn(async move {
        let tags = test_tags();
        super::handle_configuration(
            &mut server,
            &data,
            &tags,
            Some(&manifest),
            false,
            &Default::default(),
        )
        .await
        .unwrap()
    });
    let _features = client
        .read_frame_in(mc_protocol::State::Configuration)
        .await
        .unwrap();
    read_channel_register(&mut client).await;
    // The probe precedes the known-pack request.
    let probe = client
        .read_frame_in(mc_protocol::State::Configuration)
        .await
        .unwrap();
    assert_eq!(
        probe.id,
        mc_protocol::packets::configuration::ClientboundCustomPayload::ID
    );
    let packs = client
        .read_frame_in(mc_protocol::State::Configuration)
        .await
        .unwrap();
    assert_eq!(packs.id, ClientboundKnownPacks::ID);
    let packs = ClientboundKnownPacks::decode(&mut packs.body.clone()).unwrap();
    // A vanilla client ignores the unknown payload and just answers known packs.
    client
        .send_packet(&ServerboundKnownPacks { packs: packs.packs })
        .await
        .unwrap();
    let channels = channels_until_finish(&mut client).await;
    assert!(
        !channels
            .iter()
            .any(|channel| channel == "solaris:loader/manifest"),
        "a client that never answered the probe is never sent the manifest: {channels:?}"
    );
    client
        .send_packet(&AcknowledgeFinishConfiguration)
        .await
        .unwrap();
    let outcome = server_task.await.unwrap();
    assert!(outcome.loader_session.is_none());
}

#[tokio::test]
async fn loader_hello_before_known_packs_starts_negotiation_of_optional_bundles() {
    let (mut client, mut server) = loopback_pair().await;
    let data = test_data();
    let manifest = items_bundle(false);
    let server_task = tokio::spawn(async move {
        let tags = test_tags();
        // The client below never acknowledges the manifest; the task is dropped.
        let _ = super::handle_configuration(
            &mut server,
            &data,
            &tags,
            Some(&manifest),
            false,
            &Default::default(),
        )
        .await;
    });
    let _features = client
        .read_frame_in(mc_protocol::State::Configuration)
        .await
        .unwrap();
    read_channel_register(&mut client).await;
    let probe = client
        .read_frame_in(mc_protocol::State::Configuration)
        .await
        .unwrap();
    let mut body = probe.body.clone();
    match mc_protocol::packets::CustomPayload::decode(&mut body).unwrap() {
        mc_protocol::packets::CustomPayload::Unknown { channel, payload } => {
            assert_eq!(channel, *mc_loader::loader_probe_channel());
            assert!(payload.is_empty());
        }
        other => panic!("expected the probe payload, got {other:?}"),
    }
    let packs = client
        .read_frame_in(mc_protocol::State::Configuration)
        .await
        .unwrap();
    let packs = ClientboundKnownPacks::decode(&mut packs.body.clone()).unwrap();
    client
        .send_packet(
            &mc_protocol::packets::configuration::ServerboundCustomPayload {
                payload: mc_protocol::packets::CustomPayload::Unknown {
                    channel: mc_loader::loader_hello_channel().clone(),
                    payload: Vec::new(),
                },
            },
        )
        .await
        .unwrap();
    client
        .send_packet(&ServerboundKnownPacks { packs: packs.packs })
        .await
        .unwrap();
    // The first custom payload after registries/tags/brand is the manifest.
    let mut manifest_seen = false;
    for _ in 0..64 {
        let frame = client
            .read_frame_in(mc_protocol::State::Configuration)
            .await
            .unwrap();
        if frame.id == mc_protocol::packets::configuration::ClientboundCustomPayload::ID
            && let mc_protocol::packets::CustomPayload::Unknown { channel, .. } =
                mc_protocol::packets::CustomPayload::decode(&mut frame.body.clone()).unwrap()
            && channel == *mc_loader::loader_manifest_channel()
        {
            manifest_seen = true;
            break;
        }
    }
    assert!(
        manifest_seen,
        "a Loader client that said hello receives the manifest"
    );
    drop(client);
    let _ = server_task.await;
}

#[tokio::test]
async fn required_bundles_skip_the_probe_and_negotiate_with_every_client() {
    let (mut client, mut server) = loopback_pair().await;
    let data = test_data();
    let manifest = items_bundle(true);
    let server_task = tokio::spawn(async move {
        let tags = test_tags();
        let _ = super::handle_configuration(
            &mut server,
            &data,
            &tags,
            Some(&manifest),
            true,
            &Default::default(),
        )
        .await;
    });
    let _features = client
        .read_frame_in(mc_protocol::State::Configuration)
        .await
        .unwrap();
    read_channel_register(&mut client).await;
    let next = client
        .read_frame_in(mc_protocol::State::Configuration)
        .await
        .unwrap();
    assert_eq!(
        next.id,
        ClientboundKnownPacks::ID,
        "no probe for required content"
    );
    drop(client);
    let _ = server_task.await;
}

#[tokio::test]
async fn serverops_required_pack_is_loaded_before_finish_configuration() {
    use mc_protocol::packets::configuration::ServerboundResourcePack;
    use mc_protocol::packets::resource_pack::ResourcePackPush;
    use mc_protocol::packets::{ResourcePackAction, ResourcePackStatus};
    let (mut client, mut server) = loopback_pair().await;
    let task = tokio::spawn(async move {
        let pack = crate::config::ResourcePackSection {
            url: "https://example.org/pack.zip".into(),
            required: true,
            ..Default::default()
        };
        super::handle_configuration(&mut server, &test_data(), &test_tags(), None, false, &pack)
            .await
    });
    let _ = client
        .read_frame_in(mc_protocol::State::Configuration)
        .await
        .unwrap();
    let frame = client
        .read_frame_in(mc_protocol::State::Configuration)
        .await
        .unwrap();
    let packs = ClientboundKnownPacks::decode(&mut frame.body.clone()).unwrap();
    client
        .send_packet(&ServerboundKnownPacks { packs: packs.packs })
        .await
        .unwrap();
    let pack = loop {
        let frame = client
            .read_frame_in(mc_protocol::State::Configuration)
            .await
            .unwrap();
        assert_ne!(frame.id, FinishConfiguration::ID);
        if frame.id == ResourcePackPush::ID {
            break ResourcePackPush::decode(&mut frame.body.clone()).unwrap();
        }
    };
    assert!(pack.required);
    for action in [
        ResourcePackAction::Accepted,
        ResourcePackAction::Downloaded,
        ResourcePackAction::SuccessfullyLoaded,
    ] {
        client
            .send_packet(&ServerboundResourcePack {
                status: ResourcePackStatus {
                    id: pack.id,
                    action,
                },
            })
            .await
            .unwrap();
    }
    let frame = client
        .read_frame_in(mc_protocol::State::Configuration)
        .await
        .unwrap();
    assert_eq!(frame.id, FinishConfiguration::ID);
    client
        .send_packet(&AcknowledgeFinishConfiguration)
        .await
        .unwrap();
    assert!(task.await.unwrap().is_ok());
}

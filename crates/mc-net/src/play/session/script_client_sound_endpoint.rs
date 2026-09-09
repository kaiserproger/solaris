use std::sync::LazyLock;

use bytes::BufMut;
use mc_data::Identifier;
use mc_script::{AdmittedScriptCommand, ScriptClientSound, ScriptDtoError};

use crate::{LOADER_PROTOCOL_VERSION, LoaderContentKind, LoaderManifest, LoaderPermission};

use super::SessionRegistry;
use super::outbound::{OutboundCommand, dispatch_visibility_command};
use super::visibility::ordered_session_recipient;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ScriptClientSoundRouteError {
    InvalidCommand(ScriptDtoError),
    SoundNotOwned,
    PluginHasNoEligibleSoundBundle,
    PlayerUnavailable,
}

fn sound_payload(sound: &ScriptClientSound) -> Vec<u8> {
    let playback = sound.playback();
    let mode = playback.map_or(0, |play| if play.position().is_some() { 2 } else { 1 });
    let mut payload = Vec::with_capacity(37 + sound.sound_id().len());
    payload.put_u16(LOADER_PROTOCOL_VERSION);
    payload.put_u8(mode);
    payload.put_u16(sound.sound_id().len() as u16);
    payload.extend_from_slice(sound.sound_id().as_bytes());
    if let Some(play) = playback {
        payload.put_f32(play.volume());
        payload.put_f32(play.pitch());
        if let Some(position) = play.position() {
            payload.put_f64(position.x());
            payload.put_f64(position.y());
            payload.put_f64(position.z());
        }
    }
    payload
}

impl SessionRegistry {
    pub(crate) fn route_script_client_sound_command(
        &self,
        admitted: AdmittedScriptCommand,
        manifest: Option<&LoaderManifest>,
    ) -> Result<(), ScriptClientSoundRouteError> {
        let (plugin, player_id, sound) = admitted
            .into_client_sound()
            .map_err(ScriptClientSoundRouteError::InvalidCommand)?;
        if !sound
            .sound_id()
            .strip_prefix(plugin.plugin_id())
            .is_some_and(|suffix| suffix.starts_with(':') && suffix.len() > 1)
        {
            return Err(ScriptClientSoundRouteError::SoundNotOwned);
        }
        if !manifest.is_some_and(|manifest| {
            manifest.bundles.iter().any(|bundle| {
                bundle.owner == plugin.plugin_id()
                    && bundle.content.contains(&LoaderContentKind::Sounds)
                    && bundle.permissions.contains(&LoaderPermission::PlaySounds)
            })
        }) {
            return Err(ScriptClientSoundRouteError::PluginHasNoEligibleSoundBundle);
        }
        let recipient = {
            let inner = self.lock_inner("route script client sound");
            let session = inner
                .sessions
                .get(&player_id.value())
                .filter(|session| session.loader_session.is_some() && !session.tx.is_closed())
                .ok_or(ScriptClientSoundRouteError::PlayerUnavailable)?;
            ordered_session_recipient(player_id.value(), session)
        };
        static CHANNEL: LazyLock<Identifier> = LazyLock::new(|| {
            Identifier::parse("solaris:loader/sound").expect("static Loader sound channel")
        });
        dispatch_visibility_command(
            &recipient,
            OutboundCommand::CustomPayload {
                channel: CHANNEL.clone(),
                payload: sound_payload(&sound),
            },
        );
        Ok(())
    }
}

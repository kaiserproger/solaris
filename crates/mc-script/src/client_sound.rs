use std::sync::Arc;

use crate::{
    AdmittedScriptCommand, ScriptCommand, ScriptDtoError, ScriptPlayerId, ScriptPluginTarget,
    ScriptPosition, validate_contract_resource_id,
};

/// One-shot sound parameters. No position means listener-relative, non-attenuated audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScriptClientSoundPlayback {
    volume_bits: u32,
    pitch_bits: u32,
    position: Option<ScriptPosition>,
}

impl ScriptClientSoundPlayback {
    pub fn volume(self) -> f32 {
        f32::from_bits(self.volume_bits)
    }
    pub fn pitch(self) -> f32 {
        f32::from_bits(self.pitch_bits)
    }
    pub fn position(self) -> Option<ScriptPosition> {
        self.position
    }
}

/// Play or stop one owner-declared sound through the player's acknowledged Loader session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptClientSound {
    sound_id: String,
    playback: Option<ScriptClientSoundPlayback>,
}

impl ScriptClientSound {
    pub fn play(
        sound_id: &str,
        volume: f32,
        pitch: f32,
        position: Option<ScriptPosition>,
    ) -> Result<Self, ScriptDtoError> {
        if !(0.0..=1.0).contains(&volume) || !(0.5..=2.0).contains(&pitch) {
            return Err(ScriptDtoError::InvalidBounds);
        }
        Ok(Self {
            sound_id: validate_contract_resource_id(sound_id)?,
            playback: Some(ScriptClientSoundPlayback {
                volume_bits: volume.to_bits(),
                pitch_bits: pitch.to_bits(),
                position,
            }),
        })
    }

    pub fn stop(sound_id: &str) -> Result<Self, ScriptDtoError> {
        Ok(Self {
            sound_id: validate_contract_resource_id(sound_id)?,
            playback: None,
        })
    }

    pub fn sound_id(&self) -> &str {
        &self.sound_id
    }
    pub fn playback(&self) -> Option<ScriptClientSoundPlayback> {
        self.playback
    }
}

impl AdmittedScriptCommand {
    pub fn into_client_sound(
        self,
    ) -> Result<(ScriptPluginTarget, ScriptPlayerId, ScriptClientSound), ScriptDtoError> {
        let request =
            Arc::try_unwrap(self.request).unwrap_or_else(|request| request.as_ref().clone());
        let ScriptCommand::ClientSound { player_id, sound } = request else {
            return Err(ScriptDtoError::InconsistentResult {
                field: "client sound admission",
            });
        };
        Ok((
            ScriptPluginTarget {
                plugin_id: self.plugin_id,
            },
            player_id,
            sound,
        ))
    }
}

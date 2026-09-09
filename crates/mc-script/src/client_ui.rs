use crate::{ScriptDtoError, validate_contract_resource_id};

pub const MAX_CLIENT_UI_TITLE_BYTES: usize = 128;
pub const MAX_CLIENT_UI_BODY_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScriptClientUiMode {
    Screen,
    Hud,
    Hidden,
}

/// One owner-declared UI resource presented through the current Loader session.
/// Omitted text uses the verified resource definition; empty text is an override.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptClientUiPresentation {
    ui_id: String,
    mode: ScriptClientUiMode,
    title: Option<String>,
    body: Option<String>,
}

impl ScriptClientUiPresentation {
    pub fn try_new(
        ui_id: &str,
        mode: ScriptClientUiMode,
        title: Option<String>,
        body: Option<String>,
    ) -> Result<Self, ScriptDtoError> {
        for (text, field, max_bytes) in [
            (title.as_deref(), "UI title", MAX_CLIENT_UI_TITLE_BYTES),
            (body.as_deref(), "UI body", MAX_CLIENT_UI_BODY_BYTES),
        ] {
            if let Some(text) = text
                && text.len() > max_bytes
            {
                return Err(ScriptDtoError::ValueTooLong {
                    field,
                    max_bytes,
                    actual_bytes: text.len(),
                });
            }
        }
        Ok(Self {
            ui_id: validate_contract_resource_id(ui_id)?,
            mode,
            title,
            body,
        })
    }

    pub fn ui_id(&self) -> &str {
        &self.ui_id
    }

    pub const fn mode(&self) -> ScriptClientUiMode {
        self.mode
    }

    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    pub fn body(&self) -> Option<&str> {
        self.body.as_deref()
    }
}

/// Source transition of a client-originated Loader action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScriptLoaderInteractionPhase {
    /// A UI button was activated.
    Trigger,
    /// A declared key went down while gameplay had input focus.
    Press,
    /// A held key was released or gameplay lost input focus.
    Release,
}

impl ScriptLoaderInteractionPhase {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Trigger => "trigger",
            Self::Press => "press",
            Self::Release => "release",
        }
    }
}

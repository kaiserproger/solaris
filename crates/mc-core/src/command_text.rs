//! Text-component command arguments (`/tellraw`, `/title`, `/bossbar`).
//!
//! Vanilla `ComponentArgument` reads SNBT (JSON text is valid SNBT) and
//! `ComponentUtils.resolve` expands `selector` and `score` components against
//! the command source before the component is sent. Network components are NBT,
//! so a parsed component is sent as the tag itself.

use mc_nbt::{ListTag, Tag};

use crate::boundary::EntityId;
use crate::commands::CommandError;
use crate::tick::TickState;

/// Parse a vanilla text-component argument (SNBT: a quoted/unquoted string, a
/// `{..}` compound or a `[..]` list) into a network component tag.
pub(crate) fn parse_component(text: &str) -> Result<Tag, CommandError> {
    let value = crate::command_snbt::parse(text.trim())
        .map_err(|error| CommandError::Message(format!("Invalid chat component: {error}")))?;
    match value {
        Tag::String(_) | Tag::Compound(_) | Tag::List(_) => Ok(value),
        _ => Err(CommandError::Message(
            "Invalid chat component: expected a string, compound or list".into(),
        )),
    }
}

fn text_tag(value: impl ToString) -> Tag {
    Tag::Compound(vec![("text".into(), Tag::String(value.to_string()))])
}

/// `ComponentUtils.resolve` for `executor` (`EntityId(-1)` is the console):
/// replaces `selector` components with the matched names and `score`
/// components with the scoreboard value, recursively through `extra`, `with`
/// and lists. `nbt` components are refused rather than silently dropped.
pub(crate) fn resolve_component(
    state: &TickState,
    executor: EntityId,
    component: &Tag,
) -> Result<Tag, String> {
    match component {
        Tag::List(list) => {
            let elements = list
                .elements
                .iter()
                .map(|element| resolve_component(state, executor, element))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Tag::List(ListTag {
                element_type: list.element_type,
                elements,
            }))
        }
        Tag::Compound(fields) => resolve_compound(state, executor, fields),
        other => Ok(other.clone()),
    }
}

fn field<'a>(fields: &'a [(String, Tag)], key: &str) -> Option<&'a Tag> {
    fields
        .iter()
        .find_map(|(name, value)| (name == key).then_some(value))
}

fn resolve_compound(
    state: &TickState,
    executor: EntityId,
    fields: &[(String, Tag)],
) -> Result<Tag, String> {
    if field(fields, "nbt").is_some() {
        return Err("NBT text components are not supported by this server".into());
    }
    // Style and every other key survive; the content keys are rebuilt below.
    let mut resolved: Vec<(String, Tag)> = Vec::with_capacity(fields.len() + 1);
    let mut extra: Vec<Tag> = Vec::new();
    let mut replaced = false;
    if let Some(Tag::String(selector)) = field(fields, "selector") {
        replaced = true;
        let target = crate::commands::target_of(selector).map_err(|error| error.message())?;
        let separator = match field(fields, "separator") {
            Some(separator) => resolve_component(state, executor, separator)?,
            None => Tag::Compound(vec![
                ("text".into(), Tag::String(", ".into())),
                ("color".into(), Tag::String("gray".into())),
            ]),
        };
        for (index, entity) in crate::command_execution::resolve_target(state, executor, &target)
            .into_iter()
            .enumerate()
        {
            if index > 0 {
                extra.push(separator.clone());
            }
            extra.push(crate::command_execution::entity_name(state, entity));
        }
    } else if let Some(Tag::Compound(score)) = field(fields, "score") {
        replaced = true;
        let (Some(Tag::String(name)), Some(Tag::String(objective))) =
            (field(score, "name"), field(score, "objective"))
        else {
            return Err("Invalid score component".into());
        };
        let holder = if name.starts_with('@') {
            let target = crate::commands::target_of(name).map_err(|error| error.message())?;
            let found = crate::command_execution::resolve_target(state, executor, &target);
            match found.as_slice() {
                [entity] => state.connections.name_of(*entity).map(str::to_owned),
                _ => None,
            }
        } else if name == "*" {
            state.connections.name_of(executor).map(str::to_owned)
        } else {
            Some(name.clone())
        };
        if let Some(value) = holder
            .and_then(|holder| state.scoreboard.scores.get(&holder))
            .and_then(|scores| scores.get(objective))
        {
            extra.push(text_tag(value.value));
        }
    }
    for (key, value) in fields {
        match key.as_str() {
            "selector" | "separator" | "score" if replaced => {}
            "extra" => {
                let Tag::List(list) = value else {
                    resolved.push((key.clone(), value.clone()));
                    continue;
                };
                for element in &list.elements {
                    extra.push(resolve_component(state, executor, element)?);
                }
            }
            "with" => resolved.push((key.clone(), resolve_component(state, executor, value)?)),
            _ => resolved.push((key.clone(), value.clone())),
        }
    }
    if replaced {
        resolved.push(("text".into(), Tag::String(String::new())));
    }
    if !extra.is_empty() {
        resolved.push((
            "extra".into(),
            Tag::List(ListTag {
                element_type: mc_nbt::tag_type::COMPOUND,
                elements: extra
                    .into_iter()
                    .map(|tag| match tag {
                        Tag::Compound(_) => tag,
                        other => Tag::Compound(vec![("text".into(), other)]),
                    })
                    .collect(),
            }),
        ));
    }
    Ok(Tag::Compound(resolved))
}
/// `/title` action: the component-carrying forms store resolved
/// per-player network tags; `times` ticks use the vanilla `time` grammar
/// (bare ticks or a `<float>d|s|t` suffix, min 0); `clear` resets only when
/// `reset_times` is set (vanilla `reset`).
#[derive(Debug, Clone, PartialEq)]
pub enum TitleAction {
    Clear {
        reset_times: bool,
    },
    ShowTitle(mc_nbt::Tag),
    ShowSubtitle(mc_nbt::Tag),
    ShowActionBar(mc_nbt::Tag),
    Times {
        fade_in: i32,
        stay: i32,
        fade_out: i32,
    },
}
impl TitleAction {
    /// Vanilla feedback infix (`cleared`, `reset`, `show.title`, ...).
    pub(crate) fn feedback_kind(&self) -> &'static str {
        match self {
            Self::Clear { reset_times: false } => "cleared",
            Self::Clear { reset_times: true } => "reset",
            Self::ShowTitle(_) => "show.title",
            Self::ShowSubtitle(_) => "show.subtitle",
            Self::ShowActionBar(_) => "show.actionbar",
            Self::Times { .. } => "times",
        }
    }
}

/// Parse the tail of `/tellraw <targets> <message...>`: the target selector
/// plus the untouched component text (kept raw so `{..}`/`[..]`/quoted JSON
/// survives `command_words` splitting; components resolve per target).
pub(crate) fn parse_tellraw(
    args: &[&str],
    command: &str,
) -> Result<(crate::commands::Target, mc_nbt::Tag), crate::commands::CommandError> {
    let [target_word, ..] = args else {
        return Err(crate::commands::CommandError::Usage(
            "tellraw <targets> <message>",
        ));
    };
    let target = crate::commands::target_of(target_word)?;
    let tail = tail_after(command, &["tellraw", target_word])?;
    let message = parse_component(&tail)?;
    Ok((target, message))
}

/// Parse the tail of `/title <targets> <clear|reset|title|subtitle|actionbar|times> ...`.
pub(crate) fn parse_title(
    args: &[&str],
    command: &str,
) -> Result<(crate::commands::Target, TitleAction), crate::commands::CommandError> {
    let [target_word, verb, ..] = args else {
        return Err(crate::commands::CommandError::Usage(
            "title <targets> <clear|reset|title|subtitle|actionbar|times> ...",
        ));
    };
    let target = crate::commands::target_of(target_word)?;
    let action = match *verb {
        "clear" if args.len() == 2 => TitleAction::Clear { reset_times: false },
        "reset" if args.len() == 2 => TitleAction::Clear { reset_times: true },
        "title" | "subtitle" | "actionbar" => {
            let tail = tail_after(command, &["title", target_word, verb])?;
            let component = parse_component(&tail)?;
            match *verb {
                "title" => TitleAction::ShowTitle(component),
                "subtitle" => TitleAction::ShowSubtitle(component),
                _ => TitleAction::ShowActionBar(component),
            }
        }
        "times" if args.len() == 5 => {
            let [_, _, fade_in, stay, fade_out, ..] = args else {
                return Err(crate::commands::CommandError::Usage(
                    "title <targets> times <fadeIn> <stay> <fadeOut>",
                ));
            };
            TitleAction::Times {
                fade_in: title_time(fade_in)?,
                stay: title_time(stay)?,
                fade_out: title_time(fade_out)?,
            }
        }
        _ => {
            return Err(crate::commands::CommandError::Usage(
                "title <targets> <clear|reset|title|subtitle|actionbar|times> ...",
            ));
        }
    };
    Ok((target, action))
}

/// Vanilla `TimeArgument` for title times: bare ticks or `<float>d|s|t`,
/// rounded, clamped to `i32`, min 0 (negative is a usage error).
fn title_time(word: &str) -> Result<i32, crate::commands::CommandError> {
    let ticks = crate::commands::time_ticks(word).ok_or(crate::commands::CommandError::Message(
        format!("Invalid time '{word}'"),
    ))?;
    i32::try_from(ticks)
        .map_err(|_| crate::commands::CommandError::Message(format!("Invalid time '{word}'")))
        .and_then(|ticks| {
            (ticks >= 0)
                .then_some(ticks)
                .ok_or(crate::commands::CommandError::Message(format!(
                    "Invalid time '{word}'"
                )))
        })
}

/// Raw-text tail after the given leading words (whitespace-separated prefix, as in
/// `command_bossbar::tail_after` but local to this module so BossBar need not
/// expose its helper). `command_words` keeps balanced `{..}`/`[..]`/quoted
/// text as one word, so component JSON survives intact here.
fn tail_after(command: &str, leading: &[&str]) -> Result<String, crate::commands::CommandError> {
    use crate::commands::CommandError;
    let mut rest = command.trim_start();
    rest = rest.strip_prefix('/').unwrap_or(rest);
    for word in leading {
        let stripped = rest
            .strip_prefix(word)
            .ok_or(CommandError::Usage("tellraw <targets> <message>"))?;
        if !stripped.is_empty() && !stripped.starts_with(char::is_whitespace) {
            return Err(CommandError::Usage("tellraw <targets> <message>"));
        }
        rest = stripped.trim_start();
        if rest.is_empty() {
            break;
        }
    }
    if rest.is_empty() {
        return Err(CommandError::Usage("tellraw <targets> <message>"));
    }
    Ok(rest.to_owned())
}

/// `/tellraw`: resolve the component once per target player (entity override =
/// that player, mirroring vanilla's per-player `getResolvedComponent`) and
/// deliver it as system chat. Vanilla sends no feedback to the sender.
pub(crate) fn execute_tellraw(
    state: &mut TickState,
    connection: crate::boundary::ConnectionId,
    executor: EntityId,
    target: &crate::commands::Target,
    message: &mc_nbt::Tag,
) -> crate::command_execution::CommandResult {
    let targets = crate::command_execution::player_targets(state, executor, target);
    if targets.is_empty() {
        crate::command_execution::feedback(
            state,
            connection,
            "argument.entity.notfound.player",
            Vec::new(),
        );
        return crate::command_execution::CommandResult::Failed;
    }
    let count = targets.len() as i32;
    for target in targets {
        let resolved = match resolve_component(state, target, message) {
            Ok(tag) => tag,
            Err(message) => {
                error_text(state, connection, message);
                return crate::command_execution::CommandResult::Failed;
            }
        };
        let recipient = state.connections.entity_connection(target);
        if let Some(recipient) = recipient {
            state.push_player(
                recipient,
                crate::boundary::PlayerChange::StyledChat { content: resolved },
            );
        }
    }
    crate::command_execution::CommandResult::Success(count)
}

/// `/title`: resolve title/subtitle/actionbar components per target player and
/// send the matching `TitleChange`; `times`/`clear`/`reset` need no per-player
/// work. Feedback keys are exactly vanilla's.
pub(crate) fn execute_title(
    state: &mut TickState,
    connection: crate::boundary::ConnectionId,
    executor: EntityId,
    target: &crate::commands::Target,
    action: &TitleAction,
) -> crate::command_execution::CommandResult {
    let targets = crate::command_execution::player_targets(state, executor, target);
    if targets.is_empty() {
        crate::command_execution::feedback(
            state,
            connection,
            "argument.entity.notfound.player",
            Vec::new(),
        );
        return crate::command_execution::CommandResult::Failed;
    }
    match action {
        TitleAction::Clear { reset_times } => {
            for target in &targets {
                push_title(
                    state,
                    *target,
                    crate::boundary::TitleChange::Clear {
                        reset_times: *reset_times,
                    },
                );
            }
        }
        TitleAction::ShowTitle(component)
        | TitleAction::ShowSubtitle(component)
        | TitleAction::ShowActionBar(component) => {
            for target in &targets {
                let resolved = match resolve_component(state, *target, component) {
                    Ok(tag) => tag,
                    Err(message) => {
                        error_text(state, connection, message);
                        return crate::command_execution::CommandResult::Failed;
                    }
                };
                let change = match action {
                    TitleAction::ShowTitle(_) => crate::boundary::TitleChange::Title(resolved),
                    TitleAction::ShowSubtitle(_) => {
                        crate::boundary::TitleChange::Subtitle(resolved)
                    }
                    _ => crate::boundary::TitleChange::ActionBar(resolved),
                };
                push_title(state, *target, change);
            }
        }
        TitleAction::Times {
            fade_in,
            stay,
            fade_out,
        } => {
            for target in &targets {
                push_title(
                    state,
                    *target,
                    crate::boundary::TitleChange::Times {
                        fade_in: *fade_in,
                        stay: *stay,
                        fade_out: *fade_out,
                    },
                );
            }
        }
    }
    title_feedback(state, connection, &targets, action);
    crate::command_execution::CommandResult::Success(targets.len() as i32)
}

fn push_title(state: &mut TickState, target: EntityId, change: crate::boundary::TitleChange) {
    let connection = state.connections.entity_connection(target);
    if let Some(connection) = connection {
        state.push_player(connection, crate::boundary::PlayerChange::Title(change));
    }
}

fn title_feedback(
    state: &mut TickState,
    connection: crate::boundary::ConnectionId,
    targets: &[EntityId],
    action: &TitleAction,
) {
    let kind = match action {
        TitleAction::Clear { reset_times: false } => "cleared",
        TitleAction::Clear { reset_times: true } => "reset",
        TitleAction::ShowTitle(_) => "show.title",
        TitleAction::ShowSubtitle(_) => "show.subtitle",
        TitleAction::ShowActionBar(_) => "show.actionbar",
        TitleAction::Times { .. } => "times",
    };
    if targets.len() == 1 {
        let label = crate::command_execution::entity_name(state, targets[0]);
        crate::command_execution::feedback(
            state,
            connection,
            &format!("commands.title.{kind}.single"),
            vec![label],
        );
    } else {
        crate::command_execution::feedback(
            state,
            connection,
            &format!("commands.title.{kind}.multiple"),
            vec![text_tag(targets.len())],
        );
    }
}
fn error_text(state: &mut TickState, connection: crate::boundary::ConnectionId, message: String) {
    state.push_player(connection, crate::boundary::PlayerChange::Chat { message });
}

#[cfg(test)]
#[path = "command_text_tests.rs"]
mod tests;

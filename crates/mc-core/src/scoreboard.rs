//! Vanilla scoreboard and teams, owned by the v2 tick thread.
use crate::boundary::{ConnectionId, EntityId, PlayerChange};
use crate::command_execution::CommandResult;
use crate::commands::{CommandError, Target};
use crate::tick::TickState;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Components remain structured across persistence and packet encoding.
pub fn component(text: &str) -> mc_nbt::Tag {
    fn convert(value: serde_json::Value) -> mc_nbt::Tag {
        use mc_nbt::{ListTag, Tag};
        match value {
            serde_json::Value::String(value) => Tag::String(value),
            serde_json::Value::Bool(value) => Tag::Byte(i8::from(value)),
            serde_json::Value::Number(value) => Tag::Double(value.as_f64().unwrap_or(0.0)),
            serde_json::Value::Object(value) => Tag::Compound(
                value
                    .into_iter()
                    .map(|(key, value)| (key, convert(value)))
                    .collect(),
            ),
            serde_json::Value::Array(value) => {
                // Component lists accept strings and objects; normalize each
                // string to its equivalent compound for homogeneous NBT.
                let elements: Vec<_> = value
                    .into_iter()
                    .map(|value| match convert(value) {
                        Tag::String(text) => {
                            Tag::Compound(vec![("text".to_owned(), Tag::String(text))])
                        }
                        tag => tag,
                    })
                    .collect();
                Tag::List(ListTag {
                    element_type: mc_nbt::tag_type::COMPOUND,
                    elements,
                })
            }
            serde_json::Value::Null => Tag::String(String::new()),
        }
    }
    serde_json::from_str::<serde_json::Value>(text)
        .map_or_else(|_| mc_nbt::Tag::String(text.to_owned()), convert)
}

pub fn chat_component(state: &TickState, entity: EntityId, message: &str) -> mc_nbt::Tag {
    use mc_nbt::{ListTag, Tag};
    let name = state.connections.name_of(entity).unwrap_or("");
    let mut styled = vec![("text".to_owned(), Tag::String(name.to_owned()))];
    if let Some(team) = state.scoreboard.team_for(name) {
        if let Some(color) = usize::try_from(team.color).ok().and_then(|i| COLORS.get(i)) {
            styled.push(("color".to_owned(), Tag::String((*color).to_owned())));
        }
        // Prefix precedes the name, while suffix follows it.
        styled.remove(0);
        styled.push(("text".to_owned(), Tag::String(String::new())));
        let wrap = |tag| match tag {
            Tag::Compound(_) => tag,
            tag => Tag::Compound(vec![("text".to_owned(), tag)]),
        };
        styled.push((
            "extra".to_owned(),
            Tag::List(ListTag {
                element_type: mc_nbt::tag_type::COMPOUND,
                elements: vec![
                    wrap(component(&team.prefix)),
                    Tag::Compound(vec![("text".to_owned(), Tag::String(name.to_owned()))]),
                    wrap(component(&team.suffix)),
                ],
            }),
        ));
    }
    Tag::Compound(vec![
        ("text".to_owned(), Tag::String("<".to_owned())),
        (
            "extra".to_owned(),
            Tag::List(ListTag {
                element_type: mc_nbt::tag_type::COMPOUND,
                elements: vec![
                    Tag::Compound(styled),
                    Tag::Compound(vec![(
                        "text".to_owned(),
                        Tag::String(format!("> {message}")),
                    )]),
                ],
            }),
        ),
    ])
}

pub const COLORS: [&str; 16] = [
    "black",
    "dark_blue",
    "dark_green",
    "dark_aqua",
    "dark_red",
    "dark_purple",
    "gold",
    "gray",
    "dark_gray",
    "blue",
    "green",
    "aqua",
    "red",
    "light_purple",
    "yellow",
    "white",
];
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Team {
    pub display_name: String,
    pub color: i32,
    pub friendly_fire: bool,
    pub see_friendly_invisibles: bool,
    pub nametag_visibility: i32,
    pub death_message_visibility: i32,
    pub collision_rule: i32,
    pub prefix: String,
    pub suffix: String,
    pub members: BTreeSet<String>,
}
impl Team {
    fn new(name: &str, display_name: String) -> Self {
        Self {
            display_name: if display_name.is_empty() {
                name.to_owned()
            } else {
                display_name
            },
            color: 21,
            friendly_fire: true,
            see_friendly_invisibles: true,
            nametag_visibility: 0,
            death_message_visibility: 0,
            collision_rule: 0,
            prefix: String::new(),
            suffix: String::new(),
            members: BTreeSet::new(),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Objective {
    pub criteria: String,
    pub display_name: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Score {
    pub value: i32,
    pub locked: bool,
}
impl Default for Score {
    fn default() -> Self {
        Self {
            value: 0,
            locked: true,
        }
    }
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Scoreboard {
    pub teams: BTreeMap<String, Team>,
    pub objectives: BTreeMap<String, Objective>,
    pub scores: BTreeMap<String, BTreeMap<String, Score>>,
    pub display_slots: BTreeMap<i32, String>,
}
#[derive(Debug, Clone, PartialEq)]
pub enum ScoreboardEvent {
    Team {
        name: String,
        method: u8,
        team: Option<Team>,
        members: Vec<String>,
    },
    Objective {
        name: String,
        method: u8,
        objective: Option<Objective>,
    },
    Score {
        owner: String,
        objective: String,
        score: i32,
    },
    Reset {
        owner: String,
        objective: Option<String>,
    },
    Display {
        slot: i32,
        objective: String,
    },
}
#[derive(Debug, Clone, PartialEq)]
pub struct ScoreboardCommand {
    root: String,
    args: Vec<String>,
}
const USAGE: &str =
    "team <add|remove|empty|join|leave|list|modify> ... or scoreboard <objectives|players> ...";
impl ScoreboardCommand {
    pub(crate) fn supports_store_result(&self) -> bool {
        self.root == "scoreboard"
            && matches!(self.args.as_slice(), [players, operation, ..]
                if players == "players"
                    && matches!(operation.as_str(),
                        "get" | "set" | "add" | "remove" | "reset" | "operation"))
    }
    pub fn parse(root: &str, args: &[&str]) -> Result<Self, CommandError> {
        let valid = match (root, args) {
            ("team", ["add", _, ..]) => true,
            ("team", ["remove" | "empty" | "leave", _]) => true,
            ("team", ["join", _] | ["join", _, _]) => true,
            ("team", ["list"] | ["list", _]) => true,
            ("team", ["modify", _, "displayName" | "prefix" | "suffix", _, ..]) => true,
            (
                "team",
                [
                    "modify",
                    _,
                    "color"
                    | "friendlyFire"
                    | "seeFriendlyInvisibles"
                    | "nametagVisibility"
                    | "deathMessageVisibility"
                    | "collisionRule",
                    _,
                ],
            ) => true,
            (
                "scoreboard",
                [
                    "objectives",
                    "add",
                    _,
                    "dummy" | "trigger" | "deathCount" | "playerKillCount" | "totalKillCount"
                    | "health" | "food" | "air" | "armor" | "xp" | "level",
                    ..,
                ],
            ) => true,
            ("scoreboard", ["objectives", "add", _, criterion, ..])
                if criterion
                    .strip_prefix("teamkill.")
                    .or_else(|| criterion.strip_prefix("killedByTeam."))
                    .is_some_and(|color| COLORS.contains(&color)) =>
            {
                true
            }
            ("scoreboard", ["objectives", "remove", _] | ["objectives", "list"]) => true,
            (
                "scoreboard",
                ["objectives", "setdisplay", _] | ["objectives", "setdisplay", _, _],
            ) => true,
            ("scoreboard", ["players", "set" | "add" | "remove", _, _, n]) => {
                n.parse::<i32>().is_ok()
            }
            (
                "scoreboard",
                ["players", "reset", _]
                | ["players", "reset", _, _]
                | ["players", "enable", _, _]
                | ["players", "get", _, _]
                | ["players", "list"]
                | ["players", "list", _],
            ) => true,
            (
                "scoreboard",
                [
                    "players",
                    "operation",
                    _,
                    _,
                    "=" | "+=" | "-=" | "*=" | "/=" | "%=" | "<" | ">" | "><",
                    _,
                    _,
                ],
            ) => true,
            ("trigger", [_]) => true,
            ("trigger", [_, "add" | "set", value]) => value.parse::<i32>().is_ok(),
            _ => false,
        };
        if !valid {
            return Err(CommandError::Usage(USAGE));
        }
        Ok(Self {
            root: root.to_owned(),
            args: args.iter().map(|s| (*s).to_owned()).collect(),
        })
    }
}
impl Scoreboard {
    pub fn team_for(&self, name: &str) -> Option<&Team> {
        self.teams.values().find(|t| t.members.contains(name))
    }
    pub fn permits_damage(&self, attacker: &str, victim: &str) -> bool {
        self.team_for(attacker)
            .is_none_or(|team| team.friendly_fire || !team.members.contains(victim))
    }
    pub(crate) fn permits_death_message(&self, victim: &str, recipient: &str) -> bool {
        self.team_for(victim)
            .is_none_or(|team| match team.death_message_visibility {
                1 => false,
                2 => team.members.contains(recipient),
                3 => !team.members.contains(recipient),
                _ => true,
            })
    }
    pub fn snapshot_events(&self) -> Vec<ScoreboardEvent> {
        let mut events = Vec::new();
        for (name, team) in &self.teams {
            events.push(ScoreboardEvent::Team {
                name: name.clone(),
                method: 0,
                team: Some(team.clone()),
                members: team.members.iter().cloned().collect(),
            });
        }
        for (name, objective) in &self.objectives {
            events.push(ScoreboardEvent::Objective {
                name: name.clone(),
                method: 0,
                objective: Some(objective.clone()),
            });
        }
        for (owner, scores) in &self.scores {
            for (objective, score) in scores {
                events.push(ScoreboardEvent::Score {
                    owner: owner.clone(),
                    objective: objective.clone(),
                    score: score.value,
                });
            }
        }
        for (slot, objective) in &self.display_slots {
            events.push(ScoreboardEvent::Display {
                slot: *slot,
                objective: objective.clone(),
            });
        }
        events
    }
}
pub fn permits_player_damage(state: &TickState, attacker: EntityId, victim: EntityId) -> bool {
    match (
        state.connections.name_of(attacker),
        state.connections.name_of(victim),
    ) {
        (Some(a), Some(v)) => state.scoreboard.permits_damage(a, v),
        _ => true,
    }
}
pub fn sync_join(state: &mut TickState, connection: ConnectionId) {
    for event in state.scoreboard.snapshot_events() {
        state.push_player(connection, PlayerChange::Scoreboard { event });
    }
}
fn publish(state: &mut TickState, events: Vec<ScoreboardEvent>) {
    if events.is_empty() {
        return;
    }
    state.scoreboard_dirty = true;
    let connections: Vec<_> = state.connections.all_connections().collect();
    for event in events {
        for connection in &connections {
            state.push_player(
                *connection,
                PlayerChange::Scoreboard {
                    event: event.clone(),
                },
            );
        }
        if state.dimension_globals.active {
            state
                .dimension_globals
                .broadcasts
                .push(crate::dimensions::globals::Broadcast::Scoreboard(event));
        }
    }
}

/// Apply an execute callback through the scoreboard's persistence/broadcast owner.
pub(crate) fn store_result(state: &mut TickState, holders: &[String], objective: &str, value: i32) {
    let mut events = Vec::with_capacity(holders.len());
    for holder in holders {
        state
            .scoreboard
            .scores
            .entry(holder.clone())
            .or_default()
            .entry(objective.to_owned())
            .or_default()
            .value = value;
        events.push(ScoreboardEvent::Score {
            owner: holder.clone(),
            objective: objective.to_owned(),
            score: value,
        });
    }
    publish(state, events);
}
fn holders(state: &TickState, executor: EntityId, input: &str) -> Result<Vec<String>, String> {
    if input == "*" {
        return Ok(state.scoreboard.scores.keys().cloned().collect());
    }
    if !input.starts_with('@') {
        return Ok(vec![input.to_owned()]);
    }
    if input == "@a" && state.dimension_globals.active {
        let mut names: Vec<_> = state
            .dimension_globals
            .roster
            .iter()
            .map(|player| player.name.clone())
            .collect();
        for connection in state.connections.all_connections() {
            let entity = state
                .connections
                .entity_of(connection)
                .expect("joined entity");
            let name = state.connections.name_of(entity).expect("joined name");
            if !names.iter().any(|existing| existing == name) {
                names.push(name.to_owned());
            }
        }
        return Ok(names);
    }
    let target = match input {
        "@s" => Target::Executor,
        "@a" => Target::AllPlayers,
        "@p" | "@n" => Target::Nearest,
        _ => return Err("Unsupported player selector".to_owned()),
    };
    Ok(
        crate::command_execution::resolve_target(state, executor, &target)
            .into_iter()
            .filter_map(|id| state.connections.name_of(id).map(str::to_owned))
            .collect(),
    )
}
fn slot(name: &str) -> Option<i32> {
    match name {
        "list" => Some(0),
        "sidebar" => Some(1),
        "below_name" => Some(2),
        _ => name
            .strip_prefix("sidebar.team.")
            .and_then(|color| COLORS.iter().position(|c| *c == color))
            .map(|i| i as i32 + 3),
    }
}
pub(crate) fn execute(
    state: &mut TickState,
    executor: EntityId,
    command: &ScoreboardCommand,
) -> (String, CommandResult) {
    match execute_inner(state, executor, command) {
        Ok(reply) => reply,
        Err(message) => (message, CommandResult::Failed),
    }
}
fn execute_inner(
    state: &mut TickState,
    executor: EntityId,
    command: &ScoreboardCommand,
) -> Result<(String, CommandResult), String> {
    if command.root == "trigger" {
        return execute_trigger(state, executor, &command.args)
            .map(|message| (message, CommandResult::Completed));
    }
    let a = &command.args;
    let mut events = Vec::new();
    let message;
    if command.root == "team" {
        match a[0].as_str() {
            "add" => {
                if state.scoreboard.teams.contains_key(&a[1]) {
                    return Err("That team already exists".to_owned());
                }
                let team = Team::new(&a[1], a[2..].join(" "));
                events.push(ScoreboardEvent::Team {
                    name: a[1].clone(),
                    method: 0,
                    team: Some(team.clone()),
                    members: Vec::new(),
                });
                state.scoreboard.teams.insert(a[1].clone(), team);
                message = format!("Created team {}", a[1]);
            }
            "list" => {
                return Ok((
                    if a.len() == 1 {
                        format!(
                            "Teams: {}",
                            state
                                .scoreboard
                                .teams
                                .keys()
                                .cloned()
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    } else {
                        format!(
                            "Members: {}",
                            state
                                .scoreboard
                                .teams
                                .get(&a[1])
                                .ok_or("Unknown team")?
                                .members
                                .iter()
                                .cloned()
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    },
                    CommandResult::Completed,
                ));
            }
            "remove" => {
                state.scoreboard.teams.remove(&a[1]).ok_or("Unknown team")?;
                events.push(ScoreboardEvent::Team {
                    name: a[1].clone(),
                    method: 1,
                    team: None,
                    members: Vec::new(),
                });
                message = format!("Removed team {}", a[1]);
            }
            "empty" => {
                let team = state
                    .scoreboard
                    .teams
                    .get_mut(&a[1])
                    .ok_or("Unknown team")?;
                let members = std::mem::take(&mut team.members).into_iter().collect();
                events.push(ScoreboardEvent::Team {
                    name: a[1].clone(),
                    method: 4,
                    team: None,
                    members,
                });
                message = format!("Emptied team {}", a[1]);
            }
            "join" | "leave" => {
                let input = if a[0] == "join" {
                    a.get(2).map_or("@s", String::as_str)
                } else {
                    &a[1]
                };
                let members = holders(state, executor, input)?;
                if a[0] == "join" && !state.scoreboard.teams.contains_key(&a[1]) {
                    return Err("Unknown team".to_owned());
                }
                for member in &members {
                    for (name, team) in &mut state.scoreboard.teams {
                        if team.members.remove(member) {
                            events.push(ScoreboardEvent::Team {
                                name: name.clone(),
                                method: 4,
                                team: None,
                                members: vec![member.clone()],
                            });
                        }
                    }
                    if a[0] == "join" {
                        state
                            .scoreboard
                            .teams
                            .get_mut(&a[1])
                            .expect("validated team")
                            .members
                            .insert(member.clone());
                        events.push(ScoreboardEvent::Team {
                            name: a[1].clone(),
                            method: 3,
                            team: None,
                            members: vec![member.clone()],
                        });
                    }
                }
                message = format!("Updated team membership for {} players", members.len());
            }
            "modify" => {
                let team = state
                    .scoreboard
                    .teams
                    .get_mut(&a[1])
                    .ok_or("Unknown team")?;
                let value = a[3..].join(" ");
                match a[2].as_str() {
                    "displayName" => team.display_name = value,
                    "prefix" => team.prefix = value,
                    "suffix" => team.suffix = value,
                    "color" => {
                        team.color = if value == "reset" {
                            21
                        } else {
                            COLORS
                                .iter()
                                .position(|c| *c == value)
                                .ok_or("Unknown color")? as i32
                        }
                    }
                    "friendlyFire" => {
                        team.friendly_fire = value.parse().map_err(|_| "Expected true or false")?
                    }
                    "seeFriendlyInvisibles" => {
                        team.see_friendly_invisibles =
                            value.parse().map_err(|_| "Expected true or false")?
                    }
                    "nametagVisibility" | "deathMessageVisibility" => {
                        let visibility = ["always", "never", "hideForOtherTeams", "hideForOwnTeam"]
                            .iter()
                            .position(|s| *s == value)
                            .ok_or("Unknown visibility")?
                            as i32;
                        if a[2] == "nametagVisibility" {
                            team.nametag_visibility = visibility;
                        } else {
                            team.death_message_visibility = visibility;
                        }
                    }
                    "collisionRule" => {
                        team.collision_rule = ["always", "never", "pushOtherTeams", "pushOwnTeam"]
                            .iter()
                            .position(|s| *s == value)
                            .ok_or("Unknown collision rule")?
                            as i32
                    }
                    _ => return Err("Unknown team option".to_owned()),
                }
                events.push(ScoreboardEvent::Team {
                    name: a[1].clone(),
                    method: 2,
                    team: Some(team.clone()),
                    members: Vec::new(),
                });
                message = format!("Modified team {}", a[1]);
            }
            _ => return Err("Unknown team command".to_owned()),
        }
    } else if a[0] == "objectives" {
        match a[1].as_str() {
            "list" => {
                return Ok((
                    format!(
                        "Objectives: {}",
                        state
                            .scoreboard
                            .objectives
                            .keys()
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    CommandResult::Completed,
                ));
            }
            "add" => {
                if state.scoreboard.objectives.contains_key(&a[2]) {
                    return Err("That objective already exists".to_owned());
                }
                let objective = Objective {
                    criteria: a[3].clone(),
                    display_name: if a.len() > 4 {
                        a[4..].join(" ")
                    } else {
                        a[2].clone()
                    },
                };
                events.push(ScoreboardEvent::Objective {
                    name: a[2].clone(),
                    method: 0,
                    objective: Some(objective.clone()),
                });
                state.scoreboard.objectives.insert(a[2].clone(), objective);
                message = format!("Created objective {}", a[2]);
            }
            "remove" => {
                state
                    .scoreboard
                    .objectives
                    .remove(&a[2])
                    .ok_or("Unknown objective")?;
                for scores in state.scoreboard.scores.values_mut() {
                    scores.remove(&a[2]);
                }
                state
                    .scoreboard
                    .scores
                    .retain(|_, scores| !scores.is_empty());
                let slots: Vec<_> = state
                    .scoreboard
                    .display_slots
                    .iter()
                    .filter(|(_, n)| *n == &a[2])
                    .map(|(s, _)| *s)
                    .collect();
                for slot in slots {
                    state.scoreboard.display_slots.remove(&slot);
                    events.push(ScoreboardEvent::Display {
                        slot,
                        objective: String::new(),
                    });
                }
                events.push(ScoreboardEvent::Objective {
                    name: a[2].clone(),
                    method: 1,
                    objective: None,
                });
                message = format!("Removed objective {}", a[2]);
            }
            "setdisplay" => {
                let slot = slot(&a[2]).ok_or("Unknown display slot")?;
                let objective = a.get(3).cloned().unwrap_or_default();
                if !objective.is_empty() && !state.scoreboard.objectives.contains_key(&objective) {
                    return Err("Unknown objective".to_owned());
                }
                if objective.is_empty() {
                    state.scoreboard.display_slots.remove(&slot);
                } else {
                    state
                        .scoreboard
                        .display_slots
                        .insert(slot, objective.clone());
                }
                events.push(ScoreboardEvent::Display { slot, objective });
                message = "Updated display slot".to_owned();
            }
            _ => return Err("Unknown objective command".to_owned()),
        }
    } else {
        return execute_players(state, executor, a);
    }
    publish(state, events);
    Ok((message, CommandResult::Completed))
}

fn execute_trigger(
    state: &mut TickState,
    executor: EntityId,
    args: &[String],
) -> Result<String, String> {
    let name = state
        .connections
        .name_of(executor)
        .ok_or("Only players can trigger an objective")?;
    let objective = &args[0];
    if state
        .scoreboard
        .objectives
        .get(objective)
        .is_none_or(|value| value.criteria != "trigger")
    {
        return Err("That objective is not a trigger".into());
    }
    let score = state
        .scoreboard
        .scores
        .get_mut(name)
        .and_then(|scores| scores.get_mut(objective))
        .filter(|score| !score.locked)
        .ok_or("That trigger is not enabled")?;
    let amount = args
        .get(2)
        .map_or(Ok(1), |value| value.parse::<i32>())
        .map_err(|_| "Expected integer")?;
    score.value = if args.get(1).is_some_and(|operation| operation == "set") {
        amount
    } else {
        score.value.wrapping_add(amount)
    };
    score.locked = true;
    let value = score.value;
    let owner = name.to_owned();
    publish(
        state,
        vec![ScoreboardEvent::Score {
            owner,
            objective: objective.clone(),
            score: value,
        }],
    );
    Ok(format!("Triggered {objective} with value {value}"))
}
fn execute_players(
    state: &mut TickState,
    executor: EntityId,
    a: &[String],
) -> Result<(String, CommandResult), String> {
    if a[1] == "list" {
        return Ok((
            if let Some(owner) = a.get(2) {
                let names = holders(state, executor, owner)?;
                if names.len() != 1 {
                    return Err("Expected one score holder".to_owned());
                }
                format!(
                    "Scores for {}: {:?}",
                    names[0],
                    state
                        .scoreboard
                        .scores
                        .get(&names[0])
                        .cloned()
                        .unwrap_or_default()
                )
            } else {
                format!(
                    "Score holders: {}",
                    state
                        .scoreboard
                        .scores
                        .keys()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            },
            CommandResult::Completed,
        ));
    }
    let owners = holders(state, executor, &a[2])?;
    if owners.is_empty() {
        return Err("No score holders matched".to_owned());
    }
    if a[1] == "get" {
        if owners.len() != 1 {
            return Err("Expected one score holder".to_owned());
        }
        let score = state
            .scoreboard
            .scores
            .get(&owners[0])
            .and_then(|s| s.get(&a[3]))
            .ok_or("Score is not set")?;
        return Ok((
            format!("{} has {} [{}]", owners[0], score.value, a[3]),
            CommandResult::Success(score.value),
        ));
    }
    let objective = a.get(3);
    if let Some(objective) = objective {
        let definition = state
            .scoreboard
            .objectives
            .get(objective)
            .ok_or("Unknown objective")?;
        if a[1] == "enable" {
            if definition.criteria != "trigger" {
                return Err("That objective is not a trigger".into());
            }
        } else if a[1] != "reset"
            && matches!(
                definition.criteria.as_str(),
                "health" | "food" | "air" | "armor" | "xp" | "level"
            )
        {
            return Err("That objective is read-only".to_owned());
        }
    }
    let sources = if a[1] == "operation" {
        let source = holders(state, executor, &a[5])?;
        if source.is_empty() {
            return Err("No source score holders matched".to_owned());
        }
        if !state.scoreboard.objectives.contains_key(&a[6]) {
            return Err("Unknown source objective".to_owned());
        }
        if a[4] == "><"
            && matches!(
                state.scoreboard.objectives[&a[6]].criteria.as_str(),
                "health" | "food" | "air" | "armor" | "xp" | "level"
            )
        {
            return Err("Source objective is read-only".to_owned());
        }
        Some(source)
    } else {
        None
    };
    // Stage only affected holders so a failed arithmetic operation cannot
    // partially mutate authoritative scores or copy unrelated holders.
    let mut next = BTreeMap::new();
    for holder in owners.iter().chain(sources.iter().flatten()) {
        next.entry(holder.clone()).or_insert_with(|| {
            state
                .scoreboard
                .scores
                .get(holder)
                .cloned()
                .unwrap_or_default()
        });
    }
    let mut events = Vec::new();
    let mut total = 0_i32;
    for owner in &owners {
        if a[1] == "reset" {
            if let Some(obj) = objective {
                if let Some(scores) = next.get_mut(owner) {
                    scores.remove(obj);
                }
            } else {
                next.get_mut(owner).expect("staged holder").clear();
            }
            events.push(ScoreboardEvent::Reset {
                owner: owner.clone(),
                objective: objective.cloned(),
            });
            continue;
        }
        let obj = &a[3];
        if a[1] == "enable" {
            let score = next
                .get_mut(owner)
                .expect("staged holder")
                .entry(obj.clone())
                .or_default();
            score.locked = false;
            events.push(ScoreboardEvent::Score {
                owner: owner.clone(),
                objective: obj.clone(),
                score: score.value,
            });
            continue;
        }
        let mut value = next
            .get(owner)
            .and_then(|s| s.get(obj))
            .map(|score| score.value)
            .unwrap_or(0);
        if let Some(sources) = &sources {
            for source in sources {
                let rhs = &mut next
                    .entry(source.clone())
                    .or_default()
                    .entry(a[6].clone())
                    .or_default()
                    .value;
                let old = value;
                value = match a[4].as_str() {
                    "=" => *rhs,
                    "+=" => value.wrapping_add(*rhs),
                    "-=" => value.wrapping_sub(*rhs),
                    "*=" => value.wrapping_mul(*rhs),
                    "/=" => {
                        if *rhs == 0 {
                            return Err("Cannot divide by zero".to_owned());
                        }
                        let (left, right) = (i64::from(value), i64::from(*rhs));
                        let quotient = left / right;
                        let remainder = left % right;
                        (quotient - i64::from(remainder != 0 && (remainder < 0) != (right < 0)))
                            as i32
                    }
                    "%=" => {
                        if *rhs == 0 {
                            return Err("Cannot divide by zero".to_owned());
                        }
                        let (left, right) = (i64::from(value), i64::from(*rhs));
                        let remainder = left % right;
                        (if remainder != 0 && (remainder < 0) != (right < 0) {
                            remainder + right
                        } else {
                            remainder
                        }) as i32
                    }
                    "<" => value.min(*rhs),
                    ">" => value.max(*rhs),
                    "><" => {
                        let result = *rhs;
                        *rhs = old;
                        result
                    }
                    _ => return Err("Unknown operation".to_owned()),
                };
                events.push(ScoreboardEvent::Score {
                    owner: source.clone(),
                    objective: a[6].clone(),
                    score: *rhs,
                });
                next.entry(owner.clone())
                    .or_default()
                    .entry(obj.clone())
                    .or_default()
                    .value = value;
            }
        } else {
            let amount: i32 = a[4].parse().map_err(|_| "Expected integer")?;
            if a[1] != "set" && amount < 0 {
                return Err("Expected nonnegative integer".to_owned());
            }
            value = match a[1].as_str() {
                "set" => amount,
                "add" => value.wrapping_add(amount),
                "remove" => value.wrapping_sub(amount),
                _ => return Err("Unknown players command".to_owned()),
            };
        }
        total = total.wrapping_add(value);
        next.entry(owner.clone())
            .or_default()
            .entry(obj.clone())
            .or_default()
            .value = value;
        events.push(ScoreboardEvent::Score {
            owner: owner.clone(),
            objective: obj.clone(),
            score: value,
        });
    }
    for (holder, scores) in next {
        if scores.is_empty() {
            state.scoreboard.scores.remove(&holder);
        } else {
            state.scoreboard.scores.insert(holder, scores);
        }
    }
    publish(state, events);
    let result = match a[1].as_str() {
        "reset" => CommandResult::Success(owners.len() as i32),
        "enable" => CommandResult::Completed,
        _ => CommandResult::Success(total),
    };
    Ok((
        format!("Updated scores for {} holders", owners.len()),
        result,
    ))
}
/// Read-only criteria updated from authoritative player state, not client input.
pub fn automatic(state: &mut TickState, deaths: &[EntityId], kills: &[(EntityId, EntityId)]) {
    let mut events = Vec::new();
    let definitions: Vec<_> = state
        .scoreboard
        .objectives
        .iter()
        .filter(|(_, o)| !matches!(o.criteria.as_str(), "dummy" | "trigger"))
        .map(|(n, o)| (n.clone(), o.criteria.clone()))
        .collect();
    for (objective, criteria) in definitions {
        for (id, player) in &state.players {
            let Some(name) = state.connections.name_of(*id) else {
                continue;
            };
            let old = state
                .scoreboard
                .scores
                .get(name)
                .and_then(|s| s.get(&objective))
                .map(|score| score.value);
            let value = match criteria.as_str() {
                "health" => (player.vitals.health + player.active_effects.absorption).ceil() as i32,
                "food" => player.vitals.food,
                "air" => player.vitals.air,
                "xp" => player.vitals.experience.total,
                "level" => player.vitals.experience.level,
                "armor" => {
                    let rules = state.registry.item_rules();
                    player.inventory.slots[36..40]
                        .iter()
                        .filter(|stack| !stack.is_empty())
                        .filter_map(|stack| rules.facts.facts_for_stack(stack, rules.items))
                        .filter_map(|facts| facts.armor.as_ref())
                        .map(|armor| armor.armor)
                        .sum::<f32>()
                        .floor() as i32
                }
                "deathCount" if deaths.contains(id) => old.unwrap_or(0).wrapping_add(1),
                "playerKillCount" | "totalKillCount" => {
                    let count = kills
                        .iter()
                        .filter(|(killer, victim)| {
                            killer == id
                                && (criteria == "totalKillCount"
                                    || state.players.contains_key(victim))
                        })
                        .count() as i32;
                    if count == 0 {
                        continue;
                    }
                    old.unwrap_or(0).wrapping_add(count)
                }
                criterion
                    if criterion.starts_with("teamkill.")
                        || criterion.starts_with("killedByTeam.") =>
                {
                    let (prefix, color) = criterion.split_once('.').expect("team criterion");
                    let color = COLORS
                        .iter()
                        .position(|candidate| *candidate == color)
                        .expect("validated team color") as i32;
                    let count = kills
                        .iter()
                        .filter(|(killer, victim)| {
                            let other = if prefix == "teamkill" {
                                if killer != id {
                                    return false;
                                }
                                victim
                            } else {
                                if victim != id {
                                    return false;
                                }
                                killer
                            };
                            state
                                .connections
                                .name_of(*other)
                                .and_then(|name| state.scoreboard.team_for(name))
                                .is_some_and(|team| team.color == color)
                        })
                        .count() as i32;
                    if count == 0 {
                        continue;
                    }
                    old.unwrap_or(0).wrapping_add(count)
                }
                _ => continue,
            };
            if old != Some(value) {
                state
                    .scoreboard
                    .scores
                    .entry(name.to_owned())
                    .or_default()
                    .entry(objective.clone())
                    .or_default()
                    .value = value;
                events.push(ScoreboardEvent::Score {
                    owner: name.to_owned(),
                    objective: objective.clone(),
                    score: value,
                });
            }
        }
    }
    publish(state, events);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_slots_cover_vanilla_slots_and_team_colors() {
        assert_eq!(slot("list"), Some(0));
        assert_eq!(slot("sidebar"), Some(1));
        assert_eq!(slot("below_name"), Some(2));
        for (index, color) in COLORS.iter().enumerate() {
            assert_eq!(
                slot(&format!("sidebar.team.{color}")),
                Some(index as i32 + 3)
            );
        }
        assert_eq!(slot("sidebar.team.reset"), None);
    }

    #[test]
    fn friendly_fire_only_blocks_members_of_the_same_team() {
        let mut board = Scoreboard::default();
        let mut team = Team::new("allies", String::new());
        team.friendly_fire = false;
        team.members.extend(["Alice".to_owned(), "Bob".to_owned()]);
        board.teams.insert("allies".to_owned(), team);
        assert!(!board.permits_damage("Alice", "Bob"));
        assert!(board.permits_damage("Alice", "Eve"));
        assert!(board.permits_damage("Eve", "Bob"));
    }

    #[test]
    fn parser_rejects_incomplete_and_unknown_command_shapes() {
        for (root, args) in [
            ("team", vec!["modify", "red", "friendlyFire"]),
            ("team", vec!["modify", "red", "unknown", "true"]),
            ("scoreboard", vec!["objectives", "add", "wins", "unknown"]),
            ("scoreboard", vec!["players", "set", "Alice", "wins", "bad"]),
            (
                "scoreboard",
                vec!["players", "operation", "Alice", "wins", "?", "Bob", "wins"],
            ),
        ] {
            assert!(ScoreboardCommand::parse(root, &args).is_err());
        }
    }
}

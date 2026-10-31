//! Native `/settlement` registry/membership commands and household relocation.
//!
//! Parse lives here so `commands::parse` stays a thin root dispatch; execution
//! lives in `command_execution` next to every other native command. Owner
//! identity is the connection UUID string, never the display name; the console
//! owns as `"Server"`. Membership itself stays in villager columns via the
//! parent-owned `settlements::{bind,unbind,remove,member_count}` helpers.

use crate::commands::CommandError;
use crate::settlements::SettlementRecord;

#[cfg(test)]
#[path = "commands_tests.rs"]
mod tests;

/// Command-only create default (block units). The registry owns the 1..=256
/// bound; this default is the UX choice for a bare `create <name>`.
pub const DEFAULT_RADIUS: u32 = 48;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EconomyDomain {
    Production,
    Commerce,
    Payroll,
    Caravan,
}

/// One parsed `/settlement` subcommand. Coordinates are absolute block
/// positions; the executor pose supplies the create anchor.
#[derive(Debug, Clone, PartialEq)]
pub enum SettlementCommand {
    Create {
        name: String,
        radius: u32,
    },
    List,
    /// Claim the nearest autonomous settlement containing the player.
    Claim,
    Info {
        name: Option<String>,
    },
    Rename {
        name: String,
        new_name: String,
    },
    Remove {
        name: String,
    },
    Bind {
        settlement: String,
        villager: Option<String>,
    },
    Unbind {
        villager: Option<String>,
    },
    Relocate {
        settlement: String,
        household: String,
        destination: String,
    },
    Economy {
        domain: EconomyDomain,
        settlement: String,
        arguments: Vec<String>,
    },
    BuildingCertify {
        kind: String,
        min: [i32; 3],
        max: [i32; 3],
    },
    BuildingUpgrade {
        id: u64,
    },
    BuildingList,
    /// Start or queue an owner blueprint project in the executor's settlement.
    /// The reserved `cancel` argument clears only its pending owner order.
    Build {
        blueprint: String,
    },
    /// Start or queue a blueprint upgrade for a globally addressed building,
    /// authorized against the enclosing settlement. Builders later apply
    /// geometry and debit storage; `building upgrade` only recertifies blocks.
    BuildUpgrade {
        id: u64,
    },
    /// Native soldiers in the executor's enclosing settlement.
    Army {
        command: ArmyCommand,
    },
    /// Open the settlement dialogs (optionally on a named page).
    Menu {
        page: Option<String>,
    },
}

/// One parsed `/settlement army ...` action. The executor's enclosing
/// settlement is the target; follow/guard resolve the live executor pose at
/// execution, so the parser only validates the order word.
#[derive(Debug, Clone, PartialEq)]
pub enum ArmyCommand {
    List,
    Recruit,
    Dismiss {
        soldier: String,
    },
    Order {
        target: String,
        order: ArmyOrderKind,
    },
}

/// Order word for `/settlement army order <all|uuid> <word>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArmyOrderKind {
    Follow,
    Guard,
    Patrol,
    Return,
}

pub fn parse(args: &[&str]) -> Result<SettlementCommand, CommandError> {
    let usage: &'static str = "settlement <guide|create|claim|list|info|rename|remove|bind|unbind|relocate|production|commerce|payroll|caravan|building|build|army>";
    let [sub, rest @ ..] = args else {
        return Ok(SettlementCommand::Menu { page: None });
    };
    match *sub {
        "claim" if rest.is_empty() => Ok(SettlementCommand::Claim),
        "menu" => match rest {
            [] => Ok(SettlementCommand::Menu { page: None }),
            [page] => Ok(SettlementCommand::Menu {
                page: Some((*page).to_owned()),
            }),
            _ => Err(CommandError::Usage(usage)),
        },
        "create" => match rest {
            [name] => Ok(SettlementCommand::Create {
                name: (*name).to_owned(),
                radius: DEFAULT_RADIUS,
            }),
            [name, radius] => {
                let radius: u32 = radius.parse().map_err(|_| CommandError::Usage(usage))?;
                if !(1..=super::registry::MAX_RADIUS).contains(&radius) {
                    return Err(CommandError::Usage(usage));
                }
                Ok(SettlementCommand::Create {
                    name: (*name).to_owned(),
                    radius,
                })
            }
            _ => Err(CommandError::Usage(usage)),
        },
        "list" => {
            if rest.is_empty() {
                Ok(SettlementCommand::List)
            } else {
                Err(CommandError::Usage(usage))
            }
        }
        "info" => match rest {
            [] => Ok(SettlementCommand::Info { name: None }),
            [name] => Ok(SettlementCommand::Info {
                name: Some((*name).to_owned()),
            }),
            _ => Err(CommandError::Usage(usage)),
        },
        "rename" => match rest {
            [name, new_name] => Ok(SettlementCommand::Rename {
                name: (*name).to_owned(),
                new_name: (*new_name).to_owned(),
            }),
            _ => Err(CommandError::Usage(usage)),
        },
        "remove" => match rest {
            [name] => Ok(SettlementCommand::Remove {
                name: (*name).to_owned(),
            }),
            _ => Err(CommandError::Usage(usage)),
        },
        "bind" => match rest {
            [settlement] => Ok(SettlementCommand::Bind {
                settlement: (*settlement).to_owned(),
                villager: None,
            }),
            [settlement, villager] => Ok(SettlementCommand::Bind {
                settlement: (*settlement).to_owned(),
                villager: Some((*villager).to_owned()),
            }),
            _ => Err(CommandError::Usage(usage)),
        },
        "unbind" => match rest {
            [] => Ok(SettlementCommand::Unbind { villager: None }),
            [villager] => Ok(SettlementCommand::Unbind {
                villager: Some((*villager).to_owned()),
            }),
            _ => Err(CommandError::Usage(usage)),
        },
        "relocate" => match rest {
            [settlement, household, destination] => Ok(SettlementCommand::Relocate {
                settlement: (*settlement).to_owned(),
                household: (*household).to_owned(),
                destination: (*destination).to_owned(),
            }),
            _ => Err(CommandError::Usage(usage)),
        },
        "production" | "commerce" | "payroll" | "caravan" => {
            let [settlement, _operation, ..] = rest else {
                return Err(CommandError::Usage(usage));
            };
            let domain = match *sub {
                "production" => EconomyDomain::Production,
                "commerce" => EconomyDomain::Commerce,
                "payroll" => EconomyDomain::Payroll,
                "caravan" => EconomyDomain::Caravan,
                _ => unreachable!(),
            };
            Ok(SettlementCommand::Economy {
                domain,
                settlement: (*settlement).to_owned(),
                arguments: rest[1..].iter().map(|word| (*word).to_owned()).collect(),
            })
        }
        "building" => match rest {
            ["certify", kind, x1, y1, z1, x2, y2, z2] => {
                let parse_triplet = |words: [&str; 3]| -> Result<[i32; 3], CommandError> {
                    let mut out = [0i32; 3];
                    for (index, word) in words.iter().enumerate() {
                        out[index] = word.parse().map_err(|_| CommandError::Usage(usage))?;
                    }
                    Ok(out)
                };
                let min = parse_triplet([x1, y1, z1])?;
                let max = parse_triplet([x2, y2, z2])?;
                Ok(SettlementCommand::BuildingCertify {
                    kind: (*kind).to_owned(),
                    min,
                    max,
                })
            }
            ["upgrade", id] => {
                let id: u64 = id.parse().map_err(|_| CommandError::Usage(usage))?;
                Ok(SettlementCommand::BuildingUpgrade { id })
            }
            ["list"] => Ok(SettlementCommand::BuildingList),
            _ => Err(CommandError::Usage(usage)),
        },
        "build" => match rest {
            [blueprint] => Ok(SettlementCommand::Build {
                blueprint: (*blueprint).to_owned(),
            }),
            ["upgrade", id] => {
                let id: u64 = id.parse().map_err(|_| CommandError::Usage(usage))?;
                Ok(SettlementCommand::BuildUpgrade { id })
            }
            _ => Err(CommandError::Usage(usage)),
        },
        "army" => match rest {
            ["list"] => Ok(SettlementCommand::Army {
                command: ArmyCommand::List,
            }),
            ["recruit"] => Ok(SettlementCommand::Army {
                command: ArmyCommand::Recruit,
            }),
            ["dismiss", soldier] => Ok(SettlementCommand::Army {
                command: ArmyCommand::Dismiss {
                    soldier: (*soldier).to_owned(),
                },
            }),
            ["order", target, order] => {
                let order = match *order {
                    "follow" => ArmyOrderKind::Follow,
                    "guard" => ArmyOrderKind::Guard,
                    "patrol" => ArmyOrderKind::Patrol,
                    "return" => ArmyOrderKind::Return,
                    _ => return Err(CommandError::Usage(usage)),
                };
                Ok(SettlementCommand::Army {
                    command: ArmyCommand::Order {
                        target: (*target).to_owned(),
                        order,
                    },
                })
            }
            _ => Err(CommandError::Usage(usage)),
        },
        _ => Err(CommandError::Usage(usage)),
    }
}

/// Borrow metadata only; economic manifests and receipts are never cloned by
/// registry commands. Mutation callers detach the small identity they need.
pub(crate) fn lookup<'a>(
    state: &'a crate::tick::TickState,
    name: &str,
) -> Option<&'a SettlementRecord> {
    state.settlements.find(name)
}

/// Shared admission for command and dialog claim requests. Console and remote
/// dialog payloads cannot bypass the live player-position fence.
pub(crate) fn claim(
    state: &mut crate::tick::TickState,
    connection: crate::boundary::ConnectionId,
    target: Option<super::SettlementId>,
) -> Result<String, &'static str> {
    let owner = state
        .connections
        .uuid_of(connection)
        .ok_or("Only an in-world player can claim a settlement.")?
        .to_string();
    let entity = state
        .connections
        .entity_of(connection)
        .ok_or("Only an in-world player can claim a settlement.")?;
    let slot = state
        .entities
        .slot_of(entity)
        .ok_or("Player position is unavailable.")?;
    let at = state.entities.pos(slot).map(|value| value.floor() as i32);
    let id = target
        .or_else(|| {
            state
                .settlements
                .records()
                .filter(|record| record.contains(at))
                .min_by_key(|record| {
                    at.iter()
                        .zip(record.anchor)
                        .map(|(a, b)| (i64::from(*a) - i64::from(b)).pow(2))
                        .sum::<i64>()
                })
                .map(|record| record.id)
        })
        .ok_or("Stand inside an autonomous settlement to claim it.")?;
    state.settlements.claim(id, owner, at)?;
    super::refresh_owner_names(state);
    let name = &state.settlements.get(id).expect("claimed settlement").name;
    Ok(format!("Claimed {name}."))
}

use super::*;

#[derive(Deserialize)]
#[serde(untagged)]
pub(super) enum RawBlockDrops {
    Simple(RawDropList),
    Contextual(RawContextualDrops),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawContextualDrops {
    sequence: String,
    #[serde(default)]
    silk_touch: Option<String>,
    #[serde(default)]
    tool: Option<RawToolDrop>,
    drops: Vec<RawChanceDrop>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawToolDrop {
    item: String,
    drop: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawChanceDrop {
    item: String,
    #[serde(default = "one")]
    min: u32,
    #[serde(default = "one")]
    max: u32,
    #[serde(default)]
    chance: Option<f32>,
    #[serde(default)]
    fortune_chances: Vec<f32>,
    #[serde(default)]
    fortune_multiplier: Option<u32>,
    #[serde(default)]
    survives_explosion: bool,
    #[serde(default)]
    explosion_decay: bool,
}

const fn one() -> u32 {
    1
}

pub(super) fn parse_blocks(
    path: &Path,
    blocks: BTreeMap<String, RawBlockDrops>,
) -> Result<LoadedBlockLoot, LootError> {
    let mut drops = BTreeMap::new();
    let mut rules = BTreeMap::new();
    for (id, raw) in blocks {
        let id = parse_id(path, id)?;
        match raw {
            RawBlockDrops::Simple(raw) => {
                drops.insert(
                    id,
                    raw.into_items()
                        .into_iter()
                        .map(|item| parse_id(path, item).map(LootDrop::single))
                        .collect::<Result<_, _>>()?,
                );
            }
            RawBlockDrops::Contextual(raw) => {
                let silk_touch_drops = raw
                    .silk_touch
                    .map(|item| {
                        parse_id(path, item).map(|id| BlockLootDrop::plain(LootDrop::single(id)))
                    })
                    .transpose()?
                    .into_iter()
                    .collect();
                let tool_drops = raw
                    .tool
                    .map(|tool| -> Result<_, LootError> {
                        Ok((
                            parse_id(path, tool.item)?,
                            vec![BlockLootDrop::plain(LootDrop::single(parse_id(
                                path, tool.drop,
                            )?))],
                        ))
                    })
                    .transpose()?;
                let regular_drops = raw
                    .drops
                    .into_iter()
                    .map(|raw| -> Result<_, LootError> {
                        let mut drop = BlockLootDrop::plain(LootDrop {
                            item: parse_id(path, raw.item)?,
                            count: if raw.min == raw.max {
                                LootCount::Fixed(raw.min)
                            } else {
                                LootCount::UniformInclusive {
                                    min: raw.min,
                                    max: raw.max,
                                }
                            },
                        });
                        drop.random_chance = raw.chance;
                        drop.fortune_chances = raw.fortune_chances;
                        drop.fortune_bonus = raw.fortune_multiplier.map(|bonus_multiplier| {
                            FortuneBonus::UniformBonusCount { bonus_multiplier }
                        });
                        drop.survives_explosion = raw.survives_explosion;
                        drop.explosion_decay = raw.explosion_decay;
                        Ok(drop)
                    })
                    .collect::<Result<_, _>>()?;
                rules.insert(
                    id,
                    BlockLoot {
                        random_sequence: Some(parse_id(path, raw.sequence)?),
                        silk_touch_drops,
                        tool_drops,
                        regular_drops,
                        conditional_pools: Vec::new(),
                    },
                );
            }
        }
    }
    Ok(LoadedBlockLoot { drops, rules })
}

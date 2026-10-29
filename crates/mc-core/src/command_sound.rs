//! Positional sound command delivery.
use crate::command_context::Context;
use crate::command_execution::CommandResult;
use crate::tick::TickState;

pub(crate) fn execute(state: &mut TickState, context: Context, input: &str) -> CommandResult {
    match run(state, context, input) {
        Ok(count) => CommandResult::Success(count),
        Err(message) => {
            state.push_player(
                context.connection,
                crate::boundary::PlayerChange::Chat { message },
            );
            CommandResult::Failed
        }
    }
}

fn run(state: &mut TickState, context: Context, input: &str) -> Result<i32, String> {
    let words: Vec<_> = input.split_whitespace().collect();
    if words.len() < 2 || words.len() > 11 {
        return Err(
            "Expected playsound <sound> [source] [targets] [pos] [volume] [pitch] [minVolume]"
                .into(),
        );
    }
    let sound = mc_data::Identifier::parse(words[1]).map_err(|e| e.to_string())?;
    let source = match words.get(2).copied().unwrap_or("master") {
        "master" => 0,
        "music" => 1,
        "record" => 2,
        "weather" => 3,
        "block" => 4,
        "hostile" => 5,
        "neutral" => 6,
        "player" => 7,
        "ambient" => 8,
        "voice" => 9,
        _ => return Err("Invalid sound source".into()),
    };
    let target = crate::commands::target_of(words.get(3).copied().unwrap_or("@s"))
        .map_err(|e| e.message())?;
    let position = if words.len() > 4 {
        if words.len() < 7 {
            return Err("Expected three sound coordinates".into());
        }
        let coords = crate::commands::coords(&words[4..7]).ok_or("Invalid coordinates")?;
        crate::commands::resolve_coords(
            coords,
            context.coordinate_origin(coords[0].local),
            context.rotation,
        )
    } else {
        context.position
    };
    let number = |index: usize, default: f32, min: f32, max: f32| -> Result<f32, String> {
        let value = match words.get(index) {
            Some(value) => value
                .parse::<f32>()
                .map_err(|_| "Invalid sound parameter")?,
            None => default,
        };
        if !value.is_finite() || value < min || value > max {
            return Err("Invalid sound parameter".into());
        }
        Ok(value)
    };
    let volume = number(7, 1.0, 0.0, f32::MAX)?;
    let pitch = number(8, 1.0, 0.0, 2.0)?;
    let minimum = number(9, 0.0, 0.0, 1.0)?;
    if words.len() > 10
        || position
            .iter()
            .any(|n| !n.is_finite() || *n < i32::MIN as f64 / 8.0 || *n > i32::MAX as f64 / 8.0)
    {
        return Err("Invalid sound position or trailing arguments".into());
    }
    let targets = crate::dimensions::travel::context_targets(state, context, &target)?;
    let mut count = 0;
    for entity in targets {
        let Some((dimension, slot)) = crate::dimensions::travel::locate(state, entity) else {
            continue;
        };
        let world = if dimension == state.dimension {
            &mut *state
        } else {
            state
                .other_dimensions
                .get_mut(&dimension)
                .ok_or("Unknown dimension")?
        };
        let Some(connection) = world.connections.entity_connection(entity) else {
            continue;
        };
        let listener = world.entities.pos(slot);
        let offset = std::array::from_fn::<_, 3, _>(|i| position[i] - listener[i]);
        let distance = offset.iter().map(|v| v * v).sum::<f64>().sqrt();
        let (location, effective_volume) = if distance > f64::from(volume.max(1.0) * 16.0) {
            if minimum == 0.0 {
                continue;
            }
            (
                std::array::from_fn(|i| listener[i] + offset[i] / distance * 2.0),
                minimum,
            )
        } else {
            (position, volume)
        };
        world.push_player(
            connection,
            crate::boundary::PlayerChange::CommandSound {
                sound: sound.clone(),
                source,
                position: location.map(|v| (v * 8.0).floor() as i32),
                volume: effective_volume,
                pitch,
            },
        );
        count += 1;
    }
    if count == 0 {
        return Err("No player can hear the sound".into());
    }
    Ok(count)
}

type InputState = { press: number, release: number }
local inputs: { [number]: InputState } = {}

local function input_state(player_id: number): InputState
    if inputs[player_id] == nil then
        inputs[player_id] = { press = 0, release = 0 }
    end
    return inputs[player_id]
end

local function input_status(state: InputState): string
    return "Sapphire input status: key=" .. state.press .. "/" .. state.release .. "."
end

function on_player_command(event)
    if event.root ~= "loader_sapphire" then
        return
    end
    local mode = string.match(event.arguments, "^%s*(%S*)%s*$")
    if mode == "" then
        solaris.grant_loader_block_item("grant-sapphire", event.player_id, "sapphire-live:sapphire_block", 1)
        solaris.present_client_ui(event.player_id, "sapphire-live:showcase", { mode = "screen" })
    elseif mode == "hud" or mode == "update" then
        solaris.present_client_ui(event.player_id, "sapphire-live:showcase", {
            mode = "hud",
            title = "Sapphire HUD",
            body = mode == "update" and "Sapphire HUD updated." or "Sapphire HUD active.",
        })
        solaris.send_message(event.player_id, "Sapphire UI request: " .. mode .. ".")
    elseif mode == "hide" then
        solaris.present_client_ui(event.player_id, "sapphire-live:showcase", { mode = "hidden" })
        solaris.send_message(event.player_id, "Sapphire UI request: hide.")
    elseif mode == "input_status" then
        solaris.send_message(event.player_id, input_status(input_state(event.player_id)))
    elseif mode == "sound" then
        solaris.play_client_sound(event.player_id, "sapphire-live:tone", {})
        solaris.send_message(event.player_id, "Sapphire sound: sound.")
    elseif mode == "sound_stop" then
        solaris.stop_client_sound(event.player_id, "sapphire-live:tone")
        solaris.send_message(event.player_id, "Sapphire sound: sound_stop.")
    else
        solaris.send_message(event.player_id, "Usage: /loader_sapphire [hud|update|hide|input_status]")
    end
end

function on_loader_interaction(event)
    if event.interaction_id == "sapphire-live:confirm" and event.phase == "trigger" then
        solaris.send_message(event.player_id, "Sapphire Loader interaction reached sapphire-live.")
    elseif event.interaction_id == "sapphire-live:key"
        and (event.phase == "press" or event.phase == "release") then
        local state = input_state(event.player_id)
        state[event.phase] = state[event.phase] + 1
        solaris.send_message(event.player_id,
            "Sapphire key " .. event.phase .. " #" .. state[event.phase] .. ".")
        solaris.present_client_ui(event.player_id, "sapphire-live:showcase", {
            mode = "hud", title = "Sapphire input", body = input_status(state),
        })
    end
end

function on_player_left(event)
    inputs[event.player_id] = nil
end

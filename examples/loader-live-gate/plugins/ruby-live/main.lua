type Counts = { press: number, release: number }
type InputState = { key: Counts, jump: Counts, escape: Counts, f2: Counts, f11: Counts, modal: boolean }
local inputs: { [number]: InputState } = {}

local function input_state(player_id: number): InputState
    if inputs[player_id] == nil then
        inputs[player_id] = {
            key = { press = 0, release = 0 }, jump = { press = 0, release = 0 }, modal = false,
            escape = { press = 0, release = 0 }, f2 = { press = 0, release = 0 },
            f11 = { press = 0, release = 0 },
        }
    end
    return inputs[player_id]
end

local function input_status(state: InputState): string
    return "Ruby input status: key=" .. state.key.press .. "/" .. state.key.release
        .. " jump=" .. state.jump.press .. "/" .. state.jump.release .. "."
end

function on_player_command(event)
    if event.root ~= "loader_ruby" then
        return
    end
    local x, y, z = string.match(event.arguments, "^sound_world%s+(%S+)%s+(%S+)%s+(%S+)$")
    if x and y and z then
        solaris.play_client_sound(event.player_id, "ruby-live:tone", {
            position = { x = tonumber(x), y = tonumber(y), z = tonumber(z) },
        })
        solaris.send_message(event.player_id, "Ruby sound: world.")
        return
    end
    local mode = string.match(event.arguments, "^%s*(%S*)%s*$")
    if mode == "" then
        solaris.grant_loader_block_item("grant-ruby", event.player_id, "ruby-live:ruby_block", 1)
        solaris.present_client_ui(event.player_id, "ruby-live:showcase", { mode = "screen" })
    elseif mode == "hud" or mode == "update" then
        input_state(event.player_id).modal = false
        solaris.present_client_ui(event.player_id, "ruby-live:showcase", {
            mode = "hud",
            title = "Ruby HUD",
            body = mode == "update" and "Ruby HUD updated." or "Ruby HUD active.",
        })
        solaris.send_message(event.player_id, "Ruby UI request: " .. mode .. ".")
    elseif mode == "hide" then
        solaris.present_client_ui(event.player_id, "ruby-live:showcase", { mode = "hidden" })
        solaris.send_message(event.player_id, "Ruby UI request: hide.")
    elseif mode == "input_status" then
        solaris.send_message(event.player_id, input_status(input_state(event.player_id)))
    elseif mode == "edge_status" then
        local state = input_state(event.player_id)
        solaris.send_message(event.player_id,
            "Ruby edge status: escape=" .. state.escape.press .. "/" .. state.escape.release
                .. " f2=" .. state.f2.press .. "/" .. state.f2.release
                .. " f11=" .. state.f11.press .. "/" .. state.f11.release .. ".")
    elseif mode == "input_modal" then
        input_state(event.player_id).modal = true
        solaris.send_message(event.player_id, "Ruby input modal armed.")
    elseif mode == "sound" or mode == "sound_quiet" or mode == "sound_pitch" then
        solaris.play_client_sound(event.player_id, "ruby-live:tone", {
            volume = mode == "sound_quiet" and 0.25 or 1,
            pitch = mode == "sound_pitch" and 1.5 or 1,
        })
        solaris.send_message(event.player_id, "Ruby sound: " .. mode .. ".")
    elseif mode == "sound_stop" or mode == "sound_foreign_stop" then
        solaris.stop_client_sound(event.player_id,
            mode == "sound_foreign_stop" and "sapphire-live:tone" or "ruby-live:tone")
        solaris.send_message(event.player_id, "Ruby sound: " .. mode .. ".")
    else
        solaris.send_message(event.player_id, "Usage: /loader_ruby [hud|update|hide|input_status|input_modal|edge_status]")
    end
end

function on_loader_interaction(event)
    if event.interaction_id == "ruby-live:confirm" and event.phase == "trigger" then
        solaris.send_message(event.player_id, "Ruby Loader interaction reached ruby-live.")
    elseif (event.interaction_id == "ruby-live:key" or event.interaction_id == "ruby-live:jump"
        or event.interaction_id == "ruby-live:escape" or event.interaction_id == "ruby-live:f2"
        or event.interaction_id == "ruby-live:f11")
        and (event.phase == "press" or event.phase == "release") then
        local state = input_state(event.player_id)
        local kind = string.sub(event.interaction_id, 11)
        state[kind][event.phase] = state[kind][event.phase] + 1
        solaris.send_message(event.player_id,
            "Ruby " .. kind .. " " .. event.phase .. " #" .. state[kind][event.phase] .. ".")
        if state.modal then
            if event.phase == "press" then
                solaris.present_client_ui(event.player_id, "ruby-live:showcase", { mode = "screen" })
            end
        else
            solaris.present_client_ui(event.player_id, "ruby-live:showcase", {
                mode = "hud", title = "Ruby input", body = input_status(state),
            })
        end
    end
end

function on_player_left(event)
    inputs[event.player_id] = nil
end

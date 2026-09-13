local function showcase_model()
    return {
        page = 0,
        page_count = 1,
        rows = { { cells = { "Sapphire Loader Fixture" } } },
        actions = { { action_id = "sapphire-live:confirm", enabled = true, label = "Confirm Sapphire" } },
        markers = { { marker_id = "anchor" } },
    }
end

function on_player_command(event)
    if event.root ~= "loader_sapphire" then
        return
    end
    solaris.grant_loader_block_item("grant-sapphire", event.player_id, "sapphire-live:sapphire_block", 1)
    solaris.open_client_view("sapphire-view", event.player_id, "sapphire-live:showcase", showcase_model())
end

function on_loader_view_action(event)
    if event.action_id == "sapphire-live:confirm" then
        solaris.send_message(event.player_id, "Sapphire Loader view action reached sapphire-live.")
    end
end

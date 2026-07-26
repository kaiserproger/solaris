function on_player_command(event)
    if event.root ~= "loader_sapphire" then
        return
    end
    solaris.grant_loader_block_item(event.player_id, "sapphire-live:sapphire_block", 1)
    solaris.open_client_screen(event.player_id, "sapphire-live:showcase")
end

function on_loader_interaction(event)
    if event.interaction_id == "sapphire-live:confirm" then
        solaris.send_message(event.player_id, "Sapphire Loader interaction reached sapphire-live.")
    end
end

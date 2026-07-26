function on_player_command(event)
    if event.root ~= "loader_ruby" then
        return
    end
    solaris.grant_loader_block_item(event.player_id, "ruby-live:ruby_block", 1)
    solaris.open_client_screen(event.player_id, "ruby-live:showcase")
end

function on_loader_interaction(event)
    if event.interaction_id == "ruby-live:confirm" then
        solaris.send_message(event.player_id, "Ruby Loader interaction reached ruby-live.")
    end
end

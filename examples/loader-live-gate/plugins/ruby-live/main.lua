local function showcase_model()
    return {
        page = 0,
        page_count = 1,
        rows = { { cells = { "Ruby Loader Fixture" } } },
        actions = { { action_id = "ruby-live:confirm", enabled = true, label = "Confirm Ruby" } },
        markers = { { marker_id = "anchor" } },
    }
end

function on_player_command(event)
    if event.root ~= "loader_ruby" then
        return
    end
    solaris.grant_loader_block_item("grant-ruby", event.player_id, "ruby-live:ruby_block", 1)
    solaris.open_client_view("ruby-view", event.player_id, "ruby-live:showcase", showcase_model())
end

function on_loader_view_action(event)
    if event.action_id == "ruby-live:confirm" then
        solaris.send_message(event.player_id, "Ruby Loader view action reached ruby-live.")
    end
end

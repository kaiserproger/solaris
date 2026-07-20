-- Script API 0.6 acceptance fixture.
--
-- FAIL-CLOSED PRODUCTION GAP: mc-net currently has no adapter for the atomic
-- InventoryStorageTransaction or inventory.storage_transaction.result. Menu
-- and zone commands/events are live. The fixture will not mutate its ledger
-- without an exact targeted transaction result.

local config = {
    shop_id = "market",
    menu_prefix = "catalog",
    currency = {
        resource = "minecraft:emerald",
        singular = "Emerald",
        plural = "Emeralds",
    },
    zone = {
        id = "catalog-square",
        dimension = "minecraft:overworld",
        minimum = { x = -8, y = 60, z = -8 },
        maximum = { x = 8, y = 100, z = 8 },
    },
    catalog = {
        { resource = "minecraft:apple", count = 2, label = "Apples", price = 3 },
        { resource = "minecraft:bread", count = 1, label = "Bread", price = 5 },
        { resource = "minecraft:cooked_beef", count = 2, label = "Steak", price = 8 },
        { resource = "minecraft:iron_pickaxe", count = 1, label = "Iron Pickaxe", price = 18 },
    },
    max_active_players = 64,
    max_purchases_per_item = 999999,
}

local pending_reads = {}
local pending_read_by_player = {}
local pending_transactions = {}
local ledgers = {}
local active_menus = {}
local deferred_notices = {}
local last_batch = nil

local function table_size(values)
    local count = 0
    for _ in pairs(values) do
        count = count + 1
    end
    return count
end

local function validate_config()
    assert(#config.catalog > 0 and #config.catalog <= 16, "catalog must contain 1..16 entries")
    local resources = { [config.currency.resource] = true }
    for _, item in ipairs(config.catalog) do
        assert(item.resource ~= config.currency.resource, "catalog item cannot be the shop currency")
        assert(not resources[item.resource], "catalog resources must be unique")
        assert(item.count >= 1 and item.count <= 64, "catalog item count must be 1..64")
        assert(item.price >= 1 and item.price <= 64, "catalog price must be 1..64")
        resources[item.resource] = true
    end
end

validate_config()

local function ledger_key(uuid)
    return "shop:" .. config.shop_id .. ":" .. uuid
end

local function empty_counts()
    local counts = {}
    for index = 1, #config.catalog do
        counts[index] = 0
    end
    return counts
end

local function copy_counts(source)
    local counts = {}
    for index = 1, #config.catalog do
        counts[index] = source[index]
    end
    return counts
end

local function encode_counts(counts)
    local encoded = {}
    for index = 1, #config.catalog do
        encoded[index] = tostring(counts[index])
    end
    return "v1|" .. table.concat(encoded, ",")
end

local function decode_counts(value)
    if value == nil then
        return empty_counts()
    end
    if string.sub(value, 1, 3) ~= "v1|" then
        return nil
    end

    local counts = {}
    for token in string.gmatch(string.sub(value, 4), "([^,]+)") do
        if not string.match(token, "^%d+$") then
            return nil
        end
        local count = tonumber(token)
        if count == nil or count > config.max_purchases_per_item then
            return nil
        end
        counts[#counts + 1] = count
    end
    if #counts ~= #config.catalog then
        return nil
    end
    return counts
end

local function currency_name(amount)
    if amount == 1 then
        return config.currency.singular
    end
    return config.currency.plural
end

local function menu_id_for(version)
    if version == nil then
        return config.menu_prefix .. "-new"
    end
    return config.menu_prefix .. "-v" .. tostring(version)
end

local function remember_notice(player_id, message)
    deferred_notices[player_id] = message
end

local function send_message(player_id, message)
    if last_batch == nil then
        last_batch = { kind = "message", player_id = player_id }
    end
    solaris.send_message(player_id, message)
end

local function queue_read(player_id, uuid, request_id, key)
    if pending_read_by_player[player_id] ~= nil then
        return false
    end
    local pending = {
        player_id = player_id,
        uuid = uuid,
        key = key,
    }
    pending_reads[request_id] = pending
    pending_read_by_player[player_id] = request_id
    last_batch = { kind = "read", request_id = request_id, player_id = player_id }
    solaris.storage_get(request_id, key)
    return true
end

local function open_catalog(player_id)
    local ledger = ledgers[player_id]
    if ledger == nil then
        return
    end

    local slots = {}
    for index, item in ipairs(config.catalog) do
        slots[index] = {
            slot = index - 1,
            resource = item.resource,
            count = item.count,
            label = item.label
                .. " | buy " .. item.price .. " " .. currency_name(item.price)
                .. " | refund | owned " .. ledger.counts[index],
        }
    end

    local menu_id = menu_id_for(ledger.version)
    active_menus[player_id] = menu_id
    last_batch = { kind = "open", player_id = player_id, menu_id = menu_id }
    solaris.open_inventory_menu(
        player_id,
        menu_id,
        "Market - " .. config.currency.plural,
        slots
    )
end

local function clear_player(player_id)
    local read_id = pending_read_by_player[player_id]
    if read_id ~= nil then
        pending_reads[read_id] = nil
        pending_read_by_player[player_id] = nil
    end
    for request_id, pending in pairs(pending_transactions) do
        if pending.player_id == player_id then
            pending_transactions[request_id] = nil
        end
    end
    ledgers[player_id] = nil
    active_menus[player_id] = nil
    deferred_notices[player_id] = nil
end

function on_server_started(_event)
    last_batch = { kind = "zone_registration" }
    local zone = config.zone
    solaris.upsert_zone(
        zone.id,
        zone.dimension,
        zone.minimum.x,
        zone.minimum.y,
        zone.minimum.z,
        zone.maximum.x,
        zone.maximum.y,
        zone.maximum.z
    )
end

function on_player_zone_entered(event)
    last_batch = nil
    if event.zone_id ~= config.zone.id then
        return
    end
    if active_menus[event.player_id] ~= nil
        or pending_read_by_player[event.player_id] ~= nil
    then
        return
    end
    for _, pending in pairs(pending_transactions) do
        if pending.player_id == event.player_id then
            return
        end
    end
    if ledgers[event.player_id] == nil
        and table_size(ledgers) + table_size(pending_reads) >= config.max_active_players
    then
        send_message(event.player_id, "Catalog unavailable: active-player limit reached.")
        return
    end

    local notice = deferred_notices[event.player_id]
    if notice ~= nil then
        deferred_notices[event.player_id] = nil
        send_message(event.player_id, notice)
    end
    queue_read(
        event.player_id,
        event.uuid,
        "enter-" .. tostring(event.player_id),
        ledger_key(event.uuid)
    )
end

function on_plugin_storage_get_result(event)
    last_batch = nil
    local pending = pending_reads[event.request_id]
    if pending == nil then
        return
    end
    pending_reads[event.request_id] = nil
    pending_read_by_player[pending.player_id] = nil

    if event.key ~= pending.key then
        send_message(pending.player_id, "Catalog unavailable: storage result mismatch.")
        return
    end
    if event.failure ~= nil then
        send_message(
            pending.player_id,
            "Catalog unavailable: storage " .. event.failure .. "."
        )
        return
    end

    local counts = decode_counts(event.value)
    if counts == nil then
        send_message(pending.player_id, "Catalog unavailable: invalid ledger record.")
        return
    end
    ledgers[pending.player_id] = {
        uuid = pending.uuid,
        key = pending.key,
        version = event.version,
        counts = counts,
    }
    open_catalog(pending.player_id)
end

function on_inventory_menu_clicked(event)
    last_batch = nil
    local ledger = ledgers[event.player_id]
    if ledger == nil or active_menus[event.player_id] ~= event.menu_id then
        return
    end
    local notice = deferred_notices[event.player_id]
    if notice ~= nil then
        deferred_notices[event.player_id] = nil
        send_message(event.player_id, notice)
    end
    if event.click ~= "primary" and event.click ~= "secondary" then
        return
    end

    local item_index = event.slot + 1
    local item = config.catalog[item_index]
    if item == nil then
        return
    end
    for _, pending in pairs(pending_transactions) do
        if pending.player_id == event.player_id then
            return
        end
    end

    local operation = event.click == "primary" and "buy" or "refund"
    if operation == "refund" and ledger.counts[item_index] == 0 then
        send_message(event.player_id, "Nothing from this shop is eligible for refund.")
        return
    end

    local next_counts = copy_counts(ledger.counts)
    local inventory
    if operation == "buy" then
        next_counts[item_index] = next_counts[item_index] + 1
        inventory = {
            { resource = config.currency.resource, delta = -item.price },
            { resource = item.resource, delta = item.count },
        }
    else
        next_counts[item_index] = next_counts[item_index] - 1
        inventory = {
            { resource = config.currency.resource, delta = item.price },
            { resource = item.resource, delta = -item.count },
        }
    end

    local revision = ledger.version == nil and "new" or tostring(ledger.version)
    local request_id = operation
        .. "-" .. tostring(event.player_id)
        .. "-" .. revision
        .. "-" .. tostring(event.slot)
    pending_transactions[request_id] = {
        player_id = event.player_id,
        uuid = event.uuid,
        key = ledger.key,
        operation = operation,
        item_label = item.label,
        next_counts = next_counts,
        old_menu_id = event.menu_id,
    }
    active_menus[event.player_id] = nil
    last_batch = {
        kind = "transaction",
        request_id = request_id,
        player_id = event.player_id,
        old_menu_id = event.menu_id,
    }
    solaris.close_inventory_menu(event.player_id, event.menu_id)
    solaris.inventory_storage_transaction(
        event.player_id,
        request_id,
        inventory,
        {
            {
                operation = "cas",
                key = ledger.key,
                expected_version = ledger.version,
                value = encode_counts(next_counts),
            },
        }
    )
end

function on_inventory_storage_transaction_result(event)
    last_batch = nil
    local pending = pending_transactions[event.request_id]
    if pending == nil then
        return
    end
    pending_transactions[event.request_id] = nil
    ledgers[pending.player_id] = nil

    if event.committed then
        send_message(
            pending.player_id,
            pending.operation == "buy"
                and ("Purchased " .. pending.item_label .. ".")
                or ("Refunded " .. pending.item_label .. ".")
        )
    else
        send_message(
            pending.player_id,
            "Transaction rejected: inventory or storage precondition changed."
        )
    end

    queue_read(
        pending.player_id,
        pending.uuid,
        "refresh-" .. event.request_id,
        pending.key
    )
end

function on_player_left(event)
    last_batch = nil
    clear_player(event.player_id)
end

function on_command_batch_rejected(result)
    local batch = last_batch
    last_batch = nil
    if batch == nil then
        return
    end

    if batch.kind == "read" then
        pending_reads[batch.request_id] = nil
        pending_read_by_player[batch.player_id] = nil
        remember_notice(batch.player_id, "Catalog request rejected: " .. result.reason .. ".")
    elseif batch.kind == "open" then
        active_menus[batch.player_id] = nil
        remember_notice(batch.player_id, "Catalog menu rejected: " .. result.reason .. ".")
    elseif batch.kind == "transaction" then
        pending_transactions[batch.request_id] = nil
        active_menus[batch.player_id] = batch.old_menu_id
        remember_notice(batch.player_id, "Catalog transaction rejected: " .. result.reason .. ".")
    elseif batch.kind == "message" then
        remember_notice(batch.player_id, "Catalog response rejected: " .. result.reason .. ".")
    end
end

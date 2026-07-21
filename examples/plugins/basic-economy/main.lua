local config = solaris.config()
local wallet_key_prefix = "wallet:"
local request_sequence = 0
local pending_reads = {}
local pending_cas = {}
local pending_purchases = {}
local wallets = {}
local active_menus = {}
local last_batch = nil

local function validate_config()
    assert(math.type(config.starting_balance) == "integer")
    assert(config.starting_balance >= 0 and config.starting_balance <= 999999999)
    assert(type(config.currency_name) == "string" and #config.currency_name > 0)
    assert(#config.currency_name <= 64)
    assert(type(config.products) == "table" and #config.products >= 1 and #config.products <= 16)
    local resources = {}
    for _, product in ipairs(config.products) do
        assert(type(product.resource) == "string")
        assert(string.match(product.resource, "^[a-z0-9_.-]+:[a-z0-9_./-]+$") ~= nil)
        assert(not resources[product.resource])
        resources[product.resource] = true
        assert(math.type(product.count) == "integer" and product.count >= 1 and product.count <= 64)
        assert(math.type(product.price) == "integer" and product.price >= 1 and product.price <= 999999999)
        assert(type(product.label) == "string" and #product.label >= 1 and #product.label <= 96)
    end
end

validate_config()

local function next_request(prefix)
    request_sequence = request_sequence + 1
    return prefix .. "-" .. tostring(request_sequence)
end

local function wallet_key(uuid)
    return wallet_key_prefix .. uuid
end

local function decode_balance(value)
    if value == nil then
        return config.starting_balance
    end
    if string.match(value, "^%d+$") == nil then
        return nil
    end
    local balance = tonumber(value)
    if balance == nil or balance > 999999999 then
        return nil
    end
    return balance
end

local function read_wallet(event, action, amount)
    local request_id = next_request("wallet")
    pending_reads[request_id] = {
        player_id = event.player_id,
        uuid = event.uuid,
        action = action,
        amount = amount,
        key = wallet_key(event.uuid),
    }
    last_batch = { kind = "read", request_id = request_id }
    solaris.storage_get(request_id, wallet_key(event.uuid))
end

local function open_shop(player_id, wallet)
    local slots = {}
    for index, product in ipairs(config.products) do
        slots[index] = {
            slot = index - 1,
            resource = product.resource,
            count = product.count,
            label = product.label .. " | " .. tostring(product.price) .. " " .. config.currency_name,
        }
    end
    local menu_id = "economy-" .. (wallet.version == nil and "new" or tostring(wallet.version))
    active_menus[player_id] = menu_id
    last_batch = { kind = "open", player_id = player_id }
    solaris.open_inventory_menu(
        player_id,
        menu_id,
        "Balance: " .. tostring(wallet.balance) .. " " .. config.currency_name,
        slots
    )
end

function on_player_command(event)
    last_batch = nil
    if event.root ~= "economy" then
        return
    end
    local command, amount = string.match(event.arguments, "^%s*(%S*)%s*(%d*)%s*$")
    if command == nil then
        solaris.send_message(event.player_id, "Usage: /economy [balance|shop|grant <amount>]")
        return
    end
    if command == "" or command == "shop" then
        read_wallet(event, "shop", nil)
    elseif command == "balance" then
        read_wallet(event, "balance", nil)
    elseif command == "grant" and event.operator and amount ~= "" then
        local value = tonumber(amount)
        if value == nil or value < 1 or value > 100000 then
            solaris.send_message(event.player_id, "Grant amount must be 1..100000.")
            return
        end
        read_wallet(event, "grant", value)
    else
        solaris.send_message(event.player_id, "Usage: /economy [balance|shop|grant <amount>]")
    end
end

function on_plugin_storage_get_result(event)
    last_batch = nil
    local pending = pending_reads[event.request_id]
    if pending == nil then
        return
    end
    pending_reads[event.request_id] = nil
    if event.failure ~= nil or event.key ~= pending.key then
        solaris.send_message(pending.player_id, "Economy storage is unavailable.")
        return
    end
    local balance = decode_balance(event.value)
    if balance == nil then
        solaris.send_message(pending.player_id, "Economy wallet is corrupt; ask an operator.")
        return
    end
    local wallet = { key = pending.key, version = event.version, balance = balance, uuid = pending.uuid }
    wallets[pending.player_id] = wallet
    if pending.action == "shop" then
        open_shop(pending.player_id, wallet)
    elseif pending.action == "balance" then
        solaris.send_message(
            pending.player_id,
            "Balance: " .. tostring(balance) .. " " .. config.currency_name .. "."
        )
    elseif pending.action == "grant" then
        local next_balance = balance + pending.amount
        if next_balance > 999999999 then
            solaris.send_message(pending.player_id, "Balance limit reached.")
            return
        end
        local request_id = next_request("grant")
        pending_cas[request_id] = {
            player_id = pending.player_id,
            uuid = pending.uuid,
            amount = pending.amount,
        }
        last_batch = { kind = "cas", request_id = request_id }
        solaris.storage_cas(request_id, pending.key, event.version, tostring(next_balance))
    end
end

function on_plugin_storage_cas_result(event)
    last_batch = nil
    local pending = pending_cas[event.request_id]
    if pending == nil then
        return
    end
    pending_cas[event.request_id] = nil
    if event.failure ~= nil or not event.applied then
        solaris.send_message(pending.player_id, "Balance changed concurrently; retry.")
        return
    end
    solaris.send_message(
        pending.player_id,
        "Granted " .. tostring(pending.amount) .. " " .. config.currency_name .. "."
    )
end

function on_inventory_menu_clicked(event)
    last_batch = nil
    local wallet = wallets[event.player_id]
    if wallet == nil or active_menus[event.player_id] ~= event.menu_id or event.click ~= "primary" then
        return
    end
    local product = config.products[event.slot + 1]
    if product == nil then
        return
    end
    if wallet.balance < product.price then
        solaris.send_message(event.player_id, "Insufficient " .. config.currency_name .. ".")
        return
    end
    local request_id = next_request("buy")
    pending_purchases[request_id] = {
        player_id = event.player_id,
        uuid = event.uuid,
        label = product.label,
    }
    active_menus[event.player_id] = nil
    last_batch = {
        kind = "purchase",
        request_id = request_id,
        player_id = event.player_id,
        menu_id = event.menu_id,
    }
    solaris.close_inventory_menu(event.player_id, event.menu_id)
    solaris.inventory_storage_transaction(
        event.player_id,
        request_id,
        { { resource = product.resource, delta = product.count } },
        {
            {
                operation = "cas",
                key = wallet.key,
                expected_version = wallet.version,
                value = tostring(wallet.balance - product.price),
            },
        }
    )
end

function on_inventory_storage_transaction_result(event)
    last_batch = nil
    local pending = pending_purchases[event.request_id]
    if pending == nil then
        return
    end
    pending_purchases[event.request_id] = nil
    wallets[pending.player_id] = nil
    if event.committed then
        solaris.send_message(pending.player_id, "Purchased " .. pending.label .. ".")
    else
        solaris.send_message(pending.player_id, "Purchase rejected; balance or inventory changed.")
    end
end

function on_player_left(event)
    last_batch = nil
    wallets[event.player_id] = nil
    active_menus[event.player_id] = nil
end

function on_command_batch_rejected(_result)
    local batch = last_batch
    last_batch = nil
    if batch == nil then
        return
    end
    if batch.kind == "read" then
        pending_reads[batch.request_id] = nil
    elseif batch.kind == "cas" then
        pending_cas[batch.request_id] = nil
    elseif batch.kind == "open" then
        active_menus[batch.player_id] = nil
    elseif batch.kind == "purchase" then
        pending_purchases[batch.request_id] = nil
        active_menus[batch.player_id] = batch.menu_id
    end
end

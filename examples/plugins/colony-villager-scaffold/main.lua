--!strict

-- Script API 0.6 acceptance fixture.
--
-- Production supports colony registration, ephemeral villager binding, and
-- home/hold orders through the regional entity owner. Roles remain plugin
-- metadata; API 0.6 exposes no durable entity handle, inventory, or arbitrary
-- villager memory access.

local config: any = {
    colony = {
        id = "starter-colony",
        name = "Starter Colony",
        dimension = "minecraft:overworld",
        home = { x = 0, y = 64, z = 0 },
    },
    zone = {
        id = "starter-colony-zone",
        minimum = { x = -16, y = 50, z = -16 },
        maximum = { x = 16, y = 100, z = 16 },
    },
    binding_radius = 16,
    default_role = "worker",
    default_order = "home",
    roles = { "worker", "builder", "farmer", "guard" },
    orders = { "home", "hold" },
    max_active_players = 64,
    max_generation = 999999,
}

local role_allowed: any = {}
local order_allowed: any = {}
for _, role in ipairs(config.roles) do
    role_allowed[role] = true
end
for _, order in ipairs(config.orders) do
    order_allowed[order] = true
end

local colony_request_pending = false
local colony_outcome: string = "not_started"
local pending_gets: any = {}
local pending_get_by_player: any = {}
local pending_cas: any = {}
local pending_bindings: any = {}
local pending_orders: any = {}
local active_bindings: any = {}
local records: any = {}
local zone_seen: any = {}
local deferred_notices: any = {}
local last_batch: any = nil

local function table_size(values: any): number
    local count = 0
    for _ in pairs(values) do
        count = count + 1
    end
    return count
end

local function binding_key(uuid: string): string
    return "villager:" .. uuid
end

local function colony_key()
    return "colony:" .. config.colony.id
end

local function encode_record(record: any): string
    return table.concat({
        "v1",
        record.status,
        record.role,
        record.order,
        tostring(record.generation),
    }, "|")
end

local function decode_record(value: any): (any, string?)
    if value == nil then
        return nil, nil
    end
    local status, role, order, generation = string.match(
        value,
        "^v1|([a-z_]+)|([a-z_]+)|([a-z_]+)|(%d+)$"
    )
    local generation_number = tonumber(generation)
    if (status ~= "recruiting" and status ~= "active" and status ~= "rejected")
        or not role_allowed[role]
        or not order_allowed[order]
        or generation_number == nil
        or generation_number > config.max_generation
    then
        return nil, "invalid"
    end
    return {
        status = status,
        role = role,
        order = order,
        generation = generation_number,
    }, nil
end

local function copy_record(record: any): any
    return {
        status = record.status,
        role = record.role,
        order = record.order,
        generation = record.generation,
    }
end

local function remember_notice(player_id: number?, message: string)
    if player_id ~= nil then
        deferred_notices[player_id] = message
    end
end

local function send_message(player_id: number, message: string)
    if last_batch == nil then
        last_batch = { kind = "message", player_id = player_id }
    end
    solaris.send_message(player_id, message)
end

local function send_notice(player_id: number)
    local notice = deferred_notices[player_id]
    if notice ~= nil then
        deferred_notices[player_id] = nil
        send_message(player_id, notice)
    end
end

local function publish_colony_ready()
    colony_outcome = "ready"
    last_batch = { kind = "message" }
    solaris.broadcast("Colony plugin ready.")
end

local function action_request_id(prefix: string, player_id: number, version: any): string
    return prefix
        .. "-" .. tostring(player_id)
        .. "-" .. (version == nil and "new" or tostring(version))
end

local function queue_get(player_id: number?, uuid: string?, action: string, argument: string?, request_id: string, key: string): boolean
    if player_id ~= nil and pending_get_by_player[player_id] ~= nil then
        return false
    end
    pending_gets[request_id] = {
        player_id = player_id,
        uuid = uuid,
        action = action,
        argument = argument,
        key = key,
    }
    if player_id ~= nil then
        pending_get_by_player[player_id] = request_id
    end
    last_batch = { kind = "get", request_id = request_id, player_id = player_id }
    solaris.storage_get(request_id, key)
    return true
end

local function queue_cas(player_id: number?, uuid: string?, key: string, expected_version: any, next_record: any, after: any, prefix: string)
    local request_id = action_request_id(prefix, player_id or 0, expected_version)
    pending_cas[request_id] = {
        player_id = player_id,
        uuid = uuid,
        key = key,
        expected_version = expected_version,
        next_record = next_record,
        after = after,
    }
    last_batch = { kind = "cas", request_id = request_id, player_id = player_id }
    solaris.storage_cas(request_id, key, expected_version, encode_record(next_record))
end

local function queue_binding(player_id: number, uuid: string, key: string, version: any, record: any, x: number, y: number, z: number, purpose: string)
    local request_id = action_request_id("bind", player_id, version)
    pending_bindings[request_id] = {
        player_id = player_id,
        uuid = uuid,
        key = key,
        version = version,
        record = record,
        purpose = purpose,
    }
    last_batch = { kind = "binding", request_id = request_id, player_id = player_id }
    solaris.bind_nearest_villager(
        request_id,
        config.colony.id,
        x,
        y,
        z,
        config.binding_radius
    )
end

local function queue_order(pending: any, binding_token: string)
    local request_id = action_request_id("order", pending.player_id, pending.version)
    pending.binding_token = binding_token
    pending_orders[request_id] = pending
    last_batch = { kind = "order", request_id = request_id, player_id = pending.player_id }
    solaris.set_villager_order(
        request_id,
        config.colony.id,
        binding_token,
        pending.record.order
    )
end

local function split_arguments(arguments: string): any
    local values = {}
    for value in string.gmatch(arguments, "%S+") do
        if #values == 3 then
            return nil
        end
        values[#values + 1] = value
    end
    return values
end

local function status_message(record: any): string
    if colony_outcome == "record_pending" then
        return "Colony unavailable: waiting for colony.record_result from UpsertColony."
    end
    if colony_outcome == "record_rejected" then
        return "Colony unavailable: UpsertColony was rejected."
    end
    if colony_outcome ~= "ready" then
        return "Colony unavailable: state=" .. colony_outcome .. "."
    end
    if record == nil then
        return "No villager is recruited for this player."
    end
    if record.status == "recruiting" then
        return "Recruitment pending: waiting for colony.villager_binding_result from RequestVillagerBinding."
    end
    if record.status == "rejected" then
        return "Recruitment rejected or unavailable; API 0.6 does not distinguish the cause."
    end
    return "Recruited villager: role metadata=" .. record.role
        .. ", stored order intent=" .. record.order .. "."
end

local function handle_player_state(pending: any, value: any, version: any)
    local player_id = pending.player_id
    local record, decode_error = decode_record(value)
    if decode_error ~= nil then
        send_message(player_id, "Colony state rejected: invalid durable record.")
        return
    end
    records[player_id] = record == nil and nil or {
        uuid = pending.uuid,
        key = pending.key,
        version = version,
        value = record,
    }

    if pending.action == "status" then
        send_message(player_id, status_message(record))
        return
    end

    if colony_outcome ~= "ready" then
        send_message(player_id, status_message(record))
        return
    end

    if pending.action == "recruit" then
        if record ~= nil and record.status == "active" then
            send_message(player_id, "Recruitment ignored: a villager is already active.")
            return
        end
        local role = pending.argument or config.default_role
        if not role_allowed[role] then
            send_message(player_id, "Recruitment rejected: unsupported role.")
            return
        end
        if record ~= nil and record.status == "recruiting" then
            queue_binding(
                player_id,
                pending.uuid,
                pending.key,
                version,
                record,
                pending.x,
                pending.y,
                pending.z,
                "recruit"
            )
            return
        end
        local next_record = {
            status = "recruiting",
            role = role,
            order = config.default_order,
            generation = record == nil and 1 or record.generation + 1,
        }
        queue_cas(
            player_id,
            pending.uuid,
            pending.key,
            version,
            next_record,
            {
                kind = "bind",
                x = pending.x,
                y = pending.y,
                z = pending.z,
            },
            "recruit"
        )
        return
    end

    if record == nil or record.status ~= "active" then
        send_message(player_id, "Update rejected: recruit an active villager first.")
        return
    end

    local next_record = copy_record(record)
    next_record.generation = next_record.generation + 1
    if next_record.generation > config.max_generation then
        send_message(player_id, "Update rejected: generation limit reached.")
        return
    end
    if pending.action == "role" then
        if not role_allowed[pending.argument] then
            send_message(player_id, "Role rejected: expected worker, builder, farmer, or guard.")
            return
        end
        next_record.role = pending.argument
    elseif pending.action == "order" then
        if not order_allowed[pending.argument] then
            send_message(player_id, "Order rejected: expected home or hold.")
            return
        end
        next_record.order = pending.argument
    else
        send_message(player_id, "Colony command rejected: unsupported action.")
        return
    end
    queue_cas(
        player_id,
        pending.uuid,
        pending.key,
        version,
        next_record,
        pending.action == "order"
            and {
                kind = "apply_order",
                x = pending.x,
                y = pending.y,
                z = pending.z,
            }
            or { kind = "updated", field = pending.action },
        pending.action
    )
end

local function clear_player(player_id: number)
    local get_id = pending_get_by_player[player_id]
    if get_id ~= nil then
        pending_gets[get_id] = nil
        pending_get_by_player[player_id] = nil
    end
    for request_id, pending in pairs(pending_cas) do
        if pending.player_id == player_id then
            pending_cas[request_id] = nil
        end
    end
    for request_id, pending in pairs(pending_bindings) do
        if pending.player_id == player_id then
            pending_bindings[request_id] = nil
        end
    end
    for request_id, pending in pairs(pending_orders) do
        if pending.player_id == player_id then
            pending_orders[request_id] = nil
        end
    end
    active_bindings[player_id] = nil
    records[player_id] = nil
    zone_seen[player_id] = nil
    deferred_notices[player_id] = nil
end

function on_server_started(_event: any)
    colony_request_pending = true
    colony_outcome = "record_pending"
    last_batch = { kind = "startup" }
    solaris.upsert_colony(
        "register-starter-colony",
        config.colony.id,
        config.colony.name,
        config.colony.dimension,
        config.colony.home.x,
        config.colony.home.y,
        config.colony.home.z
    )
    solaris.upsert_zone(
        config.zone.id,
        config.colony.dimension,
        config.zone.minimum.x,
        config.zone.minimum.y,
        config.zone.minimum.z,
        config.zone.maximum.x,
        config.zone.maximum.y,
        config.zone.maximum.z
    )
end

function on_player_joined(event: any)
    if colony_outcome == "ready" then
        send_message(event.player_id, "Colony plugin ready.")
    end
end

function on_colony_record_result(event: any)
    last_batch = nil
    if not colony_request_pending
        or event.request_id ~= "register-starter-colony"
        or event.colony_id ~= config.colony.id
    then
        return
    end
    colony_request_pending = false
    if not event.accepted then
        colony_outcome = "record_rejected"
        return
    end
    colony_outcome = "record_accepted"
    queue_get(nil, nil, "colony_record", nil, "load-colony-record", colony_key())
end

function on_player_zone_entered(event: any)
    last_batch = nil
    if event.zone_id ~= config.zone.id or zone_seen[event.player_id] == event.uuid then
        return
    end
    zone_seen[event.player_id] = event.uuid
    send_notice(event.player_id)
    send_message(event.player_id, "Starter Colony: use /colony status or /colony recruit [role].")
end

function on_player_command(event: any)
    last_batch = nil
    if event.root ~= "colony" then
        return
    end
    send_notice(event.player_id)
    if pending_get_by_player[event.player_id] ~= nil then
        send_message(event.player_id, "Colony request rejected: another request is pending.")
        return
    end
    for _, pending in pairs(pending_cas) do
        if pending.player_id == event.player_id then
            send_message(event.player_id, "Colony request rejected: another request is pending.")
            return
        end
    end
    for _, pending in pairs(pending_bindings) do
        if pending.player_id == event.player_id then
            send_message(event.player_id, "Colony request rejected: another request is pending.")
            return
        end
    end
    for _, pending in pairs(pending_orders) do
        if pending.player_id == event.player_id then
            send_message(event.player_id, "Colony request rejected: another request is pending.")
            return
        end
    end
    if records[event.player_id] == nil
        and table_size(records) + table_size(pending_gets) >= config.max_active_players
    then
        send_message(event.player_id, "Colony request rejected: active-player limit reached.")
        return
    end

    local arguments = split_arguments(event.arguments)
    if arguments == nil or #arguments > 2 then
        send_message(event.player_id, "Usage: /colony status|recruit [role]|role <role>|order <order>.")
        return
    end
    local action = arguments[1] or "status"
    local argument = arguments[2]
    if (action == "status" and argument ~= nil)
        or (action == "recruit" and argument ~= nil and not role_allowed[argument])
        or (action == "role" and (argument == nil or not role_allowed[argument]))
        or (action == "order" and (argument == nil or not order_allowed[argument]))
        or (action ~= "status" and action ~= "recruit" and action ~= "role" and action ~= "order")
    then
        send_message(event.player_id, "Colony command rejected: invalid bounded action.")
        return
    end

    local request_id = "state-" .. tostring(event.player_id)
    local queued = queue_get(
        event.player_id,
        event.uuid,
        action,
        argument,
        request_id,
        binding_key(event.uuid)
    )
    if queued then
        local pending = pending_gets[request_id]
        pending.x = event.x
        pending.y = event.y
        pending.z = event.z
    end
end

function on_plugin_storage_get_result(event: any)
    last_batch = nil
    local pending = pending_gets[event.request_id]
    if pending == nil then
        return
    end
    pending_gets[event.request_id] = nil
    if pending.player_id ~= nil then
        pending_get_by_player[pending.player_id] = nil
    end
    if event.key ~= pending.key then
        if pending.action == "colony_record" then
            colony_outcome = "storage_result_mismatch"
        else
            send_message(pending.player_id, "Colony storage result rejected: correlation mismatch.")
        end
        return
    end
    if event.failure ~= nil then
        if pending.action == "colony_record" then
            colony_outcome = "storage_" .. event.failure
        else
            send_message(pending.player_id, "Colony storage unavailable: " .. event.failure .. ".")
        end
        return
    end

    if pending.action == "colony_record" then
        if event.value == nil then
            local record = {
                status = "active",
                role = config.default_role,
                order = config.default_order,
                generation = 1,
            }
            queue_cas(nil, nil, pending.key, event.version, record, { kind = "colony_record" }, "persist-colony")
        elseif event.value == "v1|active|worker|home|1" then
            publish_colony_ready()
        else
            colony_outcome = "invalid_colony_record"
        end
        return
    end

    handle_player_state(pending, event.value, event.version)
end

function on_plugin_storage_cas_result(event: any)
    last_batch = nil
    local pending = pending_cas[event.request_id]
    if pending == nil then
        return
    end
    pending_cas[event.request_id] = nil
    if event.key ~= pending.key then
        if pending.after.kind == "colony_record" then
            colony_outcome = "storage_result_mismatch"
        else
            records[pending.player_id] = nil
            send_message(pending.player_id, "Colony update rejected: storage result mismatch.")
        end
        return
    end
    if event.failure ~= nil then
        if pending.after.kind == "colony_record" then
            colony_outcome = "storage_" .. event.failure
        else
            send_message(pending.player_id, "Colony update unavailable: " .. event.failure .. ".")
        end
        return
    end
    if not event.applied or event.version == nil then
        if pending.after.kind == "colony_record" then
            colony_outcome = "stale_colony_record"
        else
            records[pending.player_id] = nil
            send_message(pending.player_id, "Colony update rejected: stale storage revision.")
        end
        return
    end
    if pending.after.kind == "colony_record" then
        publish_colony_ready()
        return
    end

    records[pending.player_id] = {
        uuid = pending.uuid,
        key = pending.key,
        version = event.version,
        value = pending.next_record,
    }
    if pending.after.kind == "bind" then
        queue_binding(
            pending.player_id,
            pending.uuid,
            pending.key,
            event.version,
            pending.next_record,
            pending.after.x,
            pending.after.y,
            pending.after.z,
            "recruit"
        )
    elseif pending.after.kind == "apply_order" then
        local binding = active_bindings[pending.player_id]
        if binding == nil then
            queue_binding(
                pending.player_id,
                pending.uuid,
                pending.key,
                event.version,
                pending.next_record,
                pending.after.x,
                pending.after.y,
                pending.after.z,
                "apply_order"
            )
        else
            queue_order({
                player_id = pending.player_id,
                uuid = pending.uuid,
                key = pending.key,
                version = event.version,
                record = pending.next_record,
                purpose = "apply_order",
                binding_expires_at_tick = binding.expires_at_tick,
                retry_binding = true,
                x = pending.after.x,
                y = pending.after.y,
                z = pending.after.z,
            }, binding.token)
        end
    elseif pending.after.kind == "updated" then
        send_message(pending.player_id, "Stored " .. pending.after.field .. " intent.")
    elseif pending.after.kind == "binding_complete" then
        send_message(pending.player_id, "Villager recruitment recorded durably.")
    elseif pending.after.kind == "binding_rejected" then
        send_message(
            pending.player_id,
            "Villager binding rejected or unavailable; API 0.6 does not distinguish the cause."
        )
    end
end

function on_colony_villager_binding_result(event: any)
    last_batch = nil
    local pending = pending_bindings[event.request_id]
    if pending == nil then
        return
    end
    pending_bindings[event.request_id] = nil
    if event.colony_id ~= config.colony.id then
        send_message(pending.player_id, "Binding result rejected: colony mismatch.")
        return
    end
    if (event.binding_token == nil) ~= (event.binding_expires_at_tick == nil) then
        send_message(pending.player_id, "Binding result rejected: incomplete token lease.")
        return
    end

    if event.binding_token == nil then
        if pending.purpose == "apply_order" then
            send_message(
                pending.player_id,
                "Stored order intent, but no villager was available to apply it."
            )
            return
        end
        local next_record = copy_record(pending.record)
        next_record.generation = next_record.generation + 1
        if next_record.generation > config.max_generation then
            send_message(pending.player_id, "Binding result rejected: generation limit reached.")
            return
        end
        next_record.status = "rejected"
        queue_cas(
            pending.player_id,
            pending.uuid,
            pending.key,
            pending.version,
            next_record,
            { kind = "binding_rejected" },
            "reject"
        )
    else
        pending.binding_expires_at_tick = event.binding_expires_at_tick
        pending.retry_binding = false
        queue_order(pending, event.binding_token)
    end
end

function on_colony_villager_order_result(event: any)
    last_batch = nil
    local pending = pending_orders[event.request_id]
    if pending == nil then
        return
    end
    pending_orders[event.request_id] = nil
    if event.colony_id ~= config.colony.id or event.order ~= pending.record.order then
        send_message(pending.player_id, "Villager order result rejected: correlation mismatch.")
        return
    end

    if pending.purpose == "apply_order" then
        if event.accepted then
            active_bindings[pending.player_id] = {
                token = pending.binding_token,
                expires_at_tick = pending.binding_expires_at_tick,
            }
        else
            active_bindings[pending.player_id] = nil
            if pending.retry_binding then
                queue_binding(
                    pending.player_id,
                    pending.uuid,
                    pending.key,
                    pending.version,
                    pending.record,
                    pending.x,
                    pending.y,
                    pending.z,
                    "apply_order"
                )
                return
            end
        end
        send_message(
            pending.player_id,
            event.accepted
                and ("Applied villager order " .. pending.record.order .. ".")
                or ("Stored order intent, but villager order "
                    .. pending.record.order .. " was rejected.")
        )
        return
    end

    local next_record = copy_record(pending.record)
    next_record.generation = next_record.generation + 1
    if next_record.generation > config.max_generation then
        send_message(pending.player_id, "Villager order result rejected: generation limit reached.")
        return
    end
    if event.accepted then
        active_bindings[pending.player_id] = {
            token = pending.binding_token,
            expires_at_tick = pending.binding_expires_at_tick,
        }
        next_record.status = "active"
        queue_cas(
            pending.player_id,
            pending.uuid,
            pending.key,
            pending.version,
            next_record,
            { kind = "binding_complete" },
            "activate"
        )
    else
        active_bindings[pending.player_id] = nil
        next_record.status = "rejected"
        queue_cas(
            pending.player_id,
            pending.uuid,
            pending.key,
            pending.version,
            next_record,
            { kind = "binding_rejected" },
            "reject"
        )
    end
end

function on_player_left(event: any)
    last_batch = nil
    clear_player(event.player_id)
end

function on_command_batch_rejected(result: any)
    local batch = last_batch
    last_batch = nil
    if batch == nil then
        return
    end
    if batch.kind == "startup" then
        colony_request_pending = false
        colony_outcome = "startup_" .. result.reason
    elseif batch.kind == "get" then
        local pending = pending_gets[batch.request_id]
        pending_gets[batch.request_id] = nil
        if batch.player_id ~= nil then
            pending_get_by_player[batch.player_id] = nil
        elseif pending ~= nil and pending.action == "colony_record" then
            colony_outcome = "storage_" .. result.reason
        end
        remember_notice(batch.player_id, "Colony read rejected: " .. result.reason .. ".")
    elseif batch.kind == "cas" then
        pending_cas[batch.request_id] = nil
        remember_notice(batch.player_id, "Colony update rejected: " .. result.reason .. ".")
    elseif batch.kind == "binding" then
        pending_bindings[batch.request_id] = nil
        remember_notice(batch.player_id, "Villager binding request rejected: " .. result.reason .. ".")
    elseif batch.kind == "order" then
        pending_orders[batch.request_id] = nil
        remember_notice(batch.player_id, "Villager order rejected: " .. result.reason .. ".")
    elseif batch.kind == "message" then
        remember_notice(batch.player_id, "Colony response rejected: " .. result.reason .. ".")
    end
end

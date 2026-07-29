--!strict

-- Colony identity, roles, orders, home policy, and durable intent live in Luau.
-- Rust exposes only bounded storage, zones, an opaque villager binding, and
-- generic idle/follow-position goals through the regional entity owner.

local raw_config: any = solaris.config()
local colony_config: any = raw_config.colony
local villager_config: any = raw_config.villagers
local limits_config: any = raw_config.limits
if colony_config == nil
    or colony_config.home == nil
    or colony_config.zone == nil
    or villager_config == nil
    or limits_config == nil
then
    error("colony-villager-scaffold requires config.toml")
end

local config: any = {
    colony = {
        id = colony_config.id,
        name = colony_config.name,
        dimension = colony_config.dimension,
        home = {
            x = colony_config.home.x,
            y = colony_config.home.y,
            z = colony_config.home.z,
        },
    },
    zone = {
        id = colony_config.zone.id,
        minimum = {
            x = colony_config.zone.min_x,
            y = colony_config.zone.min_y,
            z = colony_config.zone.min_z,
        },
        maximum = {
            x = colony_config.zone.max_x,
            y = colony_config.zone.max_y,
            z = colony_config.zone.max_z,
        },
    },
    binding_radius = villager_config.binding_radius,
    home_speed = villager_config.home_speed,
    default_role = villager_config.default_role,
    default_order = villager_config.default_order,
    roles = villager_config.roles,
    orders = villager_config.orders,
    max_pending_requests = limits_config.max_pending_requests,
    max_active_players = limits_config.max_active_players,
    max_generation = 999999,
}

local function valid_text(value: any, maximum: number): boolean
    return type(value) == "string"
        and #value > 0
        and #value <= maximum
        and string.find(value, "|", 1, true) == nil
end

local function valid_number(value: any): boolean
    return type(value) == "number" and value == value and math.abs(value) < math.huge
end

if not valid_text(config.colony.id, 128)
    or not valid_text(config.colony.name, 128)
    or not valid_text(config.colony.dimension, 256)
    or not valid_text(config.zone.id, 128)
    or not valid_number(config.colony.home.x)
    or not valid_number(config.colony.home.y)
    or not valid_number(config.colony.home.z)
    or not valid_number(config.binding_radius)
    or config.binding_radius <= 0
    or config.binding_radius > 64
    or not valid_number(config.home_speed)
    or config.home_speed <= 0
    or config.home_speed > 4
    or type(config.roles) ~= "table"
    or type(config.orders) ~= "table"
    or type(config.max_pending_requests) ~= "number"
    or config.max_pending_requests < 1
    or config.max_pending_requests > 128
    or type(config.max_active_players) ~= "number"
    or config.max_active_players < 1
    or config.max_active_players > 256
then
    error("invalid colony-villager-scaffold config")
end

local role_allowed: any = {}
local order_allowed: any = {}
for _, role in ipairs(config.roles) do
    if not valid_text(role, 64) then
        error("invalid configured colony role")
    end
    role_allowed[role] = true
end
for _, order in ipairs(config.orders) do
    if order ~= "home" and order ~= "hold" then
        error("configured orders must be home or hold")
    end
    order_allowed[order] = true
end
if not role_allowed[config.default_role] or not order_allowed[config.default_order] then
    error("default colony role/order must be declared")
end

local startup_state: string = "starting"
local pending_gets: any = {}
local pending_get_by_player: any = {}
local pending_cas: any = {}
local pending_bindings: any = {}
local pending_goals: any = {}
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

local function pending_count(): number
    return table_size(pending_gets)
        + table_size(pending_cas)
        + table_size(pending_bindings)
        + table_size(pending_goals)
end

local function member_key(uuid: string): string
    return "member:" .. uuid
end

local function metadata_key(): string
    return "colony:" .. config.colony.id
end

local function metadata_value(): string
    local home = config.colony.home
    return table.concat({
        "v1",
        config.colony.id,
        config.colony.name,
        config.colony.dimension,
        tostring(home.x),
        tostring(home.y),
        tostring(home.z),
    }, "|")
end

local function encode_record(record: any): string
    return table.concat({
        "v2",
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
        "^v2|([a-z_]+)|([a-z_]+)|([a-z_]+)|(%d+)$"
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

local function request_id(prefix: string, player_id: number?, version: any): string
    return prefix
        .. "-" .. tostring(player_id or 0)
        .. "-" .. (version == nil and "new" or tostring(version))
end

local function queue_get(
    player_id: number?,
    uuid: string?,
    action: string,
    argument: string?,
    id: string,
    key: string
): boolean
    if pending_count() >= config.max_pending_requests then
        remember_notice(player_id, "Colony request rejected: pending-request limit reached.")
        return false
    end
    if player_id ~= nil and pending_get_by_player[player_id] ~= nil then
        return false
    end
    pending_gets[id] = {
        player_id = player_id,
        uuid = uuid,
        action = action,
        argument = argument,
        key = key,
    }
    if player_id ~= nil then
        pending_get_by_player[player_id] = id
    end
    last_batch = { kind = "get", request_id = id, player_id = player_id }
    solaris.storage_get(id, key)
    return true
end

local function queue_cas(
    player_id: number?,
    uuid: string?,
    key: string,
    expected_version: any,
    value: string,
    next_record: any,
    after: any,
    prefix: string
): boolean
    if pending_count() >= config.max_pending_requests then
        remember_notice(player_id, "Colony update rejected: pending-request limit reached.")
        return false
    end
    local id = request_id(prefix, player_id, expected_version)
    pending_cas[id] = {
        player_id = player_id,
        uuid = uuid,
        key = key,
        next_record = next_record,
        after = after,
    }
    last_batch = { kind = "cas", request_id = id, player_id = player_id }
    solaris.storage_cas(id, key, expected_version, value)
    return true
end

local function queue_binding(pending: any): boolean
    if pending_count() >= config.max_pending_requests then
        remember_notice(pending.player_id, "Villager binding rejected: pending-request limit reached.")
        return false
    end
    local id = request_id("bind", pending.player_id, pending.version)
    pending_bindings[id] = pending
    last_batch = { kind = "binding", request_id = id, player_id = pending.player_id }
    solaris.bind_nearest_villager(id, pending.x, pending.y, pending.z, config.binding_radius)
    return true
end

local function expected_goal(order: string): string
    return order == "home" and "follow_position" or "idle"
end

local function queue_goal(pending: any, lease_id: string): boolean
    if pending_count() >= config.max_pending_requests then
        remember_notice(pending.player_id, "Villager goal rejected: pending-request limit reached.")
        return false
    end
    local id = request_id("goal", pending.player_id, pending.version)
    pending.lease_id = lease_id
    pending_goals[id] = pending
    last_batch = { kind = "goal", request_id = id, player_id = pending.player_id }
    if pending.record.order == "home" then
        local home = config.colony.home
        solaris.move_villager_to(id, lease_id, home.x, home.y, home.z, config.home_speed)
    else
        solaris.set_villager_idle(id, lease_id)
    end
    return true
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
    if startup_state ~= "ready" then
        return "Colony unavailable: state=" .. startup_state .. "."
    end
    if record == nil then
        return config.colony.name .. ": no villager is recruited for this player."
    end
    return config.colony.name
        .. ": status=" .. record.status
        .. ", role=" .. record.role
        .. ", order=" .. record.order
        .. ", generation=" .. tostring(record.generation) .. "."
end

local function queue_goal_for_record(pending: any, lease_id: string)
    pending.binding_expires_at_tick = pending.binding_expires_at_tick or 0
    queue_goal(pending, lease_id)
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
    if startup_state ~= "ready" then
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
            encode_record(next_record),
            next_record,
            { kind = "bind", x = pending.x, y = pending.y, z = pending.z },
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
        next_record.role = pending.argument
    elseif pending.action == "order" then
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
        encode_record(next_record),
        next_record,
        pending.action == "order"
            and { kind = "apply_order", x = pending.x, y = pending.y, z = pending.z }
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
    for id, pending in pairs(pending_cas) do
        if pending.player_id == player_id then
            pending_cas[id] = nil
        end
    end
    for id, pending in pairs(pending_bindings) do
        if pending.player_id == player_id then
            pending_bindings[id] = nil
        end
    end
    for id, pending in pairs(pending_goals) do
        if pending.player_id == player_id then
            pending_goals[id] = nil
        end
    end
    active_bindings[player_id] = nil
    records[player_id] = nil
    zone_seen[player_id] = nil
    deferred_notices[player_id] = nil
end

function on_server_started(_event: any)
    startup_state = "metadata_pending"
    last_batch = { kind = "startup" }
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
    queue_get(nil, nil, "metadata", nil, "load-colony-metadata", metadata_key())
end

function on_player_joined(event: any)
    if startup_state == "ready" then
        send_message(event.player_id, config.colony.name .. " plugin ready.")
    end
end

function on_player_zone_entered(event: any)
    last_batch = nil
    if event.zone_id ~= config.zone.id or zone_seen[event.player_id] == event.uuid then
        return
    end
    zone_seen[event.player_id] = event.uuid
    send_notice(event.player_id)
    send_message(event.player_id, config.colony.name .. ": use /colony status or /colony recruit [role].")
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
    for _, pending in pairs(pending_goals) do
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
        send_message(event.player_id, "Usage: /colony status|recruit [role]|role <role>|order <home|hold>.")
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

    local id = "state-" .. tostring(event.player_id)
    if queue_get(event.player_id, event.uuid, action, argument, id, member_key(event.uuid)) then
        local pending = pending_gets[id]
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
        if pending.action == "metadata" then
            startup_state = "metadata_result_mismatch"
        else
            send_message(pending.player_id, "Colony storage result rejected: correlation mismatch.")
        end
        return
    end
    if event.failure ~= nil then
        if pending.action == "metadata" then
            startup_state = "storage_" .. event.failure
        else
            send_message(pending.player_id, "Colony storage unavailable: " .. event.failure .. ".")
        end
        return
    end

    if pending.action == "metadata" then
        local expected = metadata_value()
        if event.value == nil then
            queue_cas(
                nil,
                nil,
                pending.key,
                event.version,
                expected,
                nil,
                { kind = "metadata" },
                "persist-colony"
            )
        elseif event.value == expected then
            startup_state = "ready"
            solaris.broadcast(config.colony.name .. " plugin ready.")
        else
            startup_state = "metadata_config_mismatch"
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
        if pending.after.kind == "metadata" then
            startup_state = "metadata_result_mismatch"
        else
            records[pending.player_id] = nil
            send_message(pending.player_id, "Colony update rejected: storage result mismatch.")
        end
        return
    end
    if event.failure ~= nil then
        if pending.after.kind == "metadata" then
            startup_state = "storage_" .. event.failure
        else
            send_message(pending.player_id, "Colony update unavailable: " .. event.failure .. ".")
        end
        return
    end
    if not event.applied or event.version == nil then
        if pending.after.kind == "metadata" then
            startup_state = "stale_metadata"
        else
            records[pending.player_id] = nil
            send_message(pending.player_id, "Colony update rejected: stale storage revision.")
        end
        return
    end
    if pending.after.kind == "metadata" then
        startup_state = "ready"
        solaris.broadcast(config.colony.name .. " plugin ready.")
        return
    end

    records[pending.player_id] = {
        uuid = pending.uuid,
        key = pending.key,
        version = event.version,
        value = pending.next_record,
    }
    if pending.after.kind == "bind" then
        queue_binding({
            player_id = pending.player_id,
            uuid = pending.uuid,
            key = pending.key,
            version = event.version,
            record = pending.next_record,
            purpose = "recruit",
            retry_binding = false,
            x = pending.after.x,
            y = pending.after.y,
            z = pending.after.z,
        })
    elseif pending.after.kind == "apply_order" then
        local binding = active_bindings[pending.player_id]
        if binding == nil then
            queue_binding({
                player_id = pending.player_id,
                uuid = pending.uuid,
                key = pending.key,
                version = event.version,
                record = pending.next_record,
                purpose = "apply_order",
                retry_binding = false,
                x = pending.after.x,
                y = pending.after.y,
                z = pending.after.z,
            })
        else
            queue_goal_for_record({
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
            }, binding.lease_id)
        end
    elseif pending.after.kind == "updated" then
        send_message(pending.player_id, "Stored " .. pending.after.field .. " intent in Luau storage.")
    elseif pending.after.kind == "binding_complete" then
        send_message(pending.player_id, "Villager recruitment recorded durably by the Luau plugin.")
    elseif pending.after.kind == "binding_rejected" then
        send_message(pending.player_id, "Villager binding failed: " .. pending.after.failure .. ".")
    end
end

function on_villager_binding_result(event: any)
    last_batch = nil
    local pending = pending_bindings[event.request_id]
    if pending == nil then
        return
    end
    pending_bindings[event.request_id] = nil
    if (event.binding_token == nil) ~= (event.binding_expires_at_tick == nil) then
        send_message(pending.player_id, "Binding result rejected: incomplete lease.")
        return
    end
    if event.binding_token == nil then
        local failure = event.failure or "not_found"
        if failure == "busy" and not pending.retry_binding then
            pending.retry_binding = true
            queue_binding(pending)
            return
        end
        if pending.purpose == "apply_order" then
            send_message(pending.player_id, "Stored order intent, but binding failed: " .. failure .. ".")
            return
        end
        local next_record = copy_record(pending.record)
        next_record.status = "rejected"
        next_record.generation = next_record.generation + 1
        queue_cas(
            pending.player_id,
            pending.uuid,
            pending.key,
            pending.version,
            encode_record(next_record),
            next_record,
            { kind = "binding_rejected", failure = failure },
            "reject"
        )
        return
    end
    pending.binding_expires_at_tick = event.binding_expires_at_tick
    pending.retry_binding = false
    queue_goal_for_record(pending, event.binding_token)
end

function on_villager_goal_result(event: any)
    last_batch = nil
    local pending = pending_goals[event.request_id]
    if pending == nil then
        return
    end
    pending_goals[event.request_id] = nil
    if event.goal ~= expected_goal(pending.record.order) then
        send_message(pending.player_id, "Villager goal result rejected: correlation mismatch.")
        return
    end

    if event.accepted then
        active_bindings[pending.player_id] = {
            lease_id = pending.lease_id,
            expires_at_tick = pending.binding_expires_at_tick,
        }
        if pending.purpose == "recruit" then
            local next_record = copy_record(pending.record)
            next_record.status = "active"
            next_record.generation = next_record.generation + 1
            queue_cas(
                pending.player_id,
                pending.uuid,
                pending.key,
                pending.version,
                encode_record(next_record),
                next_record,
                { kind = "binding_complete" },
                "activate"
            )
        else
            send_message(pending.player_id, "Applied Luau order " .. pending.record.order .. ".")
        end
        return
    end

    active_bindings[pending.player_id] = nil
    local failure = event.failure or "binding_unavailable"
    if failure == "binding_unavailable" and pending.retry_binding then
        pending.retry_binding = false
        queue_binding(pending)
        return
    end
    if pending.purpose == "recruit" then
        local next_record = copy_record(pending.record)
        next_record.status = "rejected"
        next_record.generation = next_record.generation + 1
        queue_cas(
            pending.player_id,
            pending.uuid,
            pending.key,
            pending.version,
            encode_record(next_record),
            next_record,
            { kind = "binding_rejected", failure = failure },
            "reject"
        )
    else
        send_message(pending.player_id, "Stored order intent, but goal failed: " .. failure .. ".")
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
        startup_state = "startup_" .. result.reason
    elseif batch.kind == "get" then
        local pending = pending_gets[batch.request_id]
        pending_gets[batch.request_id] = nil
        if batch.player_id ~= nil then
            pending_get_by_player[batch.player_id] = nil
        elseif pending ~= nil and pending.action == "metadata" then
            startup_state = "storage_" .. result.reason
        end
        remember_notice(batch.player_id, "Colony read rejected: " .. result.reason .. ".")
    elseif batch.kind == "cas" then
        pending_cas[batch.request_id] = nil
        remember_notice(batch.player_id, "Colony update rejected: " .. result.reason .. ".")
    elseif batch.kind == "binding" then
        pending_bindings[batch.request_id] = nil
        remember_notice(batch.player_id, "Villager binding request rejected: " .. result.reason .. ".")
    elseif batch.kind == "goal" then
        pending_goals[batch.request_id] = nil
        remember_notice(batch.player_id, "Villager goal request rejected: " .. result.reason .. ".")
    elseif batch.kind == "message" then
        remember_notice(batch.player_id, "Colony response rejected: " .. result.reason .. ".")
    end
end

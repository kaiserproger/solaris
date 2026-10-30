//! Per-connection lifecycle: accept, login/config/play handling, play writer.
//! Moved verbatim from `server.rs`.

use super::*;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum AcceptFailure {
    /// One pending connection failed; the listener is fine.
    Transient,
    /// Descriptor/buffer exhaustion while our own sessions hold sockets: wait
    /// for one to end instead of failing the whole server.
    WaitForSession,
    /// Nothing of ours can free the resource, or the listener itself broke.
    Fatal,
}

pub(super) fn accept_failure(error: &std::io::Error, open_sessions: usize) -> AcceptFailure {
    use std::io::ErrorKind;
    match error.kind() {
        ErrorKind::ConnectionAborted | ErrorKind::ConnectionReset | ErrorKind::Interrupted => {
            return AcceptFailure::Transient;
        }
        _ => {}
    }
    // EMFILE, ENFILE, ENOBUFS, ENOMEM.
    let exhausted = matches!(error.raw_os_error(), Some(24 | 23 | 105 | 12));
    if exhausted && open_sessions > 0 {
        AcceptFailure::WaitForSession
    } else {
        AcceptFailure::Fatal
    }
}

struct PrePlayPermit<'a> {
    limiter: &'a crate::conn_limit::AddressLimiter,
    ip: std::net::IpAddr,
}

impl Drop for PrePlayPermit<'_> {
    fn drop(&mut self) {
        self.limiter.release(self.ip);
    }
}

/// Per-connection pipeline. Status connections return after the ping;
/// Play connections run until disconnect or overflow.
pub(super) async fn handle_connection(
    stream: TcpStream,
    server: Arc<EdgeServer>,
) -> anyhow::Result<()> {
    let peer = stream
        .peer_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| "<unknown>".to_owned());
    let id = server.connections.alloc_id();
    let mut lifecycle = ConnectionLifecycle::new(&server.connections, &server.admin, id);
    let cancelled = lifecycle.cancelled.take().expect("one connection owner");
    let result = tokio::select! {
        biased;
        _ = cancelled => {
            warn!(%peer, connection = id.0, "connection closed by delivery policy");
            Err(anyhow::anyhow!("connection delivery stopped"))
        }
        result = run_connection(stream, Arc::clone(&server), id, peer.clone(), &mut lifecycle) => result,
    };
    lifecycle.stop_writer().await;
    result
}

async fn run_connection(
    stream: TcpStream,
    server: Arc<EdgeServer>,
    id: ConnectionId,
    peer: String,
    lifecycle: &mut ConnectionLifecycle<'_>,
) -> anyhow::Result<()> {
    let permit = if server.velocity.is_none() {
        let ip = stream.peer_addr()?.ip();
        if !server.pre_play_addresses.admit(ip) {
            warn!(%ip, "too many simultaneous pre-play connections from this address");
            anyhow::bail!("Too many simultaneous pre-play connections from this address");
        }
        Some(PrePlayPermit {
            limiter: &server.pre_play_addresses,
            ip,
        })
    } else {
        None
    };
    let mut conn = crate::connection::Connection::new(stream);
    let pre_play = async {
        // --- Handshake: route to status or login. Every `?` below logs the
        // --- peer + stage + reason first: a silent TCP close is undebuggable.
        let Some(handshake) = (match crate::handshake::read_handshake(
            &mut conn,
            server.cfg.server.accepts_transfers,
            server.legacy.as_ref(),
        )
        .await
        {
            Ok(next) => next,
            Err(error) => {
                warn!(%peer, connection = id.0, stage = "handshake", %error, "connection closed");
                return Err(error);
            }
        }) else {
            warn!(%peer, connection = id.0, stage = "handshake", "handshake refused (disconnect packet sent); connection closed");
            return Ok(None);
        };
        let next_state = handshake.next;
        if next_state == crate::handshake::NextState::Status {
            let motd = server.cfg.server.motd.clone();
            let max = server.cfg.server.max_players;
            let online = server.connections.len();
            if let Err(error) =
                crate::status::handle_status(&mut conn, &motd, max, online, handshake.legacy).await
            {
                warn!(%peer, connection = id.0, stage = "status", %error, "connection closed");
                return Err(error);
            }
            return Ok(None);
        }

        // --- Login (offline, or online verification when configured). ---
        let peer_ip = stream_peer_ip(&peer);
        let outcome = match crate::login::handle_login_admitted(
            &mut conn,
            &crate::login::LoginOpts {
                online_mode: server.cfg.auth.online_mode,
                compression_threshold: crate::login::DEFAULT_COMPRESSION_THRESHOLD,
                velocity: server.velocity.clone(),
            },
            server.online_authentication.as_ref(),
            peer_ip,
            |uuid, ip| {
                if *server.stop.borrow() {
                    return Some(
                        serde_json::json!({"translate":"multiplayer.disconnect.server_shutdown"}),
                    );
                }
                server.admin.admission(uuid, ip, false)
            },
        )
        .await
        {
            Ok(outcome) => outcome,
            Err(error) => {
                warn!(%peer, connection = id.0, stage = "login", %error, "connection closed");
                return Err(error);
            }
        };
        let Some(outcome) = outcome else {
            warn!(%peer, connection = id.0, stage = "login", "login rejected (disconnect packet sent); connection closed");
            return Ok(None);
        };

        Ok(Some(outcome))
    };
    let Some(outcome) = tokio::time::timeout(crate::connection::PRE_PLAY_TOTAL_TIMEOUT, pre_play)
        .await
        .map_err(|_| anyhow::anyhow!("total login deadline exceeded"))??
    else {
        return Ok(());
    };

    // Configuration has its own per-read and resource-pack deadlines.
    let legacy = conn.legacy().cloned();
    let (data, tags) = match &legacy {
        Some(profile) => (&profile.data, &profile.tags),
        None => (&*server.data, &*server.tags),
    };
    let config_outcome = match crate::configuration::handle_configuration(
        &mut conn,
        data,
        tags,
        server.loader_manifest.as_ref(),
        server.loader_required,
        &server.cfg.resource_pack,
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(error) => {
            warn!(%peer, connection = id.0, stage = "configuration", name = %outcome.name, %error, "connection closed");
            return Err(error);
        }
    };
    drop(permit);
    // Access-list changes during Configuration revoke admission before Join.
    if let Some(reason) =
        server
            .connections
            .admit(id, server.cfg.server.max_players as usize, |full| {
                server.admin.admission(
                    outcome.uuid,
                    outcome.remote_address.or(stream_peer_ip(&peer)),
                    full,
                )
            })
    {
        warn!(%peer, %reason, "admission refused after configuration");
        conn.send_packet(&mc_protocol::packets::play::PlayDisconnect {
            reason_nbt: crate::connection::text_component_nbt(
                "Server is full or access was revoked",
            )?,
        })
        .await?;
        return Ok(());
    }

    // The radius and locale precede Join on the reliable queue: the first
    // residency reconciliation never expands this client to the server-wide
    // maximum, and a dialog opened right after join already reads the client
    // language.
    let view_distance = effective_view_distance(
        config_outcome.client_view_distance,
        server.cfg.server.view_distance,
    );
    lifecycle.native = Some(
        server.hub.join_session(
            id,
            outcome.uuid,
            outcome.name.clone(),
            server.admin.level(outcome.uuid),
            view_distance as u8,
            config_outcome.client_language.clone(),
        ).inspect_err(|error| {
            warn!(%peer, connection = id.0, stage = "join-submit", name = %outcome.name, %error, "native session refused; connection closed");
        })?
    );

    // --- Wait for our Joined (bounded): the tick loop answers with the
    // --- entity id + spawn the burst needs.
    let (entity, spawn, game_mode, dimension) = match wait_for_join(
        &server,
        id,
        &outcome.name,
        config_outcome.client_view_distance,
        &config_outcome.client_language,
    )
    .await
    {
        Ok(joined) => joined,
        Err(error) => {
            warn!(%peer, connection = id.0, stage = "join-wait", name = %outcome.name, %error, "connection closed");
            return Err(error);
        }
    };

    if config_outcome.loader_session.is_none() {
        // A client without the Loader cannot resolve plugin item models: it sees
        // the carrier item instead of the vanilla missing-model cube.
        crate::vanilla_items::mark(id);
        conn.show_vanilla_item_models();
    }

    if config_outcome.loader_session.is_some() {
        // The input follows Join on the same reliable authority queue. It is
        // never inferred from declared bundles or a client-provided brand.
        let ready = PlayerInput::PluginClientReady { connection: id };
        anyhow::ensure!(
            server.hub.submit(ready),
            "authoritative Loader session admission queue is full or stopped"
        );
    }

    // --- Join burst (compressed from here on). ---
    let simulation_distance = i32::from(server.cfg.server.simulation_distance);
    let latest = latest_output(&server);
    if let Err(error) = crate::play::send_join_burst(crate::play::JoinBurst {
        conn: &mut conn,
        data: &server.data,
        out: &latest,
        viewer_entity: entity,
        dimension,
        pos: spawn,
        view_distance,
        simulation_distance,
        game_mode,
        recipes: &server.recipes,
        op_level: if server.hub.session_locked(id) {
            0
        } else {
            server.admin.level(outcome.uuid)
        },
        plugin_commands: server.hub.plugin_commands(),
        online_mode: server.cfg.auth.online_mode,
    })
    .await
    {
        warn!(%peer, connection = id.0, stage = "join-burst", name = %outcome.name, %error, "connection closed");
        return Err(error);
    }

    // --- Register outbound queues; then replay the stashed pre-join burst:
    // --- the pump may have drained the Join tick before registration, in
    // --- which case `route_outbox` stashed the whole ordered reliable list
    // --- (Joined + menus/health/experience/...) for this connection.
    // --- Replay it in order through the same `plan_player_changes` mapping
    // --- so a rejoining player gets the authoritative inventory/XP sync.
    let (reliable_tx, reliable_rx) =
        mpsc::channel::<bytes::Bytes>(crate::fanout::OUTBOUND_QUEUE_BOUND);
    let (world_tx, world_rx) = mpsc::channel::<bytes::Bytes>(crate::fanout::OUTBOUND_QUEUE_BOUND);
    let stashed = server.connections.register(
        id,
        ConnectionHandles {
            reliable_tx,
            world_tx,
        },
    )?;
    if !stashed.is_empty() {
        // Adopt the Joined entity id if stashed (matches the pump path).
        let joined_entity = stashed.iter().find_map(|change| match change {
            mc_core::boundary::PlayerChange::Joined { entity, .. } => Some(*entity),
            _ => None,
        });
        let mut replay_viewer = configured_viewer(
            &server,
            id,
            joined_entity.unwrap_or(entity),
            spawn,
            dimension,
        );
        if let Err(error) = replay_join_tail(&server.connections, id, stashed, &mut replay_viewer) {
            warn!(%peer, connection = id.0, stage = "join-replay", name = %outcome.name, %error, "stashed join burst failed; connection closed");
            return Ok(());
        }
    }
    server.connections.set_name(id, outcome.name.clone());
    if let Some(ip) = outcome.remote_address.or(stream_peer_ip(&peer)) {
        server
            .admin
            .register(id, outcome.uuid, outcome.name.clone(), ip);
    }
    server.feedback(id, "Settlements / Поселения: /settlement guide")?;
    if *server.stop.borrow() {
        server.kick(
            id,
            serde_json::json!({"translate":"multiplayer.disconnect.server_shutdown"}),
        )?;
    }
    // Far-terrain LOD: only the negotiated native client is told about it and
    // may ask for tiles (`[lod]`).
    let native_client = config_outcome
        .loader_session
        .as_ref()
        .is_some_and(|session| session.platform() == mc_loader::LoaderPlatform::Native);
    let mut lod = (server.lod.is_some() && native_client)
        .then(|| crate::lod::LodConnection::new(spawn, entity, server.hub.subscribe()));
    if lod.is_some()
        && let Some(handles) = server.connections.get(id)
    {
        for frame in crate::lod::hello_frames(&server)? {
            let _ = handles.reliable_tx.try_send(frame);
        }
    }
    let (dh_tx, dh_rx) = mpsc::channel(128);
    let dh =
        server.dh.as_ref().filter(|_| !native_client).map(|_| {
            crate::dh::Connection::start(Arc::clone(&server), id, entity, dimension, dh_tx)
        });
    if dh.is_some()
        && let Some(handles) = server.connections.get(id)
    {
        let _ = handles.reliable_tx.try_send(crate::dh::advertisement()?);
    }
    let computer = if server.cfg.computer.enabled {
        Some(crate::computer::Connection::start(
            Arc::clone(&server),
            id,
            entity,
            peer.parse()?,
        ))
    } else {
        None
    };
    // Solaris Loader clients (native, or the Fabric mod that admitted the monitor
    // bundle's `solaris:screen` channel) take part in screen sharing.
    let screen_client = supports_screen_sharing(
        config_outcome
            .loader_session
            .as_ref()
            .map(|session| session.platform()),
    );
    let screen = screen_client.then(|| {
        crate::screen_service::Connection::start(
            Arc::clone(&server),
            id,
            entity,
            outcome.uuid,
            outcome.name.clone(),
        )
    });

    let voice = native_client.then(|| {
        crate::voice_service::Connection::start(Arc::clone(&server), id, entity, outcome.uuid)
    });

    // --- Play: reads and writes progress independently. A writer task owns
    // --- the write half (keepalive, then reliable before world); this task
    // --- reads inputs, so a slow socket write never delays the client's
    // --- inputs/acks. Queue discipline: a full reliable queue disconnects
    // --- at route time; world packets are planned within free slots.
    let mut decode_state = crate::decode::DecodeState::default();
    let mut inputs = Vec::new();
    let mut sheds = 0u32;
    let mut packet_limiter = PacketLimiter::new(&server.cfg.anti_cheat);
    let mut pending_frames = PendingPlayFrames::default();
    let (mut reader, writer) = conn.into_play_halves();
    let (keepalive_tx, keepalive_rx) = mpsc::channel(1);
    lifecycle.writer = Some(tokio::spawn(play_writer(
        writer,
        Arc::clone(&server.net_counters),
        Arc::clone(&server.connections),
        id,
        outcome.uuid,
        PlayReceivers {
            reliable: reliable_rx,
            world: world_rx,
            dh: dh_rx,
            keepalive: keepalive_rx,
        },
    )));
    let writer_task = lifecycle.writer.as_mut().expect("one writer owner");
    let mut writer_finished = false;
    let exit_stage: &'static str = 'play_loop: loop {
        tokio::select! {
            ended = &mut *writer_task => {
                writer_finished = true;
                match ended {
                    Ok((stage, Some(reason))) => {
                        warn!(%peer, connection = id.0, stage = "play", name = %outcome.name, reason = %reason, "core-ordered disconnect flushed; connection closed");
                        break 'play_loop stage;
                    }
                    Ok((stage, None)) => break 'play_loop stage,
                    Err(_) => break 'play_loop "writer-panicked",
                }
            }
            // No read deadline in Play: client silence is governed by
            // keepalive, never by the pre-play 10 s timeout.
            frame = async {
                if let Some(frame) = pending_frames.pop_front() {
                    Ok((frame, true))
                } else {
                    reader.read_frame().await.map(|frame| (frame, false))
                }
            } => {
                let (frame, admitted) = match frame {
                    Ok(frame) => frame,
                    Err(error) => {
                        warn!(%peer, connection = id.0, stage = "play-read", name = %outcome.name, %error, "connection closed");
                        break 'play_loop "read-frame";
                    }
                };
                if !admitted && !packet_limiter.allow(frame.id) {
                    warn!(%peer, connection = id.0, stage = "play-rate-limit", name = %outcome.name, packet = frame.id, "play packet rate limit exceeded");
                    if let Err(error) = server.kick(id, serde_json::json!({"text":"Too many packets of this type"})) {
                        warn!(%peer, connection = id.0, %error, "rate-limit kick undeliverable; connection closed");
                        break 'play_loop "rate-limit-kick";
                    }
                    continue;
                }
                if frame.id == mc_protocol::packets::play::ServerboundKeepAlive::ID {
                    if let Err(stage) = forward_keepalive(&frame.body, &keepalive_tx) {
                        break 'play_loop stage;
                    }
                    continue;
                }
                match server.handle_edge_frame(id, frame.id, &frame.body).await {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(error) => {
                        warn!(%error, connection = id.0, "edge command decode/delivery failed");
                        break 'play_loop "edge-command-error";
                    }
                }
                if let Some(voice) = &voice
                    && frame.id == mc_protocol::packets::play::ServerboundCustomPayload::ID
                {
                    match crate::voice_service::payload(&frame.body) {
                        Ok(Some(payload)) => {
                            voice.receive(&payload);
                            continue;
                        }
                        Ok(None) => {}
                        Err(error) => {
                            warn!(%error, connection = id.0, "voice packet malformed");
                            break 'play_loop "voice-decode";
                        }
                    }
                }
                if let Some(screen) = &screen
                    && frame.id == mc_protocol::packets::play::ServerboundCustomPayload::ID
                {
                    match crate::screen_service::payload(&frame.body) {
                        Ok(Some(payload)) => {
                            screen.receive(&payload);
                            continue;
                        }
                        Ok(None) => {}
                        Err(error) => {
                            warn!(%error, connection = id.0, "screen packet malformed");
                            break 'play_loop "screen-decode";
                        }
                    }
                }
                if let Some(computer) = &computer
                    && frame.id == mc_protocol::packets::play::ServerboundCustomPayload::ID
                {
                    match crate::computer::payload(&frame.body) {
                        Ok(Some(payload)) => {
                            if let Err(error) = computer.receive(&payload) {
                                warn!(%error, connection = id.0, "computer input refused");
                                break 'play_loop "computer-input";
                            }
                            continue;
                        }
                        Ok(None) => {}
                        Err(error) => {
                            warn!(%error, connection = id.0, "computer packet malformed");
                            break 'play_loop "computer-decode";
                        }
                    }
                }
                if let Some(dh) = &dh
                    && frame.id == mc_protocol::packets::play::ServerboundCustomPayload::ID
                    && let Some(payload) = crate::dh::payload(&frame.body)
                {
                    dh.receive(payload);
                    continue;
                }
                if let Some(lod) = &mut lod
                    && frame.id == mc_protocol::packets::play::ServerboundCustomPayload::ID
                    && let Some(payload) = crate::lod::request_payload(&frame.body)
                {
                    if let Err(error) = lod.request(&server, id, &payload) {
                        warn!(%peer, connection = id.0, stage = "lod-request", name = %outcome.name, %error, "connection closed");
                        break 'play_loop "lod-request-decode";
                    }
                    continue;
                }
                // A Loader view request is admitted only from the connection
                // that completed Configuration negotiation. It bypasses the
                // generic decoder so the same custom payload is never decoded
                // twice, while other Loader action-channel messages remain
                // intentionally inert until their native authorities land.
                let handled_loader = if config_outcome.loader_session.is_some()
                    && frame.id == mc_protocol::packets::play::ServerboundCustomPayload::ID
                {
                    match loader_view_ingress(
                        server
                            .loader_manifest
                            .as_ref()
                            .expect("negotiated Loader session retains its manifest"),
                        id,
                        &frame.body,
                    ) {
                        Ok(LoaderViewIngress::NotLoader) => false,
                        Ok(LoaderViewIngress::Discard) => true,
                        Ok(LoaderViewIngress::Request(input)) => {
                            inputs.push(input);
                            true
                        }
                        Err(error) => {
                            warn!(%peer, connection = id.0, stage = "loader-view-decode", name = %outcome.name, %error, "connection closed");
                            break 'play_loop "loader-view-decode";
                        }
                    }
                } else {
                    false
                };
                if !handled_loader
                    && let Err(reason) = crate::decode::decode_play_frame(
                        id,
                        frame.id,
                        &frame.body,
                        &mut decode_state,
                        &mut inputs,
                    )
                {
                    warn!(%peer, connection = id.0, stage = "play-decode", name = %outcome.name, frame = frame.id, reason = %reason, "connection closed");
                    if reason == crate::decode::ILLEGAL_CHAT_CHARACTERS {
                        server.kick(id, serde_json::json!({"translate":"multiplayer.disconnect.illegal_characters"}))?;
                        let _ = (&mut *writer_task).await;
                        writer_finished = true;
                        break 'play_loop "illegal-chat-characters";
                    }
                    break 'play_loop "decode-error";
                }
                if inputs.iter().any(|input| matches!(input, PlayerInput::Respawn { .. }))
                    && let Some(dh) = &dh
                {
                    dh.respawn();
                }
                for mut input in inputs.drain(..) {
                    // Mutes block communication, not unrelated command rights.
                    if input.is_communication()
                        && !server.hub.session_locked(id)
                        && server
                            .admin
                            .uuid_of(id)
                            .is_some_and(|uuid| server.admin.muted(uuid))
                    {
                        if let Err(error) = server.feedback(id, "You are muted.") {
                            warn!(%peer, connection = id.0, %error, "mute feedback undeliverable; connection closed");
                            break 'play_loop "mute-feedback";
                        }
                        continue;
                    }
                    if let PlayerInput::Command { command, permission_level, .. } = &mut input {
                        // A locked session keeps no edge-cached permission: no
                        // username or op-list state may escalate it. The fence
                        // refuses admin roots and the native gate admits only
                        // login/register roots at level 0.
                        *permission_level = if server.hub.session_locked(id) {
                            0
                        } else {
                            server.admin.session_level(id)
                        };
                        let completed = tokio::select! {
                            ended = &mut *writer_task => {
                                writer_finished = true;
                                match ended {
                                    Ok((stage, _)) => break 'play_loop stage,
                                    Err(_) => break 'play_loop "writer-panicked",
                                }
                            }
                            completed = await_play_operation(
                                server.execute_admin_command(id, command, *permission_level),
                                &mut reader,
                                &keepalive_tx,
                                &mut pending_frames,
                                &mut packet_limiter,
                            ) => completed,
                        };
                        let completed = match completed {
                            Ok(completed) => completed,
                            Err(error) if error.is::<PendingPlayOverflow>() || error.is::<PendingPlayRateLimit>() => {
                                warn!(%error, connection = id.0, "administrative input rejected; disconnecting");
                                server.kick(id, serde_json::json!({"text": error.to_string()}))?;
                                let _ = (&mut *writer_task).await;
                                writer_finished = true;
                                break 'play_loop "admin-input-overflow";
                            }
                            Err(error) => {
                                warn!(%error, connection = id.0, "administrative wait read failed");
                                break 'play_loop "admin-wait-read";
                            }
                        };
                        match completed {
                            Ok(true) => continue,
                            Ok(false) => {}
                            Err(error) => {
                                warn!(%error, connection = id.0, "administrative command failed");
                                break 'play_loop "admin-command-error";
                            }
                        }
                        // A locked session always reaches the native gate, which
                        // answers every non-login command with the login hint.
                        if !server.hub.session_locked(id)
                            && !crate::command_tree::permitted(
                            command,
                            *permission_level,
                            server.hub.plugin_commands(),
                        ) {
                            if server.feedback(id, "Unknown or incomplete command").is_err() {
                                break 'play_loop "command-feedback-overflow";
                            }
                            continue;
                        }
                    }
                    let submitted = match server.hub.admit_build(input).await {
                        Ok(input) => server.hub.submit(input),
                        Err(refusal) => server.hub.resync_build(refusal),
                    };
                    if submitted {
                        sheds = 0;
                    } else {
                        sheds += 1;
                        if sheds >= MAX_SUBMIT_SHEDS {
                            warn!(%peer, connection = id.0, stage = "play-submit", name = %outcome.name, "hub queue full; connection closed");
                            break 'play_loop "submit-shed";
                        }
                    }
                }
            }
        }
    };
    if !writer_finished {
        // Wait for the cancelled writer to drop its socket half, so the
        // session's end (which re-opens accept after fd exhaustion) really
        // means the socket is released.
        writer_task.abort();
        let _ = writer_task.await;
    }
    lifecycle.writer.take();
    warn!(%peer, connection = id.0, stage = "play", exit = exit_stage, name = %outcome.name, "connection closed");

    Ok(())
}

/// Account frame overhead too, so empty packets cannot grow the queue unbounded.
const PENDING_PLAY_BYTES: usize = 1024 * 1024;

#[derive(Default)]
pub(super) struct PendingPlayFrames {
    frames: std::collections::VecDeque<mc_protocol::frame::RawFrame>,
    bytes: usize,
}

#[derive(Debug, thiserror::Error)]
#[error("Too much pending input while an administrative command is running (1 MiB limit)")]
struct PendingPlayOverflow;

#[derive(Debug, thiserror::Error)]
#[error("Too many packets of this type")]
struct PendingPlayRateLimit;

impl PendingPlayFrames {
    fn cost(frame: &mc_protocol::frame::RawFrame) -> usize {
        std::mem::size_of_val(frame) + frame.body.len()
    }

    fn push(&mut self, frame: mc_protocol::frame::RawFrame) -> Result<(), PendingPlayOverflow> {
        let cost = Self::cost(&frame);
        if cost > PENDING_PLAY_BYTES - self.bytes {
            return Err(PendingPlayOverflow);
        }
        self.bytes += cost;
        self.frames.push_back(frame);
        Ok(())
    }

    pub(super) fn pop_front(&mut self) -> Option<mc_protocol::frame::RawFrame> {
        let frame = self.frames.pop_front()?;
        self.bytes -= Self::cost(&frame);
        Some(frame)
    }

    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
}

fn forward_keepalive(body: &[u8], replies: &mpsc::Sender<i64>) -> Result<(), &'static str> {
    let mut body = body;
    let reply = mc_protocol::packets::play::ServerboundKeepAlive::decode(&mut body)
        .map_err(|_| "keepalive-malformed")?;
    if !body.is_empty() {
        return Err("keepalive-malformed");
    }
    replies
        .try_send(reply.id)
        .map_err(|_| "keepalive-reply-overflow")
}

/// Keep reading control frames while ordered administrative completion is pending.
pub(super) async fn await_play_operation<F: Future>(
    operation: F,
    reader: &mut crate::connection::PlayReader,
    keepalive: &mpsc::Sender<i64>,
    pending: &mut PendingPlayFrames,
    limiter: &mut PacketLimiter,
) -> anyhow::Result<F::Output> {
    tokio::pin!(operation);
    loop {
        tokio::select! {
            biased;
            completed = &mut operation => return Ok(completed),
            frame = reader.read_frame() => {
                let frame = frame?;
                if !limiter.allow(frame.id) {
                    return Err(PendingPlayRateLimit.into());
                }
                if frame.id == mc_protocol::packets::play::ServerboundKeepAlive::ID {
                    forward_keepalive(&frame.body, keepalive).map_err(anyhow::Error::msg)?;
                } else {
                    pending.push(frame)?;
                }
            }
        }
    }
}

pub(super) struct PlayReceivers {
    pub(super) reliable: mpsc::Receiver<bytes::Bytes>,
    pub(super) world: mpsc::Receiver<bytes::Bytes>,
    pub(super) dh: mpsc::Receiver<crate::dh::Outbound>,
    pub(super) keepalive: mpsc::Receiver<i64>,
}
pub(super) fn smooth_latency(previous: i32, rtt: i32) -> i32 {
    ((i64::from(previous) * 3 + i64::from(rtt)) / 4) as i32
}

/// Play writer task: keepalive first, then reliable before world, so a
/// reliable frame never waits behind a world backlog. Frames already queued
/// go out together in one write (one syscall instead of one per frame),
/// taken in the same priority order and bounded by [`WRITE_BATCH_BYTES`].
/// Returns the exit stage and, for a core-ordered kick, its reason.
pub(super) async fn play_writer(
    mut writer: crate::connection::PlayWriter,
    net_counters: Arc<NetCounters>,
    connections: Arc<ConnectionRegistry>,
    id: ConnectionId,
    profile_id: uuid::Uuid,
    receivers: PlayReceivers,
) -> (&'static str, Option<String>) {
    let PlayReceivers {
        reliable: mut reliable_rx,
        world: mut world_rx,
        dh: mut dh_rx,
        keepalive: mut keepalive_rx,
    } = receivers;
    // Vanilla Play KeepAlive: the server pings every 15 s and the client
    // kicks after ~30 s of silence. Without this a parked-but-live
    // connection looks dead to vanilla even while the pump is healthy.
    let mut keepalive = tokio::time::interval(std::time::Duration::from_secs(15));
    keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Skip the immediate first tick: the join burst just spoke.
    keepalive.reset();
    let mut challenge = 0_i64;
    let mut pending = None;
    let mut reply_deadline = tokio::time::Instant::now();
    let mut sent_at = tokio::time::Instant::now();
    let mut latency = 0_i32;
    let mut batch: Vec<u8> = Vec::with_capacity(WRITE_BATCH_BYTES);
    loop {
        let mut reliable = false;
        tokio::select! {
            biased;
            _ = tokio::time::sleep_until(reply_deadline), if pending.is_some() => {
                return ("keepalive-timeout", None);
            }
            reply = keepalive_rx.recv(), if !keepalive_rx.is_closed() => {
                if reply.is_none() || reply != pending {
                    return ("keepalive-invalid-reply", None);
                }
                let rtt = i32::try_from(sent_at.elapsed().as_millis()).unwrap_or(i32::MAX);
                latency = smooth_latency(latency, rtt);
                let update = mc_protocol::packets::play::PlayerInfoUpdate {
                    actions: mc_protocol::packets::play::PlayerInfoActions::UPDATE_LATENCY,
                    entries: vec![mc_protocol::packets::play::PlayerInfoEntry {
                        profile_id,
                        name: String::new(),
                        properties: Vec::new(),
                        listed: false,
                        latency,
                        game_mode: 0,
                        list_order: 0,
                        show_hat: false,
                    }],
                };
                if let Err(error) = writer.send_packet(&update).await {
                    warn!(%error, connection = id.0, "latency update write failed");
                    return ("latency-write-failed", None);
                }
                let mut body = Vec::new();
                if let Err(error) = update.encode(&mut body) {
                    warn!(%error, connection = id.0, "latency update encode failed");
                    return ("latency-encode-failed", None);
                }
                let mut frame = bytes::BytesMut::new();
                if let Err(error) = mc_protocol::frame::append_frame(
                    &mut frame,
                    mc_protocol::packets::play::PlayerInfoUpdate::ID,
                    &body,
                    mc_protocol::frame::Compression::Threshold(256),
                ) {
                    warn!(%error, connection = id.0, "latency frame encode failed");
                    return ("latency-encode-failed", None);
                }
                let frame = frame.freeze();
                let recipients: Vec<_> = connections.inner.lock().handles.iter()
                    .filter(|(other, _)| **other != id)
                    .map(|(other, handles)| (*other, handles.reliable_tx.clone()))
                    .collect();
                for (other, sender) in recipients {
                    if sender.try_send(frame.clone()).is_err() {
                        connections.remove(other);
                        net_counters.reliable_drops.fetch_add(1, Ordering::Relaxed);
                    }
                }
                pending = None;
                continue;
            }
            _ = keepalive.tick() => {
                if pending.is_some() {
                    continue;
                }
                challenge = challenge.wrapping_add(1);
                sent_at = tokio::time::Instant::now();
                pending = Some(challenge);
                reply_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
                let packet = mc_protocol::packets::play::ClientboundKeepAlive {
                    id: challenge,
                };
                if let Err(error) = writer.send_packet(&packet).await {
                    warn!(%error, connection = id.0, "keepalive write failed");
                    return ("keepalive-write-failed", None);
                }
                continue;
            }
            msg = reliable_rx.recv() => {
                let Some(bytes) = msg else { return ("pump-closed", None) };
                batch.extend_from_slice(&bytes);
                reliable = true;
            }
            msg = world_rx.recv() => {
                let Some(bytes) = msg else { return ("pump-closed", None) };
                batch.extend_from_slice(&bytes);
            }
            msg = dh_rx.recv(), if !dh_rx.is_closed() || !dh_rx.is_empty() => {
                if let Some(message) = msg {
                    if !message.valid() { continue; }
                    // Never batch DH ahead of queued ordinary gameplay.
                    if let Err(error) = writer.write_bytes(&message.bytes).await {
                        warn!(%error, connection = id.0, "dh write failed");
                        return ("dh-write-failed", None);
                    }
                    net_counters.bytes_written.fetch_add(message.bytes.len() as u64, Ordering::Relaxed);
                }
                continue;
            }
        }
        while batch.len() < WRITE_BATCH_BYTES {
            if let Ok(bytes) = reliable_rx.try_recv() {
                batch.extend_from_slice(&bytes);
                reliable = true;
            } else if let Ok(bytes) = world_rx.try_recv() {
                batch.extend_from_slice(&bytes);
            } else {
                break;
            }
        }
        if let Err(error) = writer.write_bytes(&batch).await {
            warn!(%error, connection = id.0, reliable, bytes = batch.len(), "play write failed");
            return (
                if reliable {
                    "reliable-write-failed"
                } else {
                    "world-write-failed"
                },
                None,
            );
        }
        net_counters
            .bytes_written
            .fetch_add(batch.len() as u64, Ordering::Relaxed);
        batch.clear();
        // The queue check shares the publication lock with edge-ordered kicks.
        let terminal_reason = if reliable {
            let mut inner = connections.inner.lock();
            if reliable_rx.is_empty() {
                inner.disconnect_reasons.remove(&id)
            } else {
                None
            }
        } else {
            None
        };
        if let Some(reason) = terminal_reason {
            return ("core-disconnect", Some(reason));
        }
    }
}

/// Replay the authoritative join tail after outbound registration.
pub(super) fn replay_join_tail(
    registry: &ConnectionRegistry,
    id: ConnectionId,
    stashed: Vec<mc_core::boundary::PlayerChange>,
    viewer: &mut crate::plan::ViewerState,
) -> anyhow::Result<()> {
    let handles = registry
        .get(id)
        .ok_or_else(|| anyhow::anyhow!("connection closed before join replay"))?;
    // The join burst already sent teleport id 1 and initial presentation.
    // Replay only the authoritative tail, not another Joined/reset.
    let tail: Vec<_> = stashed
        .into_iter()
        .skip_while(|change| !matches!(change, mc_core::boundary::PlayerChange::Joined { .. }))
        .skip(1)
        .collect();
    for planned in crate::plan::plan_player_changes(&tail, viewer) {
        anyhow::ensure!(
            handles.reliable_tx.try_send(planned.data).is_ok(),
            "stashed join burst overflows outbound queue"
        );
    }
    Ok(())
}

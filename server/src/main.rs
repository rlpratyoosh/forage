use axum::{
    Router,
    body::Bytes,
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::Response,
    routing,
};
use forage_core::{Settings, World};
use forage_network::{ChunkDelta, ChunkSnapshot, ClientPacket, Error as NetError, ServerPacket};
use futures_util::{sink::SinkExt, stream::StreamExt};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_stream::{StreamMap, wrappers::BroadcastStream};
use tower_http::services::ServeDir;

const PLAYER_COUNT: usize = 1000;
const ANTS_PER_PLAYER: u32 = 500;
const ANT_DENSITY: f32 = 0.05;
const TICKS_PER_SECOND: u8 = 10;

enum EngineCommand {
    AddPlayer(oneshot::Sender<Result<u32, NetError>>),
    RemovePlayer(u32),
    GetSnapshot(u32, oneshot::Sender<Result<ChunkSnapshot, NetError>>),
    SpawnFood {
        chunk_idx: u32,
        local_idx: u16,
        quantity: u8,
        sender: oneshot::Sender<Result<(), NetError>>,
    },
}

struct ServerState {
    engine_tx: mpsc::Sender<EngineCommand>,
    chunk_broadcasts: Vec<broadcast::Sender<ChunkDelta>>,
    map_area: u64,
    no_of_chunks: u32,
    chunks_per_player: u16,
}

type WsSender = futures_util::stream::SplitSink<WebSocket, Message>;

#[tokio::main]
async fn main() {
    let (engine_tx, engine_rx) = mpsc::channel::<EngineCommand>(1024);

    let settings = Settings::new(PLAYER_COUNT, ANTS_PER_PLAYER, ANT_DENSITY);
    let no_of_chunks = settings.get_no_of_chunks() as usize;

    let mut chunk_broadcasts = Vec::with_capacity(no_of_chunks);

    for _ in 0..no_of_chunks {
        let (tx, _) = broadcast::channel::<ChunkDelta>(16);
        chunk_broadcasts.push(tx);
    }

    let chunk_broadcasts_engine = chunk_broadcasts.clone();

    let server_state = Arc::new(ServerState {
        engine_tx,
        chunk_broadcasts,
        map_area: settings.get_map_area() as u64,
        no_of_chunks: settings.get_no_of_chunks(),
        chunks_per_player: settings.get_chunks_per_player(),
    });

    let handle = std::thread::spawn(move || {
        run_engine(engine_rx, chunk_broadcasts_engine, settings);
    });

    let Ok(listener) = tokio::net::TcpListener::bind("127.0.0.1:8080").await else {
        eprintln!("Failed to bind port 8080!");
        return;
    };

    let server = Router::new()
        .route("/health", routing::get("Online!"))
        .route("/join", routing::any(join_handler))
        .fallback_service(ServeDir::new("client"))
        .with_state(server_state);

    axum::serve(listener, server).await.unwrap();
    let _ = handle.join();
}

async fn join_handler(
    ws: WebSocketUpgrade,
    State(server_state): State<Arc<ServerState>>,
) -> Response {
    ws.on_upgrade(|socket| handle_connection(socket, server_state))
}

macro_rules! send_packet {
    ($sender:expr, $packet:expr) => {{
        if let Ok(bytes) = wincode::serialize($packet) {
            $sender
                .send(Message::Binary(Bytes::from(bytes)))
                .await
                .is_ok()
        } else {
            false
        }
    }};
}

macro_rules! cleanup_player {
    ($nest_id:expr, $engine_tx:expr) => {{
        if let Some(id) = $nest_id {
            let _ = $engine_tx.send(EngineCommand::RemovePlayer(id)).await;
        }
    }};
}

async fn handle_connection(socket: WebSocket, server_state: Arc<ServerState>) {
    let (mut sender, mut receiver) = socket.split();
    let viewport_capacity = (server_state.chunks_per_player.isqrt() as usize + 1).pow(2);

    let mut nest_id: Option<u32> = None;
    let mut broadcast_receivers = StreamMap::with_capacity(viewport_capacity);
    let engine_tx = &server_state.engine_tx;

    loop {
        tokio::select! {
            Some(msg) = receiver.next() => {
                let Ok(Message::Binary(bytes)) = msg else {
                    cleanup_player!(nest_id, engine_tx);
                    break;
                };

                let Ok(client_packet) = wincode::deserialize::<ClientPacket>(&bytes) else {
                    let _ = send_packet!(sender, &NetError::InvalidRequest);
                    continue;
                };

                match client_packet {
                    ClientPacket::Join => {
                        if !process_join(&mut sender, &mut broadcast_receivers, &mut nest_id, &server_state).await {
                            cleanup_player!(nest_id, engine_tx);
                            break;
                        }
                    }
                    ClientPacket::UpdateViewport { chunks } => {
                        if !update_viewport(&mut sender, &mut broadcast_receivers, chunks, &server_state).await {
                            cleanup_player!(nest_id, engine_tx);
                            break;
                        };
                    }
                    ClientPacket::SpawnFood { chunk_idx, local_idx, quantity } => {
                        if !spawn_food(&mut sender, chunk_idx, local_idx, quantity, engine_tx).await {
                            cleanup_player!(nest_id, engine_tx);
                            break;
                        }
                    }
                    ClientPacket::Quit => {
                        cleanup_player!(nest_id, engine_tx);
                        break;
                    }
                }
            }

            Some((_, delta)) = broadcast_receivers.next(), if !broadcast_receivers.is_empty() => {
                if let Ok(delta) = delta {
                    if !send_packet!(sender, &ServerPacket::Delta(delta)) {
                        cleanup_player!(nest_id, engine_tx);
                        break;
                    }
                }
            }
        }
    }
}

async fn process_join(
    sender: &mut WsSender,
    broadcast_receivers: &mut StreamMap<usize, BroadcastStream<ChunkDelta>>,
    nest_id: &mut Option<u32>,
    server_state: &Arc<ServerState>,
) -> bool {
    if nest_id.is_some() {
        return send_packet!(sender, &NetError::BadRequest);
    }

    let engine_tx = &server_state.engine_tx;
    let (tx, rx) = oneshot::channel();

    if engine_tx.send(EngineCommand::AddPlayer(tx)).await.is_err() {
        return send_packet!(sender, &NetError::EngineFailure);
    }

    let Ok(Ok(id)) = rx.await else {
        return send_packet!(sender, &NetError::EngineFailure);
    };

    let map_width_zeros = server_state.no_of_chunks.isqrt().trailing_zeros() as usize;
    let territory_width = server_state.chunks_per_player.isqrt() as usize;
    let start = (id as usize) << territory_width;

    let mut snapshot_receivers = Vec::with_capacity(territory_width * territory_width);

    for r in 0..territory_width {
        let r_id = start + (r << map_width_zeros);
        for c in 0..territory_width {
            let chunk_id = r_id + c;
            let (snapshot_tx, snapshot_rx) = oneshot::channel();

            if engine_tx
                .send(EngineCommand::GetSnapshot(chunk_id as u32, snapshot_tx))
                .await
                .is_ok()
            {
                snapshot_receivers.push(snapshot_rx);
            } else {
                let _ = send_packet!(sender, &NetError::EngineFailure);
                return false;
            }

            let broadcast_rx =
                BroadcastStream::new(server_state.chunk_broadcasts[chunk_id].subscribe());
            broadcast_receivers.insert(chunk_id, broadcast_rx);
        }
    }

    let mut snapshots = Vec::with_capacity(territory_width * territory_width);
    for result in futures::future::join_all(snapshot_receivers).await {
        match result {
            Ok(Ok(snapshot)) => snapshots.push(snapshot),
            Ok(Err(_)) => {
                let _ = send_packet!(sender, &NetError::ServerFailure);
                return false;
            }
            Err(_) => {
                let _ = send_packet!(sender, &NetError::EngineFailure);
                return false;
            }
        }
    }

    let packet = ServerPacket::Welcome {
        nest_idx: id,
        map_area: server_state.map_area,
        no_of_chunks: server_state.no_of_chunks,
        chunks_per_player: server_state.chunks_per_player,
        snapshots,
    };

    if send_packet!(sender, &packet) {
        *nest_id = Some(id);
        true
    } else {
        false
    }
}

async fn update_viewport(
    sender: &mut WsSender,
    broadcast_receivers: &mut StreamMap<usize, BroadcastStream<ChunkDelta>>,
    chunks: Vec<u32>,
    server_state: &Arc<ServerState>,
) -> bool {
    let mut keys_to_remove = Vec::new();
    for key in broadcast_receivers.keys() {
        if !chunks.contains(&(*key as u32)) {
            keys_to_remove.push(*key);
        }
    }

    for key in keys_to_remove {
        broadcast_receivers.remove(&key);
    }

    let engine_tx = &server_state.engine_tx;
    let mut snapshot_receivers = Vec::new();

    for chunk in chunks {
        let chunk = chunk as usize;
        if !broadcast_receivers.contains_key(&chunk) {
            let broadcast_rx =
                BroadcastStream::new(server_state.chunk_broadcasts[chunk].subscribe());
            broadcast_receivers.insert(chunk, broadcast_rx);

            let (snapshot_tx, snapshot_rx) = oneshot::channel();

            if engine_tx
                .send(EngineCommand::GetSnapshot(chunk as u32, snapshot_tx))
                .await
                .is_ok()
            {
                snapshot_receivers.push(snapshot_rx);
            } else {
                let _ = send_packet!(sender, &NetError::EngineFailure);
                return false;
            }
        }
    }

    for result in futures::future::join_all(snapshot_receivers).await {
        match result {
            Ok(Ok(snapshot)) => {
                let _ = send_packet!(sender, &ServerPacket::Snapshot(snapshot));
            }
            Ok(Err(_)) => {
                let _ = send_packet!(sender, &NetError::BadRequest);
            }
            Err(_) => {
                let _ = send_packet!(sender, &NetError::EngineFailure);
                return false;
            }
        }
    }

    true
}

async fn spawn_food(
    sender: &mut WsSender,
    chunk_idx: u32,
    local_idx: u16,
    quantity: u8,
    engine_tx: &mpsc::Sender<EngineCommand>,
) -> bool {
    let (tx, rx) = oneshot::channel();
    if engine_tx
        .send(EngineCommand::SpawnFood {
            chunk_idx,
            local_idx,
            quantity,
            sender: tx,
        })
        .await
        .is_err()
    {
        let _ = send_packet!(sender, &NetError::EngineFailure);
        return false;
    }

    if let Ok(Err(e)) = rx.await {
        return send_packet!(sender, &e);
    }
    true
}

fn run_engine(
    mut engine_rx: mpsc::Receiver<EngineCommand>,
    chunk_broadcasts_engine: Vec<broadcast::Sender<ChunkDelta>>,
    settings: Settings,
) {
    let no_of_chunks = settings.get_no_of_chunks() as usize;
    let mut world = World::new(settings);
    let millis_per_tick = 1000 / TICKS_PER_SECOND as u64;
    let duration_per_tick = std::time::Duration::from_millis(millis_per_tick);

    loop {
        let start = std::time::Instant::now();

        world.tick();

        while let Ok(cmd) = engine_rx.try_recv() {
            match cmd {
                EngineCommand::AddPlayer(sender) => {
                    let _ = match world.add_player() {
                        Ok(id) => sender.send(Ok(id as u32)),
                        Err(e) => sender.send(Err(e)),
                    };
                }
                EngineCommand::RemovePlayer(id) => {
                    let _ = world.remove_player(id as usize);
                }
                EngineCommand::GetSnapshot(id, sender) => {
                    let snapshot = world.get_snapshot(id);
                    let _ = sender.send(snapshot);
                }
                EngineCommand::SpawnFood {
                    chunk_idx,
                    local_idx,
                    quantity,
                    sender,
                } => {
                    let res = world.add_food(chunk_idx as usize, local_idx as usize, quantity);
                    let _ = sender.send(res);
                }
            };
        }

        for i in 0..no_of_chunks {
            let broadcast = &chunk_broadcasts_engine[i];
            if broadcast.receiver_count() > 0 {
                if let Ok(delta) = world.get_delta(i as u32) {
                    let _ = broadcast.send(delta);
                }
            }
        }

        let elapsed = start.elapsed();
        if elapsed > duration_per_tick {
            println!(
                "Server lagging by {:.2}ms!",
                (elapsed - duration_per_tick).as_secs_f64() * 1000.0
            );
            continue;
        }
        std::thread::sleep(duration_per_tick - elapsed);
    }
}

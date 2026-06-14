use forage_core::{World, Settings};
use forage_network::{ChunkSnapshot, ChunkDelta, ServerPacket, ClientPacket};
use tokio::sync::{mpsc, oneshot, broadcast};
use std::sync::Arc;
use axum::{
    Router,
    routing,
    extract::{ws::{WebSocketUpgrade, WebSocket, Message}, State },
    response::Response,
    body::Bytes,
};
use futures_util::{sink::SinkExt, stream::StreamExt};
use tokio_stream::{StreamMap, wrappers::BroadcastStream};

const PLAYER_COUNT: usize = 1000;
const ANTS_PER_PLAYER: u32 = 500;
const ANT_DENSITY: f32 = 0.05;
const TICKS_PER_SECOND: u8 = 10;

enum EngineCommad {
    AddPlayer(oneshot::Sender<Result<u32, String>>),
    RemovePlayer(u32),
    GetSnapshot(u32, oneshot::Sender<ChunkSnapshot>),
    SpawnFood { chunk_idx: u32, local_idx: u16, quantity: u8 },
}

struct ServerState {
    engine_tx: mpsc::Sender<EngineCommad>,
    chunk_broadcasts: Vec<broadcast::Sender<ChunkDelta>>,
    map_area: u64,
    no_of_chunks: u32,
    chunks_per_player: u16,
}

#[tokio::main]
async fn main() {
    let (engine_tx, mut engine_rx) = mpsc::channel::<EngineCommad>(1024);

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
        let mut world = World::new(settings);
        let millis_per_tick = 1000 / TICKS_PER_SECOND as u64;
        let duration_per_tick = std::time::Duration::from_millis(millis_per_tick);

        loop {
            let start = std::time::Instant::now();

            while let Ok(cmd) = engine_rx.try_recv() {
                match cmd {
                    EngineCommad::AddPlayer(sender) => {
                        let _ = match world.add_player() {
                            Ok(id) => sender.send(Ok(id as u32)),
                            Err(e) => sender.send(Err(e.to_string())),
                        };
                    }

                    EngineCommad::RemovePlayer(id) => world.remove_player(id as usize),

                    EngineCommad::GetSnapshot(id, sender) => {
                        let snapshot = world.get_snapshot(id);
                        let _ = sender.send(snapshot);
                    }

                    EngineCommad::SpawnFood { chunk_idx, local_idx, quantity } => {
                        world.add_food(chunk_idx as usize, local_idx as usize, quantity);
                    }
                };
            }

            world.tick();

            for i in 0..no_of_chunks {
                let broadcast = &chunk_broadcasts_engine[i];
                if broadcast.receiver_count() > 0 {
                    let delta = world.get_delta(i as u32);
                    let _ = broadcast.send(delta);
                }
            }

            let elapsed = start.elapsed();
            if elapsed > duration_per_tick {
                println!("Server lagging by {:.2}ms!", (elapsed - duration_per_tick).as_secs_f64() * 1000.0);
                continue;
            }
            std::thread::sleep(duration_per_tick - elapsed);
        }
    });

    let Ok(listener) = tokio::net::TcpListener::bind("127.0.0.1:8080").await else {
        eprintln!("Failed to bind port 8080!");
        return;
    };

    let server = Router::new()
        .route("/health", routing::get("Online!"))
        .route("/join", routing::any(join_handler))
        .with_state(server_state);

    axum::serve(listener, server).await.unwrap(); // Safe to unwrap as it never returns Err
    let _  = handle.join();
}

async fn join_handler(ws: WebSocketUpgrade, State(server_state): State<Arc<ServerState>>) -> Response {
    ws.on_upgrade(|socket| handle_connection(socket, server_state))
}

async fn handle_connection(socket: WebSocket, server_state: Arc<ServerState>) {
    let (mut sender, mut receiver) = socket.split();

    let map_width = server_state.no_of_chunks.isqrt() as usize;
    let territory_width = server_state.chunks_per_player.isqrt() as usize;
    let viewport_width = territory_width + 1;
    let viewport_capacity = viewport_width * viewport_width;
    let mut nest_id = None;

    let mut broadcast_receivers = StreamMap::with_capacity(viewport_capacity);

    let engine_tx = &server_state.engine_tx;

     loop {
        tokio::select! {
            Some(msg) = receiver.next() => {
                let Ok(msg) = msg else { return; };

                if let Message::Binary(bytes) = msg {
                    let res = wincode::deserialize::<ClientPacket>(&bytes);

                    if let Ok(client_packet) = res {
                        match client_packet {
                            ClientPacket::Join => {
                                let (tx, rx) = oneshot::channel::<Result<u32, String>>();
                                if let Ok(()) = engine_tx.send(EngineCommad::AddPlayer(tx)).await && let Ok(id) = rx.await {
                                    match  id {
                                        Ok(id) => {
                                            let start = (id as usize) << territory_width;
                                            let mut snapshot_receivers = Vec::with_capacity(territory_width * territory_width);

                                            for r in 0..territory_width {
                                                let r_id = start + (r << map_width.trailing_zeros() as usize);
                                                for c in 0..territory_width {
                                                    let chunk_id = r_id + c;

                                                    let (snapshot_tx, snapshot_rx) = oneshot::channel::<ChunkSnapshot>();
                                                    if let Ok(()) = engine_tx.send(EngineCommad::GetSnapshot(chunk_id as u32, snapshot_tx)).await{
                                                        snapshot_receivers.push(snapshot_rx);
                                                    }

                                                    let broadcast_rx = BroadcastStream::new(server_state.chunk_broadcasts[chunk_id].subscribe());
                                                    broadcast_receivers.insert(chunk_id, broadcast_rx);
                                                }
                                            }

                                            let mut snapshots = Vec::with_capacity(territory_width * territory_width);
                                            let results = futures::future::join_all(snapshot_receivers).await;
                                            for result in results {
                                                if let Ok(snapshot) = result {
                                                    snapshots.push(snapshot);
                                                }
                                            }

                                            let packet = ServerPacket::Welcome {
                                                nest_idx: id,
                                                map_area: server_state.map_area,
                                                no_of_chunks: server_state.no_of_chunks,
                                                chunks_per_player: server_state.chunks_per_player,
                                                snapshots,
                                            };
                                            if let Ok(bytes) = wincode::serialize(&packet) {
                                                let _ = sender.send(Message::Binary(Bytes::from(bytes))).await;
                                            }

                                            nest_id = Some(id);
                                        }
                                        Err(_e) => {
                                            // TO DO: Error implementation
                                        }
                                    }
                                }
                            }

                            ClientPacket::UpdateViewport { chunks } => {
                                let mut keys_to_remove = Vec::new();

                                for key in broadcast_receivers.keys() {
                                    let k = *key as u32;
                                    if chunks.contains(&k) { continue; }
                                    keys_to_remove.push(*key);
                                }

                                for key in keys_to_remove {
                                    broadcast_receivers.remove(&key);
                                }

                                for chunk in chunks {
                                    let chunk = chunk as usize;
                                    if ! broadcast_receivers.contains_key(&chunk) {
                                        let broadcast_rx = BroadcastStream::new(server_state.chunk_broadcasts[chunk].subscribe());
                                        broadcast_receivers.insert(chunk, broadcast_rx);
                                    }
                                }
                            }

                            ClientPacket::Quit => {
                                if let Some(id) = nest_id {
                                    let _ = engine_tx.send(EngineCommad::RemovePlayer(id)).await;
                                }
                                break;
                            }

                            ClientPacket::SpawnFood { chunk_idx, local_idx, quantity } => {
                                let _ = engine_tx.send(EngineCommad::SpawnFood { chunk_idx, local_idx, quantity });
                            }
                        }
                    }
                }
            }

            Some((_, delta)) = broadcast_receivers.next(), if !broadcast_receivers.is_empty() => {
                if let Ok(delta) = delta {
                    let packet = ServerPacket::Delta(delta);
                    if let Ok(bytes) = wincode::serialize(&packet) {
                        let _ = sender.send(Message::Binary(Bytes::from(bytes))).await;
                    }
                }
            }
        }
    }
}


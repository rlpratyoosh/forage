use forage_core::{World, Settings};
use forage_network::{ChunkSnapshot, ChunkDelta, ServerPacket, ClientPacket};
use tokio::sync::{mpsc, oneshot, broadcast};
use std::sync::Arc;

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
    chunk_broadcasts: Vec<broadcast::Sender<ChunkDelta>>
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
        chunk_broadcasts
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
                    _ => {}
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

    let _  = handle.join();
}

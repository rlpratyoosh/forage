use forage_network::{ClientPacket, ServerPacket};
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::time::{Duration, interval};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

const CLIENTS_TO_SPAWN: usize = 999;
const SERVER_URL: &str = "ws://127.0.0.1:8080/join";

#[derive(Default)]
struct Metrics {
    bytes_received: AtomicUsize,
    packets_received: AtomicUsize,
    clients_connected: AtomicUsize,
}

#[tokio::main]
async fn main() {
    println!("Starting Benchmark...");
    println!(
        "Spawning {} clients (leaving 1 slot open)...",
        CLIENTS_TO_SPAWN
    );

    let metrics = Arc::new(Metrics::default());
    let mut handles = Vec::new();

    for id in 0..CLIENTS_TO_SPAWN {
        let metrics_clone = metrics.clone();

        let handle = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis((id * 5) as u64)).await;

            let ws_stream = match connect_async(SERVER_URL).await {
                Ok((stream, _)) => stream,
                Err(e) => {
                    eprintln!("Client {} failed to connect: {:?}", id, e);
                    return;
                }
            };

            metrics_clone
                .clients_connected
                .fetch_add(1, Ordering::Relaxed);
            let (mut write, mut read) = ws_stream.split();

            let join_packet = ClientPacket::Join;
            if let Ok(bytes) = wincode::serialize(&join_packet) {
                let _ = write.send(Message::Binary(bytes.into())).await;
            }

            let mut viewport_update_interval = interval(Duration::from_secs(5));
            let mut spawn_food_interval = interval(Duration::from_secs(3));
            let mut current_chunks: Vec<u32> = Vec::new();
            let mut chunk_shift = 0;

            loop {
                tokio::select! {
                    msg = read.next() => {
                        match msg {
                            Some(Ok(Message::Binary(bytes))) => {
                                metrics_clone.bytes_received.fetch_add(bytes.len(), Ordering::Relaxed);
                                metrics_clone.packets_received.fetch_add(1, Ordering::Relaxed);
                                if let Ok(packet) = wincode::deserialize::<ServerPacket>(&bytes) {
                                    if let ServerPacket::Welcome { snapshots, .. } = packet {
                                        current_chunks = snapshots.into_iter().map(|s| s.chunk_idx).collect();
                                    }
                                }
                            }
                            Some(Ok(_)) => {}
                            Some(Err(_)) | None => {
                                metrics_clone.clients_connected.fetch_sub(1, Ordering::Relaxed);
                                break;
                            }
                        }
                    }
                    _ = viewport_update_interval.tick() => {
                        if !current_chunks.is_empty() {
                            chunk_shift = (chunk_shift + 1) % 5;
                            let mut new_chunks = current_chunks.clone();
                            if chunk_shift % 2 == 0 {
                                for chunk in new_chunks.iter_mut() {
                                    *chunk = chunk.saturating_add(1);
                                }
                            }

                            let packet = ClientPacket::UpdateViewport { chunks: new_chunks };
                            if let Ok(bytes) = wincode::serialize(&packet) {
                                let _ = write.send(Message::Binary(bytes.into())).await;
                            }
                        }
                    }
                    _ = spawn_food_interval.tick() => {
                        if !current_chunks.is_empty() {
                            let local_idx = ((id * 17 + chunk_shift) % 1024) as u16;
                            let packet = ClientPacket::SpawnFood {
                                chunk_idx: current_chunks[0],
                                local_idx,
                                quantity: 200,
                            };
                            if let Ok(bytes) = wincode::serialize(&packet) {
                                let _ = write.send(Message::Binary(bytes.into())).await;
                            }
                        }
                    }
                }
            }
        });
        handles.push(handle);
    }

    let mut ticker = interval(Duration::from_secs(1));
    let mut elapsed = 0;

    println!("Waiting for clients to connect...");
    tokio::time::sleep(Duration::from_secs(6)).await;

    println!("All clients initiated. Monitoring...");
    println!(
        "{:<10} | {:<15} | {:<20} | {:<15}",
        "Time (s)", "Connected", "Throughput (MB/s)", "Packets/s"
    );
    println!("{:-<68}", "-");

    let mut total_bytes = 0;
    let mut total_packets = 0;

    let test_duration = 100;

    for _ in 0..test_duration {
        ticker.tick().await;
        elapsed += 1;

        let bytes = metrics.bytes_received.swap(0, Ordering::Relaxed);
        let packets = metrics.packets_received.swap(0, Ordering::Relaxed);
        let connected = metrics.clients_connected.load(Ordering::Relaxed);

        let mb_per_sec = bytes as f64 / 1024.0 / 1024.0;
        total_bytes += bytes;
        total_packets += packets;

        println!(
            "{:<10} | {:<15} | {:<20.2} | {:<15}",
            elapsed, connected, mb_per_sec, packets
        );
    }

    println!("\n=== Benchmark Summary ===");
    println!("Duration: {} seconds", test_duration);
    println!(
        "Avg Throughput: {:.2} MB/s",
        (total_bytes as f64 / 1024.0 / 1024.0) / test_duration as f64
    );
    println!("Avg Packets/sec: {}", total_packets / test_duration);
    println!(
        "Clients Connected: {}",
        metrics.clients_connected.load(Ordering::Relaxed)
    );
    println!("\nBenchmark completed. Keeping connections open indefinitely.");
    println!("You can now join the server at http://127.0.0.1:8080/ to observe.");

    loop {
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}

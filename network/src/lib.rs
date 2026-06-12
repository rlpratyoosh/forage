use wincode::{ SchemaRead, SchemaWrite };

#[derive(SchemaRead, SchemaWrite, Debug, PartialEq)]
pub enum ServerPacket {
    Welcome {
        nest_idx: u32,
        map_area: u64,
        no_of_chunks: u32,
        chunks_per_player: u32,
        snapshot: Vec<ChunkSnapshot>
    },
    Snapshot(ChunkSnapshot),
    Delta(ChunkDelta),
    SpawnFood { chunk_idx: u32, local_idx: u16, quantity: u8 },
}

#[derive(SchemaRead, SchemaWrite, Debug, PartialEq)]
pub struct ChunkSnapshot {
    pub chunk_idx: u32,
    pub ant_bitboards: [u64; 16],
    pub pheromone_strengths: [u8; 1024], 
    pub food_quantities: Vec<(u16, u8)>,
}

#[derive(SchemaRead, SchemaWrite, Debug, PartialEq, Clone)]
pub struct ChunkDelta {
    pub chunk_idx: u32,
    pub ant_bitboards: [u64; 16],
    pub pheromone_bitboards: [u64; 16],
    pub dirty_food: Vec<(u16, u8)>,
}

#[derive(SchemaRead, SchemaWrite, Debug, PartialEq)]
pub enum ClientPacket {
    Join,
    UpdateViewport {
        chunks: Vec<u32>,
    },
    Quit,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_welcome_packet() {
        let packet = ServerPacket::Welcome {
            nest_idx: 1,
            map_area: 100,
            no_of_chunks: 10,
            chunks_per_player: 2,
            snapshot: vec![
                ChunkSnapshot {
                    chunk_idx: 0,
                    ant_bitboards: [0; 16],
                    pheromone_strengths: [0; 1024],
                    food_quantities: vec![(0, 10), (1, 20)],
                },
                ChunkSnapshot {
                    chunk_idx: 1,
                    ant_bitboards: [0; 16],
                    pheromone_strengths: [0; 1024],
                    food_quantities: vec![(2, 30)],
                },
            ],
        };

        let encoded = wincode::serialize(&packet).unwrap();
        println!("Encoded Welcome ServerPacket Size: {:.2}KB", encoded.len() as f32 / 1024.0);
        let decoded = wincode::deserialize::<ServerPacket>(&encoded).unwrap();
        assert_eq!(packet, decoded);
    }

    #[test]
    fn server_snapshot_packet() {
        let packet = ServerPacket::Snapshot(ChunkSnapshot {
            chunk_idx: 0,
            ant_bitboards: [0; 16],
            pheromone_strengths: [0; 1024],
            food_quantities: vec![(0, 10), (1, 20)],
        });

        let encoded = wincode::serialize(&packet).unwrap();
        println!("Encoded Snapshot ServerPacket Size: {:.2}KB", encoded.len() as f32 / 1024.0);
        let decoded = wincode::deserialize::<ServerPacket>(&encoded).unwrap();
        assert_eq!(packet, decoded);
    }

    #[test]
    fn server_delta_packet() {
        let packet = ServerPacket::Delta(ChunkDelta {
            chunk_idx: 0,
            ant_bitboards: [0; 16],
            pheromone_bitboards: [0; 16],
            dirty_food: vec![(0, 10), (1, 20)],
        });

        let encoded = wincode::serialize(&packet).unwrap();
        println!("Encoded Delta ServerPacket Size: {:.2}KB", encoded.len() as f32 / 1024.0);
        let decoded = wincode::deserialize::<ServerPacket>(&encoded).unwrap();
        assert_eq!(packet, decoded);
    }

    #[test]
    fn client_packet() {
        let packet = ClientPacket::UpdateViewport {
            chunks: vec![0, 1, 2],
        };
        let encoded = wincode::serialize(&packet).unwrap();
        let decoded = wincode::deserialize::<ClientPacket>(&encoded).unwrap();
        assert_eq!(decoded, packet);

        let packet = ClientPacket::Join;
        let encoded = wincode::serialize(&packet).unwrap();
        let decoded = wincode::deserialize::<ClientPacket>(&encoded).unwrap();
        assert_eq!(decoded, packet);

        let packet = ClientPacket::Quit;
        let encoded = wincode::serialize(&packet).unwrap();
        let decoded = wincode::deserialize::<ClientPacket>(&encoded).unwrap();
        assert_eq!(decoded, packet);
    }
}


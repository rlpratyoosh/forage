use wincode::{ SchemaRead, SchemaWrite };

#[derive(SchemaRead, SchemaWrite)]
pub enum ServerPacket {
    ChunkKeyframe {
        chunk_idx: u32,
        ants: Vec<u16>,
        food: Vec<(u16, u8)>,
        pheromones: Vec<u8>,
    },
    ChunkDelta {
        chunk_idx: u32,
        ants: Vec<u16>,
        food_changes: Vec<(u16, u8)>,
    }
}

// #[derive(SchemaRead, SchemaWrite)]
// pub struct ClientMessage {
//
// }

#[cfg(test)]
mod tests {
    use super::*;

}


pub enum AntState {
    Searching,
    Returning,
}

pub struct AntPool {
    pos: Vec<usize>,
    state: Vec<AntState>,
    nest_ids: Vec<u32>,
}

impl AntPool {
    pub fn new(player_count: usize, ants_per_nest: usize) -> Self {
        let capacity = player_count * ants_per_nest;

        let mut pos = Vec::with_capacity(capacity);
        let mut state = Vec::with_capacity(capacity);
        let mut nest_ids = Vec::with_capacity(capacity);

        let mut cur_nest_id = 0;
        let mut cur_pos = 0;

        for _ in (0..capacity).step_by(ants_per_nest) {
            for _ in 0..ants_per_nest {
                pos.push(cur_pos);
                state.push(AntState::Searching);
                nest_ids.push(cur_nest_id);
            }
            cur_nest_id += 1;
            cur_pos += 1;
        }

        Self {
            pos,
            state,
            nest_ids,
        }
    }
}

pub struct FoodPool {
    quantity: Vec<u8>,
}

impl FoodPool {
    pub fn new(map_area: usize) -> Self {
        Self {
            quantity: vec![0; map_area],
        }
    }
}

pub struct PheromonePool {
    strength: Vec<f32>,
    active_chunks: Vec<usize>,
}

impl PheromonePool {
    pub fn new(map_area: usize) -> Self {
        let no_of_chunks = map_area / 1024;

        Self {
            strength: vec![0.0; map_area],
            active_chunks: Vec::with_capacity(no_of_chunks),
        }
    }
}

pub struct NestPool {
    pos: Vec<usize>,
    player_ids: Vec<u32>,
    cursor: usize,
}

impl NestPool {
    pub fn new(player_count: usize, map_area: usize) -> Self {
        let mut pos = Vec::with_capacity(player_count);
        let width = map_area.isqrt();

        for r in (0..width).step_by(32) {
            for c in (0..width).step_by(32) {
                let idx = r * width + c;
                pos.push(idx);
            }
        }

        Self {
            pos,
            player_ids: vec![0; player_count],
            cursor: 0,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Settings {
    map_area: usize,
    player_count: u32,
    ants_per_nest: u32,
    no_of_chunks: u32,
    chunks_per_player: u16,
}

fn prev_power_of_two(n: usize) -> usize {
    if n == 0 {
        0
    } else {
        1 << (usize::BITS - n.leading_zeros() - 1)
    }
}

impl Settings {
    pub fn new(player_count: usize, ants_per_nest: u32, ant_density: f32) -> Self {
        let required_area = (player_count * ants_per_nest as usize) as f32 / ant_density;
        let width = (required_area.sqrt() as usize).next_power_of_two();
        let map_area = width * width;
        let no_of_chunks = (map_area / 1024) as u32;
        let chunks_per_player = prev_power_of_two((no_of_chunks as usize / player_count).isqrt()).pow(2) as u16;
        let player_count = no_of_chunks / chunks_per_player as u32;

        Self {
            player_count,
            ants_per_nest,
            map_area,
            no_of_chunks,
            chunks_per_player,
        }
    }
}

pub struct World {
    ant_pool: AntPool,
    food_pool: FoodPool,
    pheromone_pool: PheromonePool,
    nest_pool: NestPool,
    settings: Settings,
}

impl World {
    pub fn new(settings: Settings) -> Self {
        Self {
            ant_pool: AntPool::new(settings.player_count as usize, settings.ants_per_nest as usize),
            food_pool: FoodPool::new(settings.map_area),
            pheromone_pool: PheromonePool::new(settings.map_area),
            nest_pool: NestPool::new(settings.player_count as usize, settings.map_area as usize),
            settings,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings() {
        let settings = Settings::new(500, 500, 0.05);
        assert_eq!(settings, Settings { player_count: 1024, ants_per_nest: 500, map_area: 16_777_216, no_of_chunks: 16_384, chunks_per_player: 16 } );
        let settings = Settings::new(2000, 1000, 0.05);
        assert_eq!(settings, Settings { player_count: 4096, ants_per_nest: 1000, map_area: 67_108_864, no_of_chunks: 65_536, chunks_per_player: 16 } );
        let settings = Settings::new(1000, 500, 0.05);
        assert_eq!(settings, Settings { player_count: 1024, ants_per_nest: 500, map_area: 16_777_216, no_of_chunks: 16_384, chunks_per_player: 16 } );
    }
}

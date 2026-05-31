use fastrand::Rng;

#[derive(Debug, PartialEq, Eq)]
pub struct AntPool {
    positions: Vec<usize>, // On global map
    states: Vec<u8>, // 0 for searching, 1 for returning
    nest_ids: Vec<u32>,
}

impl AntPool {
    pub fn new(player_count: usize, ants_per_nest: usize, nest_pos: &[usize]) -> Self {
        let capacity = player_count * ants_per_nest;

        let mut positions = Vec::with_capacity(capacity);
        let mut nest_ids = Vec::with_capacity(capacity);

        let mut i = 0;

        for _ in (0..capacity).step_by(ants_per_nest) {
            for _ in 0..ants_per_nest {
                positions.push(nest_pos[i]);
                nest_ids.push(i as u32);
            }
            i += 1;
        }

        Self {
            positions,
            states: vec![0; capacity],
            nest_ids,
        }
    }

}

pub struct FoodPool {
    quantities: Vec<u8>, // On global map
}

impl FoodPool {
    pub fn new(map_area: usize) -> Self {
        Self {
            quantities: vec![0; map_area],
        }
    }
}

#[derive(Debug)]
pub struct PheromonePool {
    strengths: Vec<f32>, // Pheromone strength for each chunk id. 0..1024 represents chunk 0
    active_chunks: Vec<usize>, // A chunk is 32x32 = 1024 position
    chunk_flags: Vec<u8>, // For O(1) lookups to check if given chunk is active
}

impl PheromonePool {
    pub fn new(map_area: usize) -> Self {
        let no_of_chunks = map_area / 1024;

        Self {
            strengths: vec![0.0; map_area],
            active_chunks: Vec::with_capacity(no_of_chunks),
            chunk_flags: vec![0; no_of_chunks],
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct NestPool {
    positions: Vec<usize>, // On the global map
    player_ids: Vec<u32>,
    cursor: usize,
    food_counts: Vec<u64>,
}

impl NestPool {
    pub fn new(player_count: usize, map_area: usize, chunks_per_player: u16) -> Self {
        let mut positions = Vec::with_capacity(player_count);
        let width = map_area.isqrt();
        let steps = (chunks_per_player.isqrt() * 32) as usize;

        for r in (0..width).step_by(steps) {
            for c in (0..width).step_by(steps) {
                let idx = r * width + c;
                positions.push(idx);
            }
        }

        Self {
            positions,
            player_ids: vec![0; player_count],
            cursor: 0,
            food_counts: vec![0; player_count],
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
        let mut rough_chunks_per_player = no_of_chunks as usize / player_count;
        if rough_chunks_per_player == 0 {
            rough_chunks_per_player = 1;
        }
        let chunks_per_player = prev_power_of_two(rough_chunks_per_player.isqrt()).pow(2) as u16;
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
        let nest_pool = NestPool::new(settings.player_count as usize, settings.map_area as usize, settings.chunks_per_player);

        Self {
            ant_pool: AntPool::new(settings.player_count as usize, settings.ants_per_nest as usize, &nest_pool.positions),
            food_pool: FoodPool::new(settings.map_area),
            pheromone_pool: PheromonePool::new(settings.map_area),
            nest_pool,
            settings,
        }
    }

    pub fn tick(&mut self) {
        let &mut World {
            ref mut ant_pool,
            ref mut nest_pool,
            ref mut pheromone_pool,
            ref settings,
            ref mut food_pool,
        } = self;

        World::move_ants(ant_pool, pheromone_pool, settings.map_area.isqrt(), nest_pool, food_pool, settings.no_of_chunks as usize);
        World::evaporate(pheromone_pool, 0.99);
    }

    fn move_ants(ant_pool: &mut AntPool, pheromone_pool: &mut PheromonePool, map_width: usize, nest_pool: &mut NestPool, food_pool: &mut FoodPool, no_of_chunks: usize) {
        let mask = map_width -1;
        let shift = map_width.trailing_zeros();
        let pheromone_strengths = &mut pheromone_pool.strengths;
        let nest_positions = &nest_pool.positions;
        let chunks_per_side = no_of_chunks.isqrt();
        let chunk_shift = chunks_per_side.trailing_zeros();

        let directions = [(0, 1), (1, 0), (1, 1), (0, -1), (-1, 0), (-1, -1), (1, -1), (-1, 1)];
        let mut random_generator = Rng::new();

        for i in 0..ant_pool.positions.len() {
            let current_pos = ant_pool.positions[i];
            let current_state = ant_pool.states[i];
            let nest_id = ant_pool.nest_ids[i] as usize;
            let nest_pos = nest_positions[nest_id];

            let mut chosen_pos;

            let (r, c) = World::world_idx_to_rc(current_pos, shift, mask);

            if current_state == 0 {
                let mut neighbors = [0usize; 8];
                let mut valid_count = 0;

                for (row_step, col_step) in directions.iter() {
                    let new_r = r as isize + row_step;
                    let new_c = c as isize + col_step;
                    if new_r >= 0 && new_r < map_width as isize && new_c >= 0 && new_c < map_width as isize {
                        let new_pos_idx = World::rc_to_world_idx(new_r as usize, new_c as usize, shift);
                        neighbors[valid_count] = new_pos_idx;
                        valid_count += 1;
                    }
                }

                chosen_pos = neighbors[0];

                let mut total_weight = 0.0;
                for j in 0..valid_count {
                    let neighbor = neighbors[j];
                    let (r, c) = World::world_idx_to_rc(neighbor, shift, mask);
                    let (memory_idx, _) = World::world_rc_to_chunk_meta(r, c, chunk_shift);
                    total_weight += 1.0 + pheromone_strengths[memory_idx];
                }

                let k = random_generator.f32_inclusive() * total_weight;
                let mut cur = 0.0;

                for j in 0..valid_count {
                    let neighbor = neighbors[j];
                    let (r, c) = World::world_idx_to_rc(neighbor, shift, mask);
                    let (memory_idx, _) = World::world_rc_to_chunk_meta(r, c, chunk_shift);
                    cur += 1.0 + pheromone_strengths[memory_idx];
                    if cur >= k {
                        chosen_pos = neighbor;
                        break;
                    }
                }
            } else {
                let (r_nest, c_nest) = World::world_idx_to_rc(nest_pos, shift, mask);

                let r_diff = r_nest as isize - r as isize;
                let c_diff = c_nest as isize - c as isize;

                let row_step = r_diff.signum();
                let col_step = c_diff.signum();

                let new_r = r as isize + row_step;
                let new_c = c as isize + col_step;

                chosen_pos = World::rc_to_world_idx(new_r as usize, new_c as usize, shift);

                let (memory_idx, chunk_idx) = World::world_rc_to_chunk_meta(r, c, chunk_shift);
                pheromone_strengths[memory_idx] += 10.0;

                if pheromone_pool.chunk_flags[chunk_idx] == 0 {
                    pheromone_pool.chunk_flags[chunk_idx] = 1;
                    pheromone_pool.active_chunks.push(chunk_idx);
                }
            }

            ant_pool.positions[i] = chosen_pos;
            if current_state == 1 && chosen_pos == nest_pos {
                nest_pool.food_counts[nest_id] += 2;
                ant_pool.states[i] = 0;
            }
            if current_state == 0 && food_pool.quantities[chosen_pos] > 1 {
                food_pool.quantities[chosen_pos] -= 2;
                ant_pool.states[i] = 1;
            }
        }
    }

    fn world_idx_to_rc(world_idx: usize, shift: u32, mask: usize) -> (usize, usize) {
        (world_idx >> shift, world_idx & mask)
    }

    fn rc_to_world_idx(r: usize, c: usize, shift: u32) -> usize {
        r << shift | c
    }

    fn world_rc_to_chunk_meta(r: usize, c: usize, chunk_shift: u32) -> (usize, usize) {
        let chunk_r = r >> 5;
        let chunk_c = c >> 5;
        let chunk_idx = chunk_r << chunk_shift | chunk_c;

        let chunk_local_r = r & 31;
        let chunk_local_c = c & 31;
        let chunk_local_idx = chunk_local_r << 5 | chunk_local_c;

        (((chunk_idx << 10) + chunk_local_idx), chunk_idx)
    }

    fn evaporate(pheromone_pool: &mut PheromonePool, evaporation_strength: f32) {

        pheromone_pool.active_chunks.retain(|&chunk_id| {
            let start_idx = chunk_id << 10;

            let mut chunk_is_empty = true;
            for i in 0..1024 {
                let idx = start_idx + i;
                if pheromone_pool.strengths[idx] > 0.0 {
                    pheromone_pool.strengths[idx] *= evaporation_strength;
                    if pheromone_pool.strengths[idx] > 0.01 {
                        chunk_is_empty = false;
                    } else {
                        pheromone_pool.strengths[idx] = 0.0;
                    }
                }
            }

            if chunk_is_empty {
                pheromone_pool.chunk_flags[chunk_id] = 0;
            }

            !chunk_is_empty
        });
    }

    pub fn add_food(&mut self, idx: usize, amount: u8) {
        self.food_pool.quantities[idx] = amount;
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_settings() {
        let settings = Settings::new(500, 500, 0.05);
        assert_eq!(settings, Settings { player_count: 1024, ants_per_nest: 500, map_area: 16_777_216, no_of_chunks: 16_384, chunks_per_player: 16 } );
        let settings = Settings::new(2000, 1000, 0.05);
        assert_eq!(settings, Settings { player_count: 4096, ants_per_nest: 1000, map_area: 67_108_864, no_of_chunks: 65_536, chunks_per_player: 16 } );
        let settings = Settings::new(1000, 500, 0.05);
        assert_eq!(settings, Settings { player_count: 1024, ants_per_nest: 500, map_area: 16_777_216, no_of_chunks: 16_384, chunks_per_player: 16 } );
    }

    #[test]
    fn world() {
        let settings = Settings::new(4, 1, 0.001);
        println!("{:?}", settings);

        let mut world = World::new(settings);

        // NestPool
        let nest_pool = &world.nest_pool;
        assert_eq!(nest_pool.positions, vec![0, 32, 2048, 2080]);
        assert_eq!(nest_pool.player_ids, vec![0; 4]);
        assert_eq!(nest_pool.cursor, 0);
        assert_eq!(nest_pool.food_counts, vec![0; 4]);

        // AntPool
        let ant_pool = &world.ant_pool;
        assert_eq!(ant_pool.positions, vec![0, 32, 2048, 2080]);
        assert_eq!(ant_pool.states, vec![0; 4]);
        assert_eq!(ant_pool.nest_ids, vec![0, 1, 2, 3]);
        assert_eq!(ant_pool.positions, nest_pool.positions);
        assert_eq!(nest_pool.positions[ant_pool.nest_ids[0] as usize], ant_pool.positions[0]);

        // PheromonePool
        let pheromone_pool = &world.pheromone_pool;
        assert_eq!(pheromone_pool.strengths, vec![0.0; 4096]);
        assert_eq!(pheromone_pool.chunk_flags, vec![0; world.settings.no_of_chunks as usize]);

        // FoodPool
        let food_pool = &world.food_pool;
        assert_eq!(food_pool.quantities, vec![0; 4096]);

        world.add_food(65, 255);
        assert_eq!(world.food_pool.quantities[65], 255);

        // Movement
        world.tick();
        assert_ne!(world.ant_pool.positions, vec![0, 32, 2048, 2080]);
    }
}

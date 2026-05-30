use fastrand::Rng;

#[derive(Debug, PartialEq, Eq)]
pub struct AntPool {
    pos: Vec<usize>,
    state: Vec<u8>,
    nest_ids: Vec<u32>,
}

impl AntPool {
    pub fn new(player_count: usize, ants_per_nest: usize, nest_pos: &[usize]) -> Self {
        let capacity = player_count * ants_per_nest;

        let mut pos = Vec::with_capacity(capacity);
        let mut nest_ids = Vec::with_capacity(capacity);

        let mut i = 0;

        for _ in (0..capacity).step_by(ants_per_nest) {
            for _ in 0..ants_per_nest {
                pos.push(nest_pos[i]);
                nest_ids.push(i as u32);
            }
            i += 1;
        }

        Self {
            pos,
            state: vec![0; capacity],
            nest_ids,
        }
    }

    pub fn move_ants(&mut self, pheromone_strengths: &[f32], map_width: usize, nest_positions: &[usize]) {
        let mask = map_width -1;
        let shift = map_width.trailing_zeros();

        let directions = [(0, 1), (1, 0), (1, 1), (0, -1), (-1, 0), (-1, -1), (1, -1), (-1, 1)];
        let mut random_generator = Rng::new();

        for i in 0..self.pos.len() {
            let current_pos = self.pos[i];
            let current_state = self.state[i];
            let mut chosen_pos;

            let r = current_pos >> shift;
            let c = current_pos & mask;

            if current_state == 0 {
                let mut neighbors = [0usize; 8];
                let mut valid_count = 0;

                for (row_step, col_step) in directions.iter() {
                    let new_r = r as isize + row_step;
                    let new_c = c as isize + col_step;
                    if new_r >= 0 && new_r < map_width as isize && new_c >= 0 && new_c < map_width as isize {
                        let new_pos_idx = ((new_r as usize) << shift) | (new_c as usize);
                        neighbors[valid_count] = new_pos_idx;
                        valid_count += 1;
                    }
                }

                chosen_pos = neighbors[0];

                let mut total_weight = 0.0;
                for j in 0..valid_count {
                    let neighbor = neighbors[j];
                    total_weight += 1.0 + pheromone_strengths[neighbor];
                }

                let k = random_generator.f32_inclusive() * total_weight;
                let mut cur = 0.0;

                for j in 0..valid_count {
                    let neighbor = neighbors[j];
                    cur += 1.0 + pheromone_strengths[neighbor];
                    if cur >= k {
                        chosen_pos = neighbor;
                        break;
                    }
                }
            } else {
                let nest_id = self.nest_ids[i];
                let nest_pos = nest_positions[nest_id];

                let r_nest = nest_pos >> shift;
                let c_nest = nest_pos & mask;

                let r_diff = r_nest as isize - r as isize;
                let c_diff = c_nest as isize - c as isize;

                let row_step = r_diff.signum();
                let col_step = c_diff.signum();

                let new_r = r as isize + row_step;
                let new_c = c as isize + col_step;

                chosen_pos = ((new_r as usize) << shift) | (new_c as usize)
            }

            self.pos[i] = chosen_pos;
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

    pub fn evaporate(&mut self, evaporation_strength: f32, map_width: usize, no_of_chunks: usize) {
        let chunks_per_side = no_of_chunks.isqrt();

        self.active_chunks.retain(|&chunk_id| {
            let chunk_r = chunk_id / chunks_per_side;
            let chunk_c = chunk_id % chunks_per_side;
            let world_r = chunk_r * 32;
            let world_c = chunk_c * 32;
            let world_idx = world_r * map_width + world_c;

            let mut chunk_is_empty = true;
            for r in 0..32 {
                let row_idx = world_idx + r * map_width;
                for c in 0..32 {
                    let idx = row_idx + c;
                    if self.strength[idx] > 0.0 {
                        self.strength[idx] *= evaporation_strength;
                        if self.strength[idx] > 0.01 {
                            chunk_is_empty = false;
                        } else {
                            self.strength[idx] = 0.0;
                        }
                    }
                }
            }

            !chunk_is_empty
        });
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct NestPool {
    pos: Vec<usize>,
    player_ids: Vec<u32>,
    cursor: usize,
}

impl NestPool {
    pub fn new(player_count: usize, map_area: usize, chunks_per_player: u16) -> Self {
        let mut pos = Vec::with_capacity(player_count);
        let width = map_area.isqrt();
        let steps = (chunks_per_player.isqrt() * 32) as usize;

        for r in (0..width).step_by(steps) {
            for c in (0..width).step_by(steps) {
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
            ant_pool: AntPool::new(settings.player_count as usize, settings.ants_per_nest as usize, &nest_pool.pos),
            food_pool: FoodPool::new(settings.map_area),
            pheromone_pool: PheromonePool::new(settings.map_area),
            nest_pool,
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

    #[test]
    fn nest_pool() {
        let nest_pool = NestPool::new(4, 4096, 1);
        assert_eq!(nest_pool.pos.len(), 4);
        assert_eq!(nest_pool.pos, vec![0, 32, 2048, 2080]);
        assert_eq!(nest_pool.player_ids, vec![0; 4]);
        assert_eq!(nest_pool.cursor, 0);
    }

    #[test]
    fn ant_pool() {
        let ant_pool = AntPool::new(4, 1, &vec![0, 32, 2048, 2080]);
        let nest_pool = NestPool::new(4, 4096, 1);
        assert_eq!(ant_pool.pos.len(), 4);
        assert_eq!(ant_pool.pos, vec![0, 32, 2048, 2080]);
        assert_eq!(ant_pool.state, vec![0; 4]);
        assert_eq!(ant_pool.nest_ids, vec![0, 1, 2, 3]);
        assert_eq!(ant_pool.pos, nest_pool.pos);
        assert_eq!(nest_pool.pos[ant_pool.nest_ids[0] as usize], ant_pool.pos[0]);
    }

    #[test]
    fn pheromone_pool() {
        let pheromone_pool = PheromonePool::new(4096);
    }
}

//! A high performance, zero allocation Entity-Component-System (ECS) engine 
//! for large scale ant colony simulations.
//!
//! This engine is built on Data Oriented Design (DOD) principles. It utilizes 
//! flat, cache aligned memory pools (Structure of Arrays), block major ordering 
//! for spatial partitioning, and bitwise mathematics for pathfinding. 
//! All memory is allocated at the boot time to prevent heap fragmentation and 
//! guarantee deterministic execution during the simulation loop.

use fastrand::Rng;
use std::collections::VecDeque;
use forage_network::{ChunkSnapshot, ChunkDelta};

#[derive(Debug, PartialEq, Eq)]
struct AntPool {
    positions: Vec<usize>, // Data represents index of global map cells
    states: Vec<u8>, // 0 for searching, 1 for returning
    nest_ids: Vec<u32>,
    // One bit reprsenting presence of an ant in the corresponding cell.
    // One field represents 64 cells. So 16 fields are needed to represent one chunk of 1024 cells.
    ant_bitboards: Vec<u64>,
}

impl AntPool {
     fn new(player_count: usize, ants_per_nest: usize, nest_pos: &[usize], no_of_chunks: usize) -> Self {
        let capacity = player_count * ants_per_nest;

        let mut positions = Vec::with_capacity(capacity);
        let mut nest_ids = Vec::with_capacity(capacity);

        let mut i = 0;

        for j in 0..capacity {
            positions.push(nest_pos[i]);
            nest_ids.push(i as u32);
            // Increase nest id when the current nest is full.
            if j % ants_per_nest == ants_per_nest-1 { i += 1; }
        }

        Self {
            positions,
            states: vec![0; capacity],
            nest_ids,
            ant_bitboards: vec![0; no_of_chunks << 4], // no_of_chunks * 16
        }
    }

}

struct FoodPool {
    quantities: Vec<u8>, // Food quantities for each index of a chunk. 0..1024 represents chunk and so on.
    food_bitboards: Vec<u64>, // Tracks food changes every tick
}

impl FoodPool {
    fn new(settings: &Settings) -> Self {
        Self {
            quantities: vec![0; settings.map_area],
            food_bitboards: vec![0; (settings.no_of_chunks as usize) << 4usize], // no_of_chunks * 16
        }
    }
}

#[derive(Debug)]
struct PheromonePool {
    strengths: Vec<u8>, // Pheromone strength for each index of a chunk. 0..1024 represents chunk 0
    active_chunks: Vec<usize>, // A chunk is 32x32 = 1024 position
    chunk_flags: Vec<u8>, // For O(1) lookups to check if given chunk is active
    pheromone_bitboards: Vec<u64>, // Represents changed pheromones this tick. (Not for evaporation)
}

impl PheromonePool {
    fn new(map_area: usize) -> Self {
        let no_of_chunks = map_area / 1024;

        Self {
            strengths: vec![0; map_area],
            active_chunks: Vec::with_capacity(no_of_chunks),
            chunk_flags: vec![0; no_of_chunks],
            pheromone_bitboards: vec![0; no_of_chunks << 4] // no_of_chunks * 16
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct NestPool {
    positions: Vec<usize>,
    cursor: usize, // Tracks active player count and serves as the allocation index for the next joining player
    free_list: VecDeque<usize>,
    active_nests: Vec<u8>,
    food_counts: Vec<u64>,
}

impl NestPool {
    fn new(player_count: usize, map_area: usize, chunks_per_player: u16) -> Self {
        let mut positions = Vec::with_capacity(player_count);
        let width = map_area.isqrt();
        let steps = (chunks_per_player.isqrt() * 32) as usize; // Horizontal and vertical distance between nests

        for r in (0..width).step_by(steps) {
            for c in (0..width).step_by(steps) {
                let idx = r * width + c;
                positions.push(idx);
            }
        }

        Self {
            positions,
            cursor: 0,
            free_list: VecDeque::with_capacity(player_count),
            active_nests: vec![0; player_count],
            food_counts: vec![0; player_count],
        }
    }
}

/// Defines the structural geometry and mathematical constraints of the simulation map.
///
/// The engine enforces strictly power of two map dimensions and territory distributions.
/// This allowed the maths to replace expensive ALU operations to bitwise operations.
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
    /// Bootstraps the map geometry based on desired player count and ant density.
    ///
    /// Instead of hardcoding a map size, this calculates the necessary surface area 
    /// to maintain the requested density, and then strictly snaps the map width and 
    /// chunk allocations to the next optimal power of two.
    ///
    /// # Examples
    ///
    /// ```
    /// use forage_core::Settings;
    ///
    /// // Initialize for 1000 players, 500 ants each, at 5% map density
    /// let settings = Settings::new(1000, 500, 0.05);
    ///
    /// assert_eq!(format!("{:?}", settings), "Settings { map_area: 16777216, player_count: 1024, ants_per_nest: 500, no_of_chunks: 16384, chunks_per_player: 16 }".to_string() );
    /// ```
    pub fn new(player_count: usize, ants_per_nest: u32, ant_density: f32) -> Self {
        let required_area = (player_count * ants_per_nest as usize) as f32 / ant_density;

        // Map area should be a square that is a power of two, for easier calculations
        let width = (required_area.sqrt() as usize).next_power_of_two();
        let map_area = width * width;

        let no_of_chunks = (map_area / 1024) as u32; // Chunks are 32*32 = 1024 cells
        let mut rough_chunks_per_player = no_of_chunks as usize / player_count;

        // Chunks per player can't be zero
        if rough_chunks_per_player == 0 { rough_chunks_per_player = 1; }

        // Chunks per player represent a territory,
        // This territory should also be a square that is a power of two.
        // This fills up the whole map area, leaving nothing to waste.
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

    pub fn get_map_area(&self) -> usize {
        self.map_area
    }

    pub fn get_player_count(&self) -> u32 {
        self.player_count
    }

    pub fn get_ants_per_nest(&self) -> u32 {
        self.ants_per_nest
    }

    pub fn get_no_of_chunks(&self) -> u32 {
        self.no_of_chunks
    }

    pub fn get_chunks_per_player(&self) -> u16 {
        self.chunks_per_player
    }
}

/// The master system orchestrator and ECS state container.
///
/// `World` acts as the black box boundary for the simulation. It owns all memory 
/// pools (Ants, Nests, Pheromones, Food) and safely mutates intersecting systems
/// simultaneously without runtime lock contention.
pub struct World {
    ant_pool: AntPool,
    food_pool: FoodPool,
    pheromone_pool: PheromonePool,
    nest_pool: NestPool,
    settings: Settings,
    random_generator: Rng,
    tick_count: u16,
}

impl World {
    /// Allocates and initializes all ECS memory pools based on the provided settings.
    ///
    /// Once `World::new` resolves, all vectors are pre-warmed to their 
    /// maximum required capacity. No further heap allocations will 
    /// occur during standard simulation ticks.
    ///
    /// # Examples
    ///
    /// ```
    /// use forage_core::{Settings, World};
    ///
    /// let settings = Settings::new(4, 100, 0.05);
    /// let mut world = World::new(settings);
    /// ```
    pub fn new(settings: Settings) -> Self {
        let nest_pool = NestPool::new(settings.player_count as usize, settings.map_area as usize, settings.chunks_per_player);
        let random_generator = Rng::new();

        Self {
            ant_pool: AntPool::new(settings.player_count as usize, settings.ants_per_nest as usize, &nest_pool.positions, settings.no_of_chunks as usize),
            food_pool: FoodPool::new(&settings),
            pheromone_pool: PheromonePool::new(settings.map_area),
            nest_pool,
            settings,
            random_generator,
            tick_count: 0,
        }
    }

    /// Advances the simulation state by a single discrete time step.
    ///
    /// This is the master heartbeat of the ECS engine.
    /// It safely passes multiple mutable arrays into systems (movement and evaporation) simultaneously.
    ///
    /// The pipeline executes in a strict chronological order:
    /// 1. **Movement Phase:** Ants process probabilities, move, drop pheromones, harvest and store food.
    /// 2. **Evaporation Phase:** Evaporates the active pheromones on the map.
    ///
    /// # Examples
    ///
    /// ```
    /// use forage_core::{Settings, World};
    ///
    /// let mut world = World::new(Settings::new(4, 100, 0.05));
    /// 
    /// // Advance the engine by one frame
    /// world.tick();
    /// ```
    pub fn tick(&mut self) {
        let &mut World {
            ref mut ant_pool,
            ref mut nest_pool,
            ref mut pheromone_pool,
            ref settings,
            ref mut food_pool,
            ref mut random_generator,
            ref mut tick_count,
        } = self;
        *tick_count += 1;
        World::move_ants(ant_pool, pheromone_pool, settings, nest_pool, food_pool, random_generator);
        if *tick_count == 10 {
            World::evaporate(pheromone_pool, 1);
            *tick_count = 0;
        }
    }

    fn move_ants(ant_pool: &mut AntPool, pheromone_pool: &mut PheromonePool, settings: &Settings, nest_pool: &mut NestPool, food_pool: &mut FoodPool, random_generator: &mut Rng) {
        let map_width = settings.map_area.isqrt();
        let no_of_chunks = settings.no_of_chunks;

        let mask = map_width - 1;
        let shift = map_width.trailing_zeros();
        let pheromone_strengths = &mut pheromone_pool.strengths;
        let nest_positions = &nest_pool.positions;
        let chunks_per_side = no_of_chunks.isqrt();
        let chunk_shift = chunks_per_side.trailing_zeros();

        ant_pool.ant_bitboards.fill(0);
        pheromone_pool.pheromone_bitboards.fill(0);
        food_pool.food_bitboards.fill(0);

        const DIRECTIONS: [(isize, isize); 8] = [(0, 1), (1, 0), (1, 1), (0, -1), (-1, 0), (-1, -1), (1, -1), (-1, 1)];
        let active_ants = nest_pool.cursor as usize * settings.ants_per_nest as usize;

        for i in 0..active_ants {
            let nest_id = ant_pool.nest_ids[i] as usize;
            let nest_active = nest_pool.active_nests[nest_id] == 1;
            let current_pos = ant_pool.positions[i];
            let current_state = ant_pool.states[i];
            let nest_pos = nest_positions[nest_id];

            if !nest_active && current_pos == nest_pos { continue; }

            let mut chosen_pos;
            let (r, c) = World::world_idx_to_rc(current_pos, shift, mask);

            if current_state == 0 && nest_active { // Searching
                let mut neighbors = [0usize; 8];
                let mut neighbor_memory_idxs = [0usize; 8];
                let mut valid_count = 0;

                for (row_step, col_step) in DIRECTIONS.iter() {
                    let new_r = r as isize + row_step;
                    let new_c = c as isize + col_step;
                    if new_r >= 0 && new_r < map_width as isize && new_c >= 0 && new_c < map_width as isize {
                        let new_r = new_r as usize;
                        let new_c = new_c as usize;
                        neighbors[valid_count] = World::rc_to_world_idx(new_r, new_c, shift);
                        let (chunk_local_idx, chunk_idx) = World::world_rc_to_chunk_meta(new_r, new_c, chunk_shift);
                        let memory_idx = (chunk_idx << 10) + chunk_local_idx;
                        neighbor_memory_idxs[valid_count] = memory_idx;
                        valid_count += 1;
                    }
                }

                let mut weights = [0u16; 8];
                let mut total_weight = 0u16;
                for j in 0..valid_count {
                    let w = 1 + pheromone_strengths[neighbor_memory_idxs[j]] as u16;
                    weights[j] = w;
                    total_weight += w;
                }

                chosen_pos = neighbors[0];
                let k = random_generator.u16(0..=total_weight);
                let mut cur = 0u16;
                for j in 0..valid_count {
                    cur += weights[j];
                    if cur >= k {
                        chosen_pos = neighbors[j];
                        break;
                    }
                }
            } else { // Returning
                let (r_nest, c_nest) = World::world_idx_to_rc(nest_pos, shift, mask);

                let row_step = (r_nest as isize - r as isize).signum();
                let col_step = (c_nest as isize - c as isize).signum();

                let new_r = (r as isize + row_step) as usize;
                let new_c = (c as isize + col_step) as usize;

                chosen_pos = World::rc_to_world_idx(new_r, new_c, shift);

                if nest_active {
                    let (chunk_local_idx, chunk_idx) = World::world_rc_to_chunk_meta(r, c, chunk_shift);
                    let memory_idx = (chunk_idx << 10) + chunk_local_idx;
                    let start = chunk_idx << 4;
                    let board_idx = chunk_local_idx >> 6;
                    let bit_idx = chunk_local_idx & 63;
                    let current_board = pheromone_pool.pheromone_bitboards[start + board_idx];
                    let is_set = (current_board >> bit_idx) & 1;
                    let strength = (is_set ^ 1) * 10;

                    pheromone_strengths[memory_idx] = pheromone_strengths[memory_idx].saturating_add(strength as u8);
                    pheromone_pool.pheromone_bitboards[start + board_idx] |= 1u64 << bit_idx;

                    if pheromone_pool.chunk_flags[chunk_idx] == 0 {
                        pheromone_pool.chunk_flags[chunk_idx] = 1;
                        pheromone_pool.active_chunks.push(chunk_idx);
                    }
                }
            }

            ant_pool.positions[i] = chosen_pos;

            let (chose_r, chose_c) = World::world_idx_to_rc(chosen_pos, shift, mask);
            let (chunk_local_idx, chunk_idx) = World::world_rc_to_chunk_meta(chose_r, chose_c, chunk_shift);
            let memory_idx = (chunk_idx << 10) + chunk_local_idx;
            let start = chunk_idx << 4;
            let board_idx = chunk_local_idx >> 6;
            let bit_idx = chunk_local_idx & 63;

            if nest_active && current_state == 1 && chosen_pos == nest_pos {
                nest_pool.food_counts[nest_id] += 2;
                ant_pool.states[i] = 0;
            }
            if nest_active && current_state == 0 && food_pool.quantities[memory_idx] > 1 {
                food_pool.quantities[memory_idx] -= 2;
                food_pool.food_bitboards[start + board_idx] |= 1u64 << bit_idx;
                ant_pool.states[i] = 1;
            }
            if chosen_pos != nest_pos {
                ant_pool.ant_bitboards[start + board_idx] |= 1u64 << bit_idx;
            }
        }
    }

    /// Converts a flat 1D global world index into 2D row and column coordinates.
    ///
    /// Uses high speed bitwise shifting and masking. The `shift` and `mask` 
    /// parameters must be pre-calculated from the map width's trailing zeros.
    ///
    /// # Examples
    ///
    /// ```
    /// use forage_core::World;
    ///
    /// let map_width = 4096usize;
    /// let shift = map_width.trailing_zeros();
    /// let mask = map_width - 1;
    /// 
    /// let (r, c) = World::world_idx_to_rc(4097, shift, mask);
    /// assert_eq!((r, c), (1, 1));
    /// ```
    pub fn world_idx_to_rc(world_idx: usize, shift: u32, mask: usize) -> (usize, usize) {
        (world_idx >> shift, world_idx & mask)
    }

    /// Flattens 2D row and column coordinates into a 1D global world index.
    ///
    /// # Examples
    ///
    /// ```
    /// use forage_core::World;
    ///
    /// let map_width = 4096usize;
    /// let shift = map_width.trailing_zeros();
    /// 
    /// let idx = World::rc_to_world_idx(1, 1, shift);
    /// assert_eq!(idx, 4097);
    /// ```
    pub fn rc_to_world_idx(r: usize, c: usize, shift: u32) -> usize {
        r << shift | c
    }

    /// Translates global 2D coordinates into Block Major (Tiled) memory addresses.
    ///
    /// To maximize L1 cache hits during evaporation sweeps, pheromone data is stored 
    /// in contiguous 32x32 chunks rather than row major order. This function extracts 
    /// the local chunk coordinates and returns the physical memory index alongside 
    /// the broad phase chunk ID.
    ///
    /// # Examples
    ///
    /// ```
    /// use forage_core::World;
    ///
    /// let chunks_per_side = 4usize;
    /// let chunk_shift = chunks_per_side.trailing_zeros();
    /// let r = 1;
    /// let c = 1;
    /// let (chunk_local_idx, chunk_idx) = World::world_rc_to_chunk_meta(r, c, chunk_shift);
    /// assert_eq!(chunk_local_idx, 33);
    /// assert_eq!(chunk_idx, 0);
    /// ```
    pub fn world_rc_to_chunk_meta(r: usize, c: usize, chunk_shift: u32) -> (usize, usize) {
        let chunk_r = r >> 5; // Chunk is 32x32
        let chunk_c = c >> 5;
        let chunk_idx = chunk_r << chunk_shift | chunk_c;

        let chunk_local_r = r & 31;
        let chunk_local_c = c & 31;
        let chunk_local_idx = chunk_local_r << 5 | chunk_local_c;

        (chunk_local_idx, chunk_idx)
    }

    fn evaporate(pheromone_pool: &mut PheromonePool, evaporation_strength: u8) {
        let mut i = 0;

        while i < pheromone_pool.active_chunks.len() {
            let chunk_id = pheromone_pool.active_chunks[i];
            let start_idx = chunk_id << 10;

            let chunk = &mut pheromone_pool.strengths[start_idx..start_idx + 1024];
            let mut chunk_is_empty = true;

            for s in chunk.iter_mut() {
                *s = s.saturating_sub(evaporation_strength);
                chunk_is_empty &= *s == 0;
            }

            if chunk_is_empty {
                pheromone_pool.chunk_flags[chunk_id] = 0;
                pheromone_pool.active_chunks.swap_remove(i);
            } else {
                i += 1;
            }
        }
    }

    /// Spawns a concentrated unit of food at the specified global index.
    ///
    /// *Note: Given amount should always be even, if it is odd, 
    /// it'd automatically be converted to even*
    ///
    /// # Examples
    ///
    /// ```
    /// use forage_core::{Settings, World};
    ///
    /// let mut world = World::new(Settings::new(4, 100, 0.05));
    ///
    /// world.add_food(0, 1, 254);
    /// let food_quantities = world.get_food_quantities();
    /// assert_eq!(food_quantities[1], 254);
    /// ```
    pub fn add_food(&mut self, chunk_idx: usize, chunk_local_idx: usize, amount: u8) {
        if chunk_idx >= self.settings.no_of_chunks as usize || chunk_local_idx >= 1024 {
            return; // To Do: Return an Error
        }

        let start = chunk_idx << 4;
        let board_idx = chunk_local_idx >> 6;
        let bit_idx = chunk_local_idx & 63;
        self.food_pool.food_bitboards[start + board_idx] |= 1u64 << bit_idx;

        let memory_idx = (chunk_idx << 10) + chunk_local_idx;
        self.food_pool.quantities[memory_idx] = amount + (amount & 1);
    }

    /// Adds a new player to the simulation, allocating a nest and its corresponding ants.
    /// 
    /// Returns an error if the maximum player count has already been reached.
    ///
    /// # Examples
    ///
    /// ```
    /// use forage_core::{Settings, World};
    ///
    /// let mut world = World::new(Settings::new(4, 1, 0.001));
    /// 
    /// let _ = world.add_player();
    /// let _ = world.add_player();
    /// let _ = world.add_player();
    /// let _ = world.add_player();
    /// let Err(_) = world.add_player() else { panic!("Should not allow more than 4 players") };
    /// ```
    pub fn add_player(&mut self) -> Result<usize, &'static str> {
        if let Some(id) = self.nest_pool.free_list.pop_back() {
            self.nest_pool.active_nests[id] = 1;
            Ok(id)
        } else {
            if self.nest_pool.cursor >= self.settings.player_count as usize {
                return Err("Maximum player count reached");
            }
            let id = self.nest_pool.cursor;
            self.nest_pool.active_nests[id] = 1;
            self.nest_pool.cursor += 1;
            Ok(id)
        }
    }

    /// Removes the given player from the map by deactivating their nest and returning their slot to the free list.
    ///
    /// # Examples
    ///
    /// ```
    /// use forage_core::{Settings, World};
    ///
    /// let mut world = World::new(Settings::new(4, 1, 0.001));
    /// 
    /// let _ = world.add_player();
    /// world.remove_player(0);
    pub fn remove_player(&mut self, id: usize) {
        self.nest_pool.active_nests[id] = 0;
        self.nest_pool.food_counts[id] = 0;
        self.nest_pool.free_list.push_front(id);
    }

    /// Returns an immutable slice of all ant global map positions.
    /// 
    /// # Examples
    /// 
    /// ```
    /// use forage_core::{Settings, World};
    ///
    /// let world = World::new(Settings::new(4, 1, 0.001));
    ///
    /// let ant_positions = world.get_ant_positions();
    /// assert_eq!(ant_positions, vec![0, 32, 2048, 2080]);
    /// ```
    pub fn get_ant_positions(&self) -> &[usize] {
        &self.ant_pool.positions
    }

    /// Returns an immutable slice of the ant bitboard.
    ///
    /// The ant bitboards represent presence of ant on a given position per tick.
    /// 1 bit represents one position, one field represents 64 positions, 16 fields represent one chunk.
    /// 
    /// # Examples
    /// 
    /// ```
    /// use forage_core::{Settings, World};
    ///
    /// let world = World::new(Settings::new(4, 1, 0.001));
    ///
    /// let ant_bitboards = world.get_ant_bitboards();
    /// assert_eq!(ant_bitboards, vec![0; 4 << 4]);
    /// ```
    pub fn get_ant_bitboards(&self) -> &[u64] {
        &self.ant_pool.ant_bitboards
    }

    /// Returns an immutable slice of all nest global map positions.
    ///
    /// # Examples
    /// 
    /// ```
    /// use forage_core::{Settings, World};
    ///
    /// let world = World::new(Settings::new(4, 1, 0.001));
    ///
    /// let nest_positions = world.get_nest_positions();
    /// assert_eq!(nest_positions, vec![0, 32, 2048, 2080]);
    /// ```
    pub fn get_nest_positions(&self) -> &[usize] {
        &self.nest_pool.positions
    }

    /// Returns an immutable slice of the entire dense food grid.
    ///
    /// # Examples
    /// 
    /// ```
    /// use forage_core::{Settings, World};
    /// 
    /// let world = World::new(Settings::new(4, 1, 0.001));
    ///
    /// let food_quantities = world.get_food_quantities();
    /// assert_eq!(food_quantities, vec![0; 4096]);
    /// ```
    pub fn get_food_quantities(&self) -> &[u8] {
        &self.food_pool.quantities
    }


    /// Returns an immutable slice of the food bitboard.
    ///
    /// The food bitboards represent changes in food quantity on a given position per tick.
    /// 1 bit represents one position, one field represents 64 positions, 16 fields represent one chunk.
    ///
    /// # Examples
    /// 
    /// ```
    /// use forage_core::{Settings, World};
    /// 
    /// let world = World::new(Settings::new(4, 1, 0.001));
    ///
    /// let food_bitboards = world.get_food_bitboards();
    /// assert_eq!(food_bitboards, vec![0; 4 << 4]);
    /// ```
    pub fn get_food_bitboards(&self) -> &[u64] {
        &self.food_pool.food_bitboards
    }

    /// Returns an immutable slice of the Block Major ordered pheromone grid.
    /// 
    /// Note: This array is NOT sorted in row major global indices. Renderers 
    /// must translate via `world_rc_to_chunk_meta` or iterate block by block.
    ///
    /// # Examples
    ///
    /// ```
    /// use forage_core::{Settings, World};
    ///
    /// let world = World::new(Settings::new(4, 1, 0.001));
    ///
    /// let pheromone_strengths = world.get_pheromone_strengths();
    /// assert_eq!(pheromone_strengths, vec![0; 4096]);
    /// ```
    pub fn get_pheromone_strengths(&self) -> &[u8] {
        &self.pheromone_pool.strengths
    }

    /// Returns an immutable slice mapping each nest to its successfully returned food count.
    ///
    /// # Examples
    ///
    /// ```
    /// use forage_core::{Settings, World};
    ///
    /// let world = World::new(Settings::new(4, 1, 0.001));
    ///
    /// let nest_food_counts = world.get_nest_food_counts();
    /// assert_eq!(nest_food_counts, vec![0; 4]);
    /// ```
    pub fn get_nest_food_counts(&self) -> &[u64] {
        &self.nest_pool.food_counts
    }

    /// Extracts a complete, standalone state of a 32x32 chunk for new network subscriptions.
    ///
    /// This function is used to build the `Welcome` payload or when a client pans 
    /// their camera into a newly visible region.
    ///
    /// # Examples
    ///
    /// ```
    /// use forage_core::{Settings, World};
    /// use forage_network::ChunkSnapshot;
    ///
    /// let mut world = World::new(Settings::new(4, 1, 0.001));
    ///
    /// world.tick();
    /// world.tick();
    /// let snapshot = world.get_snapshot(1);
    /// assert_eq!(1, snapshot.chunk_idx);
    /// ```
    pub fn get_snapshot(&self, chunk_idx: u32) -> ChunkSnapshot {
        let ant_start = (chunk_idx as usize) << 4;
        let pheromone_start = (chunk_idx as usize) << 10;

        let mut ant_bitboards = [0u64; 16];
        ant_bitboards.copy_from_slice(&self.ant_pool.ant_bitboards[ant_start..ant_start+16]);

        let mut pheromone_strengths = [0u8; 1024];
        pheromone_strengths.copy_from_slice(&self.pheromone_pool.strengths[pheromone_start..pheromone_start+1024]);

        let mut food_quantities = Vec::new();
        for local_idx in 0..1024 {
            let memory_idx = pheromone_start + local_idx;
            if self.food_pool.quantities[memory_idx] > 1 {
                food_quantities.push((local_idx as u16, self.food_pool.quantities[memory_idx]));
            }
        }

        ChunkSnapshot {
            chunk_idx,
            ant_bitboards,
            pheromone_strengths,
            food_quantities
        }
    }

    /// Extracts the minimal state changes (Deltas) for a 32x32 chunk over the last tick.
    ///
    /// # Examples
    ///
    /// ```
    /// use forage_core::{Settings, World};
    /// use forage_network::ChunkSnapshot;
    ///
    /// let mut world = World::new(Settings::new(4, 1, 0.001));
    ///
    /// world.tick();
    /// world.tick();
    /// let delta = world.get_delta(1);
    /// assert_eq!(1, delta.chunk_idx);
    /// ```
    pub fn get_delta(&self, chunk_idx: u32) -> ChunkDelta {
        let start = (chunk_idx as usize) << 4;

        let mut ant_bitboards = [0u64; 16];
        ant_bitboards.copy_from_slice(&self.ant_pool.ant_bitboards[start..start+16]);

        let mut pheromone_bitboards = [0u64; 16];
        pheromone_bitboards.copy_from_slice(&self.pheromone_pool.pheromone_bitboards[start..start+16]);

        let mut dirty_food = Vec::new();
        let board_idx = (chunk_idx as usize) << 10;
        for i in 0..16 {
            let mut board = self.food_pool.food_bitboards[start + i];
            while board != 0 {
                let trailing = board.trailing_zeros();
                let local_idx = (i << 6) + trailing as usize;
                let val = self.food_pool.quantities[board_idx + local_idx];
                dirty_food.push((local_idx as u16, val));
                board &= board - 1;
            }
        }

        ChunkDelta {
            chunk_idx,
            ant_bitboards,
            pheromone_bitboards,
            dirty_food
        }
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
        assert_eq!(nest_pool.cursor, 0);
        assert_eq!(nest_pool.active_nests, vec![0; 4]);
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
        assert_eq!(pheromone_pool.strengths, vec![0; 4096]);
        assert_eq!(pheromone_pool.chunk_flags, vec![0; world.settings.no_of_chunks as usize]);

        // FoodPool
        let food_pool = &world.food_pool;
        assert_eq!(food_pool.quantities, vec![0; 4096]);

        world.add_food(0, 1, 253);
        assert_eq!(world.food_pool.quantities[1], 254);

        // Movement
        world.tick();
        assert_eq!(world.ant_pool.positions, vec![0, 32, 2048, 2080]);
        let _ = world.add_player();
        world.tick();
        assert_ne!(world.ant_pool.positions[0], 0);
        assert_eq!(world.ant_pool.positions[1..], [32, 2048, 2080]);

        // Error check
        let _ = world.add_player();
        let _ = world.add_player();
        let _ = world.add_player();
        let Err(_) = world.add_player() else { panic!("Should not allow more than 4 players") };

        // Removing a player stops their ants from moving but doesn't affect other players
        world.remove_player(1);
        world.tick();
        assert_eq!(world.ant_pool.positions[1], 32);
        assert_ne!(world.ant_pool.positions[2..], [2048, 2080]);
    }
}

use forage_network::{ClientPacket, ServerPacket};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use wasm_bindgen::prelude::*;

struct ClientChunk {
    ant_bitboards: [u64; 16],
    pheromone_strengths: [u8; 1024],
    food_quantities: [u8; 1024],
}

struct ClientState {
    nest_idx: u32,
    map_area: u64,
    no_of_chunks: u32,
    chunks_per_player: u16,
    chunks: HashMap<u32, ClientChunk>,
    render_buffer: Vec<f32>,
}

impl Default for ClientState {
    fn default() -> Self {
        Self {
            nest_idx: 0,
            map_area: 0,
            no_of_chunks: 0,
            chunks_per_player: 0,
            chunks: HashMap::new(),
            render_buffer: Vec::new(),
        }
    }
}

#[wasm_bindgen]
pub struct GameClient {
    socket: web_sys::WebSocket,
    state: Rc<RefCell<ClientState>>,
    last_requested_chunks: Vec<u32>,
}

#[wasm_bindgen]
impl GameClient {
    #[wasm_bindgen(constructor)]
    pub fn new(url: &str) -> Result<GameClient, JsValue> {
        let socket = web_sys::WebSocket::new(url)?;
        socket.set_binary_type(web_sys::BinaryType::Arraybuffer);

        let state = Rc::new(RefCell::new(ClientState::default()));

        let onopen_socket = socket.clone();
        let onopen_callback = Closure::<dyn FnMut(web_sys::Event)>::wrap(Box::new(move |_| {
            let packet = ClientPacket::Join;
            if let Ok(bytes) = wincode::serialize(&packet) {
                let _ = onopen_socket.send_with_u8_array(&bytes);
            }
        }));
        socket.set_onopen(Some(onopen_callback.as_ref().unchecked_ref()));
        onopen_callback.forget();

        let closure_state = Rc::clone(&state);
        let onmessage_callback = Closure::<dyn FnMut(web_sys::MessageEvent)>::wrap(Box::new(
            move |e: web_sys::MessageEvent| {
                if let Ok(array_buffer) = e.data().dyn_into::<js_sys::ArrayBuffer>() {
                    let uint8_array = js_sys::Uint8Array::new(&array_buffer);
                    let raw_bytes = uint8_array.to_vec();

                    if let Ok(server_packet) = wincode::deserialize::<ServerPacket>(&raw_bytes) {
                        let mut mutable_state = closure_state.borrow_mut();
                        let mutable_state = &mut *mutable_state;

                        match server_packet {
                            ServerPacket::Welcome {
                                nest_idx,
                                map_area,
                                no_of_chunks,
                                chunks_per_player,
                                snapshots,
                            } => {
                                mutable_state.nest_idx = nest_idx;
                                mutable_state.map_area = map_area;
                                mutable_state.no_of_chunks = no_of_chunks;
                                mutable_state.chunks_per_player = chunks_per_player;
                                for snapshot in snapshots {
                                    let mut food_quantities = [0u8; 1024];

                                    for (idx, quantity) in snapshot.food_quantities {
                                        food_quantities[idx as usize] = quantity;
                                    }

                                    let client_chunk = ClientChunk {
                                        ant_bitboards: snapshot.ant_bitboards,
                                        pheromone_strengths: snapshot.pheromone_strengths,
                                        food_quantities,
                                    };
                                    mutable_state
                                        .chunks
                                        .insert(snapshot.chunk_idx, client_chunk);
                                }
                            }

                            ServerPacket::Snapshot(snapshot) => {
                                let mut food_quantities = [0u8; 1024];

                                for (idx, quantity) in snapshot.food_quantities {
                                    food_quantities[idx as usize] = quantity;
                                }

                                let client_chunk = ClientChunk {
                                    ant_bitboards: snapshot.ant_bitboards,
                                    pheromone_strengths: snapshot.pheromone_strengths,
                                    food_quantities,
                                };
                                mutable_state
                                    .chunks
                                    .insert(snapshot.chunk_idx, client_chunk);
                            }

                            ServerPacket::Delta(delta) => {
                                if let Some(client_chunk) =
                                    mutable_state.chunks.get_mut(&delta.chunk_idx)
                                {
                                    client_chunk.ant_bitboards = delta.ant_bitboards;

                                    let evaporate = 1;

                                    for strength in client_chunk.pheromone_strengths.iter_mut() {
                                        *strength = strength.saturating_sub(evaporate);
                                    }

                                    for (board_idx, mut board) in delta.pheromone_bitboards.into_iter().enumerate() {
                                        while board != 0 {
                                            let bit_idx = board.trailing_zeros();
                                            let i = (board_idx << 6) + bit_idx as usize;
                                            client_chunk.pheromone_strengths[i] = client_chunk.pheromone_strengths[i].saturating_add(10);
                                            board &= board - 1;
                                        }
                                    }

                                    for (idx, quantity) in delta.dirty_food {
                                        client_chunk.food_quantities[idx as usize] = quantity;
                                    }
                                }
                            }
                        }
                    }
                }
            },
        ));
        socket.set_onmessage(Some(onmessage_callback.as_ref().unchecked_ref()));
        onmessage_callback.forget();

        Ok(GameClient {
            socket,
            state,
            last_requested_chunks: Vec::new(),
        })
    }

    pub fn get_initial_cam_x(&self) -> f64 {
        let state = self.state.borrow();
        if state.chunks_per_player == 0 || state.no_of_chunks == 0 {
            return 0.0;
        }
        let map_width_chunks = state.no_of_chunks.isqrt() as i32;
        let territory_width = state.chunks_per_player.isqrt() as i32;
        let territory_col = (state.nest_idx as i32) % (map_width_chunks / territory_width);
        (territory_col * territory_width) as f64 * 1024.0
    }

    pub fn get_initial_cam_y(&self) -> f64 {
        let state = self.state.borrow();
        if state.chunks_per_player == 0 || state.no_of_chunks == 0 {
            return 0.0;
        }
        let map_width_chunks = state.no_of_chunks.isqrt() as i32;
        let territory_width = state.chunks_per_player.isqrt() as i32;
        let territory_row = (state.nest_idx as i32) / (map_width_chunks / territory_width);
        (territory_row * territory_width) as f64 * 1024.0
    }

    pub fn is_ready(&self) -> bool {
        self.state.borrow().map_area > 0
    }

    pub fn spawn_food(&self, x: f64, y: f64, amount: u8) {
        let chunk_col = (x / 1024.0).floor() as i32;
        let chunk_row = (y / 1024.0).floor() as i32;

        let state = self.state.borrow();
        if state.no_of_chunks == 0 {
            return;
        }
        let map_width_chunks = state.no_of_chunks.isqrt() as i32;

        if chunk_col < 0
            || chunk_row < 0
            || chunk_col >= map_width_chunks
            || chunk_row >= map_width_chunks
        {
            return;
        }

        let chunk_idx = (chunk_row * map_width_chunks + chunk_col) as u32;

        let local_x = x % 1024.0;
        let local_y = y % 1024.0;

        let local_col = (local_x / 32.0).floor() as u16;
        let local_row = (local_y / 32.0).floor() as u16;

        let local_idx = local_row * 32 + local_col;

        let packet = ClientPacket::SpawnFood {
            chunk_idx,
            local_idx,
            quantity: amount,
        };

        if let Ok(bytes) = wincode::serialize(&packet) {
            let _ = self.socket.send_with_u8_array(&bytes);
        }
    }

    pub fn get_buffer_pointer(&self) -> *const f32 {
        self.state.borrow().render_buffer.as_ptr()
    }

    pub fn get_buffer_length(&self) -> usize {
        self.state.borrow().render_buffer.len()
    }

    pub fn get_map_width_pixels(&self) -> f32 {
        let state = self.state.borrow();
        if state.no_of_chunks == 0 {
            return 0.0;
        }

        let map_width_chunks = state.no_of_chunks.isqrt() as f32;
        map_width_chunks * 1024.0
    }

    pub fn update_render_state(
        &mut self,
        cam_x: f64,
        cam_y: f64,
        viewport_width_px: f64,
        viewport_height_px: f64,
    ) {
        let mut state = self.state.borrow_mut();
        let state = &mut *state;
        state.render_buffer.clear();

        if state.chunks_per_player == 0 {
            return;
        }

        let map_width_chunks = state.no_of_chunks.isqrt() as i32;
        let territory_width = state.chunks_per_player.isqrt() as i32;

        let min_col = (cam_x / 1024.0).floor() as i32;
        let max_col = ((cam_x + viewport_width_px) / 1024.0).ceil() as i32;
        let min_row = (cam_y / 1024.0).floor() as i32;
        let max_row = ((cam_y + viewport_height_px) / 1024.0).ceil() as i32;

        let min_col = min_col.max(0);
        let max_col = max_col.min(map_width_chunks - 1);
        let min_row = min_row.max(0);
        let max_row = max_row.min(map_width_chunks - 1);

        let mut needed_chunks = Vec::new();
        for row in min_row..=max_row {
            for col in min_col..=max_col {
                needed_chunks.push((row * map_width_chunks + col) as u32);
            }
        }

        if needed_chunks != self.last_requested_chunks {
            self.last_requested_chunks = needed_chunks.clone();
            let packet = ClientPacket::UpdateViewport {
                chunks: needed_chunks,
            };
            if let Ok(bytes) = wincode::serialize(&packet) {
                let _ = self.socket.send_with_u8_array(&bytes);
            }
        }

        let total_nests = state.no_of_chunks / (state.chunks_per_player as u32);

        for nest_id in 0..total_nests {
            let territory_col = (nest_id as i32) % (map_width_chunks / territory_width);
            let territory_row = (nest_id as i32) / (map_width_chunks / territory_width);

            let nest_chunk_col = territory_col * territory_width;
            let nest_chunk_row = territory_row * territory_width;

            if nest_chunk_col >= min_col
                && nest_chunk_col <= max_col
                && nest_chunk_row >= min_row
                && nest_chunk_row <= max_row
            {
                let abs_x = (nest_chunk_col as f32) * 1024.0 + 16.0;
                let abs_y = (nest_chunk_row as f32) * 1024.0 + 16.0;

                let color = if nest_id == state.nest_idx {
                    pack_color(255, 215, 0, 255) // Gold for client's nest
                } else {
                    pack_color(80, 80, 80, 255) // Dark Gray for enemy nests
                };

                for dy in -1..=1 {
                    for dx in -1..=1 {
                        state.render_buffer.push(abs_x + (dx as f32 * 32.0));
                        state.render_buffer.push(abs_y + (dy as f32 * 32.0));
                        state.render_buffer.push(color);
                    }
                }
            }
        }

        for row in min_row..=max_row {
            for col in min_col..=max_col {
                let chunk_idx = (row as u32 * map_width_chunks as u32) + col as u32;

                if let Some(chunk) = state.chunks.get(&chunk_idx) {
                    let chunk_pixel_x = (col as f32) * 1024.0;
                    let chunk_pixel_y = (row as f32) * 1024.0;

                    for local_idx in 0..1024 {
                        let pheromone_strength = chunk.pheromone_strengths[local_idx];
                        let food_quantity = chunk.food_quantities[local_idx];

                        if pheromone_strength > 0 {
                            let local_col = (local_idx % 32) as f32;
                            let local_row = (local_idx / 32) as f32;
                            let abs_x = chunk_pixel_x + (local_col * 32.0);
                            let abs_y = chunk_pixel_y + (local_row * 32.0);
                            let color = pack_color(0, 255, 255, pheromone_strength as u32); // Cyan

                            state.render_buffer.push(abs_x);
                            state.render_buffer.push(abs_y);
                            state.render_buffer.push(color);
                        }

                        if food_quantity > 0 {
                            let local_col = (local_idx % 32) as f32;
                            let local_row = (local_idx / 32) as f32;
                            let abs_x = chunk_pixel_x + (local_col * 32.0);
                            let abs_y = chunk_pixel_y + (local_row * 32.0);
                            let color = pack_color(0, 255, 0, food_quantity as u32); // Green

                            state.render_buffer.push(abs_x);
                            state.render_buffer.push(abs_y);
                            state.render_buffer.push(color);
                        }
                    }

                    for (board_idx, mut board) in chunk.ant_bitboards.into_iter().enumerate() {
                        while board != 0 {
                            let bit_idx = board.trailing_zeros();
                            let local_idx = (board_idx << 6) + bit_idx as usize;

                            let local_col = (local_idx % 32) as f32;
                            let local_row = (local_idx / 32) as f32;

                            let ant_abs_x = chunk_pixel_x + (local_col * 32.0);
                            let ant_abs_y = chunk_pixel_y + (local_row * 32.0);

                            let color = pack_color(0, 0, 0, 255); // Black

                            state.render_buffer.push(ant_abs_x);
                            state.render_buffer.push(ant_abs_y);
                            state.render_buffer.push(color);

                            board &= board - 1;
                        }
                    }
                }
            }
        }
    }
}

fn pack_color(r: u32, g: u32, b: u32, a: u32) -> f32 {
    let packed: u32 = (r << 24) | (g << 16) | (b << 8) | a;
    f32::from_bits(packed)
}

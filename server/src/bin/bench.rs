use std::time::Instant;
use forage_core::{Settings, World};

fn run_benchmark(player_count: usize, ants_per_nest: u32, ticks: usize) {
    let settings = Settings::new(player_count, ants_per_nest, 0.05);
    let mut world = World::new(settings);

    // Activate all nests
    for _ in 0..player_count {
        let _ = world.add_player();
    }

    // Scatter some food
    let map_size = world.get_food_quantities().len();
    let no_of_chunks = map_size / 1024;

    for i in 0..no_of_chunks {
        for j in (0..1024).step_by(128) {
            world.add_food(i, j, 254);
        }
    }


    // // Warm up
    // for _ in 0..1000 {
    //     world.tick();
    // }

    println!(
        "\n=== {} players × {} ants = {} ants ===",
        player_count,
        ants_per_nest,
        player_count * ants_per_nest as usize
    );

    let mut start = Instant::now();

    for i in 0..ticks {
        world.tick();
        if i % 100 == 99 {
            let elapsed = start.elapsed();
            println!("Ticks: {}", i+1);
            println!("Elapsed: {:.3?}", elapsed);

            let total_ants = player_count * ants_per_nest as usize;

            println!(
                "Average Tick Time: {:.3} ms",
                elapsed.as_secs_f64() * 1000.0 / 100 as f64
            );

            println!(
                "Ant Updates / Second: {:.2} million",
                (total_ants * 100) as f64
                    / elapsed.as_secs_f64()
                    / 1_000_000.0
            );
            println!("");
            start = Instant::now();
        }
    }
}

fn main() {
    const TICKS: usize = 100;

    // Normal Tests
    // run_benchmark(100, 100, TICKS);
    // run_benchmark(100, 1000, TICKS);
    // run_benchmark(1000, 1000, TICKS);
    // run_benchmark(5000, 1000, TICKS);

    // Stress Tests
    run_benchmark(1000, 1000, 1000);
    // run_benchmark(10000, 1000, 1000);
}

use devilray_sui::models::PoolState;
use devilray_sui::router::TokenGraph;
use std::time::Instant;

fn generate_synthetic_graph(v_count: usize, density: usize) -> TokenGraph {
    let mut pools = Vec::new();
    // Generate V tokens: "token_0" to "token_V-1"
    // Each token connects to a few randomly chosen subsequent tokens
    for i in 0..v_count {
        for step in 1..=density {
            let j = (i + step) % v_count;
            if i != j {
                pools.push(PoolState {
                    pool_id: format!("pool_{}_{}", i, j),
                    dex_name: "Cetus".to_string(),
                    coin_type_a: format!("token_{}", i),
                    coin_type_b: format!("token_{}", j),
                    sqrt_price: 1000,
                    liquidity: 1000000,
                    fee_rate: 3000,
                    is_paused: false,
                });
            }
        }
    }

    let mut graph = TokenGraph::new();
    graph.build_from_pools(&pools);
    graph
}

fn main() {
    println!("==================================================");
    println!("DevilRay - Pathfinding Algorithms Benchmark");
    println!("==================================================");

    let sizes = vec![
        (10, 2),  // Small network (10 tokens, 20 edges)
        (50, 3),  // Medium network (50 tokens, 150 edges)
        (100, 4), // Large network (100 tokens, 400 edges)
    ];

    println!(
        "{:<15} | {:<12} | {:<12} | {:<12} | {:<12} | {:<12}",
        "Topology (V/E)", "DFS (H=2)", "DFS (H=3)", "DFS (H=4)", "Dijkstra", "Bellman-Ford"
    );
    println!("{}", "-".repeat(85));

    for (v, density) in sizes {
        let graph = generate_synthetic_graph(v, density);
        let e_count = v * density;
        let topology = format!("{}/{}", v, e_count);

        let from = "token_0";
        let to = &format!("token_{}", v - 1);

        // 1. Benchmark DFS H=2
        let start = Instant::now();
        let _paths_dfs2 = graph.find_all_paths_dfs(from, to, 2);
        let dur_dfs2 = start.elapsed();

        // 2. Benchmark DFS H=3
        let start = Instant::now();
        let _paths_dfs3 = graph.find_all_paths_dfs(from, to, 3);
        let dur_dfs3 = start.elapsed();

        // 3. Benchmark DFS H=4
        let start = Instant::now();
        let _paths_dfs4 = graph.find_all_paths_dfs(from, to, 4);
        let dur_dfs4 = start.elapsed();

        // 4. Benchmark Dijkstra
        let start = Instant::now();
        let _path_dijkstra = graph.find_shortest_path_dijkstra(from, to);
        let dur_dijkstra = start.elapsed();

        // 5. Benchmark Bellman-Ford
        let start = Instant::now();
        let _path_bf = graph.find_shortest_path_bellman_ford(from, to);
        let dur_bf = start.elapsed();

        println!(
            "{:<15} | {:<12} | {:<12} | {:<12} | {:<12} | {:<12}",
            topology,
            format!("{:?}", dur_dfs2),
            format!("{:?}", dur_dfs3),
            format!("{:?}", dur_dfs4),
            format!("{:?}", dur_dijkstra),
            format!("{:?}", dur_bf),
        );
    }
}

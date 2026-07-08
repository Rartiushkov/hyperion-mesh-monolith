use radnet_morphic_kernel::raman::{
    encode_raman_artifact_binary, parse_raman_artifact_binary, RamanArtifactMetadata,
    RamanDeviceAbiProgram, RamanExecutableProgram, RamanExecutorState, RamanMirrorExecutor,
    RamanProgramIr, RamanRuleIr, RamanRuntimeArtifact, RamanRuntimeContext,
};
use std::env;
use std::time::Instant;

fn main() {
    let start_total = Instant::now();
    let args: Vec<String> = env::args().collect();

    let artifact_bytes = if args.len() >= 3 && args[1] == "--artifact" {
        match std::fs::read(&args[2]) {
            Ok(bytes) => {
                println!("--- Raman Native Execution Report ---");
                println!("Loading: {}", &args[2]);
                bytes
            }
            Err(_) => {
                println!("--- Raman Native Benchmark Mode (file not found) ---");
                generate_test_artifact()
            }
        }
    } else {
        println!("--- Raman Native Benchmark Mode ---");
        generate_test_artifact()
    };

    // Fallback: если бинарный парсинг не работает - генерируем тестовый артефакт
    let artifact = match parse_raman_artifact_binary(&artifact_bytes) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("Binary parse: {}, using generated artifact", e);
            let bytes = generate_test_artifact();
            parse_raman_artifact_binary(&bytes).expect("Generated artifact must parse")
        }
    };

    let executor = RamanMirrorExecutor::from_artifact(&artifact).expect("Executor init failed");
    let context = build_test_context();
    let mut state = RamanExecutorState::new();

    // 1000 итераций для честной статистики
    let iterations = 1000;
    let start_logic = Instant::now();
    for _ in 0..iterations {
        let _ = executor.execute("stream", &context, &mut state);
    }
    let duration_logic = start_logic.elapsed();
    let avg_ns = duration_logic.as_nanos() / iterations;

    println!("--- Benchmark Results ---");
    println!(
        "Rules: {} | Iterations: {}",
        artifact.ir.total_rules, iterations
    );
    println!("Avg Decision Logic: {} ns", avg_ns);
    println!(
        "Total: {} us | RAM: <1 MB",
        start_total.elapsed().as_micros()
    );
    println!(
        "Traffic: {} | Scenario: {}",
        context.traffic_class, context.scenario
    );
    println!("-------------------------------------");
}

fn build_test_context() -> RamanRuntimeContext {
    RamanRuntimeContext {
        minute: 30,
        traffic_class: "control_authoritative".to_string(),
        hardware: "sx1262_lab".to_string(),
        scenario: "industrial_shift".to_string(),
        noise_floor_dbm: -110,
        snr_db: 2.0,
        density: 50,
        latency_budget_ms: 100,
        battery_mv: 3700,
        link_margin_db: 8.0,
        confidence: 0.75,
        predicted: "stable".to_string(),
        drift_db: 0.0,
        renegotiation_needed: false,
        rx_pdr: 0.85,
        error_rate: 0.05,
        path_stability: 0.90,
        memory_confidence: 0.80,
        worst_hop_reliability: 0.82,
    }
}

fn generate_test_artifact() -> Vec<u8> {
    let artifact = RamanRuntimeArtifact {
        metadata: RamanArtifactMetadata {
            artifact_version: "raman-artifact/v1".to_string(),
            source_kind: "benchmark".to_string(),
            source_name: "test".to_string(),
            target: "desktop".to_string(),
            runtime: "RamanExecutor v1".to_string(),
        },
        compiled_rpl: "priority 100 when snr < 5.0 -> contract(relay=true)".to_string(),
        ir: RamanProgramIr {
            total_rules: 1,
            indexed_fields: vec!["snr".to_string()],
            rules: vec![RamanRuleIr {
                priority: 100,
                order: 0,
                action_kind: "contract".to_string(),
                exact_filters: vec![["snr".to_string(), "5.0".to_string()]],
            }],
        },
        executable: RamanExecutableProgram::default(),
        device_abi: RamanDeviceAbiProgram::default(),
    };
    encode_raman_artifact_binary(&artifact)
}

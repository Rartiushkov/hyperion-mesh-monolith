use serde::Serialize;
use serde_json::Value;
use std::cmp::Ordering;
use std::fs;
use std::process::Command;

#[derive(Serialize)]
struct Case {
    n: usize,
    seed: u64,
    baseline_ms: f64,
    fast_ms: f64,
    speedup_x: f64,
    baseline_energy: f64,
    fast_energy: f64,
    overlap_ref: f64,
}

#[derive(Serialize)]
struct Aggregate {
    win_rate: f64,
    mean_speedup_x: f64,
    median_speedup_x: f64,
    min_speedup_x: f64,
    max_speedup_x: f64,
}

#[derive(Serialize)]
struct Report {
    test: String,
    date: String,
    architecture: String,
    solver: String,
    suite: Vec<Case>,
    aggregate: Aggregate,
}

fn median(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    let m = v.len() / 2;
    if v.len() % 2 == 1 {
        v[m]
    } else {
        0.5 * (v[m - 1] + v[m])
    }
}

fn run_case(n: usize, seed: u64) -> Result<Case, String> {
    let out = Command::new("tools/bio_quantum_singularity_fast.exe")
        .args([
            "--n",
            &n.to_string(),
            "--seed",
            &seed.to_string(),
            "--fast-steps",
            "40000",
            "--fast-starts",
            "8",
        ])
        .output()
        .map_err(|e| format!("spawn cpp solver: {e}"))?;

    if !out.status.success() {
        return Err(format!(
            "cpp solver exit={} stderr={}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        ));
    }

    let txt = String::from_utf8_lossy(&out.stdout);
    let v: Value = serde_json::from_str(&txt).map_err(|e| format!("parse json: {e}\n{txt}"))?;

    let baseline_ms = v["baseline"]["runtime_ms"]
        .as_f64()
        .ok_or("missing baseline.runtime_ms")?;
    let fast_ms = v["fast"]["runtime_ms"]
        .as_f64()
        .ok_or("missing fast.runtime_ms")?;
    let baseline_energy = v["baseline"]["energy"]
        .as_f64()
        .ok_or("missing baseline.energy")?;
    let fast_energy = v["fast"]["energy"].as_f64().ok_or("missing fast.energy")?;
    let overlap_ref = v["fast"]["overlap_ref"].as_f64().unwrap_or(0.0);

    Ok(Case {
        n,
        seed,
        baseline_ms,
        fast_ms,
        speedup_x: baseline_ms / fast_ms.max(1e-9),
        baseline_energy,
        fast_energy,
        overlap_ref,
    })
}

fn write_md(path: &str, suite: &[Case], agg: &Aggregate) -> Result<(), String> {
    let mut s = String::new();
    s.push_str("# Bio Quantum Hybrid Leaderboard\n\n");
    s.push_str(
        "| n | seed | baseline_ms | fast_ms | speedup_x | baseline_E | fast_E | overlap |\n",
    );
    s.push_str("|---:|---:|---:|---:|---:|---:|---:|---:|\n");
    for c in suite {
        s.push_str(&format!(
            "| {} | {} | {:.3} | {:.3} | {:.3} | {:.6} | {:.6} | {:.3} |\n",
            c.n,
            c.seed,
            c.baseline_ms,
            c.fast_ms,
            c.speedup_x,
            c.baseline_energy,
            c.fast_energy,
            c.overlap_ref
        ));
    }
    s.push_str("\n");
    s.push_str(&format!(
        "- win_rate: {:.3}\n- mean_speedup_x: {:.3}\n- median_speedup_x: {:.3}\n- min_speedup_x: {:.3}\n- max_speedup_x: {:.3}\n",
        agg.win_rate, agg.mean_speedup_x, agg.median_speedup_x, agg.min_speedup_x, agg.max_speedup_x
    ));
    fs::write(path, s).map_err(|e| format!("write {path}: {e}"))
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error={e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let sizes = [1000usize];
    let seeds: Vec<u64> = (1..=10).map(|i| i as u64 * 101).collect();

    let mut suite = Vec::new();
    for &n in &sizes {
        for &seed in &seeds {
            let c = run_case(n, seed)?;
            println!(
                "n={} seed={} speedup={:.3}x baseline_ms={:.2} fast_ms={:.2}",
                c.n, c.seed, c.speedup_x, c.baseline_ms, c.fast_ms
            );
            suite.push(c);
        }
    }

    let speedups: Vec<f64> = suite.iter().map(|c| c.speedup_x).collect();
    let wins = suite.iter().filter(|c| c.fast_ms < c.baseline_ms).count() as f64;
    let agg = Aggregate {
        win_rate: wins / suite.len() as f64,
        mean_speedup_x: speedups.iter().sum::<f64>() / speedups.len() as f64,
        median_speedup_x: median(speedups.clone()),
        min_speedup_x: speedups.iter().copied().fold(f64::INFINITY, f64::min),
        max_speedup_x: speedups.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    };

    let report = Report {
        test: "Bio_Quantum_Hybrid_CPPFast_RustOrchestrator".to_string(),
        date: "2026-05-11".to_string(),
        architecture: "Rust orchestrator + C++ fast solver".to_string(),
        solver: "tools/bio_quantum_singularity_fast.exe".to_string(),
        suite,
        aggregate: agg,
    };

    let json_path = "traces/bio_quantum_hybrid_leaderboard_2026-05-11.json";
    let md_path = "traces/bio_quantum_hybrid_leaderboard_2026-05-11.md";
    fs::write(
        json_path,
        serde_json::to_string_pretty(&report).map_err(|e| format!("json: {e}"))?,
    )
    .map_err(|e| format!("write {json_path}: {e}"))?;
    write_md(md_path, &report.suite, &report.aggregate)?;

    println!("saved_json={json_path}");
    println!("saved_md={md_path}");
    Ok(())
}

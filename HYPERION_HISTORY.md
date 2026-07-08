# Hyperion Mesh — Session History (2026-07-07)

## What this folder is

This is a self-contained copy of the **Hyperion Mesh** production stack from the RadNet monolith.
It includes the blockchain/RAMAN kernel, the raw ingress gateway, the PWA frontend, deployment configs, and the CI/CD scaffold.

## What was built today

1. **Production Stack scaffold**
   - `.gitignore` hardened for secrets/env/keys/frontend build artifacts.
   - `.github/workflows/ci.yml` — GitHub Actions CI for Rust + frontend.
   - `frontend/` — PWA for Cloudflare Pages (WebSocket target `wss://api.hyperion-mesh.example/ws`).
   - `Dockerfile` + `.dockerignore` + `deploy/` — DigitalOcean / Ubuntu / Docker deployment.
   - `infra/migrations/001_init.sql` — Supabase users + payments tables.
   - `.env.example` — documents `ALCHEMY_RPC_URL`, `DATABASE_URL`, `HYPERION_RAW_ADDR`.

2. **Backend integration**
   - `src/bin/hyperion_raw_ingress.rs` reads `HYPERION_RAW_ADDR`, `DATABASE_URL`, `ALCHEMY_RPC_URL`.
   - Asynchronous JSONL WAL logger writes to `traces/wal.jsonl` (non-blocking, later replayed to Supabase).
   - Background Alchemy `eth_blockNumber` poller proves RPC connectivity without touching the hot path.
   - **Bank mode** (no LoRA / no user devices): `HYPERION_RAW_REQUIRE_POPP=false` makes the gateway mint PoPP blocks from server-side jitter; `HYPERION_BANK_PLKA_KEY` secures the bank's physical_id.

3. **Benchmark**
   - `src/bin/hyperion_unified_bench.rs --events 100000` was run against the local raw ingress.
   - **PoPP mode**: mean≈`48.4 us`, P50≈`44.8 us`, P99≈`86.1 us`, throughput≈`20.6 K tx/s`.
   - **Bank (server-jitter) mode**: mean≈`55.3 us`, P50≈`55.1 us`, P99≈`90.5 us`, throughput≈`18.0 K tx/s`.
   - Comparison trace: `traces/hyperion_unified_bench_comparison.json`.

4. **RAMAN integration into the hot path**
   - `apply_raman_priority_profile()` added to `src/bin/hyperion_raw_ingress.rs`: Windows process/thread priority (`high_process + highest_thread` / `realtime_process + time_critical_thread`).
   - New `src/blockchain/raman_anomaly.rs` — sliding-window MAD anomaly detector (SORP-lite) inspired by RAMAN Theorem XV.
   - `hyperion_raw_ingress` now computes `server_latency_ns`, `raman_anomaly_z`, and `raman_is_anomaly` for every WAL record.
   - No GPLang compiler is included; the package is Hyperion-only.

5. **Documentation**
   - `README.md` updated with Hyperion Mesh overview and deployment commands.
   - `AGENTS.md` updated with `PRODUCTION_STACK_SCAFFOLD_READY`, `HYPERION_BANK_MODE_JITTER_READY`, and `HYPERION_RAMAN_MODEL_INTEGRATED` verdicts.

## How to build on another machine

```bash
cd hyperion_monolith

# Rust backend
cargo build --release
cargo test --release

# Frontend PWA
cd frontend
npm install
npm run build

# Deploy with Docker
cd ..
docker compose -f deploy/docker-compose.yml up --build
```

## Environment

Copy `.env.example` to `.env` and fill in real secrets before running production services.

Key variables:
- `ALCHEMY_RPC_URL` — Alchemy RPC endpoint.
- `DATABASE_URL` — Supabase/Postgres connection string.
- `HYPERION_RAW_ADDR` — TCP+UDP ingress bind address.
- `HYPERION_RAW_REQUIRE_POPP` — `true` for client PoPP blocks, `false` for bank-mode jitter minting.
- `HYPERION_BANK_PLKA_KEY` — 64 hex chars; the bank's secret for deriving `physical_id`.

## Ports

- `8081` TCP+UDP — raw binary RAMAN ingress (`hyperion_raw_ingress`)
- `8080` TCP — gRPC gateway (`hyperion_grpc_server`)
- Frontend PWA expects WebSocket `wss://api.hyperion-mesh.example/ws` (configure to your backend).

## Verdict

`HYPERION_RAMAN_MODEL_INTEGRATED` — verified `cargo build --release`, `cargo test --release`, `cargo fmt --all -- --check`, and frontend `npm run build`.

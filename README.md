# Hyperion Mesh

> Production stack and banking layer for the **Hyperion Mesh** sovereign payment network.  
> This repository is the monolith: blockchain core, RAMAN kernel, raw ingress gateway, PWA frontend, and deployment scripts.

## Production Stack

| Layer | Platform | Cost | Entry point |
|---|---|---|---|
| Source + CI/CD | GitHub | $0 | `.github/workflows/ci.yml` |
| Frontend PWA | Cloudflare Pages | $0 | `frontend/` |
| Backend / Blockchain | DigitalOcean | $0 first 60 days, then ~$5/mo | `Dockerfile`, `deploy/` |
| Web3 RPC (DEX) | Alchemy | $0 Free Tier | `ALCHEMY_RPC_URL` in `.env` |
| WAL / Analytics | Supabase | $0 (500 MB) | `infra/migrations/001_init.sql` |

## Current Deployment State (2026-07-08)

What is already working:

- GCP backend host is live at `35.226.240.198`
- `hyperion_grpc_server` is running on `:8080`
- `hyperion_card_webhook` is running on both `:8082` and `:8083`
- Supabase schema was migrated to the reduced legal contour:
  - `users(id, address_l2, tx_count, kyc_level)`
  - `transactions(...)`
- Card authorization smoke on `:8083` returns live JSON from the application
- Codego sandbox KYC session creation works end-to-end and returns:
  - `accepted=true`
  - `session_id`
  - `iframe_url`
  - `expires_at`
- The webhook layer now also exposes acquiring-facing server routes for:
  - T-Bank payment session init for ordinary card checkout via hosted form
  - T-Bank SBP QR generation off the same payment-init contour
  - T-Bank payment-state polling
  - T-Bank card binding init/status for future saved-card flows
  - provider-side temporary internet credentials with TTL capped at `120` seconds
  - provider-side permanent card request passthrough
- Cloudflare Pages frontend was rebuilt and deployed:
  - deployment URL: `https://af50be54.hyperion-mesh-frontend.pages.dev`
  - preview alias: `https://feature-grpc-infrastructure.hyperion-mesh-frontend.pages.dev`

What was changed in the frontend:

- `frontend/_worker.js` now routes:
  - `/api/aaio/webhook` -> backend `:8082`
  - all other `/api/*` -> backend `:8083`
- `frontend/index.html` and `frontend/app.js` now expose:
  - card authorization flow
  - Codego hosted KYC session flow
  - transit-only Shared KYC form with `passport_payload_json`
- Cloudflare Pages secrets were set for both `production` and `preview`:
  - `CIS_API_ORIGIN=35.226.240.198:8082`
  - `INTERNATIONAL_API_ORIGIN=35.226.240.198:8083`
  - `API_SCHEME=http`

Current blocker:

- Cloudflare Worker proxying to a raw backend IP currently fails with `error code: 1003`
- the Pages site itself is live, but `/api/*` through Pages is not fully usable until backend routing moves from raw IP to a hostname

## No-Domain Options

If there is still no domain in Cloudflare, these are the practical options:

1. Keep using the frontend directly and call the GCP backend from the browser by explicit URL.
   - Fastest operator contour
   - Requires opening GCP firewall / handling CORS carefully
   - Not ideal for production

2. Use Cloudflare Pages only as static hosting, but do not proxy `/api/*` through the Worker yet.
   - Frontend can show buttons / forms
   - API requests can temporarily target `http://35.226.240.198:8083` directly during testing
   - Best for short sandbox debugging

3. Put a hostname in front of GCP without buying a full product domain first.
   - Example: any domain/subdomain you control and can point at GCP
   - Best path for proper Cloudflare Worker proxying

4. Skip Cloudflare API proxying for now and stay on direct backend smoke commands.
   - Already proven working:
     - `:8082 /aaio/webhook`
     - `:8083 /card/authorize`
     - `:8083 /codego/kyc/session`

Recommended next step:

- add one real domain or subdomain to Cloudflare
- create a DNS record pointing to `35.226.240.198`
- switch Worker vars from raw IP usage to the hostname
- rerun Pages smoke through `/api/card/authorize` and `/api/codego/kyc/session`

## Payment Server Surface

The backend can now act as the first server contour for mixed payment intake:

- ordinary bank cards through T-Bank internet acquiring hosted checkout
- SBP through T-Bank QR/session generation
- T-Bank payment status callbacks through `NotificationURL`
- temporary provider-issued internet credentials for short-lived card details
- permanent-card request passthrough to the issuing provider

Important boundary:

- raw PAN/CVV entry should stay on the T-Bank hosted payment surface or another PCI-capable provider surface
- this monolith should orchestrate sessions, statuses, KYC, and provider passthroughs, not become a PCI card vault
- sensitive server routes on `:8083` should be protected by `HYPERION_OPERATOR_API_TOKEN`
- approval signatures must come from `HYPERION_SIGNING_KEY`, never from a hardcoded key in the binary

Main HTTP routes on `:8083`:

- `POST /tbank/payments/init`
- `POST /tbank/payments/state`
- `POST /tbank/notifications`
- `POST /tbank/cards/add`
- `POST /tbank/cards/add/state`
- `POST /internet/credentials`
- `POST /cards/permanent/request`

Required env:

- `TBANK_EACQ_TERMINAL_KEY`
- `TBANK_EACQ_PASSWORD`
- `TBANK_EACQ_BASE_URL`
- `TBANK_EACQ_MOCK_MODE`
- `CODEGO_API_KEY`
- `CODEGO_PERMANENT_CARD_PATH`
- `HYPERION_INTERNET_CARD_TTL_SECONDS`
- `HYPERION_OPERATOR_API_TOKEN`
- `HYPERION_SIGNING_KEY`

T-Bank acceptance flow:

- backend calls `POST /tbank/payments/init` with `Amount`, `OrderId`, optional `NotificationURL`, and returns `PaymentURL` for card checkout
- for SBP, the same init route can request QR data and returns `qr_data`
- T-Bank then calls `POST /tbank/notifications`
- the webhook verifies the notification token and returns plain `OK` with HTTP `200`, matching the T-Bank contract

Local/mock testing mode:

- set `TBANK_EACQ_MOCK_MODE=true`
- no live `TerminalKey` or `Password` is required for `Init`, `GetQr`, `GetState`, `AddCard`, and `GetAddCardState`
- `/tbank/notifications` also accepts mock payloads in this mode
- this is the fastest way to test the full orchestration idea before real T-Bank sandbox credentials are available

## Quick Start on a New Machine

```bash
git clone https://github.com/YOUR_ORG/hyperion-mesh-monolith.git
cd hyperion-mesh-monolith
cp .env.example .env
# edit .env with your ALCHEMY_RPC_URL and DATABASE_URL

cargo build --release
cargo run --release --bin hyperion_raw_ingress
# in another terminal:
cargo run --release --bin hyperion_unified_bench -- --events 100000
```

## Deploy with Docker

```bash
docker compose -f deploy/docker-compose.yml up --build
```

## Deploy to DigitalOcean / Ubuntu

```bash
ssh root@DROPLET_IP
export BIN_URL=https://github.com/YOUR_ORG/hyperion-mesh-monolith/releases/download/v0.1.0/hyperion_raw_ingress
bash deploy/bootstrap.sh
```

## Deploy Binary Build To GCP / Ubuntu

Build the Linux binaries in GitHub Actions, copy the artifact bundle onto the server,
then place the binaries under `/opt/hyperion-mesh/bin/` and run:

```bash
sudo bash /opt/hyperion-mesh/deploy/deploy_binary.sh
```

This starts:

- `hyperion_grpc_server` on `:8080`
- `hyperion_card_webhook` on `:8082`

The Cloudflare frontend can call `/codego/kyc/session` and `/card/authorize`
through the webhook layer, while the webhook talks locally to the gRPC server.
Codego should post KYC outcomes back to `/codego/webhook`, which verifies the
HMAC `Signature` header and upgrades the user's `kyc_level` in Supabase.

## Remote Deploy From Windows

If you want the same "run one script from the PC" flow used in `hft_arbitrage_core`,
use `tools/remote_deploy_source.ps1`. It uploads a minimal
source payload, builds Linux binaries on the server, and runs the binary deploy flow.

---

## Changelog

### 2026-07-07 — server-side performance and risk-control pass

- **`src/bin/hft_web3_e2e_sim.rs`** — merged richer HFT live-paper risk filters from `hft_arbitrage_core`: max spread threshold, bid/ask/mid price sanity bands, top-level size filter, per-asset cooldown, runtime JSON snapshots every 15 s, and graceful handling of non-JSON WebSocket heartbeats.
- **`src/bin/hyperion_raw_ingress.rs`** — hot-path cleanup:
  - `Instant`-based `server_latency_ns` measurement (monotonic, no `SystemTime` in the hot loop).
  - Lock-free `AtomicJitterStats` instead of a mutex clone per transaction.
  - Bank-mode block minting now uses a fixed `[u64; 4]` jitter delta array, no heap allocation.
  - WAL writer uses a bounded `sync_channel` and batches 64 records before flush.
  - TCP listener uses a pre-spawned worker pool via `TcpListener::try_clone()` instead of spawning a thread per connection.
- **`src/bin/hyperion_grpc_server.rs`** — reduced per-authorization allocations by storing ECDSA signatures as `[u8; 65]` and hashes as `[u8; 32]`; added a background FX-rate feed that updates `eur_usdt_bits` / `usd_usdt_bits` every 100 ms. Initial FX rates can be set via `HYPERION_EUR_USDT` and `HYPERION_USD_USDT`.

Verified with `cargo fmt --all && cargo check --all`.

---

# RadNet LAN

RadNet LAN is an adaptive radio data network for harsh environments.
The project treats wireless behavior as software:

- runtime context becomes input data
- the PHY profile is computed, not hardcoded
- transitions are validated before applying them
- the same logic can move from simulation to hardware drivers

## What Is In The Repo

### Rust core

The Rust crate contains the first Morphic Kernel prototype:

- `Context`
- `PhyProfile`
- `ProfilePlanner`
- `MorphicKernel`
- `RadioDriver`
- `MockRadio`
- `WritePlan` backed by fixed-capacity `heapless::Vec`
- `ProfilePreset` fast path with precomputed register transitions
- `AdaptivePowerManager` that rebalances `TxPower` from live `SNR/link_margin/battery` context
- zero-copy `SecureFrame32` authenticated packet path shared by tests and benchmarks
- burst-oriented register writes in `RadioDriver` / `RegisterBus`
- `no_std` core build path for embedded targets
- `HalSx1262Bus` for generic SPI/GPIO embedded bring-up
- `Esp32C3Sx1262Board` scaffold for board-level wiring
- `embedded-hal` adapter wrappers that can host real `esp-hal` SPI/GPIO/delay types

This is the deterministic control plane that will later drive real chips.

### Python simulation

The repository now also includes a runnable `Digital Twin Lite`:

- industrial radio-LAN scenario simulation
- LoRa-like factory and 802.11p-like vehicular scenario models
- scenario-specific virtual topologies for RadNet v1
- scenario and hardware calibration layer for more realistic metrics
- field-based calibration blending from CSV observations
- raw radio log ingestion into multidimensional calibration buckets
- realism audit with airtime, queueing, collisions, duty cycle and packet-loss traces
- adaptive vs static profile comparison
- coordinated fleet learning and skill broadcast
- packet transport with ACK and retries
- predictive hybrid switching for energy and latency tuning
- secure nonce/tag frames with replay protection metrics
- scenario replay CLI via `tools/scenario_replay.py` for `adaptive/rpl/ril` policy comparisons with weakest `flow/hop` traces and side-by-side `selected vs adaptive vs static` diffs
- tag budget comparison, key rotation, drift tolerance, session resync and autonomy blackout metrics
- trust-aware telemetry filtering and context-aware local policy cache
- `RIL` (`Raman Intent Language`) as a mission-level language that compiles into typed `RPL`, including mission composition, fallback branches and `hardware/scenario` guards
- `RPL v2` as a compact radio-native policy language that can now express `phy`, `contract`, `transport`, `ris` and `verify` actions without a separate heavyweight runtime
- `RIL v3` mission words such as `reflective_guard`, `stealth_window` and `relay_corridor`, which compile down into deterministic `RPL v2`
- `RIS-native control plane` report that turns surface choice, occupancy windows and signal memory into programmable reflection decisions
- `low_observable` waveform mission for dense weak-SNR windows where a stealth-like mode is preferable to direct emission
- relay-aware `bulk_guard` transport branch for harsh telemetry windows on sensor/relay bulk paths
- `RadNet RIS System` status layer that ties RIS control, replay, transport and live hardware evidence into one stage report
- explicit `RIS` replay traces that expose panel mode, surface, azimuth, phase profile and reflection gain minute-by-minute
- annealed `RIS phase search` that mutates minute-level `stealth/relay/control` schedules and searches for a better programmable reflective policy than the default RIS baseline
- `Signal Autopilot` that compares `adaptive/ril/ris` candidates minute-by-minute, attaches explanations and fallback readiness, and picks the strongest policy path before hardware rollout
- file-backed Raman policies in `policies/rpl/*.rpl` and `policies/ril/*.ril`, so the language now lives outside Python strings and can be checked, compiled and replayed as standalone artifacts
- `RamanExecutor` as a standalone runtime kernel for executing Raman programs with local temporal state outside `VirtualNetwork`
- compact `RamanProgramIR` with indexed candidate-rule selection for faster offline runtime and a cleaner path toward embedded execution
- `raman-artifact/v1` device/runtime packaging so a policy can be compiled once into a stable payload for HIL and future embedded consumers
- Rust-side `raman` artifact bridge that can load, validate and inspect `raman-artifact/v1` contracts without depending on the Python host runtime
- Rust-side `RamanMirrorExecutor` that can already execute compiled artifact rules with exact-match filtering and temporal `hold/cooldown` behavior
- compact binary `.rbin` artifact path and shared parity vectors so Python and Rust can validate the same runtime contract on the same expected outcomes
- executable rule subset embedded in the artifact so `execute-artifact` and `RamanMirrorExecutor` can run without reparsing the `compiled_rpl` text in the final execution path
- minimal device ABI embedded in the artifact so `hil-artifact` and future device/apply loops can derive a command plan without reinterpreting source-level Raman syntax
- host-mediated `apply-artifact` contour so a compiled Raman artifact can already produce an apply-ready device session report before direct embedded command execution exists
- board-facing `transport-artifact` serial path that writes `RAMAN_APPLY` commands to UART endpoints, reports echoed/acknowledged commands when available, and fails soft when the current COM mapping is unavailable
- `raman-listener` firmware mode for `esp32c3-sx1262-link`, which keeps radio disabled, accepts `RAMAN_APPLY ...` over board-side serial, and is designed to answer `RAMAN_ACK ...` before any over-the-air testing
- richer Raman-aware replay traces that expose `transport`, `RIS`, `verify`, and evidence fields hop-by-hop
- a standalone Raman benchmark report and first host-mediated HIL binding plan for future command-path validation
- a new `raman-core` crate (`no_std`) with fixed-buffer / fixed-ID execution contour for portable embedded runtime
- `Raman Capability Descriptor (RCD)` in `chips/*/*/capability.json` for chip-declared PHY/IRQ/runtime limits
- `RHML` (`mapping.rhml`) as a compact hardware mapping surface from Raman decisions to register-write plans
- deterministic ABI negotiation (`artifact requires` vs `device provides`) before final command-plan emission
- chip-pack format (`capability.json + mapping.rhml + selftest_vectors.json`) for reusable multi-silicon onboarding
- portable e2e proof (`same artifact -> 3 backends -> same runtime verdict`) in `tests/raman_portability_e2e.rs`
- Rust-side `autopilot` twin metrics for:
  - predictive channel degradation
  - fleet profile learning
  - twin-vs-hardware alignment scoring
  - compute-vs-SPI-vs-airtime budget
  - autonomous blackout recovery
- time-to-compromise budget estimation for authenticated control traffic
- stress recovery simulation with 30% infrastructure node outage and route healing
- harsh industrial interference v2 resilience sweep
- predictive fast reroute with one-tick recovery
- clustered underground relay model for deep attenuation environments
- routing v1.5 report with prewarmed reroute, deduplication and canary rollout
- live recovery routing with prewarmed routes, local multipath fallback and dedup counters
- throughput / reliability / latency / energy estimates
- regression tests that can run without Rust installed

## Product Direction

RadNet is being assembled as a platform with these layers:

1. `Core`
   Adaptive and verifiable Morphic Kernel.
2. `Drivers`
   Hardware abstraction for radio chips.
3. `Network`
   Packet transport and local radio LAN behavior.
4. `Skills`
   Ready-made policies such as low-power, deep indoor, or low-latency.
5. `Twin`
   Simulation environment for deployment planning and tuning.
6. `Intent`
   Mission-level directives that compile into deterministic `RPL` policy.

## Current Prototype Limits

The current repository does not yet include:

- a fully wired over-the-air SX1262 / SX1276 backend
- over-the-air packet exchange on hardware
- mesh routing
- production encryption pipeline
- full 3D digital twin

The repo now does include a first `SX1262` Rust backend scaffold and shared field-log contracts aligned with the calibration CSV flow.

## Local Run

Rust tests require a Rust toolchain:

```bash
$env:RUSTC='C:\Program Files\Rust stable MSVC 1.94\bin\rustc.exe'
$env:PATH='C:\Program Files\Rust stable MSVC 1.94\bin;' + $env:PATH
& 'C:\Program Files\Rust stable MSVC 1.94\bin\cargo.exe' test
```

Rust benchmark for pure 10-hop compute latency:

```bash
$env:RUSTC='C:\Program Files\Rust stable MSVC 1.94\bin\rustc.exe'
$env:PATH='C:\Program Files\Rust stable MSVC 1.94\bin;' + $env:PATH
& 'C:\Program Files\Rust stable MSVC 1.94\bin\cargo.exe' bench --bench morphic_bench
```

Current host-side benchmark envelope after the fast-path refactor:

- `radnet_10_hop_profile_pipeline`: about `0.32-0.36 us`
- `radnet_singularity_packet`: about `0.33-0.37 us`
- `radnet_quantum_ghost_packet`: about `3.87-4.37 us`
- `radnet_adaptive_tx_power_recalc`: about `15.6-17.8 ns`
- `radnet_quantum_resync_jump`: about `18-20 ns`
- `radnet_session_resync_two_frame`: about `19-21 ns`
- `radnet_secure_frame_zero_copy`: about `7.8-9.1 ns`

Embedded-oriented `no_std` host build:

```bash
$env:RUSTC='C:\Program Files\Rust stable MSVC 1.94\bin\rustc.exe'
$env:PATH='C:\Program Files\Rust stable MSVC 1.94\bin;' + $env:PATH
& 'C:\Program Files\Rust stable MSVC 1.94\bin\cargo.exe' build --no-default-features
```

`riscv32imc` cross-build currently works with the rustup-managed `rustc`:

```bash
$env:RUSTC=\"$env:USERPROFILE\.rustup\toolchains\stable-x86_64-pc-windows-msvc\bin\rustc.exe\"
$env:PATH='C:\Program Files\Rust stable MSVC 1.94\bin;' + $env:PATH
& 'C:\Program Files\Rust stable MSVC 1.94\bin\cargo.exe' build --release --no-default-features --target riscv32imc-unknown-none-elf
```

`embedded-hal` bridge build:

```bash
$env:RUSTC='C:\Users\r.artyshkov\.rustup\toolchains\stable-x86_64-pc-windows-msvc\bin\rustc.exe'
$env:PATH='C:\Program Files\Rust stable MSVC 1.94\bin;' + $env:PATH
& 'C:\Program Files\Rust stable MSVC 1.94\bin\cargo.exe' test --features embedded-hal-adapter
```

Python simulation and tests run with stock Python:

```bash
python -m unittest discover -s tests_py -v
python -m sim.radnet_sim.demo
python tools/scenario_replay.py --scenario industrial_shift --policy ril
python tools/scenario_replay.py --scenario industrial_shift --policy ris
python tools/ris_system.py --scenario industrial_shift
python tools/ris_phase_search.py --scenario industrial_shift --steps 9
python tools/signal_autopilot.py --scenario industrial_shift
python tools/raman_cli.py check --kind ril --file policies/ril/default.ril
python tools/raman_cli.py compile --file policies/ril/default.ril
python tools/raman_cli.py artifact --kind ril --file policies/ril/default.ril --target esp32c3_sx1262 --out contracts/raman_policy_v1.json
python tools/raman_cli.py artifact --kind ril --format bin --file policies/ril/default.ril --target esp32c3_sx1262 --out contracts/raman_policy_v1.rbin
python tools/raman_cli.py replay --kind ril --file policies/ril/default.ril
python tools/raman_cli.py bench --kind ril --file policies/ril/default.ril --iterations 500
python tools/raman_cli.py execute --kind ril --file policies/ril/default.ril --traffic truth_critical --scenario industrial_shift --limit 3
python tools/raman_cli.py execute-artifact --artifact contracts/raman_policy_v1.json --traffic truth_critical --scenario industrial_shift --limit 3
python tools/raman_cli.py execute-artifact --artifact contracts/raman_policy_v1.rbin --traffic truth_critical --scenario industrial_shift --limit 3
python tools/raman_benchmark.py --kind ril --file policies/ril/default.ril --iterations 500
python tools/raman_device_pack.py --kind ril --file policies/ril/default.ril --target esp32c3_sx1262 --out contracts/raman_policy_v1.json
python tools/raman_hil_bind.py --ports COM9 COM10 --scenario industrial_shift
python tools/raman_demo_pack.py
python tools/host_link_emulator.py --ports COM9 COM10 --duration-s 8
python tools/host_rap_handshake.py --ports COM9 COM10 --duration-s 8
python tools/antenna_readiness.py
python tools/raman_memory_stack_test.py
& 'C:\Program Files\Rust stable MSVC 1.94\bin\cargo.exe' run --bin raman_portable -- --artifact contracts/raman_policy_v1.json --chip-pack chips/espressif/esp32c3_sx1262
& 'C:\Program Files\Rust stable MSVC 1.94\bin\cargo.exe' test --test raman_portability_e2e
```

Calibration notes live in [CALIBRATION.md](/c:/Users/r.artyshkov/Desktop/RadNet/CALIBRATION.md).
Field trace format lives in [FIELD_TRACE_FORMAT.md](/c:/Users/r.artyshkov/Desktop/RadNet/FIELD_TRACE_FORMAT.md).
Hardware bring-up notes live in [HARDWARE_BRINGUP.md](/c:/Users/r.artyshkov/Desktop/RadNet/HARDWARE_BRINGUP.md).
First antenna-run checklist lives in [HARDWARE_ANTENNA_RUNBOOK.md](/c:/Users/r.artyshkov/Desktop/RadNet/HARDWARE_ANTENNA_RUNBOOK.md).
Raman language principles and progress log live in [RAMAN_LANGUAGE_MANIFESTO.md](/c:/Users/r.artyshkov/Desktop/RadNet/RAMAN_LANGUAGE_MANIFESTO.md).
Raman syntax examples live in [RAMAN_COOKBOOK.md](/c:/Users/r.artyshkov/Desktop/RadNet/RAMAN_COOKBOOK.md).
Raman formal surface lives in [RAMAN_SPEC.md](/c:/Users/r.artyshkov/Desktop/RadNet/RAMAN_SPEC.md).
Raman future contours live in [RAMAN_EVOLUTION_MAP.md](/c:/Users/r.artyshkov/Desktop/RadNet/RAMAN_EVOLUTION_MAP.md).
Mission assurance contour (`FDIR/Safety/Redundancy/Temporal/SEU/Security/Certification/Parity`) lives in [MISSION_ASSURANCE_CONTOUR.md](/c:/Users/r.artyshkov/Desktop/RadNet/MISSION_ASSURANCE_CONTOUR.md).

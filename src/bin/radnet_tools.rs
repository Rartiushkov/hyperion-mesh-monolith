use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const LEARNING_BANK_ADDR: u32 = 0x001F_0000;
const LEARNING_BANK_SIZE: usize = 0x0020_0000;
const HEADER_SIZE: usize = 4096;
const MAGIC: u32 = 0x4C52_4E42; // LRNB
const VERSION: u32 = 1;

const FLASH_STATE_ADDR: u32 = 0x003F_F000;
const FLASH_HISTORY_ADDR: u32 = 0x003F_E000;
const FLASH_RULE_ARCHIVE_ADDR: u32 = 0x003F_D000;
const FLASH_SOURCE_ARCHIVE_ADDR: u32 = 0x003F_C000;
const FLASH_LEXICON_ADDR: u32 = 0x003F_B000;

#[derive(Debug)]
struct LearningBankHeader {
    payload_len: u32,
    payload_crc32: u32,
    created_unix: u32,
}

#[derive(Serialize, Clone)]
struct Card {
    id: String,
    tag: String,
    priority: u32,
    text: String,
}

#[derive(Serialize)]
struct CardsPayload {
    schema: &'static str,
    profile_version: u32,
    cards_count: usize,
    cards: Vec<Card>,
    filters: CardsFilters,
}

#[derive(Serialize)]
struct CardsFilters {
    allowed_tags: Vec<String>,
    max_cards: usize,
    max_chars_per_card: usize,
    dropped_long: usize,
    dropped_tag: usize,
}

#[derive(Deserialize)]
struct Lesson {
    query: Option<String>,
    answer: Option<String>,
    tag: Option<String>,
}

#[derive(Clone)]
struct AstRule {
    name: String,
    priority: i32,
    when: String,
    phy: BTreeMap<String, String>,
}

#[derive(Serialize, Clone)]
struct GplArtifact {
    artifact_type: &'static str,
    compiler: &'static str,
    generated_utc: String,
    source_gpl: String,
    rules: Vec<GplRule>,
}

#[derive(Serialize, Clone)]
struct GplRule {
    name: String,
    priority: i32,
    predicates: Vec<GplPredicate>,
    action: GplAction,
}

#[derive(Serialize, Clone)]
struct GplPredicate {
    field: String,
    op: String,
    #[serde(rename = "type")]
    value_type: String,
    value: Value,
}

#[derive(Serialize, Clone)]
struct GplAction {
    kind: &'static str,
    values: BTreeMap<String, String>,
}

#[derive(Serialize)]
struct GplDebug {
    tokens_count: usize,
    rules_count: usize,
    ast: Vec<GplAstDebug>,
    ir: Vec<GplRule>,
}

#[derive(Serialize)]
struct GplAstDebug {
    name: String,
    priority: i32,
    when: String,
    phy: BTreeMap<String, String>,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("error={err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1).collect::<Vec<_>>();
    if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
        print_help();
        return Ok(());
    }

    let family = args.remove(0);
    match family.as_str() {
        "learning-bank" => run_learning_bank(args),
        "cooper-cards" => run_cooper_cards(args),
        "cooper-lessons" => run_cooper_lessons(args),
        "gpl" | "gplang" => run_gpl(args),
        _ => Err(format!("unknown command family: {family}")),
    }
}

fn print_help() {
    println!("radnet_tools");
    println!("  learning-bank build --input <file> --out-bin <file> [--created-unix <u32>]");
    println!("  learning-bank inspect --image-bin <file> [--out-payload <file>]");
    println!("  cooper-cards build --profile <json> --out-cards-json <json> --out-bank-bin <bin> [--extra-lessons-jsonl <jsonl>]");
    println!("  cooper-lessons build --in-log <file> [--in-log <file2> ...] [--out-jsonl <file>]");
    println!("  gpl compile --from-gpl <file> --out-bytecode <json> [--out-binary <bin>] [--out-ir <json>]");
    println!("  gpl mutate --from-gpl <file> --metrics-json <json> --out-gpl <file>");
}

fn run_learning_bank(mut args: Vec<String>) -> Result<(), String> {
    if args.is_empty() {
        return Err("learning-bank requires a subcommand".into());
    }
    let sub = args.remove(0);
    match sub.as_str() {
        "build" => {
            let input = required_path(&args, "--input")?;
            let out_bin = optional_path(&args, "--out-bin")
                .unwrap_or_else(|| PathBuf::from("learning_flash_bank_1mb.bin"));
            let created_unix =
                optional_u32(&args, "--created-unix")?.unwrap_or(current_unix_u32()?);
            let payload = load_payload(&input)?;
            let image = build_bank_image(&payload, created_unix)?;
            fs::write(&out_bin, image).map_err(|e| format!("write {}: {e}", out_bin.display()))?;
            println!("input={}", input.display());
            println!("payload_bytes={}", payload.len());
            println!("flash_addr=0x{LEARNING_BANK_ADDR:06X}");
            println!("bank_bytes={LEARNING_BANK_SIZE}");
            println!("header_bytes={HEADER_SIZE}");
            println!("out_bin={}", out_bin.display());
            Ok(())
        }
        "inspect" => {
            let image_bin = required_path(&args, "--image-bin")?;
            let raw =
                fs::read(&image_bin).map_err(|e| format!("read {}: {e}", image_bin.display()))?;
            let (head, payload) = parse_bank_image(&raw)?;
            if let Some(out_payload) = optional_path(&args, "--out-payload") {
                fs::write(&out_payload, payload)
                    .map_err(|e| format!("write {}: {e}", out_payload.display()))?;
                println!("out_payload={}", out_payload.display());
            }
            println!("image_bin={}", image_bin.display());
            println!("payload_bytes={}", head.payload_len);
            println!("payload_crc32=0x{:08X}", head.payload_crc32);
            println!("created_unix={}", head.created_unix);
            Ok(())
        }
        _ => Err(format!("unknown learning-bank subcommand: {sub}")),
    }
}

fn run_cooper_cards(mut args: Vec<String>) -> Result<(), String> {
    if args.is_empty() {
        return Err("cooper-cards requires a subcommand".into());
    }
    let sub = args.remove(0);
    match sub.as_str() {
        "build" => build_cooper_cards(&args),
        _ => Err(format!("unknown cooper-cards subcommand: {sub}")),
    }
}

fn run_cooper_lessons(mut args: Vec<String>) -> Result<(), String> {
    if args.is_empty() {
        return Err("cooper-lessons requires a subcommand".into());
    }
    let sub = args.remove(0);
    match sub.as_str() {
        "build" => build_cooper_offline_lessons(&args),
        _ => Err(format!("unknown cooper-lessons subcommand: {sub}")),
    }
}

fn run_gpl(mut args: Vec<String>) -> Result<(), String> {
    if args.is_empty() {
        return Err("gpl requires a subcommand".into());
    }
    let sub = args.remove(0);
    match sub.as_str() {
        "compile" => compile_gpl_command(&args),
        "mutate" => mutate_gpl_command(&args),
        _ => Err(format!("unknown gpl subcommand: {sub}")),
    }
}

fn compile_gpl_command(args: &[String]) -> Result<(), String> {
    let from_gpl = optional_path(args, "--from-gpl")
        .unwrap_or_else(|| PathBuf::from("policies/gpl/default_evolved.gpl"));
    let out_bytecode = optional_path(args, "--out-bytecode")
        .unwrap_or_else(|| PathBuf::from("policies/gpl/default_evolved.gbc.json"));
    let out_binary = optional_path(args, "--out-binary");
    let out_ir = optional_path(args, "--out-ir");

    let text =
        fs::read_to_string(&from_gpl).map_err(|e| format!("read {}: {e}", from_gpl.display()))?;
    let (artifact, debug) = compile_gpl_source(&text, &from_gpl)?;
    let artifact_json = serde_json::to_string_pretty(&artifact)
        .map_err(|e| format!("serialize bytecode json: {e}"))?;
    fs::write(&out_bytecode, artifact_json)
        .map_err(|e| format!("write {}: {e}", out_bytecode.display()))?;
    println!("bytecode_json={}", out_bytecode.display());

    if let Some(path) = out_binary {
        let binary = encode_gbc2(&artifact)?;
        fs::write(&path, binary).map_err(|e| format!("write {}: {e}", path.display()))?;
        println!("bytecode_bin={}", path.display());
    }

    if let Some(path) = out_ir {
        let debug_json =
            serde_json::to_string_pretty(&debug).map_err(|e| format!("serialize ir debug: {e}"))?;
        fs::write(&path, debug_json).map_err(|e| format!("write {}: {e}", path.display()))?;
        println!("ir_debug_json={}", path.display());
    }
    println!("rules={}", artifact.rules.len());
    Ok(())
}

fn mutate_gpl_command(args: &[String]) -> Result<(), String> {
    let from_gpl = optional_path(args, "--from-gpl")
        .unwrap_or_else(|| PathBuf::from("policies/gpl/default_evolved.gpl"));
    let metrics_json = optional_path(args, "--metrics-json")
        .unwrap_or_else(|| PathBuf::from("morphic_soul_manifest_now.json"));
    let out_gpl = optional_path(args, "--out-gpl")
        .unwrap_or_else(|| PathBuf::from("policies/gpl/default_autonomous.gpl"));

    let text =
        fs::read_to_string(&from_gpl).map_err(|e| format!("read {}: {e}", from_gpl.display()))?;
    let rules = parse_gpl(&text)?;
    let metrics = if metrics_json.exists() {
        let raw = fs::read_to_string(&metrics_json)
            .map_err(|e| format!("read {}: {e}", metrics_json.display()))?;
        serde_json::from_str::<Value>(&raw)
            .map_err(|e| format!("parse {}: {e}", metrics_json.display()))?
    } else {
        Value::Null
    };

    let observed_confidence = json_num(&metrics, "observed_confidence")
        .or_else(|| json_num(&metrics, "confidence"))
        .unwrap_or(0.0);
    let drift_score = json_num(&metrics, "drift_score").unwrap_or(0.0);
    let mutated = mutate_gpl_by_metrics(&rules, observed_confidence, drift_score);
    let rendered = render_gpl(&mutated);

    if let Some(parent) = out_gpl.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("create parent {}: {e}", parent.display()))?;
    }
    fs::write(&out_gpl, rendered).map_err(|e| format!("write {}: {e}", out_gpl.display()))?;

    let report = serde_json::json!({
        "timestamp_utc": generated_stamp()?,
        "from_gpl": from_gpl.display().to_string(),
        "out_gpl": out_gpl.display().to_string(),
        "metrics_json": metrics_json.display().to_string(),
        "rules_before": rules.len(),
        "rules_after": mutated.len(),
        "observed_confidence": observed_confidence,
        "drift_score": drift_score,
    });
    let rep_path = out_gpl.with_extension("mutation_report.json");
    fs::write(
        &rep_path,
        serde_json::to_string_pretty(&report)
            .map_err(|e| format!("serialize mutation report: {e}"))?,
    )
    .map_err(|e| format!("write {}: {e}", rep_path.display()))?;

    println!("out_gpl={}", out_gpl.display());
    println!("mutation_report_json={}", rep_path.display());
    Ok(())
}

fn build_cooper_cards(args: &[String]) -> Result<(), String> {
    let profile_path = optional_path(args, "--profile")
        .unwrap_or_else(|| PathBuf::from("policies/cooper/cooper_learning_profile.json"));
    let out_cards_json = optional_path(args, "--out-cards-json")
        .unwrap_or_else(|| PathBuf::from("traces/cooper_learning_cards.json"));
    let out_bank_bin = optional_path(args, "--out-bank-bin")
        .unwrap_or_else(|| PathBuf::from("traces/cooper_learning_bank_1mb.bin"));
    let extra_lessons = optional_path(args, "--extra-lessons-jsonl");
    let created_unix = optional_u32(args, "--created-unix")?.unwrap_or(current_unix_u32()?);

    let profile_raw = fs::read_to_string(&profile_path)
        .map_err(|e| format!("read {}: {e}", profile_path.display()))?;
    let profile: Value = serde_json::from_str(&profile_raw)
        .map_err(|e| format!("parse profile {}: {e}", profile_path.display()))?;

    let filters = profile.get("filters").unwrap_or(&Value::Null);
    let allowed_tags = string_array(filters.get("allowed_tags")).unwrap_or_else(|| {
        vec![
            "identity".into(),
            "programming".into(),
            "self_improvement".into(),
            "communication".into(),
        ]
    });
    let allowed_set = allowed_tags.iter().cloned().collect::<BTreeSet<_>>();
    let max_cards = usize_field(filters, "max_cards", 128);
    let max_chars = usize_field(filters, "max_chars_per_card", 220);

    let mut unique = HashMap::<(String, String), Card>::new();
    let mut dropped_long = 0usize;
    let mut dropped_tag = 0usize;

    for (tag, text) in profile_cards(&profile) {
        insert_card(
            &mut unique,
            &allowed_set,
            &mut dropped_long,
            &mut dropped_tag,
            max_chars,
            &tag,
            &text,
        );
    }

    if let Some(path) = extra_lessons {
        if path.exists() {
            for (tag, text) in lesson_cards(&path)? {
                insert_card(
                    &mut unique,
                    &allowed_set,
                    &mut dropped_long,
                    &mut dropped_tag,
                    max_chars,
                    &tag,
                    &text,
                );
            }
        }
    }

    let mut cards = unique.into_values().collect::<Vec<_>>();
    cards.sort_by(|a, b| b.priority.cmp(&a.priority).then_with(|| a.id.cmp(&b.id)));
    cards.truncate(max_cards);

    let payload = CardsPayload {
        schema: "cooper-learning-cards/v1",
        profile_version: u32_field(&profile, "version", 1),
        cards_count: cards.len(),
        cards,
        filters: CardsFilters {
            allowed_tags: allowed_set.into_iter().collect(),
            max_cards,
            max_chars_per_card: max_chars,
            dropped_long,
            dropped_tag,
        },
    };
    let pretty =
        serde_json::to_string_pretty(&payload).map_err(|e| format!("serialize cards: {e}"))?;
    let compact = serde_json::to_vec(&payload).map_err(|e| format!("serialize payload: {e}"))?;
    let bank = build_bank_image(&compact, created_unix)?;

    fs::write(&out_cards_json, pretty)
        .map_err(|e| format!("write {}: {e}", out_cards_json.display()))?;
    fs::write(&out_bank_bin, bank).map_err(|e| format!("write {}: {e}", out_bank_bin.display()))?;

    println!("profile={}", profile_path.display());
    println!("cards_count={}", payload.cards_count);
    println!("payload_bytes={}", compact.len());
    println!("out_cards_json={}", out_cards_json.display());
    println!("out_bank_bin={}", out_bank_bin.display());
    Ok(())
}

fn insert_card(
    unique: &mut HashMap<(String, String), Card>,
    allowed_tags: &BTreeSet<String>,
    dropped_long: &mut usize,
    dropped_tag: &mut usize,
    max_chars: usize,
    tag: &str,
    text: &str,
) {
    let tag = tag.trim();
    let text = normalize_text(text);
    if tag.is_empty() || text.is_empty() {
        return;
    }
    if !allowed_tags.contains(tag) {
        *dropped_tag += 1;
        return;
    }
    if text.chars().count() > max_chars {
        *dropped_long += 1;
        return;
    }
    let key = (tag.to_string(), text.clone());
    unique.entry(key).or_insert_with(|| Card {
        id: card_id(tag, &text),
        tag: tag.to_string(),
        priority: tag_priority(tag),
        text,
    });
}

fn profile_cards(profile: &Value) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let identity = profile.get("identity").unwrap_or(&Value::Null);
    if let Some(lines) = string_array(identity.get("core_principles")) {
        for line in lines {
            out.push(("identity".into(), line));
        }
    }
    if let Some(mission) = identity.get("mission").and_then(Value::as_str) {
        out.push(("identity".into(), mission.to_string()));
    }
    if let Some(name) = identity.get("name").and_then(Value::as_str) {
        out.push(("identity".into(), format!("My name is {name}.")));
    }
    for key in [
        "programming_basics",
        "self_improvement",
        "communication_style",
    ] {
        let tag = match key {
            "programming_basics" => "programming",
            "self_improvement" => "self_improvement",
            _ => "communication",
        };
        if let Some(lines) = string_array(profile.get(key)) {
            for line in lines {
                out.push((tag.into(), line));
            }
        }
    }
    out
}

fn lesson_cards(path: &Path) -> Result<Vec<(String, String)>, String> {
    let raw = fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let mut out = Vec::new();
    for line in raw.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let lesson: Lesson = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let q = normalize_text(&lesson.query.unwrap_or_default());
        let a = normalize_text(&lesson.answer.unwrap_or_default());
        let tag = normalize_text(&lesson.tag.unwrap_or_else(|| "communication".into()));
        if !q.is_empty() && !a.is_empty() {
            out.push((tag, format!("Q: {q} A: {a}")));
        }
    }
    Ok(out)
}

fn build_cooper_offline_lessons(args: &[String]) -> Result<(), String> {
    let in_logs = multi_paths(args, "--in-log");
    let out_jsonl = optional_path(args, "--out-jsonl")
        .unwrap_or_else(|| PathBuf::from("traces/cooper_offline_lessons.jsonl"));
    let mut queries = Vec::<String>::new();
    let mut seen = BTreeSet::<String>::new();
    for path in in_logs {
        collect_queries_from_path(&path, &mut seen, &mut queries)?;
    }
    if let Some(parent) = out_jsonl.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("create parent {}: {e}", parent.display()))?;
    }
    let now = current_unix_u32()?;
    let mut lines = Vec::<String>::new();
    for q in &queries {
        let rec = serde_json::json!({
            "ts": now,
            "query": q,
            "answer": answer_for_query(q),
            "tag": tag_for_query(q),
            "source": "offline_distill_rs_v1",
        });
        lines.push(
            serde_json::to_string(&rec).map_err(|e| format!("serialize lesson record: {e}"))?,
        );
    }
    fs::write(&out_jsonl, lines.join("\n").as_bytes())
        .map_err(|e| format!("write {}: {e}", out_jsonl.display()))?;
    println!("queries={}", queries.len());
    println!("out={}", out_jsonl.display());
    Ok(())
}

fn collect_queries_from_path(
    path: &Path,
    seen: &mut BTreeSet<String>,
    out: &mut Vec<String>,
) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    let raw = fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    if path.extension().and_then(|s| s.to_str()) == Some("json") {
        if let Ok(value) = serde_json::from_str::<Value>(&raw) {
            collect_queries_from_value(&value, seen, out);
            return Ok(());
        }
    }
    for line in raw.lines() {
        if let Some(q) = extract_unknown_query(line) {
            let norm = normalize_text(&q);
            if !norm.is_empty() && seen.insert(norm.clone()) {
                out.push(norm);
            }
        }
    }
    Ok(())
}

fn collect_queries_from_value(value: &Value, seen: &mut BTreeSet<String>, out: &mut Vec<String>) {
    match value {
        Value::Array(items) => {
            for item in items {
                collect_queries_from_value(item, seen, out);
            }
        }
        Value::Object(map) => {
            for (_k, v) in map {
                collect_queries_from_value(v, seen, out);
            }
        }
        Value::String(s) => {
            if let Some(q) = extract_unknown_query(s) {
                let norm = normalize_text(&q);
                if !norm.is_empty() && seen.insert(norm.clone()) {
                    out.push(norm);
                }
            }
        }
        _ => {}
    }
}

fn extract_unknown_query(line: &str) -> Option<String> {
    let marker = "unknown_query=\"";
    let i = line.find(marker)?;
    let tail = &line[i + marker.len()..];
    let j = tail.find('"')?;
    Some(tail[..j].to_string())
}

fn tag_for_query(query: &str) -> &'static str {
    let q = query.to_ascii_lowercase();
    if [
        "code", "program", "function", "bug", "compile", "rust", "python", "loop", "type",
    ]
    .iter()
    .any(|k| q.contains(k))
    {
        return "programming";
    }
    if ["who", "feel", "identity", "where", "self"]
        .iter()
        .any(|k| q.contains(k))
    {
        return "identity";
    }
    if ["learn", "improve", "train", "teach"]
        .iter()
        .any(|k| q.contains(k))
    {
        return "self_improvement";
    }
    "communication"
}

fn answer_for_query(query: &str) -> &'static str {
    let q = query.to_ascii_lowercase();
    if ["rust", "generics", "generic", "trait"]
        .iter()
        .any(|k| q.contains(k))
    {
        return "Generics in Rust allow one function or type to work with many concrete types through trait bounds.";
    }
    if ["compiler", "compile"].iter().any(|k| q.contains(k)) {
        return "A compiler transforms source code into machine instructions and checks many classes of errors before runtime.";
    }
    if ["bug", "debug"].iter().any(|k| q.contains(k)) {
        return "For bugs: reproduce, isolate minimal case, inspect logs and state transitions, then add a regression test.";
    }
    if ["who", "where", "feel", "identity"]
        .iter()
        .any(|k| q.contains(k))
    {
        return "I am COOPER: adaptive radio intelligence with bounded self-improvement and safety-first communication.";
    }
    if ["learn", "train", "improve", "teach"]
        .iter()
        .any(|k| q.contains(k))
    {
        return "I learn in bounded steps: queue unknown questions, distill lessons, verify integrity, then promote safely.";
    }
    "Use short keywords for faster offline answers: state, plan, memory resonance, code, compiler, bug."
}

fn mutate_gpl_by_metrics(rules: &[AstRule], confidence: f64, drift_score: f64) -> Vec<AstRule> {
    let mut out = Vec::with_capacity(rules.len());
    for rule in rules {
        let mut next = rule.clone();
        if next.when.contains("confidence >=") && confidence < 0.9 {
            next.when = relax_confidence_threshold(&next.when, confidence, drift_score);
        }
        if let Some(v) = next.phy.get("tx_power_dbm").cloned() {
            if let Ok(p) = v.parse::<i32>() {
                let boost = if confidence >= 0.7 { 1 } else { 2 };
                let boosted = (p + boost).min(20);
                next.phy
                    .insert("tx_power_dbm".to_string(), boosted.to_string());
            }
        }
        out.push(next);
    }
    out
}

fn relax_confidence_threshold(when: &str, confidence: f64, drift_score: f64) -> String {
    let mut chunks = when
        .split(" and ")
        .map(|s| s.trim().to_string())
        .collect::<Vec<_>>();
    let mut changed = false;
    for chunk in &mut chunks {
        if changed || !chunk.contains("confidence >=") {
            continue;
        }
        if let Some((field, op, raw)) = split_predicate(chunk).ok() {
            if field == "confidence" && op == ">=" {
                if let Ok(old) = raw.parse::<f64>() {
                    let relax = 0.03
                        + f64::min(
                            0.15,
                            f64::max(0.0, 0.9 - confidence) * 0.4 + drift_score * 0.1,
                        );
                    let mut new = ((old - relax) * 100.0).round() / 100.0;
                    if new < 0.5 {
                        new = 0.5;
                    }
                    *chunk = format!("confidence >= {:.2}", new);
                    changed = true;
                }
            }
        }
    }
    chunks.join(" and ")
}

fn render_gpl(rules: &[AstRule]) -> String {
    let mut lines = Vec::<String>::new();
    lines.push("# GPLang (gplc output)".to_string());
    lines.push(String::new());
    for rule in rules {
        lines.push(format!("rule {}:", rule.name));
        lines.push(format!("  priority {}", rule.priority));
        lines.push(format!("  when {}", rule.when));
        let phy = rule
            .phy
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!("  phy {phy}"));
        lines.push(String::new());
    }
    let mut out = lines.join("\n");
    while out.ends_with("\n\n") {
        out.pop();
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn compile_gpl_source(text: &str, source: &Path) -> Result<(GplArtifact, GplDebug), String> {
    let tokens_count = tokenize_gpl(text).len();
    let ast = parse_gpl(text)?;
    typecheck_gpl(&ast)?;
    let ir = lower_gpl_to_ir(&ast)?;
    let artifact = GplArtifact {
        artifact_type: "gargantua-bytecode/v2",
        compiler: "gplc-rs",
        generated_utc: generated_stamp()?,
        source_gpl: source.display().to_string(),
        rules: ir.clone(),
    };
    let debug = GplDebug {
        tokens_count,
        rules_count: ast.len(),
        ast: ast
            .iter()
            .map(|r| GplAstDebug {
                name: r.name.clone(),
                priority: r.priority,
                when: r.when.clone(),
                phy: r.phy.clone(),
            })
            .collect(),
        ir,
    };
    Ok((artifact, debug))
}

fn tokenize_gpl(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch.is_whitespace() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            continue;
        }
        if ch == '"' {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            let mut quoted = String::from("\"");
            for q in chars.by_ref() {
                quoted.push(q);
                if q == '"' {
                    break;
                }
            }
            out.push(quoted);
            continue;
        }
        if matches!(ch, ':' | ',' | '=') {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            out.push(ch.to_string());
            continue;
        }
        if matches!(ch, '>' | '<' | '!') {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            if chars.peek() == Some(&'=') {
                let eq = chars.next().unwrap();
                out.push(format!("{ch}{eq}"));
            } else {
                out.push(ch.to_string());
            }
            continue;
        }
        cur.push(ch);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn parse_gpl(text: &str) -> Result<Vec<AstRule>, String> {
    let lines = text.lines().collect::<Vec<_>>();
    let mut rules = Vec::new();
    let mut i = 0usize;
    while i < lines.len() {
        let line = lines[i].trim();
        i += 1;
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if !line.starts_with("rule ") || !line.ends_with(':') {
            return Err(format!("expected 'rule <name>:' line, got: {line}"));
        }
        let name = line["rule ".len()..line.len() - 1].trim().to_string();
        let mut priority = None;
        let mut when = None;
        let mut phy = BTreeMap::new();
        while i < lines.len() {
            let raw = lines[i];
            if raw.trim().is_empty() {
                i += 1;
                continue;
            }
            if !raw.starts_with("  ") {
                break;
            }
            let stmt = raw.trim();
            i += 1;
            if let Some(rest) = stmt.strip_prefix("priority ") {
                priority = Some(
                    rest.trim()
                        .parse::<i32>()
                        .map_err(|e| format!("invalid priority in {name}: {e}"))?,
                );
            } else if let Some(rest) = stmt.strip_prefix("when ") {
                when = Some(rest.trim().to_string());
            } else if let Some(rest) = stmt.strip_prefix("phy ") {
                phy = parse_phy_pairs(rest)?;
            } else {
                return Err(format!("unsupported statement in {name}: {stmt}"));
            }
        }
        let priority = priority.ok_or_else(|| format!("rule '{name}' missing priority"))?;
        let when = when.ok_or_else(|| format!("rule '{name}' missing when"))?;
        if phy.is_empty() {
            return Err(format!("rule '{name}' missing phy"));
        }
        rules.push(AstRule {
            name,
            priority,
            when,
            phy,
        });
    }
    Ok(rules)
}

fn parse_phy_pairs(text: &str) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    for part in text.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let (key, value) = part
            .split_once('=')
            .ok_or_else(|| format!("invalid phy field: {part}"))?;
        out.insert(
            key.trim().to_string(),
            value.trim().trim_matches('"').to_string(),
        );
    }
    Ok(out)
}

fn typecheck_gpl(rules: &[AstRule]) -> Result<(), String> {
    let mut names = BTreeSet::new();
    for rule in rules {
        if !names.insert(rule.name.clone()) {
            return Err(format!("duplicate rule name: {}", rule.name));
        }
        if !(0..=1000).contains(&rule.priority) {
            return Err(format!(
                "priority out of range: {}:{}",
                rule.name, rule.priority
            ));
        }
        if !rule.phy.contains_key("name") {
            return Err(format!("rule missing phy.name: {}", rule.name));
        }
    }
    Ok(())
}

fn lower_gpl_to_ir(rules: &[AstRule]) -> Result<Vec<GplRule>, String> {
    let mut ir = Vec::new();
    for rule in rules {
        ir.push(GplRule {
            name: rule.name.clone(),
            priority: rule.priority,
            predicates: compile_predicates(&rule.when)?,
            action: GplAction {
                kind: "phy",
                values: rule.phy.clone(),
            },
        });
    }
    ir.sort_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(ir)
}

fn compile_predicates(when: &str) -> Result<Vec<GplPredicate>, String> {
    let mut out = Vec::new();
    for chunk in when.split(" and ").map(str::trim).filter(|s| !s.is_empty()) {
        let (field, op, raw) = split_predicate(chunk)?;
        let (value_type, value) = parse_scalar(raw);
        out.push(GplPredicate {
            field: field.to_string(),
            op: op.to_string(),
            value_type,
            value,
        });
    }
    Ok(out)
}

fn split_predicate(text: &str) -> Result<(&str, &str, &str), String> {
    for op in ["==", "!=", ">=", "<=", ">", "<"] {
        if let Some((left, right)) = text.split_once(op) {
            let field = left.trim();
            if field.is_empty() {
                return Err(format!("missing predicate field: {text}"));
            }
            return Ok((field, op, right.trim()));
        }
    }
    Err(format!("unsupported predicate syntax: {text}"))
}

fn parse_scalar(raw: &str) -> (String, Value) {
    let text = raw.trim();
    if text.eq_ignore_ascii_case("true") {
        return ("bool".into(), Value::Bool(true));
    }
    if text.eq_ignore_ascii_case("false") {
        return ("bool".into(), Value::Bool(false));
    }
    if (text.starts_with('"') && text.ends_with('"'))
        || (text.starts_with('\'') && text.ends_with('\''))
    {
        return (
            "str".into(),
            Value::String(text[1..text.len() - 1].to_string()),
        );
    }
    if text.contains('.') {
        if let Ok(v) = text.parse::<f64>() {
            if let Some(n) = serde_json::Number::from_f64(v) {
                return ("num".into(), Value::Number(n));
            }
        }
    } else if let Ok(v) = text.parse::<i64>() {
        return ("num".into(), Value::Number(v.into()));
    }
    ("str".into(), Value::String(text.to_string()))
}

fn encode_gbc2(artifact: &GplArtifact) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    out.extend_from_slice(b"GBC2");
    out.extend_from_slice(&(artifact.rules.len() as u32).to_le_bytes());
    for rule in &artifact.rules {
        put_str(&mut out, &rule.name)?;
        out.extend_from_slice(&rule.priority.to_le_bytes());
        put_u16(&mut out, rule.predicates.len())?;
        for pred in &rule.predicates {
            put_str(&mut out, &pred.field)?;
            out.push(match pred.op.as_str() {
                "==" => 0,
                "!=" => 1,
                ">=" => 2,
                "<=" => 3,
                ">" => 4,
                "<" => 5,
                _ => 0,
            });
            let type_code = match pred.value_type.as_str() {
                "num" => 0,
                "bool" => 1,
                _ => 2,
            };
            out.push(type_code);
            match type_code {
                0 => {
                    let n = pred
                        .value
                        .as_f64()
                        .ok_or_else(|| format!("numeric predicate is not f64: {}", pred.field))?;
                    out.extend_from_slice(&n.to_le_bytes());
                }
                1 => out.push(if pred.value.as_bool().unwrap_or(false) {
                    1
                } else {
                    0
                }),
                _ => put_str(&mut out, pred.value.as_str().unwrap_or(""))?,
            }
        }
        out.push(0); // action kind: phy
        put_u16(&mut out, rule.action.values.len())?;
        for (key, value) in &rule.action.values {
            put_str(&mut out, key)?;
            put_str(&mut out, value)?;
        }
    }
    Ok(out)
}

fn put_u16(out: &mut Vec<u8>, value: usize) -> Result<(), String> {
    let value = u16::try_from(value).map_err(|_| format!("value too large for u16: {value}"))?;
    out.extend_from_slice(&value.to_le_bytes());
    Ok(())
}

fn put_str(out: &mut Vec<u8>, text: &str) -> Result<(), String> {
    let raw = text.as_bytes();
    put_u16(out, raw.len())?;
    out.extend_from_slice(raw);
    Ok(())
}

fn load_payload(path: &Path) -> Result<Vec<u8>, String> {
    match path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "txt" | "md" | "log" | "csv" => fs::read_to_string(path)
            .map(|s| s.into_bytes())
            .map_err(|e| format!("read {}: {e}", path.display())),
        "json" => {
            let raw =
                fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
            let value: Value =
                serde_json::from_str(&raw).map_err(|e| format!("parse {}: {e}", path.display()))?;
            serde_json::to_vec(&value).map_err(|e| format!("compact json {}: {e}", path.display()))
        }
        _ => fs::read(path).map_err(|e| format!("read {}: {e}", path.display())),
    }
}

fn build_bank_image(payload: &[u8], created_unix: u32) -> Result<Vec<u8>, String> {
    validate_layout()?;
    let max_payload = LEARNING_BANK_SIZE - HEADER_SIZE;
    if payload.len() > max_payload {
        return Err(format!(
            "payload too large: {} > {max_payload}",
            payload.len()
        ));
    }
    let crc = crc32(payload);
    let mut image = vec![0u8; LEARNING_BANK_SIZE];
    write_u32(&mut image, 0, MAGIC);
    write_u32(&mut image, 4, VERSION);
    write_u32(&mut image, 8, payload.len() as u32);
    write_u32(&mut image, 12, crc);
    write_u32(&mut image, 16, created_unix);
    image[HEADER_SIZE..HEADER_SIZE + payload.len()].copy_from_slice(payload);
    Ok(image)
}

fn parse_bank_image(image: &[u8]) -> Result<(LearningBankHeader, &[u8]), String> {
    validate_layout()?;
    if image.len() != LEARNING_BANK_SIZE {
        return Err(format!(
            "unexpected image size: {} != {LEARNING_BANK_SIZE}",
            image.len()
        ));
    }
    let magic = read_u32(image, 0)?;
    let version = read_u32(image, 4)?;
    if magic != MAGIC {
        return Err(format!("bad magic: 0x{magic:08X}"));
    }
    if version != VERSION {
        return Err(format!("bad version: {version}"));
    }
    let payload_len = read_u32(image, 8)? as usize;
    let payload_crc32 = read_u32(image, 12)?;
    let created_unix = read_u32(image, 16)?;
    let max_payload = LEARNING_BANK_SIZE - HEADER_SIZE;
    if payload_len > max_payload {
        return Err(format!(
            "payload_len in header exceeds max: {payload_len} > {max_payload}"
        ));
    }
    let payload = &image[HEADER_SIZE..HEADER_SIZE + payload_len];
    let crc = crc32(payload);
    if crc != payload_crc32 {
        return Err(format!(
            "payload crc mismatch: got=0x{crc:08X} expected=0x{payload_crc32:08X}"
        ));
    }
    Ok((
        LearningBankHeader {
            payload_len: payload_len as u32,
            payload_crc32,
            created_unix,
        },
        payload,
    ))
}

fn validate_layout() -> Result<(), String> {
    let bank0 = LEARNING_BANK_ADDR;
    let bank1 = LEARNING_BANK_ADDR + LEARNING_BANK_SIZE as u32 - 1;
    for (name, p0) in [
        ("state", FLASH_STATE_ADDR),
        ("history", FLASH_HISTORY_ADDR),
        ("rule_archive", FLASH_RULE_ARCHIVE_ADDR),
        ("source_archive", FLASH_SOURCE_ARCHIVE_ADDR),
        ("lexicon", FLASH_LEXICON_ADDR),
    ] {
        let p1 = p0 + 4095;
        if overlaps(bank0, bank1, p0, p1) {
            return Err(format!(
                "learning bank overlaps protected flash sector '{name}' bank=0x{bank0:06X}..0x{bank1:06X} protected=0x{p0:06X}..0x{p1:06X}"
            ));
        }
    }
    Ok(())
}

fn overlaps(a0: u32, a1: u32, b0: u32, b1: u32) -> bool {
    !(a1 < b0 || b1 < a0)
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in bytes {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn write_u32(buf: &mut [u8], off: usize, value: u32) {
    buf[off..off + 4].copy_from_slice(&value.to_le_bytes());
}

fn read_u32(buf: &[u8], off: usize) -> Result<u32, String> {
    let bytes = buf
        .get(off..off + 4)
        .ok_or_else(|| format!("short read at offset {off}"))?;
    Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
}

fn normalize_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn card_id(tag: &str, text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("{tag}:{text}").as_bytes());
    let digest = hasher.finalize();
    let hex = digest[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    format!("{tag}_{hex}")
}

fn tag_priority(tag: &str) -> u32 {
    match tag {
        "identity" => 100,
        "consciousness" => 96,
        "memory" => 94,
        "ethics" => 92,
        "philosophy" => 90,
        "reasoning" => 88,
        "evolution" => 86,
        "safety" => 84,
        "programming" => 80,
        "self_improvement" => 70,
        "communication" => 60,
        _ => 50,
    }
}

fn string_array(value: Option<&Value>) -> Option<Vec<String>> {
    value.and_then(Value::as_array).map(|items| {
        items
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect()
    })
}

fn usize_field(value: &Value, key: &str, default: usize) -> usize {
    value
        .get(key)
        .and_then(Value::as_u64)
        .map(|v| v as usize)
        .unwrap_or(default)
}

fn u32_field(value: &Value, key: &str, default: u32) -> u32 {
    value
        .get(key)
        .and_then(Value::as_u64)
        .map(|v| v as u32)
        .unwrap_or(default)
}

fn json_num(value: &Value, key: &str) -> Option<f64> {
    let v = value.get(key)?;
    if let Some(n) = v.as_f64() {
        return Some(n);
    }
    if let Some(s) = v.as_str() {
        return s.parse::<f64>().ok();
    }
    None
}

fn current_unix_u32() -> Result<u32, String> {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| format!("system clock before unix epoch: {e}"))?
        .as_secs();
    Ok(secs.min(u32::MAX as u64) as u32)
}

fn generated_stamp() -> Result<String, String> {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| format!("system clock before unix epoch: {e}"))?
        .as_secs();
    Ok(format!("unix:{secs}"))
}

fn required_path(args: &[String], key: &str) -> Result<PathBuf, String> {
    optional_path(args, key).ok_or_else(|| format!("missing required argument {key}"))
}

fn optional_path(args: &[String], key: &str) -> Option<PathBuf> {
    args.windows(2)
        .find(|pair| pair[0] == key)
        .map(|pair| PathBuf::from(&pair[1]))
}

fn multi_paths(args: &[String], key: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 1 < args.len() {
        if args[i] == key {
            out.push(PathBuf::from(&args[i + 1]));
            i += 2;
            continue;
        }
        i += 1;
    }
    out
}

fn optional_u32(args: &[String], key: &str) -> Result<Option<u32>, String> {
    match args.windows(2).find(|pair| pair[0] == key) {
        Some(pair) => pair[1]
            .parse::<u32>()
            .map(Some)
            .map_err(|e| format!("parse {key}: {e}")),
        None => Ok(None),
    }
}

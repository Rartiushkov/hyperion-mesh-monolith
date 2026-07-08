use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: gplc_rt <compile|run|mutate|bench> [options]");
        std::process::exit(1);
    }
    let code = match args[1].as_str() {
        "compile" => cmd_compile(&args[2..]),
        "run" => cmd_run(&args[2..]),
        "mutate" => cmd_mutate(&args[2..]),
        "bench" => cmd_bench(&args[2..]),
        other => {
            eprintln!("unknown command: {other}");
            1
        }
    };
    std::process::exit(code);
}

// ── GPL parser ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct GplRule {
    name: String,
    priority: i64,
    when: String,
    phy: Vec<(String, String)>,
}

fn parse_gpl(text: &str) -> Result<Vec<GplRule>, String> {
    let mut rules = Vec::new();
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let t = lines[i].trim();
        i += 1;
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if !t.starts_with("rule ") || !t.ends_with(':') {
            return Err(format!("expected 'rule <name>:' got: {t}"));
        }
        let name = t[5..t.len() - 1].trim().to_string();
        let mut priority: Option<i64> = None;
        let mut when: Option<String> = None;
        let mut phy: Vec<(String, String)> = Vec::new();
        while i < lines.len() {
            let row = lines[i];
            if row.trim().is_empty() {
                i += 1;
                continue;
            }
            if !row.starts_with("  ") {
                break;
            }
            let stmt = row.trim();
            i += 1;
            if let Some(rest) = stmt.strip_prefix("priority ") {
                priority = Some(
                    rest.trim()
                        .parse()
                        .map_err(|e| format!("priority parse: {e}"))?,
                );
            } else if let Some(rest) = stmt.strip_prefix("when ") {
                when = Some(rest.trim().to_string());
            } else if let Some(rest) = stmt.strip_prefix("phy ") {
                phy = parse_phy_pairs(rest)?;
            } else {
                return Err(format!("unsupported stmt in {name}: {stmt}"));
            }
        }
        let priority = priority.ok_or_else(|| format!("rule '{name}' missing priority"))?;
        let when = when.ok_or_else(|| format!("rule '{name}' missing when"))?;
        if phy.is_empty() {
            return Err(format!("rule '{name}' missing phy"));
        }
        rules.push(GplRule {
            name,
            priority,
            when,
            phy,
        });
    }
    Ok(rules)
}

fn parse_phy_pairs(text: &str) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    for part in text.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let eq = part
            .find('=')
            .ok_or_else(|| format!("invalid phy field: {part}"))?;
        let k = part[..eq].trim().to_string();
        let v = part[eq + 1..].trim().trim_matches('"').to_string();
        out.push((k, v));
    }
    Ok(out)
}

// ── IR / bytecode ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
enum PredValue {
    Num(f64),
    Bool(bool),
    Str(String),
}

#[derive(Debug, Clone)]
struct Predicate {
    field: String,
    op: String,
    value: PredValue,
}

#[derive(Debug, Clone)]
struct IrRule {
    name: String,
    priority: i64,
    predicates: Vec<Predicate>,
    action: Vec<(String, String)>,
}

fn parse_scalar(raw: &str) -> PredValue {
    let t = raw.trim().trim_matches('"');
    if t.eq_ignore_ascii_case("true") {
        return PredValue::Bool(true);
    }
    if t.eq_ignore_ascii_case("false") {
        return PredValue::Bool(false);
    }
    if let Ok(f) = t.parse::<f64>() {
        return PredValue::Num(f);
    }
    PredValue::Str(t.to_string())
}

fn compile_predicates(when: &str) -> Result<Vec<Predicate>, String> {
    let mut preds = Vec::new();
    for chunk in when.split(" and ") {
        let chunk = chunk.trim();
        if chunk.is_empty() {
            continue;
        }
        let ops = ["==", "!=", ">=", "<=", ">", "<"];
        let mut found = None;
        for op in ops {
            if let Some(pos) = chunk.find(op) {
                let field = chunk[..pos].trim().to_string();
                let raw = chunk[pos + op.len()..].trim();
                found = Some(Predicate {
                    field,
                    op: op.to_string(),
                    value: parse_scalar(raw),
                });
                break;
            }
        }
        preds.push(found.ok_or_else(|| format!("unsupported predicate: {chunk}"))?);
    }
    Ok(preds)
}

fn lower_to_ir(rules: Vec<GplRule>) -> Result<Vec<IrRule>, String> {
    let mut ir: Vec<IrRule> = rules
        .into_iter()
        .map(|r| {
            let predicates = compile_predicates(&r.when)?;
            Ok(IrRule {
                name: r.name,
                priority: r.priority,
                predicates,
                action: r.phy,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    ir.sort_by(|a, b| b.priority.cmp(&a.priority).then(a.name.cmp(&b.name)));
    Ok(ir)
}

// ── JSON serialization (no deps) ─────────────────────────────────────────────

fn json_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn ir_to_json(ir: &[IrRule], source: &str) -> String {
    let now = chrono_now();
    let rules_json: Vec<String> = ir.iter().map(|r| {
        let preds: Vec<String> = r.predicates.iter().map(|p| {
            let val_json = match &p.value {
                PredValue::Num(n)  => format!("\"type\":\"num\",\"value\":{n}"),
                PredValue::Bool(b) => format!("\"type\":\"bool\",\"value\":{b}"),
                PredValue::Str(s)  => format!("\"type\":\"str\",\"value\":{}",json_str(s)),
            };
            format!("{{\"field\":{},\"op\":{},{val_json}}}", json_str(&p.field), json_str(&p.op))
        }).collect();
        let vals: Vec<String> = r.action.iter()
            .map(|(k,v)| format!("{}:{}", json_str(k), json_str(v))).collect();
        format!(
            "{{\"name\":{},\"priority\":{},\"predicates\":[{}],\"action\":{{\"kind\":\"phy\",\"values\":{{{}}}}}}}",
            json_str(&r.name), r.priority,
            preds.join(","),
            vals.join(",")
        )
    }).collect();
    format!(
        "{{\n  \"artifact_type\": \"gargantua-bytecode/v2\",\n  \"compiler\": \"gplc_rt\",\n  \"generated_utc\": \"{now}\",\n  \"source_gpl\": {},\n  \"rules\": [\n    {}\n  ]\n}}",
        json_str(source),
        rules_json.join(",\n    ")
    )
}

fn chrono_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let s = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let (y, mo, d, h, mi, sec) = unix_to_ymd(s);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{sec:02}+00:00")
}

fn unix_to_ymd(ts: u64) -> (u64, u64, u64, u64, u64, u64) {
    let sec = ts % 60;
    let ts = ts / 60;
    let min = ts % 60;
    let ts = ts / 60;
    let hr = ts % 24;
    let mut days = ts / 24;
    let mut y = 1970u64;
    loop {
        let dy = if y % 400 == 0 || y % 4 == 0 && y % 100 != 0 {
            366
        } else {
            365
        };
        if days < dy {
            break;
        }
        days -= dy;
        y += 1;
    }
    let leap = y % 400 == 0 || y % 4 == 0 && y % 100 != 0;
    let months = [
        31u64,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut mo = 1u64;
    for &m in &months {
        if days < m {
            break;
        }
        days -= m;
        mo += 1;
    }
    (y, mo, days + 1, hr, min, sec)
}

// ── Bytecode binary (GBC2) ────────────────────────────────────────────────────

fn encode_gbc2(ir: &[IrRule]) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"GBC2");
    put_u32(&mut buf, ir.len() as u32);
    for r in ir {
        put_str(&mut buf, &r.name);
        put_i32(&mut buf, r.priority as i32);
        put_u16(&mut buf, r.predicates.len() as u16);
        for p in &r.predicates {
            put_str(&mut buf, &p.field);
            let op_code: u8 = match p.op.as_str() {
                "==" => 0,
                "!=" => 1,
                ">=" => 2,
                "<=" => 3,
                ">" => 4,
                "<" => 5,
                _ => 0,
            };
            buf.push(op_code);
            match &p.value {
                PredValue::Num(n) => {
                    buf.push(0);
                    buf.extend_from_slice(&n.to_le_bytes());
                }
                PredValue::Bool(b) => {
                    buf.push(1);
                    buf.push(*b as u8);
                }
                PredValue::Str(s) => {
                    buf.push(2);
                    put_str(&mut buf, s);
                }
            }
        }
        buf.push(0); // action kind = phy
        put_u16(&mut buf, r.action.len() as u16);
        let mut keys: Vec<&(String, String)> = r.action.iter().collect();
        keys.sort_by_key(|(k, _)| k.as_str());
        for (k, v) in keys {
            put_str(&mut buf, k);
            put_str(&mut buf, v);
        }
    }
    buf
}

fn put_u16(buf: &mut Vec<u8>, v: u16) {
    buf.extend_from_slice(&v.to_le_bytes());
}
fn put_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_le_bytes());
}
fn put_i32(buf: &mut Vec<u8>, v: i32) {
    buf.extend_from_slice(&v.to_le_bytes());
}
fn put_str(buf: &mut Vec<u8>, s: &str) {
    let b = s.as_bytes();
    put_u16(buf, b.len() as u16);
    buf.extend_from_slice(b);
}

// ── Runtime execute ───────────────────────────────────────────────────────────

fn eval_predicate(p: &Predicate, ctx: &HashMap<String, String>) -> bool {
    let Some(raw_ctx) = ctx.get(&p.field) else {
        return false;
    };
    match &p.value {
        PredValue::Num(pv) => {
            let cv: f64 = raw_ctx.parse().unwrap_or(f64::NAN);
            match p.op.as_str() {
                "==" => cv == *pv,
                "!=" => cv != *pv,
                ">=" => cv >= *pv,
                "<=" => cv <= *pv,
                ">" => cv > *pv,
                "<" => cv < *pv,
                _ => false,
            }
        }
        PredValue::Bool(pv) => {
            let cv = raw_ctx.trim().eq_ignore_ascii_case("true");
            match p.op.as_str() {
                "==" => cv == *pv,
                "!=" => cv != *pv,
                _ => false,
            }
        }
        PredValue::Str(pv) => match p.op.as_str() {
            "==" => raw_ctx == pv,
            "!=" => raw_ctx != pv,
            _ => false,
        },
    }
}

fn execute<'a>(ir: &'a [IrRule], ctx: &HashMap<String, String>) -> Option<&'a IrRule> {
    ir.iter()
        .find(|r| r.predicates.iter().all(|p| eval_predicate(p, ctx)))
}

// ── mutate ────────────────────────────────────────────────────────────────────

fn mutate_rules(rules: Vec<GplRule>, metrics: &HashMap<String, String>) -> Vec<GplRule> {
    let conf: f64 = metrics
        .get("observed_confidence")
        .or_else(|| metrics.get("confidence"))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.0);
    let drift: f64 = metrics
        .get("drift_score")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.0);
    rules
        .into_iter()
        .map(|mut r| {
            if r.when.contains("confidence >=") && conf < 0.9 {
                if let Some(pos) = r.when.find("confidence >=") {
                    let rest = &r.when[pos + 13..].trim_start();
                    let end = rest
                        .find(|c: char| !c.is_ascii_digit() && c != '.')
                        .unwrap_or(rest.len());
                    if let Ok(old) = rest[..end].parse::<f64>() {
                        let relax =
                            0.03 + f64::min(0.15, f64::max(0.0, 0.9 - conf) * 0.4 + drift * 0.1);
                        let new = f64::max(0.5, (old - relax) * 100.0).round() / 100.0;
                        r.when = r.when.replacen(
                            &format!("confidence >= {}", &rest[..end]),
                            &format!("confidence >= {new:.2}"),
                            1,
                        );
                    }
                }
            }
            for (k, v) in r.phy.iter_mut() {
                if k == "tx_power_dbm" {
                    if let Ok(p) = v.parse::<i64>() {
                        let boost: i64 = if conf >= 0.7 { 1 } else { 2 };
                        *v = format!("{}", i64::min(20, p + boost));
                    }
                }
            }
            r
        })
        .collect()
}

fn render_gpl(rules: &[GplRule]) -> String {
    let mut out = String::from("# GPLang (gplc_rt output)\n\n");
    for r in rules {
        out.push_str(&format!(
            "rule {}:\n  priority {}\n  when {}\n  phy {}\n\n",
            r.name,
            r.priority,
            r.when,
            r.phy
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    out
}

// ── commands ──────────────────────────────────────────────────────────────────

fn arg(args: &[String], flag: &str, default: &str) -> String {
    args.windows(2)
        .find(|w| w[0] == flag)
        .map(|w| w[1].clone())
        .unwrap_or_else(|| default.to_string())
}

fn cmd_compile(args: &[String]) -> i32 {
    let from = arg(args, "--from-gpl", "policies/gpl/default_evolved.gpl");
    let out_j = arg(
        args,
        "--out-bytecode",
        "policies/gpl/default_evolved.gbc.json",
    );
    let out_b = arg(args, "--out-binary", "");
    let text = match std::fs::read_to_string(&from) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("read {from}: {e}");
            return 1;
        }
    };
    let rules = match parse_gpl(&text) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("parse: {e}");
            return 1;
        }
    };
    let ir = match lower_to_ir(rules) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("lower: {e}");
            return 1;
        }
    };
    let json = ir_to_json(&ir, &from);
    if let Some(p) = PathBuf::from(&out_j).parent() {
        let _ = std::fs::create_dir_all(p);
    }
    std::fs::write(&out_j, &json).unwrap();
    println!("bytecode_json={out_j}");
    println!("rules={}", ir.len());
    if !out_b.is_empty() {
        let bin = encode_gbc2(&ir);
        if let Some(p) = PathBuf::from(&out_b).parent() {
            let _ = std::fs::create_dir_all(p);
        }
        std::fs::write(&out_b, &bin).unwrap();
        println!("bytecode_bin={out_b}  bytes={}", bin.len());
    }
    0
}

fn cmd_run(args: &[String]) -> i32 {
    let bc_path = arg(args, "--bytecode", "");
    let ctx_path = arg(args, "--context-json", "");
    if bc_path.is_empty() || ctx_path.is_empty() {
        eprintln!("run requires --bytecode and --context-json");
        return 1;
    }
    let bc_text = std::fs::read_to_string(&bc_path).unwrap_or_default();
    let ctx_text = std::fs::read_to_string(&ctx_path).unwrap_or_default();
    let ir = parse_ir_from_json(&bc_text);
    let ctx = parse_ctx_from_json(&ctx_text);
    match execute(&ir, &ctx) {
        Some(r) => {
            let _vals: Vec<String> = r
                .action
                .iter()
                .map(|(k, v)| format!("  {k}: {v}"))
                .collect();
            println!("{{");
            println!("  \"matched\": true,");
            println!("  \"rule_name\": \"{}\",", r.name);
            println!("  \"priority\": {},", r.priority);
            println!("  \"action\": {{");
            println!("    \"kind\": \"phy\",");
            println!("    \"values\": {{");
            for (i, (k, v)) in r.action.iter().enumerate() {
                let comma = if i + 1 < r.action.len() { "," } else { "" };
                println!("      \"{k}\": \"{v}\"{comma}");
            }
            println!("    }}");
            println!("  }}");
            println!("}}");
        }
        None => println!("{{\"matched\":false,\"rule_name\":\"\",\"priority\":0,\"action\":null}}"),
    }
    0
}

fn cmd_mutate(args: &[String]) -> i32 {
    let from_gpl = arg(args, "--from-gpl", "policies/gpl/default_evolved.gpl");
    let out_gpl = arg(args, "--out-gpl", "policies/gpl/default_autonomous.gpl");
    let metrics_path = arg(args, "--metrics-json", "morphic_soul_manifest_now.json");
    let text = match std::fs::read_to_string(&from_gpl) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("read {from_gpl}: {e}");
            return 1;
        }
    };
    let rules = match parse_gpl(&text) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("parse: {e}");
            return 1;
        }
    };
    let metrics = std::fs::read_to_string(&metrics_path)
        .map(|t| parse_ctx_from_json(&t))
        .unwrap_or_default();
    let mutated = mutate_rules(rules, &metrics);
    let rendered = render_gpl(&mutated);
    if let Some(p) = PathBuf::from(&out_gpl).parent() {
        let _ = std::fs::create_dir_all(p);
    }
    std::fs::write(&out_gpl, &rendered).unwrap();
    println!("out_gpl={out_gpl}");
    println!("rules_after={}", mutated.len());
    0
}

fn cmd_bench(args: &[String]) -> i32 {
    let gpl_path = arg(args, "--gpl", "policies/gpl/default_evolved.gpl");
    let out_json = arg(args, "--out-json", "gplc_rt_bench.json");
    let iters: usize = arg(args, "--iterations", "50000").parse().unwrap_or(50000);
    let text = match std::fs::read_to_string(&gpl_path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("read {gpl_path}: {e}");
            return 1;
        }
    };
    let ctx: HashMap<String, String> = [
        ("traffic", "control_authoritative"),
        ("confidence", "0.91"),
        ("predicted", "adaptive_best"),
        ("renegotiation_needed", "false"),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();

    // Bench: compile once
    let t0 = Instant::now();
    let rules = parse_gpl(&text).unwrap();
    let ir = lower_to_ir(rules).unwrap();
    let compile_us = t0.elapsed().as_secs_f64() * 1e6;

    // Bench: runtime execute
    let t0 = Instant::now();
    for _ in 0..iters {
        let _ = execute(&ir, &ctx);
    }
    let runtime_total_us = t0.elapsed().as_secs_f64() * 1e6;
    let runtime_mean_us = runtime_total_us / iters as f64;

    println!("compile_once_us={compile_us:.2}");
    println!("runtime_mean_us={runtime_mean_us:.4}");
    println!("iterations={iters}");

    let report = format!(
        "{{\n  \"compile_once_us\": {compile_us:.2},\n  \"runtime_mean_us\": {runtime_mean_us:.4},\n  \"iterations\": {iters},\n  \"rules\": {}\n}}",
        ir.len()
    );
    std::fs::write(&out_json, report).unwrap();
    println!("bench_json={out_json}");
    0
}

// ── minimal JSON readers (no serde_json dep needed here) ─────────────────────

fn parse_ctx_from_json(text: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let kv_re = regex_kv(text);
    for (k, v) in kv_re {
        map.insert(k, v);
    }
    map
}

fn regex_kv(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '"' {
            continue;
        }
        let key = read_json_str(&mut chars);
        skip_ws(&mut chars);
        if chars.next() != Some(':') {
            continue;
        }
        skip_ws(&mut chars);
        let val = match chars.peek() {
            Some('"') => {
                chars.next();
                read_json_str(&mut chars)
            }
            Some('t') => {
                for _ in 0..3 {
                    chars.next();
                }
                "true".to_string()
            }
            Some('f') => {
                for _ in 0..4 {
                    chars.next();
                }
                "false".to_string()
            }
            _ => read_json_num(&mut chars),
        };
        if !key.is_empty() {
            out.push((key, val));
        }
    }
    out
}

fn read_json_str(chars: &mut std::iter::Peekable<std::str::Chars>) -> String {
    let mut s = String::new();
    while let Some(c) = chars.next() {
        if c == '"' {
            break;
        }
        if c == '\\' {
            if let Some(e) = chars.next() {
                s.push(e);
            }
            continue;
        }
        s.push(c);
    }
    s
}

fn read_json_num(chars: &mut std::iter::Peekable<std::str::Chars>) -> String {
    let mut s = String::new();
    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() || c == '.' || c == '-' || c == 'e' || c == 'E' || c == '+' {
            s.push(c);
            chars.next();
        } else {
            break;
        }
    }
    s
}

fn skip_ws(chars: &mut std::iter::Peekable<std::str::Chars>) {
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
        } else {
            break;
        }
    }
}

fn parse_ir_from_json(text: &str) -> Vec<IrRule> {
    let rules = match parse_gpl_from_json_artifact(text) {
        Some(r) => r,
        None => return Vec::new(),
    };
    lower_to_ir(rules).unwrap_or_default()
}

fn parse_gpl_from_json_artifact(text: &str) -> Option<Vec<GplRule>> {
    let src_key = "\"source_gpl\"";
    let pos = text.find(src_key)?;
    let after =
        &text[pos + src_key.len()..].trim_start_matches(|c: char| c.is_whitespace() || c == ':');
    if !after.starts_with('"') {
        return None;
    }
    let path_str = read_json_str(&mut after[1..].chars().peekable());
    let file_text = std::fs::read_to_string(&path_str).ok()?;
    parse_gpl(&file_text).ok()
}

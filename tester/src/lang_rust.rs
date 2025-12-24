use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

fn get_tester_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

pub fn launch(suite: &str, output_dir: &Path, generated_path: &Path, type_names: &[String]) -> Result<Child, String> {
    let test_crate_dir = output_dir.join(format!("{}_testbin_rust", suite));
    fs::create_dir_all(test_crate_dir.join("src")).map_err(|e| format!("create dir: {}", e))?;

    // Get runtime path relative to tester package
    let runtime_path = get_tester_dir()
        .parent()
        .ok_or("failed to get parent dir")?
        .join("runtime-rust");
    let runtime_path_rel =
        pathdiff::diff_paths(&runtime_path, &test_crate_dir).ok_or("failed to compute relative path")?;

    // Create Cargo.toml
    let cargo_toml = format!(
        r#"[workspace]

[package]
name = "{}_testbin_rust"
version = "0.1.0"
edition = "2024"

[[bin]]
name = "{}_testbin"
path = "src/main.rs"

[dependencies]
volex = {{ path = "{}" }}
serde = {{ version = "1.0", features = ["derive"] }}
serde_json = "1.0"
hex = "0.4"

[features]
serde = []
"#,
        suite,
        suite,
        runtime_path_rel.display()
    );
    fs::write(test_crate_dir.join("Cargo.toml"), cargo_toml).map_err(|e| format!("write Cargo.toml: {}", e))?;

    // Create main.rs
    let relative_gen_path = pathdiff::diff_paths(generated_path, test_crate_dir.join("src"))
        .unwrap_or_else(|| PathBuf::from("../../basic_generated.rs"));

    // Generate use statements, handling "Result" naming conflict
    let use_types = type_names
        .iter()
        .filter(|t| *t != "Result")
        .map(|t| t.as_str())
        .collect::<Vec<_>>()
        .join(", ");

    let has_result = type_names.contains(&"Result".to_string());
    let use_result = if has_result {
        "\nuse generated::Result as SchemaResult;"
    } else {
        ""
    };

    // Generate match arms for encode/decode
    let encode_arms = type_names
        .iter()
        .map(|t| {
            let type_ref = if t == "Result" { "SchemaResult" } else { t.as_str() };
            format!(
                r#"        "{}" => {{
            let val: {} = serde_json::from_value(json)
                .map_err(|e| format!("failed to deserialize: {{}}", e))?;
            val.encode(&mut buf);
        }}"#,
                t, type_ref
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let decode_arms = type_names
        .iter()
        .map(|t| {
            let type_ref = if t == "Result" { "SchemaResult" } else { t.as_str() };
            format!(
                r#"        "{}" => {{
            let val = {}::decode(&mut slice)
                .map_err(|e| format!("decode failed: {{:?}}", e))?;
            serde_json::to_value(&val)
                .map_err(|e| format!("failed to serialize: {{}}", e))?
        }}"#,
                t, type_ref
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let main_rs = format!(
        r#"
mod generated {{
    include!("{}");
}}

use generated::{{{}}};{}
use volex::{{Encode, Decode}};
use serde_json::{{self, json}};
use std::io::{{self, BufRead, Write}};

#[derive(serde::Deserialize)]
#[serde(tag = "cmd")]
enum Request {{
    #[serde(rename = "encode")]
    Encode {{
        r#type: String,
        json: serde_json::Value,
    }},
    #[serde(rename = "decode")]
    Decode {{
        r#type: String,
        hex: String,
    }},
}}

fn encode_type(type_name: &str, json: serde_json::Value) -> std::result::Result<String, String> {{
    let mut buf = Vec::new();

    match type_name {{
{}
        _ => return Err(format!("unknown type: {{}}", type_name)),
    }}

    Ok(hex::encode(&buf))
}}

fn decode_type(type_name: &str, hex: &str) -> std::result::Result<serde_json::Value, String> {{
    let bytes = hex::decode(hex)
        .map_err(|e| format!("invalid hex: {{}}", e))?;
    let mut slice = bytes.as_slice();

    let result = match type_name {{
{}
        _ => return Err(format!("unknown type: {{}}", type_name)),
    }};

    Ok(result)
}}

fn main() {{
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {{
        let line = match line {{
            Ok(l) => l,
            Err(e) => {{
                eprintln!("Failed to read line: {{}}", e);
                break;
            }}
        }};

        let req: Request = match serde_json::from_str(&line) {{
            Ok(r) => r,
            Err(e) => {{
                let resp = json!({{
                    "ok": false,
                    "error": format!("invalid request: {{}}", e)
                }});
                writeln!(stdout, "{{}}", resp).unwrap();
                stdout.flush().unwrap();
                continue;
            }}
        }};

        let resp = match req {{
            Request::Encode {{ r#type, json }} => {{
                match encode_type(&r#type, json) {{
                    Ok(hex) => json!({{ "ok": true, "result": hex }}),
                    Err(e) => json!({{ "ok": false, "error": e }}),
                }}
            }}
            Request::Decode {{ r#type, hex }} => {{
                match decode_type(&r#type, &hex) {{
                    Ok(val) => json!({{ "ok": true, "result": val }}),
                    Err(e) => json!({{ "ok": false, "error": e }}),
                }}
            }}
        }};

        writeln!(stdout, "{{}}", resp).unwrap();
        stdout.flush().unwrap();
    }}
}}
"#,
        relative_gen_path.display(),
        use_types,
        use_result,
        encode_arms,
        decode_arms
    );
    fs::write(test_crate_dir.join("src/main.rs"), main_rs).map_err(|e| format!("write main.rs: {}", e))?;

    // Launch with cargo run (builds if needed, then runs)
    println!("  Launching with cargo run...");
    let child = Command::new("cargo")
        .args(&["run", "--release", "--features", "serde", "--quiet"])
        .current_dir(&test_crate_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("cargo run: {}", e))?;

    Ok(child)
}

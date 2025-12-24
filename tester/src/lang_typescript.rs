use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

fn get_tester_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

pub fn launch(suite: &str, output_dir: &Path, generated_path: &Path, type_names: &[String]) -> Result<Child, String> {
    let test_dir = output_dir.join(format!("{}_testbin_typescript", suite));
    fs::create_dir_all(&test_dir).map_err(|e| format!("create test dir: {}", e))?;

    // Create package.json
    let package_json = r#"{
  "name": "volex-testbin",
  "version": "1.0.0",
  "type": "module"
}
"#;
    fs::write(test_dir.join("package.json"), package_json).map_err(|e| format!("write package.json: {}", e))?;

    // Copy generated TypeScript code
    let generated_code = fs::read_to_string(generated_path).map_err(|e| format!("read generated code: {}", e))?;
    fs::write(test_dir.join("generated.ts"), generated_code).map_err(|e| format!("write generated.ts: {}", e))?;

    // Copy runtime library
    let runtime_src = get_tester_dir()
        .parent()
        .ok_or("failed to get parent dir")?
        .join("runtime-typescript/volex.ts");
    let runtime_code = fs::read_to_string(&runtime_src).map_err(|e| format!("read runtime: {}", e))?;
    fs::write(test_dir.join("volex.ts"), runtime_code).map_err(|e| format!("write runtime: {}", e))?;

    // Generate encode/decode switch cases
    let encode_cases = type_names
        .iter()
        .map(|t| {
            format!(
                r#"    case "{}":
      const val{} = convertJsonValue(req.json);
      const buf{}: Uint8Array[] = [];
      encode{}(val{}, buf{});
      return {{ ok: true, result: Array.from(runtime.flattenBuf(buf{})).map(b => b.toString(16).padStart(2, '0')).join('') }};"#,
                t, t, t, t, t, t, t
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let decode_cases = type_names
        .iter()
        .map(|t| {
            format!(
                r#"    case "{}":
      const bytes{} = req.hex ? new Uint8Array(req.hex.match(/.{{1,2}}/g)!.map(byte => parseInt(byte, 16))) : new Uint8Array(0);
      const buf{} = new runtime.Buf(bytes{});
      const result{} = decode{}(buf{});
      return {{ ok: true, result: convertToJson(result{}) }};"#,
                t, t, t, t, t, t, t, t
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    // Create main.ts with test binary protocol implementation
    let main_ts = format!(
        r#"import * as runtime from "./volex.js";
import {{
{}
}} from "./generated.js";
import * as readline from "readline";

interface Request {{
  cmd: "encode" | "decode";
  type: string;
  json?: any;
  hex?: string;
}}

interface Response {{
  ok: boolean;
  result?: any;
  error?: string;
}}

// Convert JSON value to proper TypeScript types (Objects to Maps for map fields only)
function convertJsonValue(val: any, depth: number = 0): any {{
  if (val === null || val === undefined) return val;
  if (Array.isArray(val)) {{
    return val.map(v => convertJsonValue(v, depth + 1));
  }}
  if (typeof val === 'object') {{
    const result: any = {{}};
    for (const [k, v] of Object.entries(val)) {{
      // Convert fields named 'values', 'map' to Maps if they're objects (but not if they have $tag, which indicates a union)
      if ((k === 'values' || k === 'map' || k.endsWith('_map')) &&
          v !== null && typeof v === 'object' && !Array.isArray(v) && !v.$tag) {{
        const map = new Map();
        for (const [mk, mv] of Object.entries(v)) {{
          map.set(mk, convertJsonValue(mv, depth + 1));
        }}
        result[k] = map;
      }} else if (k === '$value' && val.$tag === 'Map' && v !== null && typeof v === 'object' && !Array.isArray(v)) {{
        // For unions with tag="Map", convert $value to Map
        const map = new Map();
        for (const [mk, mv] of Object.entries(v)) {{
          map.set(mk, convertJsonValue(mv, depth + 1));
        }}
        result[k] = map;
      }} else {{
        result[k] = convertJsonValue(v, depth + 1);
      }}
    }}
    return result;
  }}
  return val;
}}

// Convert result back to JSON-serializable form (Maps to objects)
function convertToJson(val: any): any {{
  if (val === null || val === undefined) return val;
  if (val instanceof Map) {{
    const obj: any = {{}};
    for (const [k, v] of val.entries()) {{
      obj[k] = convertToJson(v);
    }}
    return obj;
  }}
  if (Array.isArray(val)) {{
    return val.map(convertToJson);
  }}
  if (typeof val === 'object') {{
    const result: any = {{}};
    for (const [k, v] of Object.entries(val)) {{
      result[k] = convertToJson(v);
    }}
    return result;
  }}
  return val;
}}

function processRequest(req: Request): Response {{
  try {{
    if (req.cmd === "encode") {{
      switch (req.type) {{
{}
        default:
          return {{ ok: false, error: `Unknown type: ${{req.type}}` }};
      }}
    }} else if (req.cmd === "decode") {{
      switch (req.type) {{
{}
        default:
          return {{ ok: false, error: `Unknown type: ${{req.type}}` }};
      }}
    }} else {{
      return {{ ok: false, error: `Unknown command: ${{req.cmd}}` }};
    }}
  }} catch (error) {{
    return {{ ok: false, error: String(error) }};
  }}
}}

// Read from stdin line by line
const rl = readline.createInterface({{
  input: process.stdin,
  output: process.stdout,
  terminal: false
}});

rl.on('line', (line: string) => {{
  if (line.trim()) {{
    const req = JSON.parse(line);
    const resp = processRequest(req);
    console.log(JSON.stringify(resp));
  }}
}});
"#,
        type_names
            .iter()
            .flat_map(|t| vec![format!("encode{}", t), format!("decode{}", t)])
            .collect::<Vec<_>>()
            .join(", "),
        encode_cases,
        decode_cases
    );

    fs::write(test_dir.join("main.ts"), main_ts).map_err(|e| format!("write main.ts: {}", e))?;

    // Create tsconfig.json
    let tsconfig = r#"{
  "compilerOptions": {
    "target": "ES2020",
    "module": "ES2020",
    "moduleResolution": "node",
    "esModuleInterop": true,
    "strict": false,
    "skipLibCheck": true
  }
}
"#;
    fs::write(test_dir.join("tsconfig.json"), tsconfig).map_err(|e| format!("write tsconfig.json: {}", e))?;

    // Launch with tsx (TypeScript execution)
    println!("  Launching with tsx...");
    let child = Command::new("npx")
        .args(&["tsx", "main.ts"])
        .current_dir(&test_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("npx tsx: {}", e))?;

    Ok(child)
}

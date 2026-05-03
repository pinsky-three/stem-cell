use serde::Deserialize;
use std::path::Path;
use std::process::Command;
use std::{fs, io::Write};

// ── Minimal YAML spec types (mirrors resource-model-macro/src/spec.rs) ──

#[derive(Deserialize)]
struct Spec {
    entities: Vec<Entity>,
    relations: Vec<Relation>,
}

#[derive(Deserialize)]
struct Entity {
    name: String,
    table: String,
    fields: Vec<Field>,
}

#[derive(Deserialize)]
struct Field {
    name: String,
    #[serde(rename = "type")]
    ty: String,
    required: bool,
    #[serde(default)]
    references: Option<Reference>,
}

#[derive(Deserialize)]
struct Reference {
    entity: String,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct Relation {
    name: String,
    kind: String,
    source: String,
    target: String,
}

// ── Minimal systems YAML types (mirrors system-model-macro/src/spec.rs) ──

#[derive(Deserialize)]
struct SystemsSpec {
    #[allow(dead_code)]
    version: u32,
    #[serde(default)]
    systems: Vec<SystemDef>,
}

#[derive(Deserialize)]
struct SystemDef {
    name: String,
    description: String,
    input: Vec<SystemInputField>,
    #[serde(default)]
    result: Vec<SystemResultField>,
}

#[derive(Deserialize)]
struct SystemInputField {
    name: String,
    #[serde(rename = "type")]
    ty: String,
    required: bool,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct SystemResultField {
    name: String,
    from: String,
}

fn main() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root not found");

    let frontend = workspace_root.join("frontend");
    let spec_path = workspace_root.join("specs/self.yaml");
    let systems_path = workspace_root.join("specs/systems.yaml");
    let index = workspace_root.join("public/index.html");

    println!("cargo:rerun-if-changed={}", index.display());
    println!("cargo:rerun-if-changed={}", spec_path.display());
    println!("cargo:rerun-if-changed={}", systems_path.display());
    println!("cargo:rerun-if-changed={}", frontend.join("src").display());
    println!(
        "cargo:rerun-if-changed={}",
        frontend.join("astro.config.mjs").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        frontend.join("package.json").display()
    );

    if std::env::var("SKIP_FRONTEND").is_ok() {
        // The `embed-assets` feature expands `rust_embed::RustEmbed` over
        // `$CARGO_MANIFEST_DIR/../../public` at macro-expansion time. Ensure
        // the directory exists (even empty) so `SKIP_FRONTEND=1 cargo check`
        // keeps working on fresh clones that haven't built the frontend yet.
        let public = workspace_root.join("public");
        if !public.exists() {
            fs::create_dir_all(&public).ok();
            println!(
                "cargo:warning=created empty {} for embed-assets macro",
                public.display()
            );
        }
        println!("cargo:warning=SKIP_FRONTEND set — skipping frontend build");
        return;
    }

    let spec = load_spec(&spec_path);
    let systems_spec = load_systems_spec(&systems_path);
    generate_admin_pages(&spec, &systems_spec, &frontend.join("src/pages/admin"));

    if !frontend.join("node_modules").exists() {
        run("npm", &["install"], &frontend);
    }

    run("npm", &["run", "build"], &frontend);
}

fn load_spec(path: &Path) -> Spec {
    let yaml =
        fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    serde_yaml::from_str(&yaml).unwrap_or_else(|e| panic!("cannot parse YAML: {e}"))
}

fn load_systems_spec(path: &Path) -> SystemsSpec {
    let yaml =
        fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    serde_yaml::from_str(&yaml).unwrap_or_else(|e| panic!("cannot parse systems YAML: {e}"))
}

fn generate_admin_pages(spec: &Spec, systems_spec: &SystemsSpec, pages_dir: &Path) {
    clean_generated_pages(pages_dir);

    let systems_dir = pages_dir.join("systems");
    clean_generated_pages(&systems_dir);

    generate_dashboard(spec, systems_spec, pages_dir);

    for entity in &spec.entities {
        let rels: Vec<&Relation> = spec
            .relations
            .iter()
            .filter(|r| r.source == entity.name && r.kind == "has_many")
            .collect();
        generate_entity_page(entity, &rels, pages_dir);
    }

    for system in &systems_spec.systems {
        generate_system_page(system, &systems_dir);
    }
}

fn clean_generated_pages(dir: &Path) {
    if dir.exists() {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "astro")
                && fs::read_to_string(&path).is_ok_and(|c| c.starts_with("<!-- @generated"))
            {
                fs::remove_file(&path).ok();
            }
        }
    } else {
        fs::create_dir_all(dir).unwrap();
    }
}

fn to_snake_case(s: &str) -> String {
    let mut result = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            result.push('_');
        }
        result.extend(c.to_lowercase());
    }
    result
}

fn input_html(field: &Field) -> String {
    let name = &field.name;
    let required = if field.required { " required" } else { "" };
    match field.ty.as_str() {
        "bool" => format!(
            r#"<label class="flex items-center gap-2 text-sm">
              <input type="checkbox" name="{name}" class="rounded border-neutral-600 bg-neutral-800 text-indigo-500 focus:ring-indigo-500" />
              {name}
            </label>"#
        ),
        "int" | "bigint" => format!(
            r#"<input type="number" name="{name}" placeholder="{name}" step="1"
              class="rounded-lg border border-neutral-700 bg-neutral-800 px-3 py-2 text-sm text-neutral-100 placeholder:text-neutral-500 focus:border-indigo-500 focus:outline-none focus:ring-1 focus:ring-indigo-500"{required} />"#
        ),
        "float" => format!(
            r#"<input type="number" name="{name}" placeholder="{name}" step="any"
              class="rounded-lg border border-neutral-700 bg-neutral-800 px-3 py-2 text-sm text-neutral-100 placeholder:text-neutral-500 focus:border-indigo-500 focus:outline-none focus:ring-1 focus:ring-indigo-500"{required} />"#
        ),
        "text" => format!(
            r#"<textarea name="{name}" placeholder="{name}" rows="2"
              class="rounded-lg border border-neutral-700 bg-neutral-800 px-3 py-2 text-sm text-neutral-100 placeholder:text-neutral-500 focus:border-indigo-500 focus:outline-none focus:ring-1 focus:ring-indigo-500"{required}></textarea>"#
        ),
        "uuid" if field.references.is_some() => format!(
            r#"<input type="text" name="{name}" placeholder="{name} (UUID)"
              pattern="[0-9a-f]{{8}}-[0-9a-f]{{4}}-[0-9a-f]{{4}}-[0-9a-f]{{4}}-[0-9a-f]{{12}}"
              class="rounded-lg border border-neutral-700 bg-neutral-800 px-3 py-2 text-sm text-neutral-100 placeholder:text-neutral-500 focus:border-indigo-500 focus:outline-none focus:ring-1 focus:ring-indigo-500"{required} />"#
        ),
        _ => format!(
            r#"<input type="text" name="{name}" placeholder="{name}"
              class="rounded-lg border border-neutral-700 bg-neutral-800 px-3 py-2 text-sm text-neutral-100 placeholder:text-neutral-500 focus:border-indigo-500 focus:outline-none focus:ring-1 focus:ring-indigo-500"{required} />"#
        ),
    }
}

fn generate_dashboard(spec: &Spec, systems_spec: &SystemsSpec, pages_dir: &Path) {
    let entity_cards: String = spec
        .entities
        .iter()
        .map(|e| {
            let table = &e.table;
            let name = &e.name;
            format!(
                r#"      <a href="/admin/{table}" class="group rounded-xl border border-neutral-800 bg-neutral-900/50 p-6 transition hover:border-indigo-600/40 hover:bg-neutral-900">
        <h3 class="text-lg font-semibold">{name}</h3>
        <p id="count-{table}" class="mt-2 text-2xl font-bold text-indigo-400">...</p>
        <p class="mt-1 text-xs text-neutral-500">/admin/{table}</p>
      </a>"#
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let system_cards: String = systems_spec
        .systems
        .iter()
        .map(|s| {
            let snake = to_snake_case(&s.name);
            let name = &s.name;
            let desc = &s.description;
            let input_count = s.input.len();
            format!(
                r#"      <a href="/admin/systems/{snake}" class="group rounded-xl border border-neutral-800 bg-neutral-900/50 p-6 transition hover:border-emerald-600/40 hover:bg-neutral-900">
        <h3 class="text-lg font-semibold">{name}</h3>
        <p class="mt-2 text-sm text-neutral-400">{desc}</p>
        <p class="mt-2 text-xs text-neutral-500">{input_count} inputs &middot; POST /api/systems/{snake}</p>
      </a>"#
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let fetch_counts: String = spec
        .entities
        .iter()
        .map(|e| {
            let table = &e.table;
            format!(
                r#"    fetch('/api/{table}').then(r=>r.json()).then(d=>{{
      document.getElementById('count-{table}').textContent=d.length+' records';
    }}).catch(()=>{{
      document.getElementById('count-{table}').textContent='--';
    }});"#
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let systems_section = if systems_spec.systems.is_empty() {
        String::new()
    } else {
        format!(
            r#"
    <h2 class="mt-14 text-xl font-bold tracking-tight text-neutral-300">Systems</h2>
    <p class="mt-1 text-sm text-neutral-500">Declarative business workflows generated from systems.yaml</p>
    <div class="mt-6 grid gap-6 sm:grid-cols-2 lg:grid-cols-3">
{system_cards}
    </div>"#
        )
    };

    let page = format!(
        r#"<!-- @generated from specs/self.yaml + systems.yaml — do not edit -->
---
import Base from "../../layouts/Base.astro";
---
<Base title="Admin Dashboard" noindex={{true}}>
  <div class="mx-auto max-w-6xl px-6 py-12">
    <div class="flex items-center justify-between">
      <h1 class="text-3xl font-bold tracking-tight">Admin Dashboard</h1>
      <a href="/api/docs" class="rounded-lg border border-neutral-700 px-4 py-2 text-sm font-medium text-neutral-300 transition hover:border-indigo-500 hover:text-indigo-400">
        API Docs
      </a>
    </div>

    <h2 class="mt-10 text-xl font-bold tracking-tight text-neutral-300">Entities</h2>
    <div class="mt-6 grid gap-6 sm:grid-cols-2 lg:grid-cols-3">
{entity_cards}
    </div>
{systems_section}
  </div>

  <script>
{fetch_counts}
  </script>
</Base>
"#
    );

    let path = pages_dir.join("index.astro");
    let mut f = fs::File::create(&path).unwrap();
    f.write_all(page.as_bytes()).unwrap();
}

fn generate_entity_page(entity: &Entity, relations: &[&Relation], pages_dir: &Path) {
    let table = &entity.table;
    let name = &entity.name;
    let entity_lower = to_snake_case(name);

    // Table headers
    let th_list: String = std::iter::once("id".to_string())
        .chain(entity.fields.iter().map(|f| f.name.clone()))
        .chain(std::iter::once("actions".to_string()))
        .map(|h| format!(r#"<th class="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-neutral-400">{h}</th>"#))
        .collect::<Vec<_>>()
        .join("\n          ");

    // Form inputs
    let form_fields: String = entity
        .fields
        .iter()
        .map(|f| input_html(f))
        .collect::<Vec<_>>()
        .join("\n        ");

    // JS: build table row from object
    let td_cells: String = std::iter::once(format!(
        r#"`<td class="px-4 py-3 font-mono text-xs text-neutral-500">${{r.id.substring(0,8)}}...</td>`"#
    ))
    .chain(entity.fields.iter().map(|f| {
        let n = &f.name;
        match f.ty.as_str() {
            "bool" => format!(
                r#"`<td class="px-4 py-3 text-sm">${{r.{n}?'<span class="text-green-400">yes</span>':'<span class="text-red-400">no</span>'}}</td>`"#
            ),
            "uuid" if f.references.is_some() => {
                let ref_entity = f.references.as_ref().unwrap();
                let ref_table = entity_table_for(&ref_entity.entity);
                format!(
                    r#"`<td class="px-4 py-3 font-mono text-xs"><a href="/admin/{ref_table}" class="text-indigo-400 hover:underline">${{r.{n}?r.{n}.substring(0,8)+'...':'—'}}</a></td>`"#
                )
            }
            _ => format!(
                r#"`<td class="px-4 py-3 text-sm text-neutral-300">${{r.{n}??'—'}}</td>`"#
            ),
        }
    }))
    .collect::<Vec<_>>()
    .join("+\n        ");

    // JS: populate form from object for editing
    let populate_fields: String = entity
        .fields
        .iter()
        .map(|f| {
            let n = &f.name;
            match f.ty.as_str() {
                "bool" => format!(r#"form.elements['{n}'].checked = item.{n};"#),
                _ => format!(r#"form.elements['{n}'].value = item.{n} ?? '';"#),
            }
        })
        .collect::<Vec<_>>()
        .join("\n      ");

    // JS: read form into object
    let read_fields: String = entity
        .fields
        .iter()
        .map(|f| {
            let n = &f.name;
            match f.ty.as_str() {
                "bool" => format!(r#"{n}: form.elements['{n}'].checked"#),
                "int" | "bigint" => format!(r#"{n}: form.elements['{n}'].value ? Number(form.elements['{n}'].value) : null"#),
                "float" => format!(r#"{n}: form.elements['{n}'].value ? parseFloat(form.elements['{n}'].value) : null"#),
                _ => {
                    if f.required {
                        format!(r#"{n}: form.elements['{n}'].value"#)
                    } else {
                        format!(r#"{n}: form.elements['{n}'].value || null"#)
                    }
                }
            }
        })
        .collect::<Vec<_>>()
        .join(",\n        ");

    // Relation links in row actions
    let rel_links: String = relations
        .iter()
        .map(|r| {
            let rn = &r.name;
            format!(
                r#"<button onclick="showRelation('${{r.id}}','{rn}')" class="text-indigo-400 hover:underline">{rn}</button>"#
            )
        })
        .collect::<Vec<_>>()
        .join(" ");

    let page = format!(
        r##"<!-- @generated from specs/self.yaml — do not edit -->
---
import Base from "../../layouts/Base.astro";
---
<Base title="{name} | Admin" noindex={{true}}>
  <div class="mx-auto max-w-7xl px-6 py-12">
    <div class="flex items-center justify-between">
      <div class="flex items-center gap-4">
        <a href="/admin" class="text-neutral-500 transition hover:text-neutral-300">&larr; Dashboard</a>
        <h1 class="text-3xl font-bold tracking-tight">{name}</h1>
      </div>
      <button id="create-btn" class="rounded-lg bg-indigo-600 px-4 py-2 text-sm font-semibold transition hover:bg-indigo-500">
        + New {entity_lower}
      </button>
    </div>

    <!-- Data table -->
    <div class="mt-8 overflow-x-auto rounded-xl border border-neutral-800">
      <table class="w-full">
        <thead class="border-b border-neutral-800 bg-neutral-900/50">
          <tr>
          {th_list}
          </tr>
        </thead>
        <tbody id="table-body" class="divide-y divide-neutral-800/50"></tbody>
      </table>
    </div>
  </div>

  <!-- Form dialog -->
  <dialog id="form-dialog" class="rounded-xl border border-neutral-700 bg-neutral-900 p-0 text-neutral-100 shadow-2xl backdrop:bg-black/60">
    <form id="entity-form" method="dialog" class="flex flex-col gap-4 p-6" style="min-width:380px">
      <h2 id="form-title" class="text-lg font-bold">New {entity_lower}</h2>
      <div class="flex flex-col gap-3">
        {form_fields}
      </div>
      <div class="flex justify-end gap-3 pt-2">
        <button type="button" id="cancel-btn" class="rounded-lg border border-neutral-700 px-4 py-2 text-sm transition hover:bg-neutral-800">Cancel</button>
        <button type="submit" class="rounded-lg bg-indigo-600 px-4 py-2 text-sm font-semibold transition hover:bg-indigo-500">Save</button>
      </div>
    </form>
  </dialog>

  <!-- Relation panel -->
  <dialog id="rel-dialog" class="rounded-xl border border-neutral-700 bg-neutral-900 p-6 text-neutral-100 shadow-2xl backdrop:bg-black/60" style="min-width:400px;max-height:80vh">
    <div class="flex items-center justify-between mb-4">
      <h2 id="rel-title" class="text-lg font-bold"></h2>
      <button onclick="document.getElementById('rel-dialog').close()" class="text-neutral-500 hover:text-neutral-300">&times;</button>
    </div>
    <pre id="rel-body" class="overflow-auto text-xs text-neutral-300 bg-neutral-800 rounded-lg p-4 max-h-96"></pre>
  </dialog>

  <script>
    const API = '/api/{table}';
    const dialog = document.getElementById('form-dialog');
    const form = document.getElementById('entity-form');
    const relDialog = document.getElementById('rel-dialog');
    let editingId = null;

    function renderRow(r) {{
      return `<tr class="transition hover:bg-neutral-900/30">` +
        {td_cells} +
        `<td class="px-4 py-3 text-sm flex gap-3">
          <button onclick="editItem('${{r.id}}')" class="text-indigo-400 hover:underline">edit</button>
          <button onclick="deleteItem('${{r.id}}')" class="text-red-400 hover:underline">del</button>
          {rel_links}
        </td></tr>`;
    }}

    async function loadData() {{
      const res = await fetch(API);
      const items = await res.json();
      document.getElementById('table-body').innerHTML = items.map(renderRow).join('');
    }}

    document.getElementById('create-btn').addEventListener('click', () => {{
      editingId = null;
      form.reset();
      document.getElementById('form-title').textContent = 'New {entity_lower}';
      dialog.showModal();
    }});

    document.getElementById('cancel-btn').addEventListener('click', () => dialog.close());

    form.addEventListener('submit', async (e) => {{
      e.preventDefault();
      const body = {{
        {read_fields}
      }};
      const url = editingId ? `${{API}}/${{editingId}}` : API;
      const method = editingId ? 'PUT' : 'POST';
      await fetch(url, {{
        method,
        headers: {{ 'Content-Type': 'application/json' }},
        body: JSON.stringify(body),
      }});
      dialog.close();
      loadData();
    }});

    window.editItem = async function(id) {{
      const res = await fetch(`${{API}}/${{id}}`);
      const item = await res.json();
      editingId = id;
      document.getElementById('form-title').textContent = 'Edit {entity_lower}';
      {populate_fields}
      dialog.showModal();
    }};

    window.deleteItem = async function(id) {{
      if (!confirm('Delete this {entity_lower}?')) return;
      await fetch(`${{API}}/${{id}}`, {{ method: 'DELETE' }});
      loadData();
    }};

    window.showRelation = async function(id, rel) {{
      document.getElementById('rel-title').textContent = rel + ' for ' + id.substring(0,8) + '...';
      document.getElementById('rel-body').textContent = 'Loading...';
      relDialog.showModal();
      const res = await fetch(`${{API}}/${{id}}/${{rel}}`);
      const data = await res.json();
      document.getElementById('rel-body').textContent = JSON.stringify(data, null, 2);
    }};

    loadData();
  </script>
</Base>
"##
    );

    let path = pages_dir.join(format!("{table}.astro"));
    let mut f = fs::File::create(&path).unwrap();
    f.write_all(page.as_bytes()).unwrap();
}

fn system_input_html(field: &SystemInputField) -> String {
    let name = &field.name;
    let required = if field.required { " required" } else { "" };
    let label = name.replace('_', " ");
    match field.ty.as_str() {
        "bool" => format!(
            r#"<label class="flex items-center gap-2 text-sm">
              <input type="checkbox" name="{name}" class="rounded border-neutral-600 bg-neutral-800 text-emerald-500 focus:ring-emerald-500" />
              {label}
            </label>"#
        ),
        "int" | "bigint" => format!(
            r#"<div class="flex flex-col gap-1">
              <label class="text-xs font-medium text-neutral-400">{label}</label>
              <input type="number" name="{name}" placeholder="{name}" step="1"
                class="rounded-lg border border-neutral-700 bg-neutral-800 px-3 py-2 text-sm text-neutral-100 placeholder:text-neutral-500 focus:border-emerald-500 focus:outline-none focus:ring-1 focus:ring-emerald-500"{required} />
            </div>"#
        ),
        "float" => format!(
            r#"<div class="flex flex-col gap-1">
              <label class="text-xs font-medium text-neutral-400">{label}</label>
              <input type="number" name="{name}" placeholder="{name}" step="any"
                class="rounded-lg border border-neutral-700 bg-neutral-800 px-3 py-2 text-sm text-neutral-100 placeholder:text-neutral-500 focus:border-emerald-500 focus:outline-none focus:ring-1 focus:ring-emerald-500"{required} />
            </div>"#
        ),
        "uuid" => format!(
            r#"<div class="flex flex-col gap-1">
              <label class="text-xs font-medium text-neutral-400">{label}</label>
              <input type="text" name="{name}" placeholder="{name} (UUID)"
                pattern="[0-9a-f]{{8}}-[0-9a-f]{{4}}-[0-9a-f]{{4}}-[0-9a-f]{{4}}-[0-9a-f]{{12}}"
                class="rounded-lg border border-neutral-700 bg-neutral-800 px-3 py-2 text-sm text-neutral-100 placeholder:text-neutral-500 focus:border-emerald-500 focus:outline-none focus:ring-1 focus:ring-emerald-500"{required} />
            </div>"#
        ),
        _ => format!(
            r#"<div class="flex flex-col gap-1">
              <label class="text-xs font-medium text-neutral-400">{label}</label>
              <input type="text" name="{name}" placeholder="{name}"
                class="rounded-lg border border-neutral-700 bg-neutral-800 px-3 py-2 text-sm text-neutral-100 placeholder:text-neutral-500 focus:border-emerald-500 focus:outline-none focus:ring-1 focus:ring-emerald-500"{required} />
            </div>"#
        ),
    }
}

fn generate_system_page(system: &SystemDef, systems_dir: &Path) {
    let snake = to_snake_case(&system.name);
    let name = &system.name;
    let desc = &system.description;

    let form_fields: String = system
        .input
        .iter()
        .map(|f| system_input_html(f))
        .collect::<Vec<_>>()
        .join("\n        ");

    let read_fields: String = system
        .input
        .iter()
        .map(|f| {
            let n = &f.name;
            match f.ty.as_str() {
                "bool" => format!(r#"{n}: form.elements['{n}'].checked"#),
                "int" | "bigint" => format!(r#"{n}: form.elements['{n}'].value ? Number(form.elements['{n}'].value) : null"#),
                "float" => format!(r#"{n}: form.elements['{n}'].value ? parseFloat(form.elements['{n}'].value) : null"#),
                _ => {
                    if f.required {
                        format!(r#"{n}: form.elements['{n}'].value"#)
                    } else {
                        format!(r#"{n}: form.elements['{n}'].value || null"#)
                    }
                }
            }
        })
        .collect::<Vec<_>>()
        .join(",\n          ");

    let result_fields_hint: String = if system.result.is_empty() {
        "No result fields".to_string()
    } else {
        system
            .result
            .iter()
            .map(|r| format!("{} (from {})", r.name, r.from))
            .collect::<Vec<_>>()
            .join(", ")
    };

    let page = format!(
        r##"<!-- @generated from specs/systems.yaml — do not edit -->
---
import Base from "../../../layouts/Base.astro";
---
<Base title="{name} | Systems" noindex={{true}}>
  <div class="mx-auto max-w-3xl px-6 py-12">
    <div class="flex items-center gap-4">
      <a href="/admin" class="text-neutral-500 transition hover:text-neutral-300">&larr; Dashboard</a>
      <h1 class="text-3xl font-bold tracking-tight">{name}</h1>
    </div>
    <p class="mt-2 text-sm text-neutral-400">{desc}</p>
    <p class="mt-1 text-xs text-neutral-600">POST /api/systems/{snake}</p>

    <form id="system-form" class="mt-8 flex flex-col gap-4">
      <div class="rounded-xl border border-neutral-800 bg-neutral-900/50 p-6 flex flex-col gap-4">
        <h2 class="text-sm font-semibold uppercase tracking-wider text-neutral-500">Input</h2>
        {form_fields}
      </div>
      <button type="submit" id="submit-btn"
        class="rounded-lg bg-emerald-600 px-6 py-2.5 text-sm font-semibold transition hover:bg-emerald-500 disabled:opacity-50 disabled:cursor-not-allowed">
        Execute
      </button>
    </form>

    <div id="result-panel" class="mt-6 hidden">
      <h2 class="text-sm font-semibold uppercase tracking-wider text-neutral-500 mb-2">Result</h2>
      <p class="text-xs text-neutral-600 mb-2">{result_fields_hint}</p>
      <pre id="result-body"
        class="overflow-auto rounded-xl border border-neutral-800 bg-neutral-900/50 p-4 text-xs text-neutral-300 max-h-96"></pre>
    </div>

    <div id="error-panel" class="mt-6 hidden">
      <h2 class="text-sm font-semibold uppercase tracking-wider text-red-400 mb-2">Error</h2>
      <pre id="error-body"
        class="overflow-auto rounded-xl border border-red-900/50 bg-red-950/30 p-4 text-xs text-red-300 max-h-96"></pre>
    </div>
  </div>

  <script>
    const form = document.getElementById('system-form');
    const submitBtn = document.getElementById('submit-btn');
    const resultPanel = document.getElementById('result-panel');
    const resultBody = document.getElementById('result-body');
    const errorPanel = document.getElementById('error-panel');
    const errorBody = document.getElementById('error-body');

    form.addEventListener('submit', async (e) => {{
      e.preventDefault();
      submitBtn.disabled = true;
      submitBtn.textContent = 'Executing...';
      resultPanel.classList.add('hidden');
      errorPanel.classList.add('hidden');

      const body = {{
          {read_fields}
      }};

      try {{
        const res = await fetch('/api/systems/{snake}', {{
          method: 'POST',
          headers: {{ 'Content-Type': 'application/json' }},
          body: JSON.stringify(body),
        }});
        const data = await res.json();
        if (res.ok) {{
          resultBody.textContent = JSON.stringify(data, null, 2);
          resultPanel.classList.remove('hidden');
        }} else {{
          errorBody.textContent = JSON.stringify(data, null, 2);
          errorPanel.classList.remove('hidden');
        }}
      }} catch (err) {{
        errorBody.textContent = err.message;
        errorPanel.classList.remove('hidden');
      }} finally {{
        submitBtn.disabled = false;
        submitBtn.textContent = 'Execute';
      }}
    }});
  </script>
</Base>
"##
    );

    let path = systems_dir.join(format!("{snake}.astro"));
    let mut f = fs::File::create(&path).unwrap();
    f.write_all(page.as_bytes()).unwrap();
}

/// Derive the table name from an entity name using the convention in self.yaml.
/// Falls back to pluralized snake_case.
fn entity_table_for(entity_name: &str) -> String {
    let snake = to_snake_case(entity_name);
    if snake.ends_with('y') && !snake.ends_with("ey") {
        format!("{}ies", &snake[..snake.len() - 1])
    } else if snake.ends_with('s') || snake.ends_with('x') || snake.ends_with("sh") {
        format!("{snake}es")
    } else {
        format!("{snake}s")
    }
}

fn run(cmd: &str, args: &[&str], dir: &Path) {
    let status = Command::new(cmd)
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap_or_else(|e| panic!("{cmd} failed to start: {e}"));

    assert!(status.success(), "{cmd} {args:?} exited with {status}");
}

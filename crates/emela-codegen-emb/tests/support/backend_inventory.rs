//! Source-bound implementation and test references. Their existence is not a correctness claim.
#[path = "test_inventory.rs"]
mod test_inventory;
use quote::ToTokens;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};
use syn::spanned::Spanned;

#[derive(Clone, Deserialize)]
struct Owner {
    path: String,
    symbol: String,
    role: String,
    #[serde(default)]
    lower_symbol: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct SourceEntry {
    path: String,
    symbol: String,
    line: usize,
    end_line: usize,
    body_sha256: String,
}

#[derive(Debug, Serialize)]
struct Record {
    id: String,
    kind: String,
    relation: &'static str,
    owner_id: String,
    role: String,
    review_status: &'static str,
    observed_definition_sha256: String,
    entry: SourceEntry,
    phase_entries: Vec<SourceEntry>,
    tests: Vec<test_inventory::TestEntry>,
    test_relation: &'static str,
}

#[derive(Debug, Serialize)]
struct Inventory {
    schema_version: u32,
    inventory_kind: &'static str,
    production_enabled: bool,
    definition_inventory_sha256: String,
    summary: BTreeMap<&'static str, usize>,
    records: Vec<Record>,
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_owned()
}

fn hash(bytes: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(bytes.as_ref()))
}

fn functions(root: &Path, path: &str) -> Result<BTreeMap<String, SourceEntry>, String> {
    if Path::new(path)
        .components()
        .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err(format!("entry path must stay inside the crate: {path}"));
    }
    let source = fs::read_to_string(root.join(path)).map_err(|e| format!("{path}: {e}"))?;
    let parsed = syn::parse_file(&source).map_err(|e| format!("{path}: {e}"))?;
    let mut result = BTreeMap::new();
    let mut insert = |symbol: String, signature: &syn::Signature, block: &syn::Block| {
        let span = block.span();
        let normalized = format!(
            "{} {}",
            signature.to_token_stream(),
            block.to_token_stream()
        );
        result.insert(
            symbol.clone(),
            SourceEntry {
                path: path.into(),
                symbol,
                line: signature.span().start().line,
                end_line: span.end().line,
                body_sha256: hash(normalized),
            },
        );
    };
    for item in parsed.items {
        match item {
            syn::Item::Fn(f) => insert(f.sig.ident.to_string(), &f.sig, &f.block),
            syn::Item::Impl(i) => {
                let syn::Type::Path(owner) = &*i.self_ty else {
                    continue;
                };
                let owner = owner.path.segments.last().unwrap().ident.to_string();
                for item in i.items {
                    if let syn::ImplItem::Fn(f) = item {
                        insert(format!("{owner}::{}", f.sig.ident), &f.sig, &f.block);
                    }
                }
            }
            _ => {}
        }
    }
    Ok(result)
}

fn collect(root: &Path, definitions: &Value) -> Result<Inventory, String> {
    let owners: BTreeMap<String, Owner> = serde_json::from_str(
        &fs::read_to_string(root.join("inventory/owners.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let containers: BTreeMap<String, Owner> = serde_json::from_str(
        &fs::read_to_string(root.join("inventory/containers.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let items = definitions["items"]
        .as_array()
        .ok_or("missing definition items")?;
    let mut by_id = BTreeMap::new();
    for item in items {
        let id = item["id"].as_str().ok_or("missing definition ID")?;
        if by_id.insert(id.to_owned(), item).is_some() {
            return Err(format!("duplicate definition: {id}"));
        }
    }
    let expected: BTreeSet<_> = items
        .iter()
        .filter(|i| i["kind"] == "variant")
        .map(|i| i["id"].as_str().unwrap().to_owned())
        .collect();
    let actual: BTreeSet<_> = owners.keys().cloned().collect();
    if expected != actual {
        return Err(format!(
            "variant entries differ: missing={:?}, orphan={:?}",
            expected.difference(&actual).collect::<Vec<_>>(),
            actual.difference(&expected).collect::<Vec<_>>()
        ));
    }
    let declarations: BTreeSet<_> = items
        .iter()
        .filter(|i| matches!(i["kind"].as_str(), Some("enum" | "struct" | "type_alias")))
        .map(|i| i["id"].as_str().unwrap().to_owned())
        .collect();
    if declarations != containers.keys().cloned().collect() {
        return Err("declaration responsibilities differ".into());
    }
    let mut cache = BTreeMap::new();
    for owner in owners.values().chain(containers.values()) {
        if !cache.contains_key(&owner.path) {
            cache.insert(owner.path.clone(), functions(root, &owner.path)?);
        }
    }
    let mut endpoints = BTreeSet::new();
    for (id, owner) in &owners {
        if !endpoints.insert((&owner.path, &owner.symbol)) {
            return Err(format!(
                "variant entry reused: {id}: {}::{}",
                owner.path, owner.symbol
            ));
        }
    }
    let mut records = Vec::new();
    for (id, item) in &by_id {
        let (owner_id, owner, relation) = if let Some(owner) = owners.get(id) {
            (id.clone(), owner, "variant_entry")
        } else if let Some(owner) = containers.get(id) {
            (id.clone(), owner, "shared_declaration")
        } else {
            let mut parent = item["parent"]
                .as_str()
                .ok_or_else(|| format!("no parent: {id}"))?;
            loop {
                if let Some(owner) = owners.get(parent) {
                    break (parent.to_owned(), owner, "member_of_variant");
                }
                if let Some(owner) = containers.get(parent) {
                    break (parent.to_owned(), owner, "member_of_declaration");
                }
                parent = by_id
                    .get(parent)
                    .and_then(|p| p["parent"].as_str())
                    .ok_or_else(|| format!("no owner: {id}"))?;
            }
        };
        let available = &cache[&owner.path];
        let entry = available
            .get(&owner.symbol)
            .ok_or_else(|| format!("missing entry {id}: {}::{}", owner.path, owner.symbol))?
            .clone();
        let mut phases = Vec::new();
        if relation == "variant_entry" {
            let mut names = vec![
                "lower",
                "emit_function",
                "emit_suspend",
                "emit_enter",
                "emit_leave",
                "emit_finish",
                "string_behavior",
                "string_bound",
                "result_ownership",
                "needs_heap",
                "is_known_empty",
                "require_runtime",
            ];
            let specific;
            if let Some(name) = id.strip_prefix("emela_lir::Callee::") {
                specific = format!("emit_suspend_{}", name.to_lowercase());
                names = vec![&specific];
            } else if id == "emela_lir::Value::Call" {
                names.extend([
                    "emit_suspend_direct",
                    "emit_suspend_indirect",
                    "emit_suspend_op",
                ]);
            }
            for name in names.into_iter().chain(owner.lower_symbol.as_deref()) {
                if name != owner.symbol
                    && let Some(entry) = available.get(name)
                    && !phases
                        .iter()
                        .any(|p: &SourceEntry| p.symbol == entry.symbol)
                {
                    phases.push(entry.clone());
                }
            }
        }
        records.push(Record {
            id: id.clone(),
            kind: item["kind"].as_str().unwrap().into(),
            relation,
            owner_id,
            role: owner.role.clone(),
            review_status: "unreviewed",
            observed_definition_sha256: item["definition_sha256"].as_str().unwrap().into(),
            entry,
            phase_entries: phases,
            tests: vec![],
            test_relation: "not_independently_required",
        });
    }
    let count = |kind: &str| items.iter().filter(|i| i["kind"] == kind).count();
    let audit_config: Value =
        serde_json::from_str(include_str!("../../tools/flow-implementation-routes.json"))
            .map_err(|e| e.to_string())?;
    let audit_owners: BTreeSet<_> = audit_config["targets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["owner"].as_str().unwrap())
        .collect();
    let primary = expected
        .iter()
        .filter(|id| audit_owners.contains(id.rsplit_once("::").unwrap().0))
        .count();
    Ok(Inventory {
        schema_version: 2,
        inventory_kind: "backend_implementation_ownership",
        production_enabled: true,
        definition_inventory_sha256: hash(serde_json::to_vec(definitions).unwrap()),
        summary: BTreeMap::from([
            ("definition_records", items.len()),
            ("variant_entries", owners.len()),
            ("primary_audit_variants", primary),
            ("additional_metadata_variants", owners.len() - primary),
            (
                "input_rejected_variants",
                owners.values().filter(|o| o.role == "reject").count(),
            ),
            ("fields", count("field")),
            ("shared_declarations", containers.len()),
            ("missing_entries", 0),
            ("orphan_entries", 0),
        ]),
        records,
    })
}

fn load_definitions() -> Result<Value, String> {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo = crate_dir.parent().unwrap().parent().unwrap();
    let definitions: Value = serde_json::from_slice(
        &fs::read(crate_dir.join("docs/generated/flow-ir-inventory.json"))
            .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    for source in definitions["sources"]
        .as_array()
        .ok_or("missing definition sources")?
    {
        let path = source["path"].as_str().ok_or("missing definition path")?;
        if hash(fs::read(repo.join(path)).map_err(|e| e.to_string())?)
            != source["sha256"].as_str().unwrap()
        {
            return Err(format!(
                "definition inventory is stale: {path}; regenerate the definition inventory first"
            ));
        }
    }
    Ok(definitions)
}

fn markdown(inventory: &Inventory) -> String {
    let mut out = String::from(
        "# Flow IR・本体実装・テストの対応表\n\n自動生成。通常の公開APIが使う入口と、明示的に関連付けた実在テストを記録する。入口・テストの存在は正しさや網羅性の保証ではない。\n\n",
    );
    out.push_str(&format!("定義 **{}** レコード、variant専用入口 **{}**、共有宣言 **{}**、field **{}**。欠落 **0**、定義外ID **0**。fieldと共有宣言を操作の実装数へ加算しない。\n\n",inventory.summary["definition_records"], inventory.summary["variant_entries"],inventory.summary["shared_declarations"],inventory.summary["fields"]));
    out.push_str(&format!("従来の監査対象 **{}** variant ＋ 追加のmetadata **{}** variant。入力拒否として登録したvariantは **{}**。\n\n",inventory.summary["primary_audit_variants"],inventory.summary["additional_metadata_variants"],inventory.summary["input_rejected_variants"]));
    out.push_str(&format!("登録済みの関連テスト: **{}** variant / **{}** 個の異なるテスト。宣言・fieldの参照は親variantとの関係を表示する。\n\n", inventory.summary["variants_with_registered_tests"], inventory.summary["distinct_registered_tests"]));
    out.push_str("| 定義ID | 区分 | 担当ID | 入口・担当箇所 | 追加の段階 | 関連テスト |\n| --- | --- | --- | --- | --- | --- |\n");
    let link =
        |entry: &SourceEntry| format!("[{}](../../{}#L{})", entry.symbol, entry.path, entry.line);
    for item in &inventory.records {
        out.push_str(&format!(
            "| `{}` | {} / {} | `{}` | {} | {} | {} |\n",
            item.id,
            item.relation,
            item.role,
            item.owner_id,
            link(&item.entry),
            item.phase_entries
                .iter()
                .map(&link)
                .collect::<Vec<_>>()
                .join(" / "),
            item.tests
                .iter()
                .map(|test| format!(
                    "[{}::{}](../../{}#L{}){}",
                    test.reference.target,
                    test.reference.name,
                    test.path,
                    test.line,
                    if test.ignored { " (ignored)" } else { "" }
                ))
                .collect::<Vec<_>>()
                .join(" / ")
        ));
    }
    out
}

fn render() -> Result<(String, String), String> {
    let mut inventory = collect(&root(), &load_definitions()?)?;
    let required = inventory
        .records
        .iter()
        .filter(|r| r.kind == "variant")
        .map(|r| r.id.clone())
        .collect();
    let (associations, catalog) = test_inventory::load(&root(), &required)?;
    for record in &mut inventory.records {
        let references: BTreeSet<_> = if let Some(tests) = associations.get(&record.owner_id) {
            record.test_relation = if record.kind == "variant" {
                "direct_variant"
            } else {
                "inherited_from_variant"
            };
            tests.iter().cloned().collect()
        } else {
            let prefix = format!("{}::", record.owner_id);
            let related = associations
                .iter()
                .filter(|(id, _)| id.starts_with(&prefix))
                .flat_map(|(_, tests)| tests.iter().cloned())
                .collect::<BTreeSet<_>>();
            if !related.is_empty() {
                record.test_relation = "related_variants";
            }
            related
        };
        record.tests = references.iter().map(|r| catalog[r].clone()).collect();
    }
    inventory
        .summary
        .insert("variants_with_registered_tests", associations.len());
    inventory.summary.insert(
        "distinct_registered_tests",
        associations
            .values()
            .flatten()
            .collect::<BTreeSet<_>>()
            .len(),
    );
    Ok((
        markdown(&inventory),
        serde_json::to_string_pretty(&inventory).map_err(|e| e.to_string())? + "\n",
    ))
}

#[test]
fn backend_inventory_matches_all_definition_ids_sources_and_tests() {
    let (md, json) = render().unwrap();
    for (name, expected) in [
        ("flow-backend-inventory.md", md),
        ("flow-backend-inventory.json", json),
    ] {
        assert_eq!(
            fs::read_to_string(root().join("docs/generated").join(name)).unwrap(),
            expected,
            "stale backend inventory; run tools/backend-inventory.sh"
        );
    }
}

#[test]
#[ignore = "writes the explicitly requested backend ownership artifacts"]
fn regenerate_backend_inventory() {
    let (md, json) = render().unwrap();
    fs::create_dir_all(root().join("docs/generated")).unwrap();
    fs::write(root().join("docs/generated/flow-backend-inventory.md"), md).unwrap();
    fs::write(
        root().join("docs/generated/flow-backend-inventory.json"),
        json,
    )
    .unwrap();
}

#[test]
fn newly_defined_variants_cannot_silently_inherit_another_entry() {
    let mut definitions = load_definitions().unwrap();
    definitions["items"].as_array_mut().unwrap().push(serde_json::json!({"id":"emela_lir::Value::NewVariant","kind":"variant","parent":"emela_lir::Value"}));
    assert!(
        collect(&root(), &definitions)
            .unwrap_err()
            .contains("variant entries differ")
    );
}

fn fixture() -> (tempfile::TempDir, Value) {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("inventory")).unwrap();
    fs::write(
        dir.path().join("backend.rs"),
        "fn dispatch() { a(); b(); } fn a() {} fn b() {}",
    )
    .unwrap();
    fs::write(
        dir.path().join("inventory/owners.json"),
        r#"{
      "fixture::Op::A":{"path":"backend.rs","symbol":"a","role":"emit"},
      "fixture::Op::B":{"path":"backend.rs","symbol":"b","role":"emit"}}
    "#,
    )
    .unwrap();
    fs::write(
        dir.path().join("inventory/containers.json"),
        r#"{
      "fixture::Op":{"path":"backend.rs","symbol":"dispatch","role":"schema"}}
    "#,
    )
    .unwrap();
    let definitions = serde_json::json!({"items":[
      {"id":"fixture::Op","kind":"enum","definition_sha256":"enum"},
      {"id":"fixture::Op::A","kind":"variant","parent":"fixture::Op","definition_sha256":"a"},
      {"id":"fixture::Op::B","kind":"variant","parent":"fixture::Op","definition_sha256":"b"}
    ]});
    (dir, definitions)
}

#[test]
fn missing_reused_and_orphan_entries_are_reported() {
    let (dir, definitions) = fixture();
    collect(dir.path(), &definitions).unwrap();
    fs::write(dir.path().join("backend.rs"), "fn dispatch() {} fn a() {}").unwrap();
    assert!(
        collect(dir.path(), &definitions)
            .unwrap_err()
            .contains("missing entry")
    );
    let config_path = dir.path().join("inventory/owners.json");
    let mut config: Value =
        serde_json::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
    config["fixture::Op::B"]["symbol"] = Value::String("a".into());
    fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();
    assert!(
        collect(dir.path(), &definitions)
            .unwrap_err()
            .contains("entry reused")
    );
    config["fixture::Op::Removed"] = config["fixture::Op::A"].clone();
    fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();
    assert!(
        collect(dir.path(), &definitions)
            .unwrap_err()
            .contains("orphan")
    );
}

#[test]
fn new_fields_remain_unreviewed_members_of_their_owner() {
    let (dir, mut definitions) = fixture();
    definitions["items"].as_array_mut().unwrap().push(serde_json::json!({
        "id":"fixture::Op::A.new_field","kind":"field","parent":"fixture::Op::A","definition_sha256":"new"
    }));
    let inventory = collect(dir.path(), &definitions).unwrap();
    let field = inventory
        .records
        .iter()
        .find(|r| r.id.ends_with(".new_field"))
        .unwrap();
    assert_eq!(field.relation, "member_of_variant");
    assert_eq!(field.owner_id, "fixture::Op::A");
    assert_eq!(field.review_status, "unreviewed");
    assert_eq!(inventory.summary["variant_entries"], 2);
}

# Flow IR一覧の生成と照合フォーマット

人が読む一覧はMarkdown、機械が照合するデータはJSONとし、一回の抽出から両方を生成する。Markdownの説明文を機械に解釈させず、同じ安定IDをキーにして定義側と実装側を結合する。

[実装箇所の抽出と比較の試作](implementation-inventory.md)も用意している。試作は `implementation_evidence` として構文上の候補を記録する。後述の `implementation_claims` は、レビューや対応範囲を記録する次段階の形式案である。

## 現在使える生成コマンド

リポジトリのrootで実行する。Rust/Cargoが必要で、初回は開発用依存のbuildが走る。コンパイラー本体の実行時依存は増やさない。

```sh
bash crates/emela-codegen-emb/tools/flow-ir-inventory.sh
bash crates/emela-codegen-emb/tools/flow-ir-inventory.sh --check
bash crates/emela-codegen-emb/tools/flow-ir-inventory.sh --out-dir /tmp/flow-ir-inventory
```

既定の生成先は [generated/flow-ir-inventory.md](generated/flow-ir-inventory.md) と [generated/flow-ir-inventory.json](generated/flow-ir-inventory.json)。`--check` は再抽出した内容と両ファイルを比較し、欠落・差分があれば終了code 1にする。ファイルは変更しない。`--out-dir` に相対pathを渡した場合は、実行時のworking directoryを基準にする。

抽出対象のsource fileを読む位置は、このリポジトリを基準に固定される。scriptは別のdirectoryからも呼べる。生成物に日時や絶対pathを埋め込まず、同じsourceと生成先なら同じ内容になる。

## 抽出範囲と限界

[`feature_inventory.rs`](../examples/support/feature_inventory.rs) の `default_scope` が対象を明示する。

- `emela-lir/src/ir.rs` と `repr.rs` のtop-level public enum・struct・type aliasを収集する。対象内に追加された型・variant・fieldは自動で増える。
- Coreから共有している `Builtin` / `FiberOp` と、`Type` / `FnType` / enum・effect・external等の指定した定義を追加する。Core全体や依存型を再帰的に収集する仕組みではない。
- publicな型の全field（非public fieldを含む）、named/tuple variantのfield、型、属性、原文doc comment、定義位置を抽出する。`syn` の構文木を使い、コメントや文字列中の見かけの宣言を拾わない。
- `cfg` は評価せず全branchを読む。親の属性は親レコードに置く。macroは展開しない。全件抽出の対象fileにmoduleやmacroが導入された場合、選択した型が消えた場合、同名定義が競合する場合などは、黙って省略せずエラーにする。
- 関数本体、verifierやloweringの規則、対象file外の新しい定義、実際にproducerが出力する範囲、型・命令の全組み合わせは対象外。新しいfileやCore側の共有型を追加したら `default_scope` も更新する。

Flow/Heapはデータ型を共有するため、この一覧にはHeap専用のvariantやFlowまでにlowerされるbuiltinも含む。**定義の存在・Flowへの入力可否・実装対応・実行試験は別々に記録する。** source commentに書かれた一般の意味論を、組み込みtargetの実装保証へ読み替えない。

## 定義側の共通レコード（実装済み）

JSONのtop-levelは `schema_version: 1`、`inventory_kind: "definitions"`、`scope`、`sources`、`items`。`sources` は対象fileのpathとSHA-256を持つ。作業treeの実際の内容が対象であり、Gitのcommitだけで同一性を判断しない。

| field | 意味 |
| --- | --- |
| `id` | crate namespace付きの安定ID。並び順や行番号をキーにしない |
| `kind` | `enum` / `struct` / `type_alias` / `variant` / `field` |
| `parent` | fieldやvariantの親ID。型宣言・aliasはnull |
| `declaration` | doc属性を除いたRust宣言のtoken表現。payloadや型・非doc属性を保持する |
| `attributes` | 当該宣言の非doc属性。親の条件を確認するときは `parent` をたどる |
| `docs` | 元のdoc comment。説明がない場合は空文字列。意味を補完・翻訳しない |
| `source` | リポジトリ相対の `path` と、1始まりの `line` / `end_line` |
| `definition_sha256` | 宣言と属性のfingerprint。コメントや位置だけの変更では変わらない。動作やverifier規則のhashではない |

IDは次のように作る。

| 対象 | IDの例 |
| --- | --- |
| enumのvariant | `emela_lir::Value::Call` |
| variantのnamed field | `emela_lir::Value::Call.suspends` |
| tuple field | `emela_lir::Term::Return.0` |
| structのfield | `emela_lir::Function.suspends` |
| Coreの共有型のvariant | `emela_core::Type::Fn` |

型宣言のfingerprintには子の形と非doc属性も含む。子レコードのfingerprintだけで親のgeneric条件や属性の変化まで検知できるとは扱わない。将来の照合では、対応する親・子の変更も一緒に再確認する。IDのrenameは削除＋追加として現れる。

Markdownは同じ `items` を型ごとの表に整形したもの。大きな親宣言は見出し相当へ短縮し、詳細はvariant・field行とJSONに残す。生成物を手で編集せず、補足説明や判定は別ファイルに保存する。

## 実装側のレコード案（次の段階）

実装側も同じIDを使い、backendごとに別のJSONを生成する。次の例は形式を示す仮の記録であり、現在の実装に対する判定ではない。

```json
{
  "schema_version": 1,
  "inventory_kind": "implementation_claims",
  "backend": "emela-codegen-emb",
  "items": [
    {
      "id": "emela_lir::Value::Call",
      "definition_sha256": "<確認した定義のhash>",
      "flow_input": "conditional",
      "handling": "partial",
      "conditions": ["<確認した条件や制限>"],
      "evidence": [
        { "kind": "implementation", "path": "<実装path>", "symbol": "<関数名>" },
        { "kind": "test", "path": "<試験path>", "symbol": "<試験名>" }
      ],
      "verification": {
        "status": "not_run",
        "command": null,
        "source_revision": null
      }
    }
  ]
}
```

判定を一つの「対応済み」にまとめず、以下の軸に分ける。

| field | 値の案 | 判定するもの |
| --- | --- | --- |
| `flow_input` | `allowed` / `conditional` / `forbidden` / `unreviewed` | 正常なFlow入力として許されるか。verifier/loweringの根拠を付ける |
| `handling` | `generated` / `lowered` / `rejected` / `partial` / `unreviewed` | backendでの処理方針。条件外の扱いも `conditions` に残す |
| `verification.status` | `not_run` / `passed` / `failed` | 記録したcommandとrevisionにおける試験結果。試験名の存在だけでpassedにしない |

明示的な拒否は、Flowで禁止された要素には必要な処理になる。`match` のarmやsymbolを発見しただけでは、対応範囲も正しさも確定しない。自動抽出した実装候補には根拠を付け、範囲が確定するまでは `unreviewed` にする。

## 突き合わせたMarkdownの案

次の比較表もJSONから生成する。下記の行は状態の表示例で、実際の照合結果ではない。

| ID | Flow入力 | backend処理 | 定義との差分 | 実装根拠 | 試験結果 | 条件 |
| --- | --- | --- | --- | --- | --- | --- |
| `emela_lir::Value::Call` | conditional | partial | unchanged | symbolへのlink | not_run | 条件を記載 |
| `emela_lir::Value::NewOperation` | unreviewed | unreviewed | missing_claim | — | not_run | 実装側の記録なし |
| `emela_lir::Value::OldOperation` | unreviewed | unreviewed | orphan_claim | — | not_run | 定義側にIDなし |

比較の基本はID集合の差分と宣言hashでよい。

- 定義にだけIDがある：`missing_claim`。未対応とは即断せず、未確認として扱う。
- 実装側にだけIDがある：`orphan_claim`。古いID・誤記・削除された定義を確認する。
- IDが同じでhashが違う：`definition_changed`。既存の対応判定を再確認する。
- IDとhashが同じ：`unchanged`。実装や意味論の正しさを保証する印ではない。

型・suspend/failsの組み合わせ、cleanupをまたぐ経路などは、variantの行だけでは区別できない。必要なケースは今後 `case_id` と関連する定義ID群を持つ別の記録として追加する。定義側のscriptは宣言の一覧を生成し、別の[実装側script](implementation-inventory.md)が構文上の候補と比較表を生成する。可否・対応範囲のレビューと試験結果の記録は次の段階となる。

# Flow IRに対する実装箇所の抽出と比較（試作）

Flow IRの定義IDに対し、codegen-embの指定した処理箇所から構文上の根拠を集める。定義側と同じMarkdown＋JSONで生成し、対応記録が欠けている項目を探すための試作である。コード生成、事前変換、入力拒否を別の役割として記録する。

## 実行方法

リポジトリrootから実行する。生成に必要なコード・設定・文書はすべて `emela-codegen-emb` 配下にある。

```sh
bash crates/emela-codegen-emb/tools/flow-implementation-inventory.sh
bash crates/emela-codegen-emb/tools/flow-implementation-inventory.sh --check
bash crates/emela-codegen-emb/tools/flow-implementation-inventory.sh --check --require-evidence
```

- [実装箇所の一覧](generated/flow-implementation-inventory.md) / [JSON](generated/flow-implementation-inventory.json)
- [Flow IR定義との比較表](generated/flow-coverage.md) / [JSON](generated/flow-coverage.json)

通常の実行は未確認項目があっても文書を生成する。`--check` は生成物が現行sourceと一致するかを検査し、差分・欠落で終了code 1となる。`--require-evidence` は対象variantの必須経路に構文上の根拠がない場合、定義にないIDを抽出した場合、または全件抽出対象のLIR fileに監査設定のないenumが増えた場合に終了code 1となる。**このflagの成功も実装完了・動作保証を意味しない。** 初期設定には抽出できない項目があり、このflagは失敗する。

`--out-dir PATH` で生成先、`--config PATH` で設定を変更できる。相対pathは実行時のdirectoryが基準。設定はこのリポジトリ内のfileを指定する。定義側の一覧も同時に再生成・照合するので、比較の両側は同じsource内容に基づく。

## 何を機械的に抽出するか

[`flow-implementation-routes.json`](../tools/flow-implementation-routes.json) が対象enum・必要な実行経路・処理関数・分岐を指定する。抽出処理は [`implementation_inventory.rs`](../examples/support/implementation_inventory.rs) にある。

設定例:

```json
{
  "id": "sync-value",
  "path": "crates/emela-codegen-emb/src/emit.rs",
  "symbol": "emit_statement_inner",
  "owner": "emela_lir::Value",
  "selector": "match",
  "expression": "value",
  "contexts": ["sync"],
  "role": "emit",
  "note": "コード生成を行う分岐の候補"
}
```

`symbol` はtop-level関数名、または `Graph::sequence` のような型名付きmethod名。`selector` は `match` / `if` / `let_else`。`expression` は照合する式をRust構文として指定する。`binding` を加えると `let expression = match op` のような束縛先を指定でき、同じ関数内のarity表や型検査表を除外できる。`top_level: true` は別のmatch内にあるmatchを除外する。matchが複数箇所に一致した場合や関数・分岐が見つからない場合はエラーにする。

抽出する情報は次のとおり。

| 情報 | 内容 |
| --- | --- |
| ID | 定義側と同じ `emela_lir::Value::Call` 等。新variantは定義側から自動で比較対象へ入る |
| 実行経路・役割 | 設定で宣言したsync / suspend / common / inputと、emit / lower / reject |
| パターンと条件 | 実際のASTにあるvariant名、or-pattern、named/tuple payload、match guard、if条件 |
| 呼び出し | branch内の通常関数・method呼出し。macroは展開しない |
| 位置・hash | source位置、source fileのhash、branch本体のtoken hash、現在観測した定義hash |
| 直接のerror戻り値 | branchの最後が直接 `Err(...)` / `return Err(...)` なら生成候補として数えない |

通常の参照、文字列・コメント、引数の型、関数名だけの存在は対応記録として数えない。`_` や `other` だけの分岐は別表へ出し、残りのvariantに自動展開しない。`if op != X` の後に続く処理も自動でXの対応とみなさない。これは欠落を隠さないための保守的な抽出である。

`required_calls` は指定した呼出しがbranch内にあることを要求する。共通LIR verifierによるHeap専用要素の拒否では、`stage_at_least` の呼出しを要求している。`only_variants` は、そのrouteで候補とするvariantを限定する。

## 比較する単位と表示

初期設定は16種類のenumのvariantを監査する。FlowとHeapで共有する定義も含め、入力禁止を別に記録する。型宣言やfield、追加のCore型は対象外の一覧へ残す。415レコードを415機能として採点しない。

| 結果 | 意味 |
| --- | --- |
| `candidate_found` | 指定した実行経路・役割の明示的な分岐を発見した。対応や完全性は未review |
| `rejection_evidence` | 設定で入力禁止としたIDに、指定した拒否routeの根拠がある |
| `missing_evidence` | 必須経路で根拠を取得できない。未実装、設定不足、抽出方式の不足を区別する必要がある |
| orphan evidence | 実装にあるIDが定義側にない。削除されたvariant、設定違いなどを確認する |

`exclusions` は、入力禁止とするID・拒否route・理由を明示する。この分類は設定上の宣言であり、ASTからFlowの適用範囲を推論した結果ではない。拒否routeが見つからなければ `missing_evidence` のままとなる。Flowへの入力可否が未判定の通常variantは `flow_input: "unreviewed"` とする。

`observed_definition_sha256` は生成時に観測したhashで、レビュー済みhashではない。すべてのレコードは `review_status: "unreviewed"`。試作ではhashの自動追随を「変更後も確認済み」として扱わない。

## 現在の限界と次の確認

役割・実行経路は設定上の宣言で、`compile_flow` からの到達性を静的に証明してはいない。branchに内部条件や別のerror経路があっても、網羅性や正しさは決まらない。ifの否定条件の後に続く処理、別関数への委譲、変換先までのつながりなどを自動追跡しないため、実装が存在しても `missing_evidence` となる場合がある。

比較表の欠落から対象箇所を調べ、設定を精密化するか、実装に近い注記・検証可能なrouteを追加するのが次の手順となる。未確認項目を埋めるためだけに解析・変数置換の関数をrouteへ登録しない。

`cfg` は評価しない。import alias・再export・generic dispatchの型解決、macro展開、型と演算の全組み合わせ、field単位の意味論、試験結果は今回の監査に含めない。将来の対応判定には、[照合フォーマット案](inventory-format.md)の条件・レビュー済みhash・試験結果を別に追加する。

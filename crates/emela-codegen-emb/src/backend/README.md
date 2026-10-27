# Flow IR backendの構成

公開APIの `compile_flow` は、共通コンパイルとボード接続を順に実行する。

```text
compile_flow
  → compiler::compile_flow
      → 正規化・検証・closure／handler／const等の準備
      → CompiledModule
  → board::link
      → compiler::emit(compiled, target, options)
          → 型配置・所有権・buffer・runtimeの計画
          → 機能別のC生成とボード用adapter・入口の生成
```

`CompiledModule` は接続先に依存しない中間結果であり、最終的なC文字列ではない。IRとclosure・constのmetadataは非公開で、接続側には可変参照を渡さない。最終生成はそのcopyを使うため、同じ中間結果を別の接続先や別の容量設定で再利用できる。接続処理が共通のloweringを再実行することはない。

| 場所 | 責任 |
| --- | --- |
| [../input/](../input) | Core→Flowの準備とFlow入力検証。公開入口はcrate rootから再公開 |
| [../contract/](../contract) | 設定・診断・生成物のデータ型。compilerやplatformの実装には依存しない |
| [compiler/mod.rs](compiler/mod.rs) | 共通コンパイル、中間結果、既存機能に対する明示的な内部facade |
| [compiler/c/](compiler/c) | C identifier・名前・literalの規則 |
| [compiler/diagnostics.rs](compiler/diagnostics.rs) | 入力拒否の診断と構文要素の表示名 |
| [compiler/validation.rs](compiler/validation.rs) | 関数値と外部signatureの適合検査 |
| [compiler/emit/](compiler/emit) | 最終配置・runtime計画・C出力の組立て。header・functions・control・valuesへ出力処理を分割 |
| [compiler/features/](compiler/features) | Value・Stmt・Term・Builtin・FiberOp等の機能別処理 |
| [compiler/foundation/](compiler/foundation) | 型配置、到達可能性、所有権、String buffer、heap・runtime計画 |
| [compiler/target.rs](compiler/target.rs) | 外部binding、header、検査、adapter、起動処理の接続契約 |
| [board/](board) | 既存の標準周辺機器adapter、ISR制約、ボード起動処理 |
| [cnative/](cnative) | 同じ中間結果をホストCで実行する内部試験用の接続 |
| [tests/separation.rs](tests/separation.rs) | 中間結果の再利用、接続先の独立性、生成Cの実行試験 |

共通コンパイラーは具体的なボード接続をimportしない。C上の `emela_board_*` hook名や生成file名は既存ABIとして維持し、その実装と標準周辺機器bindingの解釈を接続側へ委ねる。`cnative` はlibrary testでのみ有効で、CLIのtargetを追加するものではない。

設定のfieldはcontractの外から直接操作せず、builderと読み取りaccessorを通じて使う。公開型名・既定値・生成CのABIはこの整理では変更しない。`compiler/mod.rs`の内部facadeは既存featureのために残し、新しく分けた補助moduleは必要な依存を明示してimportする。

現在は段階分離の最初の内部整理までであり、`emit`内の解析・IR書換えや上流IR型への依存は残っている。同じ最終`Output`をboard/nativeに接続する変更、独立したlower/plan契約、platformの計算と実行の分離は後続の移行対象になる。`CompiledModule`を再利用できることと、同じ生成済みC一式を再利用できることは別の条件として扱う。

## 機能と共有計画

最終`Output`には[生成Cとビルドの契約](../../docs/artifact-contract.md)を表す`emela_artifact.json`を含める。後段のBSPはこの入力一覧とC headerを読み、Flow IRや`CompiledModule`に依存しない。C生成に使う接続先`Target`と、生成後のtoolchain実行の境界は別である。

各機能はC生成に加え、必要に応じて `string_behavior`、`string_bound`、`result_ownership`、`needs_heap`、`require_runtime` を提供する。例えばString連結では、定数・入力参照・動的bufferという同じ分類を生成・長さ・所有権の判断に使う。

共有のString buffer計画は、その情報から生存期間・slotの再利用・caller出力bufferの伝播を求める。heap計画は型と操作の要求を集約し、runtime計画はhelperの依存を解決して必要なものを一度ずつ生成する。共有計画は個々のBuiltinやFiberOpを直接分岐しない。

Value・Stmt・Term等のdispatcherはvariantを網羅する。新しいvariantの追加時に生成・解析・runtime要求の判断漏れをコンパイル時に検出し、子のIRの走査には共通処理を使う。

## 実装とテストの対応

[実装・テストの自動対応表](../../docs/generated/flow-backend-inventory.md)は、IRの定義IDから機能別の専用入口・追加の処理段階・関連テストへ対応付ける。variantごとの担当と関連テストを明示し、fieldは親の担当との関係を記録する。fieldや共有宣言を独立した実装数・テスト数として数えない。

通常の`cargo test`とCIでは、定義・実装・テストの欠落、生成物の古さ、sync/suspend等の必須経路の根拠を検査する。CIでは実際のテストバイナリーへの登録とignore状態も確認する。設定と生成・検査のコマンドは[crateのREADME](../../README.md#実装とテストの自動検査)を参照する。

## 検証

```sh
cargo test -p emela-codegen-emb --locked
cargo run -p emela-codegen-emb --example check_flow_coverage --locked
```

ホスト接続の実行試験は生成された全C translation unitを `-std=c11 -Wall -Werror` と `-O0` / `-O2` でコンパイルする。closure・managed値・suspend・起動時の返り値の解放・String出力bufferを確認し、ボード接続後にも元の中間結果を再利用できることを検査する。

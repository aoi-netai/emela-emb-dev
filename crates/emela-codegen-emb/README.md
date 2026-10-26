# emela-codegen-emb

EmelaのFlow IRから、組み込み向けのC11コードを生成するコンパイラーです。固定容量のメモリと、到達する機能に必要なランタイムを生成します。

## APIと実装

`compile_flow(flow, options)`が主入口です。`compile(core, options)`と`flow_from_core(core)`はCoreの単相化・Flow変換を行う便宜APIです。`verify_flow`で入力を検証でき、`derived_binding`で閉じた型の構造比較・表示をFlowへ追加できます。

[backendのREADME](src/backend/README.md)に構造、Flowの入力条件、容量設定、所有権、失敗・defect、外部C・ISRの契約とRustの最小例をまとめています。個々の定義・実装・関連テストは生成文書を参照します。

| 一覧 | 内容 |
| --- | --- |
| [Flow IR定義](docs/generated/flow-ir-inventory.md) | Rust宣言から抽出した型・variant・field。Heap専用の定義も含みます。 |
| [本体実装・テストの対応表](docs/generated/flow-backend-inventory.md) | 定義IDから担当入口・処理段階・関連テストへの対応。 |
| [構文上の実装候補](docs/generated/flow-implementation-inventory.md) | 指定した処理関数から抽出した明示的な分岐。 |
| [実行経路との比較](docs/generated/flow-coverage.md) | sync・suspend・入力拒否の必須経路に根拠があるかの照合。 |

## 組み込みインターフェース

[機能一覧](docs/embedded-interfaces/README.md)からGPIO・UART・I²C・SPI・Interrupt・Clock・Consoleの操作と利用例を確認できます。C bridge・共通status・転送bufferの所有権は[共通契約](docs/embedded-interfaces/common.md)へ、再現手順と確認範囲は各機能の文書へまとめています。

project作成、`c-embedded` target、board選択、TOML設定、build・flashは[CLIのREADME](../emela-cli/README.md)を参照してください。

## 同梱BSP

| ボードpackage | 内容 |
| --- | --- |
| [STM32F4](boards/stm32/stm32f4/README.md) | MCU/profile・配線・startup・standalone build/flash・RenodeでのFlash reset観測。 |
| [ESP32-C3](boards/esp32c3/README.md) | 予約領域・pin・startup・standalone build/flash・QEMUでのROM/Flash起動観測。 |

`src/boards.rs`はpackage資産をコンパイル時に同梱します。CLIやRust APIの利用時にsource checkoutは不要です。外部toolchainは利用者が用意します。BSPの対応範囲は各READMEと機能の文書に従います。

## 対応表の生成と検査

リポジトリrootで実行します。各scriptはMarkdownとJSONを同じ抽出結果から生成し、`--check`で現行sourceとの差分・欠落を検出します。生成物を手で編集しません。

```sh
bash crates/emela-codegen-emb/tools/flow-ir-inventory.sh
bash crates/emela-codegen-emb/tools/backend-inventory.sh
bash crates/emela-codegen-emb/tools/flow-implementation-inventory.sh

bash crates/emela-codegen-emb/tools/flow-ir-inventory.sh --check
bash crates/emela-codegen-emb/tools/backend-inventory.sh --check
bash crates/emela-codegen-emb/tools/flow-implementation-inventory.sh --check --require-evidence
python3 crates/emela-codegen-emb/tools/test-inventory.py --check-built
cargo run -p emela-codegen-emb --example check_flow_coverage --locked
```

定義一覧の抽出範囲は[`default_scope`](examples/support/feature_inventory.rs)で指定します。安定IDはcrate namespace付きの型・variant名で、fieldは親IDとfield名・tuple indexを使います。JSONは宣言・属性・元のdoc comment・source位置・hashを保持し、日時や絶対pathに依存しません。宣言hashは意味論や動作のhashではありません。

担当は[`inventory/owners.json`](inventory/owners.json)と[`containers.json`](inventory/containers.json)、関連テストは[`tests.json`](inventory/tests.json)で宣言します。variant追加時には担当とテストを登録します。テストはtargetと完全名で識別し、欠落・定義外ID・重複参照・全件ignoreを拒否します。`--check-built`は実際のテストバイナリーの登録とignore状態も照合します。`check_flow_coverage`は手書きの文書を読まず、定義JSONとenum・`Builtin::ALL`の整合性を確認します。

構文上の実装候補は[`flow-implementation-routes.json`](tools/flow-implementation-routes.json)でenum・処理関数・必要な実行経路・役割・ASTの分岐を指定します。`--require-evidence`は必須経路の根拠欠落、定義外ID、監査対象enumの追加漏れを拒否します。生成・拒否・変換を区別し、wildcardを未列挙variantの実装根拠へ自動展開しません。`--out-dir`で生成先、実装候補のscriptは`--config`で設定も変更できます。

宣言の存在、Flow入力の可否、担当入口・テストの存在、実行試験の成功は別の確認です。AST抽出はmacro展開や型解決をせず、`cfg`も評価しません。担当や分岐が見つかることは、意味論・条件の網羅性を保証しません。生成時のhash追随をレビュー済みとは扱わず、review状態は自動更新しません。

## hostでの確認

```sh
cargo test -p emela-codegen-emb --locked
cargo fmt -p emela-codegen-emb --check
cargo clippy -p emela-codegen-emb --all-targets --locked -- -D warnings
```

生成Cの実行試験にはC11/C++17 compilerとPython 3が必要です。host試験は値・制御・所有権・容量・ABIを確認し、MCUの通信波形・割り込み遅延・最大stackや実機の書き込みを検証するものではありません。

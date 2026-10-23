# emela-codegen-emb

EmelaのFlow IRから、組み込み向けのCコードを生成するコンパイラーです。Heap IRへの変換を通さず、固定容量のメモリと機能ごとの小さなランタイムを生成します。

[実装範囲と実装状況](docs/implementation.md)に、入力契約、各variantへの対応、メモリ方針と既存の検証をまとめています。

## 主な入口

- `compile_flow(flow, options)`：検証済みのFlow段階を入力とする主入口です。入力を検証し、Cソースとヘッダーを`Output`として返します。
- `compile(core, options)`／`flow_from_core(core)`：通常のCore IRを単相化してFlowへ変換する便宜入口です。成長する再帰型の入力は診断し、Flowを直接渡す経路へ分けています。
- `derived_binding(core, operation, ty, name)`：Flow生成側が、具体化された型の有限な構造比較・表示を登録するための関数を作ります。`DerivedOperation::Eq`または`Show`を指定し、返された関数をFlowモジュールへ追加して呼び出します。

`Options`でString容量、固定heap、fiberやtaskのpool容量などを指定します。必要なランタイムとボード接続用のCインターフェースは生成コードに含まれます。

## このcrateの範囲

コンパイラー本体、生成ランタイム、検証用のテストとexampleに加え、`Std.Embedded` の共通effectと共有handlerをC ABIへ接続するbridgeを提供します。[組み込みインターフェース](docs/embedded-interfaces.md)と[標準周辺機器bridge](docs/embedded-peripherals.md)に利用方法と所有権・容量の契約をまとめています。[ボードpackageと確認手順](docs/boards.md)では、STM32F4・ESP32-C3のBSP、Rust exampleからのstandalone build/flash、Renode/QEMUによるFlash起動の確認を提供します。

GPIO・UART・I²C・SPIのAPIとBSP実装に向けた操作契約は、[周辺機器インターフェース仕様](docs/interface/README.md)にまとめています。I²Cの未シフト7bit固定の方針と、その他の操作契約の案を区別して記載しています。

GPIOは初期値付き設定・専用エラー・静的ISR bindingを両BSPへ実装しています。[GPIO仕様](docs/interface/gpio-interface.md)と[CLI/Renode試験・再実行手順](docs/gpio-validation.md)を参照してください。

[CLIでの開発手順](docs/cli-embedded.md)では、projectの作成、`c-embedded` target・board選択、TOML設定、buildと入力・成果物のhash照合を伴うflashを説明します。CLIとdriverがこのcrateのAPIと同梱BSPを利用します。外部toolchainの導入と対象BSPが提供しないdriverの追加は利用側で行います。

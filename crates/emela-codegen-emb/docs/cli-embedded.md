# CLIで組み込みCを生成する

`emela build --target c-embedded`は、Emelaのソースを読み込み・型検査し、多相Coreを組み込みbackendへ渡す。backend内で単相化し、Flow IRを経由して組み込みCへ変換する。Heap IRへの変換は行わない。公開関数をボードのCコードから呼び出すライブラリとして生成するため、Emelaに`main`を用意する必要はない。

## 基本的な使い方

```sh
emela build controller.emel --target c-embedded \
  --embedded-config embedded.toml --out-dir generated
```

workspace上の開発用CLIを使う場合は次のように実行する。

```sh
cargo run --locked -q -p emela-cli -- build \
  crates/emela-codegen-emb/examples/temperature-controller/controller.emel \
  --target c-embedded \
  --embedded-config crates/emela-codegen-emb/examples/temperature-controller/embedded.toml \
  --out-dir /tmp/emela-temperature-generated
```

CLIが担当するのはCとheaderの生成までである。ARM向けcompile・link、ボードのstartupと周辺機器の初期化、Renodeでの実行はボード用のbuild手順で行う。[温度調節器example](../examples/temperature-controller/README.md)は、これらを`renode/run.sh`にまとめている。

単一ソースの代わりにプロジェクトdirectoryも指定できる。`Pome.toml`の`[build].target = "c-embedded"`で既定targetを選べる。`--embedded-config`の相対pathは実行時の作業directoryが基準で、自動的な設定ファイルの探索は行わない。省略時は下表の既定値を使用する。`--out-dir`の省略時は入力の基準directory配下の`target/emela/c-embedded`へ生成する。

`c-embedded`の`run`と`test`はホスト上の実行コマンドとして扱わない。`build`で生成したCをボード側の実行手順へ渡す。

## 設定ファイル

固定容量、外部C関数のadapter、ボード用の公開名をTOMLで指定する。温度調節器では次の設定を使う。

```toml
heap_bytes = 2048
string_capacity = 96
peripheral_buffer_capacity = 96
fiber_capacity = 4
frame_capacity = 8
frame_bytes = 384
task_capacity = 1
event_capacity = 1
deferred_capacity = 1

[exports]
temperature_controller_run_start = "run"
```

容量の単位はbyte、`*_capacity`はStringを除いて個数である。省略した項目はbackendの既定値になる。

| 項目 | 意味 | 既定値 |
| --- | --- | ---: |
| `heap_bytes` | Emela固定heapの予約容量。0はheapを無効にする | 0 |
| `string_capacity` | 動的に生成するString bufferの容量上限。1〜512 | 256 |
| `peripheral_buffer_capacity` | 標準UART・I²C・SPI bridgeの転送buffer要素数。1以上 | 256 |
| `const_diagnostic_bytes` | 実行時constのdefectメッセージを保持する容量 | 1024 |
| `fiber_capacity` | 未解放の完了済みhandleを含むfiber数 | 8 |
| `frame_capacity` | 同時に生存できるsuspend frame数 | 32 |
| `frame_bytes` | suspend frame 1個の容量 | 256 |
| `task_capacity` | 完了待ちの外部task数 | 8 |
| `event_capacity` | 待ち行列に保持する完了通知数 | 8 |
| `deferred_capacity` | 同時に生存できるDeferred数 | 8 |

容量設定はアプリが必要とする同時使用量に合わせる。frameの実サイズはCの型配置とターゲットに依存し、生成Cのcompile時にも検査する。設定に不正な数値や未知の項目があればCLIが診断する。固定heap・String・scheduler poolの実行中の容量不足は既存のboard defect契約に従う。標準周辺機器bridgeの転送buffer上限超過は`PeripheralError(-2)`で呼び出し元へ返す。

### 標準の組み込みAPI

`Std.Embedded.Board`の`BoardGpio`・`BoardUart`・`BoardI2c`・`BoardSpi`・`BoardClock`・`BoardConsole`は、共通のC ABIへ接続する共有handlerである。これらの標準APIについては、アプリのTOMLに`task_adapters`や`external_adapters`を書く必要はない。ボードのC実装が生成headerの関数を提供する。利用方法と各層の役割は[組み込み標準インターフェース](embedded-interfaces.md)を参照する。

温度調節器は標準GPIO・UART設定・Clock・Consoleを使い、独自の温度センサだけを`board.emel`の`@external`で宣言する。設定ファイルには固定容量と公開名だけを置く。

### 独自の外部関数adapter

ソースの`@external(c, "symbol")`はそのまま使用する。組み込みbackendを選ぶために`c-embedded`へ書き換える必要はない。

到達する外部関数にC用bindingがない場合は、組み込みbackendで診断する。従来のhost向けC生成とは、診断codeやソース位置の表示が異なる場合がある。

- `task_adapters`は、suspend外部関数のsymbolから、生成された`EmelaTask` ABIを実装するボード関数名への対応である。独自デバイスの待機等を、ISRからの完了通知で再開する場合に使う。標準の`BoardClock`の対応はbackendが登録する。
- `external_adapters`は、同期外部関数でString・tupleなどの生成C表現を渡す場合の明示的な契約である。`symbol`と`result`を指定する。引数は呼び出し中だけ借用される。

adapter対応表は複数のアプリで共用でき、このソースで使わないsymbolのmappingも受け入れる。未知のTOML項目と、存在しない公開関数への`exports`指定は診断する。

| `result` | 戻り値の契約 |
| --- | --- |
| `borrowed` | 生成wrapperが所有する値を取得する間、ボードが戻り値を有効に保つ |
| `owned` | この生成moduleの固定heapに属する所有値をEmelaへ渡す |
| `string-buffer` | 追加の呼び出し側bufferへ書き込み、そこを指すownerなしStringを返す。String戻り値用 |

借用の戻り値にC関数内のローカル配列へのpointerを返すことはできない。`string-buffer`では、ボードが容量を確認してから書き込む。戻り値がUnitでも、独自の外部関数でString引数を生成CのABIで受け取る場合には明示的なadapterが必要になる。標準handlerではその契約を共有bridgeが提供する。

ISR entryを生成するアプリは、`isr_entries = ["function_name"]`と`isr_safe_externals = ["c_symbol"]`も指定できる。温度調節器はこれらを使わず、ボードのISRが完了通知だけを送り、schedulerとEmelaの処理をmain側で実行する。

### ボード用の公開名

`[exports]`の左側はボードに公開するC alias、右側はEmelaの公開関数名である。

```toml
[exports]
temperature_controller_run_start = "run"
```

生成headerが、構造化された公開関数の情報から実際のC entryへ対応付ける。suspend関数では開始関数のaliasになる。ボード側で生成headerのテキストを解析したり、生成関数名の末尾の番号を固定したりする必要はない。

存在しない関数名や曖昧な関数名、C識別子として使えないalias、生成runtimeや外部symbolと衝突するaliasは診断する。生成headerで使う既存macroとの衝突もCのcompile時に検出する。

## 生成物とボード側の責務

| ファイル | 内容 |
| --- | --- |
| `emela_embedded.c` | アプリケーションと必要なruntimeのC実装 |
| `emela_embedded.h` | 値のC表現、scheduler・task API、公開関数のentry |
| `emela_embedded_exports.h` | 上記headerのincludeと設定した公開alias |
| `emela_board.h` | board defect、event lockなどのボードABI |

標準board handlerを使うと、使用する標準handlerに対応する`emela_gpio.h`・`emela_uart.h`等の共通ABI headerも生成する。ボード側は`emela_embedded_exports.h`、これらの共通header、必要な`emela_board.h`をincludeする。外部関数・task adapter・board hookを実装し、公開関数を呼び出す。suspendするアプリではschedulerをmain側で駆動する。

温度調節器のボードは`temperature_controller_run_start(samples, &result)`で開始し、`emela_scheduler_run`で処理を進める。結果の保存先はボード側が用意し、完了まで有効に保つ。完了後にfiber handleを解放する。ボード側の実装は[renode/board.c](../examples/temperature-controller/renode/board.c)を参照する。

CLIの生成成功とボードのcompile・link成功、Renodeで観測した挙動・資源使用量はそれぞれ別に確認する。温度調節器の実行手順と測定値はexampleのREADMEに記録する。

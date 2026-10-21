# CLIで組み込みC・firmwareを生成する

`emela build --target c-embedded`は、Emelaのソースを読み込み・型検査し、多相Coreを組み込みbackendへ渡す。backend内で単相化し、Flow IRを経由して組み込みCへ変換する。Heap IRへの変換は行わない。ボードを選ばない場合は公開関数をCコードから呼び出すライブラリとして生成し、Emelaに`main`を用意する必要はない。ボードを選んだ場合は、後述の公開entryをBSPへ接続してfirmwareを生成する。

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

ボードを指定しない場合は、従来どおりCとheaderを生成する。`--board`または`Pome.toml`の`[embedded].board`を指定すると、CLIに同梱したボードパッケージを展開し、ボードのtoolchainによるcompile・linkとFlash image生成まで行う。Renodeによる動作確認は[温度調節器example](../examples/temperature-controller/README.md)の手順で行う。

単一ソースの代わりにプロジェクトdirectoryも指定できる。`Pome.toml`の`[build].target = "c-embedded"`で既定targetを選べる。`--embedded-config`の相対pathは実行時の作業directoryが基準で、自動的な設定ファイルの探索は行わない。省略時は下表の既定値を使用する。`--out-dir`の省略時は入力の基準directory配下の`target/emela/c-embedded`へ生成する。

`c-embedded`の`run`と`test`はホスト上の実行コマンドとして扱わない。`build`で生成したCをボード側の実行手順へ渡す。

## ボードを選んでビルド・書き込みする

次のようなプロジェクトを作成する。Emelaのアプリだけで動かす場合、CやRustのファイルを用意する必要はない。

```text
thermostat/
  Pome.toml
  embedded.toml
  src/main.emel
```

```toml
# Pome.toml
[build]
target = "c-embedded"

[embedded]
board = "stm32f446"
config = "embedded.toml"
entry = "main"
optimization = "O2"
```

`embedded.toml`を次の内容で作成する。独自のCファイルや`[exports]`の指定は不要である。

```toml
heap_bytes = 2048
string_capacity = 96
fiber_capacity = 4
frame_capacity = 8
frame_bytes = 384
task_capacity = 1
event_capacity = 1
deferred_capacity = 1
```

`src/main.emel`のentryは、引数を持たず、`Int`を返し、未処理の失敗やeffectを持たない公開関数にする。`use Async`を付けると、ボードのschedulerへ接続する開始ABIになる。

```emela
pub fn main() -> Int use Async { 0 }
```

Pythonと[STM32F4共通packageのtoolchain・OpenOCD](../boards/stm32/stm32f4/README.md)を用意して、次を実行する。既存の`stm32f446`指定は引き続き利用できる。

```sh
emela check thermostat
emela build thermostat
emela flash thermostat --dry-run
emela flash thermostat
```

現在同梱するボードIDは次のとおり。MCU定義と基板の配線profileを分け、STM32F4はruntime・startup・build処理を共有する。

| 対象 | `board`／`--board`の値 |
| --- | --- |
| STM32F4のMCUと既定Console配線 | `stm32f407vg`、`stm32f412zg`、`stm32f429zi`、`stm32f446re` |
| STM32F4の基板profile | `stm32f4-discovery`、`nucleo-f446re` |
| 従来のSTM32F446互換ID | `stm32f446` |
| ESP32-C3 | `esp32c3` |

F4のMCU別のメモリ容量、Console USART・pin、GPIOの論理IDは[共通package](../boards/stm32/stm32f4/README.md)、Renodeで確認したplatformの一覧は[F4対応記録](stm32f4-boards.md)を参照する。未登録のF4型番や基板への自動対応は行わない。互換IDはmanifestや出力directoryでも`stm32f446`を維持する。

ESP32-C3は`board = "esp32c3"`、または`--board esp32c3`で選ぶ。書き込みにはserial portを指定する。

```sh
emela build thermostat --board esp32c3
emela flash thermostat --board esp32c3 --port /dev/ttyUSB0 --dry-run
emela flash thermostat --board esp32c3 --port /dev/ttyUSB0
```

`--board`がTOMLより優先する。ボード指定時にtargetを省略すると`c-embedded`となり、明示した別targetとは併用できない。既定の出力先は`target/emela/c-embedded/<board>/`で、`generated/`にCとheader、`bsp/`にボードパッケージ、`firmware/`にELF・BIN・map・資源情報を置く。`--out-dir`はこの3directoryの親を指定する。

ボードパッケージはコンパイラに同梱されるため、インストール後のCLIから開発時のリポジトリを参照する必要はない。Pythonの実行ファイルは`EMELA_PYTHON`で指定できる。ボードのtoolchain・書き込みtool・起動方式の条件は、[STM32F4共通package](../boards/stm32/stm32f4/README.md)と[ESP32-C3](../boards/esp32c3/README.md)を参照する。

### 配線・独自デバイスの選択

`config`・`sources`・`modules`のpathはプロジェクトdirectoryが基準である。アプリ独自のセンサやC関数が必要な場合にだけ`[embedded].sources`へ追加する。Emela moduleの選択は`[embedded.modules]`で行える。

```toml
[embedded]
board = "stm32f446"
config = "embedded.toml"
entry = "main"
sources = ["sensor.c"]
optimization = "O2"

[embedded.modules]
Controller = "../controller.emel"
Board = "../boards/{board}/board.emel"
```

`{board}`は選択されたボードIDへ置き換える。通常のmodule探索に対する読み取り専用のoverlayであり、利用者のソースやFlow IR、生成Cを書き換えない。`check`・`build`が同じ選択を使う。`check --board`による切り替えにも対応する。LSP・`doc`・host向け`run`は、このボードoverlayの対象に含めていない。

`entry`の既定値は`main`である。CLIが`board_main_start`という公開aliasを登録し、BSPがそのentryを呼び出す。独自のC-only生成を続ける場合はボードを省略でき、従来の`[exports]`と任意の公開関数signatureを利用できる。

Pome依存の`[pome].targets`には`"c-embedded"`を指定できる。省略した依存は全targetで利用できる。`"c"`はhost C向けの指定なので、組み込みC向けの依存指定とは区別する。基板を選んだ場合も依存targetの一致を検査する。`src/lib.emel`のみのプロジェクトは、上流CLIと同様にbuildで型検査だけを行い、firmwareを生成しない。

### 書き込み対象の確認

`flash`は直前の成功buildに対して、選択ボード・entry・設定・ソース・overlay・追加C・生成C・同梱BSP・firmwareのSHA-256を照合する。別ボードの成果物、変更されたimage、build後に変更したアプリは書き込み前に診断し、再buildを要求する。失敗した再buildは古い成功記録を無効にする。

Pomeを使う場合は、projectの`Pome.lock`、依存のidentity・directory・version・manifestと`src/`内のEmela・C・header等も照合する。依存moduleの追加・削除も再buildを要求する。依存の外部C実装は従来どおり`[embedded].sources`で明示し、実際に使うCソースとそのdirectory内のheaderを記録する。

`--dry-run`も同じ照合を行い、書き込みtoolへ渡すargvを表示する。デバイスへの接続や書き込みは行わない。実際の書き込みではSTM32はOpenOCD、ESP32-C3はesptoolを使用する。CLIはshellを介さず、ファイルpathやserial portを個別の引数として渡す。

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

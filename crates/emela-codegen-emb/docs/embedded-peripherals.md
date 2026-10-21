# 標準組み込みhandlerとCボードABI

利用者向けの構成・Emelaでの利用方法は[組み込みインターフェース](embedded-interfaces.md)、CLIの設定方法は[組み込み向けCLI](cli-embedded.md)を参照する。この文書は `Std.Embedded.Board` とC実装の境界を記録する。

## 接続と検査

`Std.Embedded.Board` は `BoardGpio`・`BoardUart`・`BoardI2c`・`BoardSpi`・`BoardClock`・`BoardConsole` を提供する。アプリは通常の `with` でhandlerを選択する。アプリごとの `@external`、`external_adapters`、`task_adapters` の登録は不要である。独自のセンサや既存Cライブラリには従来の汎用FFIも使える。

標準handlerの内部だけにprivateな外部関数がある。backendは次を照合し、そのC adapterを自動生成する。

- 完全修飾名 `Std.Embedded.Board.gpio_write` などと、専用C binding `__emela_peripheral_gpio_write` などが一致すること。
- 引数と戻り値が対応する標準effectのoperationの型と一致すること。
- GPIOは元の戻り値、失敗可能なoperationは `(Int, 元の戻り値)` を返し、外部関数自体は非failsであること。
- Clock・Consoleはそれぞれの固定署名に一致し、Clock.sleepだけがsuspendすること。

失敗可能なoperationのadapterは `(code, value)` を返す。共有handlerが通常のEmelaコードで `code != 0` を `PeripheralError(code)` に変換する。sourceの外部関数へ `fails` を追加したり、アプリのFlow IRを手動で置き換えたりしない。汎用FFIのadapter登録規約も変更しない。

## 生成ファイルと値型

到達する共有handlerに応じて `emela_gpio.h`・`emela_uart.h`・`emela_i2c.h`・`emela_spi.h`・`emela_clock.h`・`emela_console.h` を生成する。GPIO/UART/I²C/SPIには共通の `emela_peripheral.h` も生成する。

型名・enum定数・recordのfield名は標準ライブラリの宣言から生成する。

| Emela | C |
| --- | --- |
| `GpioPin(id: Int)` | `EmelaGpioPin`、fieldは `int32_t id` |
| `PinState` | `EmelaGpioPinState`、`EMELA_GPIO_PIN_STATE_LOW` / `HIGH` |
| `UartConfig` | `EmelaUartConfig`、`baud_rate`・`data_bits`などの名前付きfield |
| `UartId.Uart2` | `EMELA_UART_ID_UART2` |
| `Int` / `Bool` | `int32_t` |
| `Int64` | `int64_t` |

値のないenumは `int32_t`、recordは名前付きfieldのstructになる。これらはEmela内部の型IDやtag付きaggregateのlayoutに依存しない。ボードは生成headerをincludeして実装する。

## 同期operation

GPIOは元の値を返す。UART/I²C/SPIはstatusを返し、値がある場合は出力先へ書く。

```c
void emela_gpio_write(EmelaGpioPin pin, EmelaGpioPinState state);
int32_t emela_uart_configure(EmelaUartId id, EmelaUartConfig config);
int32_t emela_i2c_is_device_ready(
    EmelaI2cId id, int32_t address, int32_t trials,
    int64_t timeout_ms, int32_t *out);
```

status `0` は成功、それ以外は `PeripheralError(code)` になる。失敗時は出力値を利用しない。GPIOには失敗チャネルがないので、無効なボード設定の扱いはボード実装で定める。

operationは同期呼び出しであり、長いHAL待機を短くするschedulerへの変換は行わない。UART/I²C/SPIをISRで呼ばず、応答時間はtimeoutとボード実装で管理する。

## Listの転送

`List[Int]` はボード境界で `uint16_t` 配列と長さに変換する。UARTの9bitやSPIの16bitも表せるよう、転送単位はbyteではなく**16bitのword**である。

```c
int32_t emela_uart_transmit(
    EmelaUartId id, const uint16_t *data, uint16_t length,
    int64_t timeout_ms);
int32_t emela_uart_receive(
    EmelaUartId id, int32_t size, int64_t timeout_ms,
    uint16_t *out, uint16_t capacity, uint16_t *length);
```

CLI設定 `peripheral_buffer_capacity`、Rustの `Options::with_peripheral_buffer_capacity` はword数で、既定256、範囲1〜65,535である。1方向あたり最大 `2 × capacity` byteのCスタック領域を使う。SPI.transferは入力と出力の2方向分を使う。実際の最大stackはほかのローカル変数や呼び出し先を含むため、生成Cの `.su` や実行時の観測と合わせて評価する。

入力を黙って切り詰めない。次の2つはbridgeが予約するcodeであり、ボード独自のエラーcodeには使わない。

| 定数 | code | 条件 |
| --- | --- | --- |
| `EMELA_PERIPHERAL_INVALID_ARGUMENT` | -1 | 要素が0〜65,535の範囲外、受信sizeが負 |
| `EMELA_PERIPHERAL_CAPACITY_EXCEEDED` | -2 | 入力Listまたは受信sizeが設定容量を超過、返却lengthが容量・要求sizeを超過 |

SPI.transferの返却lengthは入力word数も超えてはならない。入力の範囲・容量エラーではボードoperationを呼ばない。ボードが失敗した場合は出力配列を読まない。成功時のlengthを検査してからListを構築する。

bridgeの要素範囲は共通の0〜65,535である。I²Cの8bit、UARTで選んだ7/8/9bit、SPIで選んだ8/16bitに収まるかは、設定を保持する**ボード実装が検査する**。上位bitをキャストで捨てる実装は契約違反となる。

配列は呼び出し中だけ借用できる。ボードはポインターを保存しない。出力は渡されたcapacity以内へ書き、成功時は実際に初期化したword数をlengthに設定する。返却後のlength検査で、C側の領域外書き込みまで防げるわけではない。

受信Listは生成runtimeの正規の型付きnode・固定heap allocatorで構築する。ボードへListのlayoutや参照カウント操作を要求しない。adapterは所有するListをEmelaへ一度だけ渡すので、借用結果のsnapshotによる二重確保はない。通常の所有権処理により、利用・破棄・失敗後に解放される。heap容量不足は既存の固定heap defect契約に従う。

固定転送配列は到達する転送adapter内に生成する。組み込み標準handlerを使わない既存のプログラムには、この機能の転送配列は追加されない。handlerのoperationを関数値として保持する現runtimeでは、使用するhandlerのほかのoperationも到達扱いになる場合がある。生成headerとlink結果で必要なボード関数を確認する。

## ClockとConsole

```c
int64_t emela_board_clock_now(void);
void emela_board_clock_sleep(EmelaTask task, int32_t milliseconds);
void emela_board_console_print_line(const char *data, uint16_t length);
void emela_board_console_print_error_line(const char *data, uint16_t length);
```

`BoardClock.now` は既存の `Std.Clock` と同様にUnix epochからのmillisecondを返す。epochをどのように設定するかはボードの責務である。温度調節器のRenode例は1970-01-01を開始時刻とする仮想時計で、現在の実日時を取得するRTCではない。

sleepのstartはmainから呼ばれる。ボードはtask handleを保持し、待機完了時に `emela_task_complete_unit(task)` で通知する。この通知はISRから呼べるが、Emelaの再開・heap操作はmainのschedulerが行う。共有handlerは負の待機時間を0にし、ボードは0以下の待機をタイマー起動なしで即時に完了通知する。この場合もEmelaの実際の再開はschedulerが行う。取消済みのtaskへ遅れて通知した場合は既存task ABIが拒否する。ハードウェア停止やボードが保持する記憶域の寿命は、既存の外部taskと同じくボードが管理する。

ConsoleはNUL終端に依存しないUTF-8のpointerとbyte長を受け取る。呼び出し中だけ借用し、ボードが末尾に改行を1つ追加する。標準エラーの出力先を標準出力と同じUARTにすることもできる。Stringを `List[Int]` に変換する必要はない。

## 今回の確認範囲

通常のCLIで共有handlerを使うEmela sourceからCを生成し、安定headerに従うhostのC stubで全23operation（GPIO 6、UART 3、I²C 6、SPI 4、Clock 2、Console 2）を実行した。O0・O2・ASan+UBSanそれぞれ1000反復、73,000件の値・呼び出し順などのチェックが成功し、各処理後の固定heap使用量は0だった。

- configの全field、enum、論理pin、timeout、転送値、13operationの非ゼロstatusから `PeripheralError` への伝播。
- 入力の負値・65,535超過・空・上限値・容量超過、受信sizeの負値・0・上限・超過、ボード返却lengthの容量・要求長超過と失敗時の無視。
- 複数の受信Listを後続転送後まで保持した場合の値と解放、NULを含むConsole出力。
- Clock.sleepの9・0・-1がC側では9・0・0となること、通知とscheduler再開の分離、解放済みtaskへの通知の拒否。

生成Cの `-std=c11 -Wall -Wextra -Werror` によるhost compileと、Cortex-M4F hardfloatでのARM O2 compileも成功した。転送容量65,535の生成Cはhostのstrict compileで、容量0はCLI診断で確認した。

手動確認のsource・config・C stub・ログは `/tmp/emela-stdlib-embedded-check/` に保存した。ASan+UBSanは `ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 UBSAN_OPTIONS=halt_on_error=1` で実行した。LeakSanitizerは実行環境のptrace制約により未実施で、固定Emela heapの返却はharnessが直接確認した。

これは共通ABIと代表入力の確認であり、全言語仕様や全入力組合せを網羅するものではない。UART/I²C/SPIの実際のデバイス実装を全operationについて提供・検証したという意味でもない。温度調節器のSTM32F446/Renode実行と資源使用量は[exampleのREADME](../examples/temperature-controller/README.md)を参照する。

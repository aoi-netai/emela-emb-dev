# 組み込みインターフェースの共通契約

[機能一覧](README.md)にあるGPIO・UART・I²C・SPIと、ISRからmainへの情報共有に適用する契約をまとめる。機器固有の操作・対応設定・C ABIは各機能の文書を参照する。

## 実行場所と言語機能

| 実行場所 | 扱う機能 | 実装条件 |
| --- | --- | --- |
| main・fiber | 型、enum、match、`fails`、effect/handler、List、String、closure、状態、fiber。 | 既存backendの対応範囲で、固定heap・poolの容量内で実行する。同期転送中はほかのfiberへ制御を渡さない。 |
| ISR | 固定サイズの値、enum、match、静的な関数呼び出し、許可されたGPIO・共有操作。 | heap・参照管理・待機を使わず、短時間で復帰する。具体化した呼び先まで検査する。 |
| BSP | レジスタ操作、通信手順、期限管理、割り込み原因の処理。 | Emelaの公開契約を満たし、機種差を吸収する。 |

利用者向けの設定・イベント・操作列にはEmelaの型を使う。C境界は基本型、固定レイアウトstruct、固定容量のdescriptorとbufferを使い、専用bridgeで変換する。headerにはC境界から必要になる型だけを出力し、Emela内部のenum・List・関数値のレイアウトをBSPへ公開しない。一般の`@external`の型制限を広げることは、前提にしない。

## ISRの最小操作と情報共有

ユーザーISRには、GPIOのread/writeと固定容量の情報受け渡しを提供する。configure、割り込み設定、toggleはmain側に置く。ISRの制約は`Immediate`とbuild時の到達検査で確認する。effect名に加えoperation・handler・到達するC symbolを検査する。同じGPIO effectを使っていても、configure等への到達は拒否する。

mainへの受け渡しには`Std.Embedded.Interrupt`を使う。単一producer・単一consumerの固定容量領域に限定し、満杯なら即座に失敗を返す。producer側を1つのISR経路、consumer側を1つのmain側経路に対応付け、ネストした別ISRを同じproducerとして扱わない。固定サイズの値をコピーし、ISRからmainのList・String・State・Cellや通常のhandler状態へ参照を渡さない。容量・保存領域を有効化前に確保し、排他と可視性はatomic操作または短いcritical sectionで保証する。4本の`WordChannel`はそれぞれ16個の`Int`を保持し、`try_push`と`try_pop`で受け渡す。

受け渡しはfiberの自動起動・再開を行わない。受信値を保存できない場合は利用側が欠落を観測できるようにし、UARTの共有保存先のoverflowは`Overrun`と同じ欠落の意味で扱う。汎用の無制限queueは追加しない。

ISR内では固定サイズの`GpioError`を捕捉し、callbackの外には失敗を伝播させない。main側でList・Stringや文字列付きエラーを使っていても、ISR経路だけがheap不使用なら共存できることを要求する。現行backendの[heap判定](../../src/backend/compiler/foundation/heap_plan.rs)にはmodule全体のmanaged error payloadを参照する処理があるため、到達する失敗型・handlerに即した判定と、必要なISR用の特殊化を行う。単にheap検査を無効化して通さない。

## 状態遷移と通知の確定

controllerの状態と、割り込みのbinding・有効状態を別に管理する。

| 状態 | 許可する操作と遷移 |
| --- | --- |
| 未設定 | configureの成功で使用可能になる。転送は`NotConfigured`。 |
| 使用可能 | 転送開始で転送中になる。割り込み停止中に再設定できる。 |
| 転送中 | 競合する転送・設定は`Busy`。完了・停止後に使用可能、または復旧が必要な状態になる。 |
| 復旧が必要 | 新たな転送を`InvalidState`で拒否する。BSPの復旧条件を満たすまで再利用しない。 |

設定検証の失敗では以前の状態を維持する。再設定途中の失敗は未設定とし、ライン等の安全な再利用も確認できない場合は復旧が必要な状態を併せて保持する。UARTの状態管理では、main側送信と割り込み受信の許可された併用を競合と扱わない。

| binding・割り込みの状態 | 遷移と条件 |
| --- | --- |
| 未binding | 実行時の`bind_interrupt`成功で停止中へ移る。有効化は`InvalidState`。 |
| binding済み・停止中 | 設定・資源を検証して有効化できる。停止はそのまま成功する。 |
| 有効 | 停止でcallback・借用参照の終了を確認して停止中へ戻る。再設定・edgeや優先度の変更は停止後に行う。 |

初期化は設定、UARTの受信方式選択、binding、割り込み有効化の順に行う。静的bindingは接続先を確定する情報であり、実行時の`bind_interrupt`が成功する前に有効化できることを意味しない。未bindingでの有効化、割り込み有効中の再設定、受信方式に反する操作は`InvalidState`とする。割り込み停止operationはmainから呼び、復帰後に対象callbackやその経路の借用buffer参照が残らないことを保証する。既に無効な割り込みの停止は成功する。

転送ごとの結果は成功または失敗へ一度だけ確定する。完了IRQとtimeoutは同じ状態を更新し、別々に結果を決めない。旧転送のpending・遅延通知を次の転送へ混入させず、bufferを再利用する前に参照終了を確認する。UARTの送信完了、I²Cの要求結果は、通知が有効ならcallback終了後に同期operationから返す。callbackや通知処理自体が無制限に待つことは許さず、BSPの有界な終了処理とISR実行量の評価に含める。

## BSPの固定資源表

pin・controller・共有IRQ・Consoleの予約をBSPの固定資源表へ集約する。GPIOの再設定もこの表を通し、UART・I²C・SPIが使用中のpinを変更できないようにする。予約や所有権の競合は`ResourceConflict`、一時的な競合は`Busy`とする。検証失敗で予約を変更せず、途中失敗でも未復旧の資源を空きとして公開しない。

開始・終了時の短いcritical sectionで所有状態を更新し、通信全体にわたって割り込みを禁止しない。基本SPI APIとtransactionは同じ所有状態を確認する。transaction内部の操作は取得済みの所有権を使い、バスを二重取得しない。ISRからの待機・spin lockによる競合解消は行わない。

## 同期転送の共通契約

各仕様の同期転送には次の契約を適用する。GPIOにはtimeoutを追加せず、回復可能な失敗は専用の`GpioError`で扱う。

### 実行文脈と設定

- 操作はmain contextから呼ぶ。呼び出し中にEmelaの別fiberへ制御を渡さず、ISRから呼ばない。
- IDは論理IDである。BSPが物理controller、pin、clock source、予約資源との対応を文書化する。
- `configure`の成功後に転送する。未設定・存在しないID・非対応の設定やoperationはエラーにする。
- 引数の検証に失敗した場合はハードウェアを操作しない。設定の検証失敗では以前の設定を維持し、再設定途中で失敗した場合は未設定として扱う。
- 同じcontrollerを別のC taskやISRからも利用するBSPは、設定と転送の競合を防ぐ。

### timeoutと空の転送

- `timeout_ms: Int64`はoperation全体の待機期限である。待機、全要素の転送、通信完了を含め、要素やphaseごとに期限を延長しない。I²Cの応答確認では全試行に共通の期限とする。
- 負値は不正引数。0は待機しない指定で、即時完了できなければtimeoutエラーとする。無期限待ちを表す特別な値は設けない。
- 0では、空転送や既に保持されているUART入力等、その場で完了できる処理だけを行う。新たな通信と完了待ちが必要なら開始前に`Timeout`を返す。UART固定長受信は要求数を待機せず確保できると確認してから消費し、不足時は消費しない。`receive_some`は少なくとも1wordを取得できる場合に適用する。
- 単調増加する時刻源で経過時間を測り、Unix epochやRTCの時刻補正に依存しない。下位APIへの変換時に桁あふれ・切り詰めを行わず、表現できない値は拒否する。
- 期限切れ後のcontroller停止等の後処理も有界にし、その追加時間の上限をBSPに記載する。これは物理的な最大実行時間の証明ではない。
- 空の送信List、受信size 0はID・設定・方向・timeout等を検証した後、通信せず成功する。受信結果は`[]`。SPIのCSはこれらのoperationでは変更しない。
- 負の受信sizeは不正引数。固定長受信は要求した要素数を全て返し、途中失敗や短い受信を成功にしない。UARTの`receive_some`には別途定める可変長の成功条件を適用する。

各BSPに内部の単調時刻とdeadline helperを実装し、転送開始から完了まで同じ期限を使う。SPI transactionではバス取得・設定・setup/holdも期限へ含める。時刻取得・換算・wrap対策を各driverで重複実装せず、期限内の待機と期限切れ後の有界な停止・通知を区別して記録する。ISRや割り込み禁止区間で時刻源の更新が止まり、期限判定が進まなくなる構成を避ける。ユーザーISRからのClock禁止と、BSP内部の期限管理は別の契約である。

### 値・容量・所有権

共有bridgeは`List[Int]`と固定長の`uint16_t`配列を相互変換する。容量はword数で、既定256、設定範囲は1〜65,535。1方向の配列は`2 × capacity` byteのC stackを使い、SPIの`transfer`は2方向分を使う。

bridgeは0〜65,535の共通word範囲・入力容量・負のsize・受信要求容量・返却lengthの上限を検査する。UARTの7/8/9bit、I²Cの8bit、SPIの8/16bitという物理的な幅はBSPが検証する。送信値をキャストで切り詰めず、成功する受信結果もその幅に収める。

配列は呼び出し中だけ借用する。BSPはpointerを保存せず、返却までにISR・DMAを含む配列の参照を終える。成功時はcapacity以内の出力を初期化してlengthを設定する。受信Listの構築・所有権管理・解放は共有bridgeが行う。List用の固定heapが足りない場合は既存runtimeのdefect契約に従う。

### エラーと部分的な副作用

C status 0は成功。GPIO/UART/I²C/SPIの非0は各専用エラーへ変換する。-1は`InvalidArgument`、-2は`CapacityExceeded`を表す共通statusであり、共有bridgeとBSPの両方から返せる。

改訂後の分類・定義元・未知statusの扱いは[エラー定義の共通化](#エラー定義の共通化)に従う。

失敗時の出力はList化しない。転送済みの値や受信済みのデータ消費は取り消せないため、自動再送しない。失敗後にFIFO・controllerをどう復帰させるかをBSPに記載し、次の操作も期限内に終了させる。

## 実装と確認の境界

共通bridgeの現在の返却長検査は「要求以下」であり、短い受信を拒否しない。データ幅・要求数全ての受信・上記timeout契約は、BSP側で満たす要求である。[既存host C mock](../../tests/embedded_peripherals/mod.rs)は値の移送・status・容量・所有権を確認し、MCUの通信波形や全ての物理設定を確認するものではない。

仕様の変更でstdlibの宣言も変更する場合は、Emela APIと生成C ABIの両方を更新する。BSP実装後は、hostで確認した契約と、周辺機器モデル・実機で確認した信号やtimingを分けて記録する。

## エラー定義の共通化

エラーの種類と意味をEmela標準API側でペリフェラルごとに定義し、BSPが機種固有のHALエラーを対応する分類へ変換する。共通化する単位は、同じペリフェラルのボード間でのエラー型と意味である。同じ機能・設定を提供するBSP間では、アプリのEmelaコードやエラー処理を変更せずに複数のボードへ対応できることを目指す。

| 共通の分類 | 意味 |
| --- | --- |
| `InvalidArgument` | 不正なID・値・負のsizeやtimeout。 |
| `CapacityExceeded` | 転送buffer・操作列・返却長等の固定容量の超過。 |
| `NotConfigured` | 未設定、または再設定失敗後の未設定状態。 |
| `Unsupported` | 対象BSPで未対応の設定・組合せ・operation。 |
| `Timeout` | operation全体の待機期限切れ。 |
| `Busy` | ほかの実行中操作による一時的な使用中。 |
| `ResourceConflict` | pin・controller・共有IRQ・Console等の予約・所有権競合。 |
| `InvalidState` | 受信方式・初期化順序・割り込み状態の違反、復旧前の再利用。 |
| `Other` | 上記およびペリフェラル固有の分類に当てはまらない失敗。 |

各専用型には必要な分類だけを含める。GPIOにtimeoutや転送容量エラーを追加しない。UARTには`Parity`・`Framing`・`Overrun`、I²Cには`Nack`・`BusError`・`ArbitrationLost`、SPIには`ModeFault`・`Overrun`を加える。詳細なHAL codeを公開APIへ網羅することは要求しない。

各層の役割は次のとおりとする。

- Emela標準API：ペリフェラルごとの専用エラー型と、分類の意味を定義する。
- BSP：HALのエラーをEmelaが定めた各ペリフェラルの共通statusへ変換する。HAL固有のcodeをそのまま共通statusとして返さない。
- Board handler：共通statusを対応する専用エラー型の分類へ変換する。アプリは数値codeではなく分類で処理を分ける。

C境界は整数statusのまま維持する。数値・名称・分類の対応を単一の定義表に置き、C header定数とEmela側の変換を同じ表から生成する。成功の0、不正引数の-1、容量超過の-2を維持し、BSPも-1/-2を返せる。数値割当は[peripheral_status.rs](../../src/backend/board/peripheral_status.rs)へ確定した。現在は共通C定数とGPIO/UART/I²C/SPI分類の変換をこの表から生成する。HALやenumの内部値から推測しない。未知の非0 statusは`Other`として扱う。

専用エラー型・GPIO設定API・C ABIを変更するときはstdlib・共有handler・生成header・同梱の両BSP・使用例・試験・関連文書を合わせて更新する。旧headerや旧BSPとの混在は互換対象とせず、再生成・再buildを移行手順に含める。

## C bridge

### 標準bindingの接続と検査

`Std.Embedded.Board` は `BoardGpio`・`BoardUart`・`BoardI2c`・`BoardSpi`・`BoardClock`・`BoardConsole` を提供する。アプリは通常の `with` でhandlerを選択する。アプリごとの `@external`、`external_adapters`、`task_adapters` の登録は不要である。独自のセンサや既存Cライブラリには従来の汎用FFIも使える。

標準handlerの内部だけにprivateな外部関数がある。backendは次を照合し、そのC adapterを自動生成する。

- 完全修飾名 `Std.Embedded.Board.gpio_write` などと、専用C binding `__emela_peripheral_gpio_write` などが一致すること。
- 引数と戻り値が対応する標準effectのoperationの型と一致すること。
- GPIOを含む失敗可能なoperationは `(Int, 元の戻り値)` を返し、外部関数自体は非failsであること。
- Clock・Consoleはそれぞれの固定署名に一致し、Clock.sleepだけがsuspendすること。

失敗可能なoperationのadapterは `(code, value)` を返す。共有handlerが通常のEmelaコードで `code != 0` を失敗に変換する。GPIOは生成された共通statusの対応表を通して`GpioError(kind)`へ変換し、UARTは`UartError(kind)`、I²Cは`I2cError(kind)`、SPIは`SpiError(kind)`を使う。sourceの外部関数へ `fails` を追加したり、アプリのFlow IRを手動で置き換えたりしない。汎用FFIのadapter登録規約も変更しない。

### 生成ファイルと値型

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

### 同期operation

GPIO/UART/I²C/SPIはstatusを返し、値がある場合は出力先へ書く。

```c
int32_t emela_gpio_write(EmelaGpioPin pin, EmelaGpioPinState state);
int32_t emela_uart_configure(EmelaUartId id, EmelaUartConfig config);
int32_t emela_i2c_is_device_ready(
    EmelaI2cId id, int32_t address, int32_t trials,
    int64_t timeout_ms, int32_t *out);
```

status `0` は成功。GPIOの非0は`GpioError(kind)`、UARTの非0は`UartError(kind)`、I²Cの非0は`I2cError(kind)`、SPIの非0は`SpiError(kind)`になる。失敗時は出力値を利用しない。GPIOのreadもstatusを返し、入力レベルは出力pointerへ書く。

operationは同期呼び出しであり、長いHAL待機を短くするschedulerへの変換は行わない。UART/I²C/SPIをISRで呼ばず、応答時間はtimeoutとボード実装で管理する。

### Listの転送

`List[Int]` はボード境界で `uint16_t` 配列と長さに変換する。UARTの9bitやSPIの16bitも表せるよう、転送単位はbyteではなく**16bitのword**である。

```c
int32_t emela_uart_transmit(
    EmelaUartId id, const uint16_t *data, uint16_t length,
    int64_t timeout_ms);
int32_t emela_uart_receive(
    EmelaUartId id, int32_t size, int64_t timeout_ms,
    uint16_t *out, uint16_t capacity, uint16_t *length);
```

Rustの `Options::with_peripheral_buffer_capacity` はword数で、既定256、範囲1〜65,535である。1方向あたり最大 `2 × capacity` byteのCスタック領域を使う。SPI.transferは入力と出力の2方向分を同じC frame上で使う。既定値では配列だけで通常512 byte、transferは1024 byteとなる。容量65,535ではそれぞれ131,070 / 262,140 byteになるため、固定上限の設定だけで小さいstackへの適合は保証しない。実際の最大stackはほかのローカル変数や呼び出し先を含むため、生成Cの `.su` や実行時の観測と合わせて評価する。

入力を黙って切り詰めない。次の2つは共通statusであり、ボードも同じ意味で返せる。残りの共通割当とGPIO分類は[単一定義表](../../src/backend/board/peripheral_status.rs)を使う。

| 定数 | code | 条件 |
| --- | --- | --- |
| `EMELA_PERIPHERAL_INVALID_ARGUMENT` | -1 | 要素が0〜65,535の範囲外、受信sizeが負 |
| `EMELA_PERIPHERAL_CAPACITY_EXCEEDED` | -2 | 入力Listまたは受信sizeが設定容量を超過、返却lengthが容量・要求sizeを超過 |

SPI.transferの返却lengthは入力word数も超えてはならない。入力の範囲・容量エラーではボードoperationを呼ばない。ボードが失敗した場合は出力配列を読まない。成功時のlengthを検査してからListを構築する。

bridgeの要素範囲は共通の0〜65,535である。I²Cの8bit、UARTで選んだ7/8/9bit、SPIで選んだ8/16bitに収まるかは、設定を保持する**ボード実装が検査する**。上位bitをキャストで捨てる実装は契約違反となる。

配列は呼び出し中だけ借用できる。ボードはポインターを保存しない。出力は渡されたcapacity以内へ書き、成功時は実際に初期化したword数をlengthに設定する。返却後のlength検査で、C側の領域外書き込みまで防げるわけではない。

受信Listは生成runtimeの正規の型付きnode・固定heap allocatorで構築する。ボードへListのlayoutや参照カウント操作を要求しない。adapterは所有するListをEmelaへ一度だけ渡すので、借用結果のsnapshotによる二重確保はない。通常の所有権処理により、利用・破棄・失敗後に解放される。heap容量不足は既存の固定heap defect契約に従う。

固定転送配列は到達する転送adapter内に生成する。組み込み標準handlerを使わない既存のプログラムには、この機能の転送配列は追加されない。handlerのoperationを関数値として保持する現runtimeでは、使用するhandlerのほかのoperationも到達扱いになる場合がある。生成headerとlink結果で必要なボード関数を確認する。

### ClockとConsole

```c
int64_t emela_board_clock_now(void);
void emela_board_clock_sleep(EmelaTask task, int32_t milliseconds);
void emela_board_console_print_line(const char *data, uint16_t length);
void emela_board_console_print_error_line(const char *data, uint16_t length);
```

`emela_clock.h` は `emela_embedded.h` をincludeし、task runtimeを生成する場合だけsleepの宣言を追加する。nowだけが到達するFlowでは `EmelaTask` の定義を要求しない。

`BoardClock.now` は既存の `Std.Clock` と同様にUnix epochからのmillisecondを返す。epochをどのように設定するかはボードの責務である。起動からの経過時間をそのままUnix時刻として扱う契約ではない。

sleepのstartはmainから呼ばれる。ボードはtask handleを保持し、待機完了時に `emela_task_complete_unit(task)` で通知する。この通知はISRから呼べるが、Emelaの再開・heap操作はmainのschedulerが行う。共有handlerは負の待機時間を0にし、ボードは0以下の待機をタイマー起動なしで即時に完了通知する。この場合もEmelaの実際の再開はschedulerが行う。取消済みのtaskへ遅れて通知した場合は既存task ABIが拒否する。ハードウェア停止やボードが保持する記憶域の寿命は、既存の外部taskと同じくボードが管理する。

ConsoleはNUL終端に依存しないUTF-8のpointerとbyte長を受け取る。呼び出し中だけ借用し、ボードが末尾に改行を1つ追加する。標準エラーの出力先を標準出力と同じUARTにすることもできる。Stringを `List[Int]` に変換する必要はない。

### hostでの確認

[`tests/embedded_peripherals`](../../tests/embedded_peripherals/mod.rs)は標準handler・生成C bridge・BSP共通driverの回帰試験をまとめる。値・status分類・所有権・容量・通知とscheduler再開の分離を確認する。C mockのword転送とMCUの物理的な通信幅・波形は区別する。機器別の確認範囲と再現手順は各機能の文書を参照する。

```sh
cargo test -p emela-codegen-emb --test emit embedded_peripherals:: --locked
```

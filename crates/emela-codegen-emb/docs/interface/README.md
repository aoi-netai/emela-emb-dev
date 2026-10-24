# 周辺機器インターフェース仕様

電子工作向けの標準effect、共有Board handler、BSPのC実装の間の契約をまとめる。各文書はEmela API、値の意味、operationの完了条件、C ABI、現在の実装範囲、BSPで確認する項目を記載する。

| 仕様 | 標準effect | 共有handler | 主な契約 |
| --- | --- | --- | --- |
| [GPIO](gpio-interface.md) | `Std.Embedded.Gpio.Gpio` | `BoardGpio` | 論理pin、入力と出力latch、出力初期値、専用エラー、割り込みbinding |
| [UART](uart-interface.md) | `Std.Embedded.Uart.Uart` | `BoardUart` | parityを除く7/8/9bitのデータ幅、送受信の完了、timeout |
| [I²C](i2c-interface.md) | `Std.Embedded.I2c.I2c` | `BoardI2c` | 未シフトの7bitアドレス、8/16bitの内部アドレス、Repeated START、応答確認 |
| [SPI](spi-interface.md) | `Std.Embedded.Spi.Spi` | `BoardSpi` | 8/16bitのword、CPOL/CPHA、双方向転送、GPIOによるCS制御 |

I²Cの7bit固定・アドレス設定項目の削除・10bit対応の先送りを維持し、GPIO・UART・I²C・SPIの拡充を以下の採用した計画に従って進める。操作契約はこれからBSPが満たす要求であり、実装済みのAPI・C ABI・確認範囲は各文書で区別する。

構成の概要は[組み込みインターフェース](../embedded-interfaces.md)、共通bridgeの実装済みの契約は[標準周辺機器bridge](../embedded-peripherals.md)、ボードごとの対応状況は[ボードpackage](../boards.md)を参照する。生成headerをC宣言の正とし、下位HALの定数値やstructをそのまま公開ABIへ流用しない。

## GPIOの実装状況（2026-10-24）

GPIOは`GpioConfig`・`GpioError`・静的binding・ISR専用read/writeを両BSPへ実装した。[具体的なMCU/profile/pin/対応設定](gpio-interface.md#対応mcuprofilepin設定)と[CLI/Renode確認](../gpio-validation.md)を参照する。`IsrSafe`の新しい言語構文は追加せず、公開署名の`Immediate`とembedded backendの到達検査で制約を実装する。mainの文字列付きエラーとISRの固定エラーが共存する試験も追加した。UARTと固定容量のISR→main共有APIは下記の更新欄を参照する。I²C/SPIとの所有権統合は後続の作業である。

## UART・固定容量共有APIの実装状況（2026-10-24）

UARTは専用エラー、同期送受信、receive_some/discard_input、固定した受信方式、静的bindingとイベント別C入口を両BSPへ実装した。[MCU/profile/controller/pin/設定表](uart-interface.md#初期対応bsp)と[検証](../uart-validation.md)を参照する。共有APIは`Std.Embedded.Interrupt`のWordChannel（4本×16 Int）、try_push/try_popを使う。ISRのchannelとproducer競合をbuildで検査し、満杯はFalseで返す。一般のC FFIや新しい言語構文は追加しない。

## 確定した実装方針

2026-10-23時点の決定として、4つのペリフェラルの拡充は、既存の同梱BSPであるSTM32F4とESP32-C3の両方で動作することを目標とする。通常処理ではEmelaの言語機能を活かし、ISRに到達する経路へ必要な制約を適用する。採用した計画を本書と各ペリフェラル仕様に記録する。実装順序と完成条件は[実装段階と受け入れ条件](#実装段階と受け入れ条件)に従う。GPIOの実装状況は上記の更新欄に記載し、UARTの実装状況は下記の更新欄を参照する。I²C/SPIの改訂は未実装である。

回復可能な失敗は、ペリフェラルごとの専用エラー型で返す方針に統一する。同じペリフェラルのエラー型と分類の意味をボード間で揃える。

| ペリフェラル | 対象BSP | 改訂後の専用エラー型 |
| --- | --- | --- |
| GPIO | STM32F4・ESP32-C3 | `Std.Embedded.Gpio.GpioError` |
| UART | STM32F4・ESP32-C3 | `Std.Embedded.Uart.UartError` |
| I²C | STM32F4・ESP32-C3 | `Std.Embedded.I2c.I2cError` |
| SPI | STM32F4・ESP32-C3 | `Std.Embedded.Spi.SpiError` |

各MCU・board profileで使うcontroller・pin・対応設定は、最初の実装段階で対応表に固定する。共通エラー分類とstatus生成の方針は[エラー定義の共通化](#エラー定義の共通化)に従う。この決定は、現行BSPの対応済み機能や、専用エラー型への移行が実装済みであることを示さない。

UART割り込みの利用者向けインターフェースは、`bind_interrupt`でUART IDと1つのユーザー関数を静的に対応付け、その関数へ`UartId`と`UartEvent`を渡す構成を採用する。BSP向けC入口はイベント別の整数・Bool引数とし、生成bridgeがEmela側でイベントを構築してISR内で直接呼び出す。[利用例](uart-interface.md#採用した利用者向けインターフェース)と[C境界の方針](uart-interface.md#割り込みイベントのc入口)に詳細を記載する。GPIO/UARTの接続を実装した。

### 実行場所と言語機能

| 実行場所 | 扱う機能 | 実装条件 |
| --- | --- | --- |
| main・fiber | 型、enum、match、`fails`、effect/handler、List、String、closure、状態、fiber。 | 既存backendの対応範囲で、固定heap・poolの容量内で実行する。同期転送中はほかのfiberへ制御を渡さない。 |
| ISR | 固定サイズの値、enum、match、静的な関数呼び出し、許可されたGPIO・共有操作。 | heap・参照管理・待機を使わず、短時間で復帰する。具体化した呼び先まで検査する。 |
| BSP | レジスタ操作、通信手順、期限管理、割り込み原因の処理。 | Emelaの公開契約を満たし、機種差を吸収する。 |

利用者向けの設定・イベント・操作列にはEmelaの型を使う。C境界は基本型、固定レイアウトstruct、固定容量のdescriptorとbufferを使い、専用bridgeで変換する。headerにはC境界から必要になる型だけを出力し、Emela内部のenum・List・関数値のレイアウトをBSPへ公開しない。一般の`@external`の型制限を広げることは、この計画の前提にしない。

### 初版の基本機能

両BSPで次の機能を同じ公開契約で動かす。最初の実機検証は各BSPの代表profileに集中し、その結果を基に他profileへ展開する。

| ペリフェラル | 初版の完成条件 |
| --- | --- |
| GPIO | 入出力、初期High/Low、edge割り込み、Emela callbackから別pinへのread/write。 |
| UART | Consoleと別controllerでの8N1送受信、通常受信のユーティリティ、割り込み受信、送信完了通知。 |
| I²C | 7bit masterの送受信、8/16bitレジスタアクセス、応答確認、要求全体の結果通知。 |
| SPI | masterの送受信・同時送受信、同じバス上の複数デバイスに対するCS付きtransaction。 |

各profileの対応表にはcontroller、pin、clock、対応設定の組合せ、IRQと優先度、予約資源、転送容量、復旧手順を記録する。enumにある全設定の実装は要求せず、未対応の組合せは`Unsupported`で返す。未対応operationにも生成ABIに適合する失敗経路を用意し、未解決C symbolを利用者向けの非対応判定にしない。

### ISRの最小操作と情報共有

初版のユーザーISRには、GPIOのread/writeと固定容量の情報受け渡しを提供する。configure、割り込み設定、toggleはmain側に置く。`IsrSafe`はコンパイラーが検査する制約とし、利用者の宣言だけでは安全性を付与できない。effect名に加えoperation・handler・到達するC symbolを検査する。同じGPIO effectを使っていても、configure等への到達は拒否する。

mainへの受け渡しはUART APIとは独立した小さな共有APIとして実装する。初版は単一producer・単一consumerの固定容量領域に限定し、満杯なら即座に失敗を返す。producer側を1つのISR経路、consumer側を1つのmain側経路に対応付け、ネストした別ISRを同じproducerとして扱わない。固定サイズの値をコピーし、ISRからmainのList・String・State・Cellや通常のhandler状態へ参照を渡さない。容量・保存領域を有効化前に確保し、排他と可視性はatomic操作または短いcritical sectionで保証する。具体的な公開名は共通ISR基盤の実装で決める。

受け渡しはfiberの自動起動・再開を行わない。受信値を保存できない場合は利用側が欠落を観測できるようにし、UARTの共有保存先のoverflowは`Overrun`と同じ欠落の意味で扱う。汎用の無制限queueは追加しない。

ISR内では固定サイズの`GpioError`を捕捉できるようにし、callbackの外には失敗を伝播させない。main側でList・Stringや文字列付きエラーを使っていても、ISR経路だけがheap不使用なら共存できることを要求する。現行backendの[heap判定](../../src/emit.rs)にはmodule全体のmanaged error payloadを参照する処理があるため、到達する失敗型・handlerに即した判定と、必要なISR用の特殊化を実装項目に含める。単にheap検査を無効化して通さない。

### 状態遷移と通知の確定

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

### BSPの固定資源表

pin・controller・共有IRQ・Consoleの予約をBSPの固定資源表へ集約する。GPIOの再設定もこの表を通し、UART・I²C・SPIが使用中のpinを変更できないようにする。予約や所有権の競合は`ResourceConflict`、一時的な競合は`Busy`とする。検証失敗で予約を変更せず、途中失敗でも未復旧の資源を空きとして公開しない。

開始・終了時の短いcritical sectionで所有状態を更新し、通信全体にわたって割り込みを禁止しない。基本SPI APIとtransactionは同じ所有状態を確認する。transaction内部の操作は取得済みの所有権を使い、バスを二重取得しない。ISRからの待機・spin lockによる競合解消は行わない。

## UART・I²C・SPIに共通する操作契約案

各仕様の同期転送には次の契約を適用する。GPIOにはtimeoutを追加せず、回復可能な失敗は専用の`GpioError`で扱う改訂方針とする。

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

C status 0は成功。現行の非0 statusは`PeripheralError(code)`になり、-1/-2は共有bridge専用の予約codeである。改訂後は専用エラー型へ変換し、-1を`InvalidArgument`、-2を`CapacityExceeded`としてBSPからも返せる共通statusにする。

改訂後の分類・定義元・未知statusの扱いは[エラー定義の共通化](#エラー定義の共通化)に従う。

失敗時の出力はList化しない。転送済みの値や受信済みのデータ消費は取り消せないため、自動再送しない。失敗後にFIFO・controllerをどう復帰させるかをBSPに記載し、次の操作も期限内に終了させる。

## 実装と確認の境界

共通bridgeの現在の返却長検査は「要求以下」であり、短い受信を拒否しない。データ幅・要求数全ての受信・上記timeout契約は、BSP側で満たす要求である。[既存host C mock](../../tests/embedded_peripherals/mod.rs)は値の移送・status・容量・所有権を確認し、MCUの通信波形や全ての物理設定を確認するものではない。

仕様の変更でstdlibの宣言も変更する場合は、Emela APIと生成C ABIの両方を更新する。BSP実装後は、hostで確認した契約と、周辺機器モデル・実機で確認した信号やtimingを分けて記録する。

## 次回アップデートの検討方針

2026-10-23時点で、「ペリフェラル機能の充実」と「エラー定義の共通化」を採用した実装計画とする。初版の基本機能と段階ごとの完成条件は本書で定め、実施時期と各profileの具体的な配線は実装準備で記録する。現行stdlib・C ABI・BSPに実装済みであることを示すものではない。

### ペリフェラル機能の充実

GPIO・UART・I²C・SPIの機能拡張と両BSPの実装を進める。UARTの[受信ユーティリティ](uart-interface.md#通常受信のユーティリティ)とSPIの[transaction](spi-interface.md#spideviceとtransactionによるcs管理)を初版の計画に含める。ADC・PWM等は別のインターフェースで提供する。

### エラー定義の共通化

エラーの種類と意味をEmela標準API側でペリフェラルごとに定義し、BSPが機種固有のHALエラーを対応する分類へ変換する。共通化する単位は、同じペリフェラルのボード間でのエラー型と意味である。同じ機能・設定を提供するBSP間では、アプリのEmelaコードやエラー処理を変更せずに複数のボードへ対応できることを目指す。

現在のUART・I²C・SPIの`PeripheralError(code: Int)`は共通のエラー型だが、`code`の意味の大半はBSPが定義する。このため、timeout等の判別がボード依存になる。改訂では上記の`GpioError`・`UartError`・`I2cError`・`SpiError`を採用し、それぞれのAPIで必要な分類をenumで表す。

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

C境界は整数statusのまま維持する。数値・名称・分類の対応を単一の定義表に置き、C header定数とEmela側の変換を同じ表から生成する。成功の0、不正引数の-1、容量超過の-2を維持し、改訂後はBSPも-1/-2を返せる。数値割当は[peripheral_status.rs](../../src/peripheral_status.rs)へ確定した。現在は共通C定数とGPIO分類の変換をこの表から生成し、通信3種の専用エラーへの移行は後続で行う。HALやenumの内部値から推測しない。未知の非0 statusは`Other`として扱う。

専用エラー型・GPIO設定API・C ABIは同じ改訂で移行する。stdlib・共有handler・生成header・同梱の両BSP・使用例・試験・関連文書を合わせて更新する。旧headerや旧BSPとの混在は互換対象とせず、再生成・再buildを移行手順に含める。

## 実装段階と受け入れ条件

全段階でEmelaソースから通常のfrontend・CLI・C生成・firmware buildまで通す。Flow IRだけの成功を完成条件にしない。各段階の試験結果にはBSP、profile、設定、確認方法を記録する。

| 段階 | 実装内容 | 完成条件 |
| --- | --- | --- |
| 1. 契約とABI | 代表profileの配線・設定表、共通statusの定義表、専用エラー、GPIO設定、必要なC型だけのheader生成。 | hostで型変換・分類・未知status・入力検証を確認し、両BSPで生成headerとのbuild/linkが通る。 |
| 2. 言語機能とGPIO ISRの成立確認 | 静的binding、ISR専用GPIO read/write、到達検査、固定サイズエラーの捕捉、状態と資源管理。 | mainでList・String・文字列付きエラーを使い、GPIO ISRでenum・match・別pin操作を行う小さなアプリが両BSPで動く。 |
| 3. 通信driverと情報共有 | UART・I²C・SPIの同期driver、UART受信ユーティリティ、UART/I²C通知、固定容量の単一producer・単一consumer受け渡し、共通deadline。 | 両BSPで正常通信・timeout・通信異常・停止後の再利用を確認し、通知と同期結果が一致する。 |
| 4. SPI transaction | 操作列の検証・descriptor化、共有バス所有権、CS timing、結果Listの構築、複数失敗の保持。 | 同じバス上の2デバイスでCSが混ざらず、失敗後の解除・復旧条件が守られる。 |
| 5. 統合とprofile展開 | 言語機能を組み合わせたアプリ、資源量・応答の評価、各profileの対応表と移行手順。 | 下表の統合試験を両BSPで実施し、他profileのbuildと対応する実機確認の範囲を明示する。 |

| 統合試験 | 主な確認方法と合格条件 |
| --- | --- |
| GPIO ISRでenumをmatchして別pinを操作 | hostでID・値・直接呼び出し、実機でedgeと出力を確認する。禁止操作・heapへの到達はコンパイル時に拒否する。 |
| UART ISRから固定領域へ保存しmainで解析 | List・Stringを使う解析をmockで試験し、実機で入力順序・満杯時の欠落観測・受信方式の排他を確認する。 |
| I²Cをeffect/handlerで抽象化し`fails`で回復 | 同じアプリのmockと実BSPを切り替える。NACK・timeout・Repeated START・最終通知・失敗後の次の操作を確認する。 |
| 2つのSPIデバイスをtransactionで操作 | 操作列と結果の対応をhostで、CS・最終clock・setup/holdを実機で確認する。転送失敗とCS解除失敗の併存を注入する。 |
| fiber・状態・文字列付きエラーとの共存 | 通常の言語機能を使うmainとISRを同居させる。heap・poolの使用量、転送配列を含むstackとISRネスト、開始遅延と処理時間を測定する。 |

host mock・周辺機器モデル・実機の確認範囲を分け、モデルにない信号を確認済みにしない。stackは生成Cの`.su`・link map・実行時high-waterを合わせて評価し、測定したアプリ・設定に対する余裕を記録する。固定容量やISR検査の成功を、任意のアプリの最大実行時間の保証にはしない。

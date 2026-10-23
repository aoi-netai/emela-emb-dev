# SPIインターフェース仕様

状態：BSP実装に向けた操作契約と採用した実装計画。基本APIは現行の公開型・4operation・C ABIを基準に記載し、改訂方針として[SPI専用エラーへの移行](#spi専用エラーへの移行方針)と[上位のSpiDeviceとtransactionによるCS管理](#spideviceとtransactionによるcs管理)を初版に含める。これらの改訂は未実装である。以下のword・完了条件・dummy送信等は準拠するhandlerとBSPへの要求であり、同梱BSPの実装済み機能を示すものではない。

実行文脈、設定、期限、空の転送、容量、エラー、所有権は[共通の操作契約案](README.md)に従う。SPI operationは同期呼び出しであり、Emelaの別fiberへ制御を渡さない。

## Emela API

宣言元は[`Std.Embedded.Spi`](../../../emela-core/lib/std/embedded/spi.emel)、共有handlerは[`BoardSpi`](../../../emela-core/lib/std/embedded/board.emel)である。以下は現行の宣言であり、`SpiError`への改訂はまだ含まない。

```emela
import Std.Embedded.Peripheral.{PeripheralError}

pub enum SpiId { Spi1, Spi2, Spi3, Spi4, Spi5, Spi6 }
pub enum SpiMode { Master, Slave }
pub enum SpiDirection { TwoLine, TwoLineReceiveOnly, OneLine }
pub enum SpiDataSize { Bits8, Bits16 }
pub enum ClockPolarity { IdleLow, IdleHigh }
pub enum ClockPhase { FirstEdge, SecondEdge }
pub enum BitOrder { MostSignificantFirst, LeastSignificantFirst }
pub enum SpiPrescaler { Div2, Div4, Div8, Div16, Div32, Div64, Div128, Div256 }
pub type SpiConfig(
  mode: SpiMode,
  direction: SpiDirection,
  data_size: SpiDataSize,
  polarity: ClockPolarity,
  phase: ClockPhase,
  prescaler: SpiPrescaler,
  bit_order: BitOrder
)

pub effect Spi {
  fn configure(id: SpiId, config: SpiConfig) -> () fails PeripheralError
  fn transmit(id: SpiId, data: List[Int], timeout_ms: Int64) -> () fails PeripheralError
  fn receive(id: SpiId, size: Int, timeout_ms: Int64) -> List[Int] fails PeripheralError
  fn transfer(id: SpiId, data: List[Int], timeout_ms: Int64) -> List[Int] fails PeripheralError
}
```

## バスと設定

`SpiId`は論理IDである。BSPが物理controller、SCK/MOSI/MISOまたは双方向線、clock source、利用できるpinと予約資源を定める。CSはGPIOで制御する別のpinであり、`SpiConfig`には含めない。

| 設定 | 契約 |
| --- | --- |
| `mode` | MasterはSCKを生成し、Slaveは外部controllerのSCKに従う。BSPは対応しないmodeをconfigureで拒否する。 |
| `direction` | TwoLineは送受信の2線、TwoLineReceiveOnlyは受信専用、OneLineは1本のデータ線を方向切替する半二重通信。 |
| `data_size` | 1 wordのbit幅。Bits8なら8、Bits16なら16 clock分のデータになる。 |
| `polarity` | SCKのidle level。IdleLowはCPOL=0、IdleHighはCPOL=1。 |
| `phase` | 各wordの最初のedgeでsampleするFirstEdgeはCPHA=0、2番目のedgeでsampleするSecondEdgeはCPHA=1。 |
| `prescaler` | MasterのSCKを、BSPが定める周辺clockの2〜256分周で生成する。実周波数と元clockをBSPに記載する。SlaveではSCKを生成せず、このfieldは周波数制御に使わない。 |
| `bit_order` | word内のbitを最上位から送るか、最下位から送るか。Listの要素順序は変えない。 |

enumの整数値を下位HALの定数として流用しない。BSPはmode・direction・幅・clock等の対応する組合せを文書化する。実現できない設定を黙って置き換えず、`PeripheralError`で拒否する。

### クロックmode

| 一般的なmode番号 | polarity | phase | sampleするedge |
| --- | --- | --- | --- |
| 0 | `IdleLow` | `FirstEdge` | 立上り |
| 1 | `IdleLow` | `SecondEdge` | 立下り |
| 2 | `IdleHigh` | `FirstEdge` | 立下り |
| 3 | `IdleHigh` | `SecondEdge` | 立上り |

`FirstEdge`は常に立上りを意味する値ではない。BSPは設定したidle levelからの最初の遷移を基準にする。

### directionとoperation

| direction | transmit | receive | transfer |
| --- | --- | --- | --- |
| `TwoLine` | 送信。付随する受信値は捨てる | 受信。Masterではclock生成のためにdummyを送る | 同時送受信 |
| `TwoLineReceiveOnly` | 非対応 | 受信 | 非対応 |
| `OneLine` | 送信方向へ切替 | 受信方向へ切替 | 非対応 |

非対応のoperationは通信開始前にエラーにする。1線の送信と受信を内部で順番に行って`transfer`の代わりにしない。Slaveで必要なNSS入力・frame境界・外部clockの条件はBSPが文書化し、現行APIで条件を満たせない構成は拒否する。

## wordとbyteの関係

| 設定 | ListとC配列の各要素 | 1要素の転送量 |
| --- | --- | --- |
| `Bits8` | 0〜255 | 8bit |
| `Bits16` | 0〜65,535 | 16bit |

`size`・`length`・`capacity`は全てword数である。Bits16で`size = 3`なら3 word、合計48bitを受信する。Cの`uint16_t`配列をbyte列へreinterpretして、hostのendiannessに依存した順序で送らない。

例えばBits16の`0x1234`は、MostSignificantFirstではbit 15から、LeastSignificantFirstではbit 0から送る。これはListの順序や、アプリが複数wordで表す数値のbyte順を変換する指定ではない。

## operationの完了条件

### configure

設定とpin割り当てを検証してから適用し、後の転送に使う。Masterでは設定したidle polarityへSCKを整える。アプリは全接続先のCSを非選択にしてから再設定する。

改訂後は[固定資源表](README.md#bspの固定資源表)でcontroller・pin・予約資源の競合を検査する。進行中の転送やtransactionとの競合は`Busy`、予約用途との競合は`ResourceConflict`とする。transaction内部の設定切替は取得済みの所有権を使い、バスを二重取得しない。

### transmit

全要素を検証してから、先頭から指定したbit順で送信する。全wordのshiftと最終clockの完了を確認して`()`を返す。FIFOへ書けただけでは完了としない。TwoLineで生じる受信値はBSPが処理して捨て、後の受信へ混入させない。

### receive

`size` wordを受信して返す。Master/TwoLineでは同じ数のゼロwordをdummy送信してclockを作る。ゼロ以外のdummy値が必要なデバイスには、アプリが`transfer`でその値を指定する。

受信専用・1線の動作では、その方向に必要なclockと線の切替をBSPが行う。Slaveでは外部clockを待つ。全wordの受信を成功条件とし、短いListを成功として返さない。

### transfer

TwoLineで、入力の各wordと同時に1 wordずつ受信する。成功時は入力Listと同じ要素数のListを返す。データを全て送ってから別途受信する実装では代用しない。

SPIにはI²Cのような標準のaddress ACKがない。operationの成功はcontroller側の転送完了を表し、接続先の存在やコマンドの受理は保証しない。

## CSと複数operation

以下は現行の基本SPI APIの契約である。標準の上位APIでCS管理をまとめる計画は、[後続のAPI検討](#spideviceとtransactionによるcs管理)に記載する。

MasterでのCS有効・無効のレベルとsetup/hold時間は接続先デバイスによる。アプリが[Gpio](gpio-interface.md)でCSを設定し、必要な転送の前後で制御する。SPIのconfigure/transmit/receive/transferはCSを自動変更しない。

コマンド送信とデータ受信等、複数operationの間でCSを選択したままにできる。アプリはその間に同じバスを別の接続先へ使わせず、必要な排他を管理する。CSを保持するtransaction scopeやbus lockは現行APIにない。

転送失敗時もBSPが勝手にアプリのGPIOを操作しない。アプリまたはCSを管理するhandlerが、通常の失敗経路でCSを非選択へ戻す。ゼロ長転送でもCSは変更せず、SCKによるデータ転送を行わない。

## timeoutと失敗後の状態

共通の期限契約に従う。MasterのFIFO・shift完了待ち、Slaveの選択・外部clock待ちも1回のoperationの期限に含める。wordごとに期限を延長しない。

`timeout_ms == 0`では、空転送は検証後に成功し、非空転送はclockを開始する前に`Timeout`で失敗する。transactionにはバス取得・設定・CSのsetup/holdも含めた1つのdeadlineを適用する。

timeout、mode fault、overrun等の異常では、部分的な受信結果を成功として返さない。転送済みのclock・データは取り消せない。BSPは返却までに借用配列の参照を終え、controllerを停止・復帰させる方法と、次の操作に再configureが必要な条件を文書化する。

### SPI専用エラーへの移行方針

2026-10-23時点の決定として、[共通の確定方針](README.md#確定した実装方針)に従い、`Std.Embedded.Spi`で定義する専用の`SpiError`へ移行する。対象BSPは既存のSTM32F4とESP32-C3とし、ボード間でエラー分類の意味を揃える。細かい機種固有エラーは`Other`へまとめ、HAL固有のcodeをそのまま利用者へ公開しない。

既存の4operationの回復可能な失敗を、次の型で表す。これは改訂後の型であり、未実装である。

```emela
pub enum SpiErrorKind {
  InvalidArgument
  CapacityExceeded
  NotConfigured
  Unsupported
  Timeout
  Busy
  ResourceConflict
  InvalidState
  ModeFault
  Overrun
  Other
}
pub error SpiError(kind: SpiErrorKind)
```

共通分類は[エラー定義の共通化](README.md#エラー定義の共通化)に従う。`ModeFault`はcontrollerのmode・master権限等の異常、`Overrun`は受信データの取りこぼしを表す。未知の非0 statusは`Other`へ変換する。共通statusの単一の定義表からC定数とEmela側の変換を生成し、0は成功、-1は`InvalidArgument`、-2は`CapacityExceeded`とする。改訂後はBSPも-1/-2を返せる。

上位のtransactionでは、SPI由来・GPIO由来の失敗とCS解除失敗を保持する専用の`SpiTransactionError`を使う。詳細は[失敗と復旧](#失敗と復旧)に従う。stdlib・共有handler・生成C ABI・BSP・試験を合わせて更新する。

## C ABI

生成された`emela_spi.h`を宣言の正とする。`EmelaSpiConfig`はEmelaの7 fieldに対応する名前付きstructである。

```c
int32_t emela_spi_configure(EmelaSpiId id, EmelaSpiConfig config);
int32_t emela_spi_transmit(
    EmelaSpiId id, const uint16_t *data, uint16_t length, int64_t timeout_ms);
int32_t emela_spi_receive(
    EmelaSpiId id, int32_t size, int64_t timeout_ms,
    uint16_t *out, uint16_t capacity, uint16_t *length);
int32_t emela_spi_transfer(
    EmelaSpiId id, const uint16_t *data, uint16_t length, int64_t timeout_ms,
    uint16_t *out, uint16_t capacity, uint16_t *out_length);
```

status 0が成功。receiveは`*length = size`、transferは`*out_length = length`とし、capacity以内でその数のwordを初期化する。Bits8では出力wordの上位8bitを0にする。Listの構築・固定heapでの所有権管理は共有bridgeが行う。

## 現在の実装と必要な確認

| 部分 | 現在の状態 |
| --- | --- |
| stdlib・BoardSpi・C bridge | 4operation、設定struct、word配列、status変換と受信Listの接続がある。 |
| STM32F4・ESP32-C3 BSP | SPIのC driverは未実装。到達する場合は追加実装が必要。 |
| host C mock | 4operationの設定・値・status・容量・返却長・所有権を確認。物理SCK・bit順・CSやmodeの組合せは確認しない。 |

共有bridgeは返却長の上限を検査するが、短いreceive/transferを成功として返すBSPを拒否しない。上の要求数全ての転送と設定幅はBSPが保証する。

BSP実装時には次を確認する。

| 対象 | 確認内容 |
| --- | --- |
| 設定 | IDとpin、Master/Slave、各direction、未設定・非対応operationの拒否、再設定。 |
| 信号 | mode 0〜3のidle/sample edge、bit順、prescalerと実SCK。 |
| word | Bits8/16の境界と範囲外、word数とbyte数の区別、host endiannessに依存しないこと。 |
| 転送 | simultaneous transfer、Masterの受信dummy、送信時の受信値破棄、最終clockまでの完了。 |
| CS | 明示したGPIOだけがCSを操作し、複数operationで選択を保持できること。 |
| 失敗・資源 | timeout、Slaveの外部clock不足、途中失敗、空転送、容量境界、配列寿命、List解放とstack。 |
| transactionの検証 | 操作数・総word数の容量、全要素の事前検証、空操作列・ゼロ長操作、Read/Transferと結果Listの対応。 |
| バスの共有 | 基本APIとの同時利用の拒否、内部操作の二重取得がないこと、2デバイスの設定切替とCSの排他、通信中に全IRQを禁止しないこと。 |
| transactionの失敗 | 転送とCS解除の同時失敗の保持、解除失敗後の再利用拒否、復旧手順、CS解除後のList構築。 |
| deadlineと信号 | 設定・setup/holdを含む期限、timeout 0でCSやclockを開始しないこと、最終clock後のhold、失敗時の有界な停止・解除。 |
| 言語機能の統合 | Emelaソースの操作enum・List・`fails`から生成C・firmwareまで通し、両BSPで2デバイスを操作すること。 |

## 後続のAPI検討

### SpiDeviceとtransactionによるCS管理

2026-10-23時点の決定として、基本のSPI・GPIO APIを分けたまま、両者をまとめる標準の上位APIを追加する計画とする。利用者が初期化時にCS用pinを指定し、通常の通信は接続先を表す`SpiDevice`と、その接続先への一連の通信を表す`transaction`を通して行えるようにする。基本SPI APIを直接使い、利用者が独自にCSを管理する経路も維持する。

`SpiDevice`はMaster側の接続先ごとに、SPIバス・`SpiConfig`・CS用の`GpioPin`・CSの有効レベル・setup/hold時間を対応付ける。時間は非負のマイクロ秒値で最小時間を指定し、BSPの分解能へ切り上げる場合も共通deadlineを守る。実現できない要求は事前に拒否する。同じSPIバスに、設定とCSの異なる複数のデバイスを接続できる。利用者がGPIOのhandler全体を引数として渡す方式は要求しない。SlaveのNSS入力は、このMaster側のCS制御とは別にBSPで扱う。

CS用pinは初期化時に固定資源表で予約し、非選択の初期値で出力へ設定する。transactionが管理するCSを、別デバイスや独立したGPIO操作が競合して変更できないようにする。設定しただけの値と、初期化・予約が完了した利用可能なデバイスを区別する。

#### 操作列と結果

利用者向けの操作列は次のenumのListで表し、main側で構築する。

```emela
pub enum SpiOperation {
  Write(data: List[Int])
  Read(size: Int)
  Transfer(data: List[Int])
}
```

`transaction(device, operations, timeout_ms)`の成功結果は`List[List[Int]]`とし、操作列の`Read`・`Transfer`に対応する受信Listだけを、その操作の出現順に返す。`Write`には結果の要素を作らない。`Read(0)`・`Transfer([])`には空Listの要素を返し、対応関係を保つ。各操作の幅・方向・dummy送信・完了条件は基本SPI APIと同じとする。

例えば初期化済みの`sensor`へ`[Write([0x80]), Read(2), Transfer([0x00])]`を渡すと、成功結果は長さ2と長さ1の受信Listを順に持つ。deviceの構築・初期化関数の正式名と引数順はABI実装段階で確定する。

CSを選択する前に、全操作の種類・方向・値の幅・size・timeout、操作数、送受信の総word数を検証する。容量は操作ごとだけでなくtransaction全体に適用し、上限を生成設定とheaderに記録する。総数計算の桁あふれも拒否する。固定容量のC descriptor、送信・受信buffer、結果のoffset/length領域を準備してから通信に入る。検証・容量不足の失敗ではCSもclockも操作しない。

空の操作列、または全操作がゼロ長なら、デバイス・設定・各引数を検証してからCS・clockを変更せず成功する。結果は上記の対応規則に従い、空の操作列なら`[]`、`[Read(0)]`なら`[[]]`になる。

#### 実行順序と期限

標準のtransaction実装は次の順序を守る。

1. 事前検証とdescriptor・bufferの準備を行う。
2. バスの所有権を取得し、全CSが非選択の状態でdeviceのバス設定を適用する。
3. 対象CSを選択し、setup条件を満たす。
4. 操作列を実行し、操作間もCSを保持する。
5. 最終clockとhold条件を満たしてからCSを解除し、buffer参照を終了してバスを解放する。
6. main側で受信bufferから結果Listを構築する。

基本SPI APIの直接利用も同じ所有状態を確認し、一時的な競合は`Busy`で返す。内部の設定・転送は取得済みの所有権を使う。開始・終了の短いcritical sectionで状態を更新し、通信全体を割り込み禁止にしない。

1つのdeadlineをバス取得・設定からsetup・全操作・holdまで引き継ぎ、操作ごとに延長しない。`timeout_ms == 0`の非空転送はCS選択前に`Timeout`とする。期限切れ後は有界な停止・CS解除を行い、その追加時間をBSPに記録する。setup/holdの待機をユーザーISRへ実装せず、main側またはBSP内部のtimer処理で実行する。

#### 失敗と復旧

回復可能な失敗では転送を停止し、CSを選択済み、または選択操作の途中失敗で状態が不確かな場合は解除を試みる。通常の失敗を表す`SpiTransactionError`には、最初の失敗の発生元と分類、およびCS解除の成否を保持する。

| 失敗 | 保持する情報 |
| --- | --- |
| SPIの検証・設定・転送の失敗 | `SpiErrorKind`を保持する。 |
| CSの初期化・選択等のGPIO操作の失敗 | `GpioErrorKind`を保持する。 |
| CS解除だけが失敗 | 解除に失敗した`GpioErrorKind`を保持する。 |
| 最初の失敗とCS解除失敗が併存 | 最初のSPI/GPIOの分類と、解除のGPIO分類の両方を保持する。 |

発生元と併存をenum・固定サイズのfieldで区別できる型にし、後の解除失敗で最初の失敗を上書きしたり、両方を1つの`Other`へ潰したりしない。C境界にも主結果とcleanup結果を別のstatus欄として設ける。正式なvariant・field名は専用エラーとABIを実装する段階で確定する。

CS解除に失敗したバスは復旧が必要な状態へ置き、新たな転送を`InvalidState`で拒否する。BSPの復旧手順でcontrollerの停止・全CSの非選択を確認するまで再利用せず、単なるバスlock解放や再configureだけで復旧済みにしない。借用bufferへの参照は失敗時も終了する。部分的な受信結果を成功として返さず、自動再送もしない。

結果Listの確保はCS解除とバス解放の後に行うため、そこでのheap不足がCSを選択したままにすることはない。runtimeのdefect全般に通常のcleanupを保証するものではなく、defect時の出力方針はBSPの契約に従う。

#### BSPとISRへの接続

操作列を先に渡し、BSPは最終操作を判別できる形にする。途中のoperation完了でCSを解除せず、低レベルの1 operationの完了とCS解除を含むtransactionの完了を区別する。C境界には固定レイアウトのdescriptor・bufferを渡し、Emelaの`SpiOperation`やListの内部レイアウトを公開しない。

ISRでCSを操作する実装には、[BSP提供のISR専用handlerを静的に接続する方針](uart-interface.md#bspによるisr専用handlerの静的供給)を適用する。pin・操作列・bufferをmain側で準備し、ISRにListの構築・参照管理・解放を要求しない。初版の完成条件は同期transactionの契約であり、最終ISRからのCS解除を必須の実装方法にはしない。

上位API・資源管理・BSP接続は未実装である。両BSPの代表profileでMaster転送を成立させた後、同じバス上の2デバイスについて上記の契約を確認する。

### その他の拡張候補

Hz指定のclock設定、可変dummy cycle、上記の操作列を超える任意の送受信phase列、非同期・DMAは別の拡張として扱う。現行の同期operationへ暗黙に追加しない。

## 参照

- [ST公式STM32F4 SPI定義](https://github.com/STMicroelectronics/stm32f4xx-hal-driver/blob/master/Inc/stm32f4xx_hal_spi.h)：mode、direction、data size、CPOL/CPHA、prescaler、bit順。
- [embedded-halのSPI busとdevice](https://docs.rs/embedded-hal/latest/embedded_hal/spi/index.html#bus-vs-device)：バスとCS付き接続先の区別、transaction中のCS保持とバスの排他、CSのtiming設定。
- [ESP-IDF ESP32-C3 SPI Master](https://docs.espressif.com/projects/esp-idf/en/stable/esp32c3/api-reference/peripherals/spi_master.html)：SPI modeと信号、CSとtransactionの区別。Emelaの現行APIはCSをGPIOで制御する。

# SPIインターフェース仕様

状態：基本SPI、専用エラー、SpiDevice・transaction・CS管理をstdlib・C bridge・STM32F4/ESP32-C3 BSPへ実装済み。以下は操作契約であり、実機の波形・電気特性まで確認済みという意味ではない。対応範囲と確認方法は[検証](../spi-validation.md)を参照する。

実行文脈、設定、期限、空の転送、容量、エラー、所有権は[共通の操作契約案](README.md)に従う。SPI operationは同期呼び出しであり、Emelaの別fiberへ制御を渡さない。

## Emela API

宣言元は[`Std.Embedded.Spi`](../../../emela-core/lib/std/embedded/spi.emel)、共有handlerは[`BoardSpi`](../../../emela-core/lib/std/embedded/board.emel)である。基本の4operationに、デバイス初期化・transaction・復旧を加える。

```emela
import Std.Embedded.Gpio.{GpioPin, PinState, GpioErrorKind}

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

pub enum SpiErrorKind {
  InvalidArgument, CapacityExceeded, NotConfigured, Unsupported, Timeout,
  Busy, ResourceConflict, InvalidState, ModeFault, Overrun, Other
}
pub error SpiError(kind: SpiErrorKind)

## A device description; create_device reserves and initializes its resources.
pub type SpiDeviceConfig(
  bus: SpiId, config: SpiConfig, cs: GpioPin, active: PinState,
  setup_us: Int64, hold_us: Int64
)
## A checked board handle. Unknown or uninitialized slots are rejected at runtime.
pub type SpiDevice(slot: Int)
pub enum SpiOperation {
  Write(data: List[Int])
  Read(size: Int)
  Transfer(data: List[Int])
}
pub enum SpiTransactionFailure {
  SpiFailure(kind: SpiErrorKind)
  GpioFailure(kind: GpioErrorKind)
}
## The first failure and a separate failure while deasserting chip select.
pub error SpiTransactionError(
  primary: Option[SpiTransactionFailure], cleanup: Option[GpioErrorKind]
)

pub effect Spi {
  fn configure(id: SpiId, config: SpiConfig) -> () fails SpiError
  fn transmit(id: SpiId, data: List[Int], timeout_ms: Int64) -> () fails SpiError
  fn receive(id: SpiId, size: Int, timeout_ms: Int64) -> List[Int] fails SpiError
  fn transfer(id: SpiId, data: List[Int], timeout_ms: Int64) -> List[Int] fails SpiError
  fn create_device(config: SpiDeviceConfig) -> SpiDevice fails SpiTransactionError
  fn transaction(device: SpiDevice, operations: List[SpiOperation], timeout_ms: Int64) -> List[List[Int]] fails SpiTransactionError
  fn recover(id: SpiId) -> () fails SpiTransactionError
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

enumの整数値を下位HALの定数として流用しない。BSPはmode・direction・幅・clock等の対応する組合せを文書化する。実現できない設定を黙って置き換えず、`SpiError`で拒否する。

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

[固定資源表](README.md#bspの固定資源表)でcontroller・pin・予約資源の競合を検査する。進行中の転送やtransactionとの競合は`Busy`、予約用途との競合は`ResourceConflict`とする。transaction内部の設定切替は取得済みの所有権を使い、バスを二重取得しない。

### transmit

全要素を検証してから、先頭から指定したbit順で送信する。全wordのshiftと最終clockの完了を確認して`()`を返す。FIFOへ書けただけでは完了としない。TwoLineで生じる受信値はBSPが処理して捨て、後の受信へ混入させない。

### receive

`size` wordを受信して返す。Master/TwoLineでは同じ数のゼロwordをdummy送信してclockを作る。ゼロ以外のdummy値が必要なデバイスには、アプリが`transfer`でその値を指定する。

受信専用・1線の動作では、その方向に必要なclockと線の切替をBSPが行う。Slaveでは外部clockを待つ。全wordの受信を成功条件とし、短いListを成功として返さない。

### transfer

TwoLineで、入力の各wordと同時に1 wordずつ受信する。成功時は入力Listと同じ要素数のListを返す。データを全て送ってから別途受信する実装では代用しない。

SPIにはI²Cのような標準のaddress ACKがない。operationの成功はcontroller側の転送完了を表し、接続先の存在やコマンドの受理は保証しない。

## CSと複数operation

以下は現行の基本SPI APIの契約である。標準の上位APIによるCS管理は、[SpiDeviceとtransaction](#spideviceとtransactionによるcs管理)に記載する。

MasterでのCS有効・無効のレベルとsetup/hold時間は接続先デバイスによる。アプリが[Gpio](gpio-interface.md)でCSを設定し、必要な転送の前後で制御する。SPIのconfigure/transmit/receive/transferはCSを自動変更しない。

コマンド送信とデータ受信等、複数operationの間でCSを選択したままにできる。アプリはその間に同じバスを別の接続先へ使わせず、必要な排他を管理する。CSを標準APIで保持したい場合は、後述の`transaction`を使う。

転送失敗時もBSPが勝手にアプリのGPIOを操作しない。アプリまたはCSを管理するhandlerが、通常の失敗経路でCSを非選択へ戻す。ゼロ長転送でもCSは変更せず、SCKによるデータ転送を行わない。

## timeoutと失敗後の状態

共通の期限契約に従う。MasterのFIFO・shift完了待ち、Slaveの選択・外部clock待ちも1回のoperationの期限に含める。wordごとに期限を延長しない。

`timeout_ms == 0`では、空転送は検証後に成功し、非空転送はclockを開始する前に`Timeout`で失敗する。transactionにはバス取得・設定・CSのsetup/holdも含めた1つのdeadlineを適用する。

timeout、mode fault、overrun等の異常では、部分的な受信結果を成功として返さない。転送済みのclock・データは取り消せない。BSPは返却までに借用配列の参照を終え、controllerを停止・復帰させる方法と、次の操作に再configureが必要な条件を文書化する。

### SPI専用エラー

全基本operationは`SpiError(kind)`、device初期化・transaction・復旧は`SpiTransactionError(primary, cleanup)`を使う。BSPのstatusと分類は[単一定義表](../../src/peripheral_status.rs)から生成する。未知の非0は`Other`となる。

| status | 分類 |
| --- | --- |
| -1 / -2 | `InvalidArgument` / `CapacityExceeded` |
| -3 / -4 / -5 | `NotConfigured` / `Unsupported` / `Timeout` |
| -6 / -7 / -8 | `Busy` / `ResourceConflict` / `InvalidState` |
| -9 | `Other` |
| -12 / -16 | `Overrun` / `ModeFault` |

`ModeFault`はmode・master権限等の異常、`Overrun`は受信データの取りこぼし。GPIO/UART/I²C固有の他statusはSPIでは`Other`となる。共有bridgeもBSPも同じstatusを返せる。HAL固有codeを公開APIへそのまま渡さない。

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

### transactionのC ABI

```c
#define EMELA_SPI_DEVICE_CAPACITY 4
/* 操作数とTX/RXの合計word数は生成設定のperipheral_buffer_capacity。 */
typedef struct { uint16_t kind, length, tx_offset, rx_offset; } EmelaSpiOperation;
typedef struct { int32_t source, status, cleanup_status; } EmelaSpiTransactionStatus;
void emela_spi_create_device(EmelaSpiDeviceConfig config, EmelaSpiDevice *out,
    EmelaSpiTransactionStatus *status);
void emela_spi_transaction(EmelaSpiDevice device,
    const EmelaSpiOperation *operations, uint16_t count,
    const uint16_t *tx, uint16_t tx_length, int64_t timeout_ms,
    uint16_t *out, uint16_t capacity, EmelaSpiTransactionStatus *status);
void emela_spi_recover(EmelaSpiId id, EmelaSpiTransactionStatus *status);
```

operation kindはWRITE=1、READ=2、TRANSFER=3（`EMELA_SPI_OPERATION_*`）。sourceはNONE=0、SPI=1、GPIO=2（`EMELA_SPI_FAILURE_*`）。`status`は最初の失敗、`cleanup_status`はCS解除の失敗を別に保持する。payload付きEmela enumやListの内部layoutはBSPへ渡さない。TX/RX offsetはoperationの順に累積し、WriteはTX、ReadはRX、Transferは両方を使う。bufferは呼出し中だけ借用し、成功時は指定されたRX領域を全て初期化する。

## 初期対応BSP

| BSP | 論理ID / controller | SCK / MISO / MOSI | clock・設定 |
| --- | --- | --- | --- |
| STM32F407VG/F412ZG/F429ZI/F446REとboard alias | `Spi2` / SPI2 | PB13 / PB14 / PB15、AF5 | APB1 16 MHz、Div2〜Div256（8 MHz〜62.5 kHz） |
| ESP32-C3 | `Spi2` / SPI2（GPSPI） | GPIO0 / GPIO10 / GPIO1、GPIO matrix | XTAL 40 MHz、Div2〜Div256（20 MHz〜156.25 kHz） |

両BSPともMaster・TwoLine、8/16bit、mode 0〜3、両bit順に対応する。Slave・TwoLineReceiveOnly・OneLineと他の論理IDは`Unsupported`。対応しない設定を別の通信方式へ置き換えない。clockは上表の名目値であり、実際の精度と配線の制約は別に確認する。

基本転送はCSを変更しない。device用CSは、bus・Console・UART/I²C・既存GPIO・他deviceとの競合を検査して非選択で初期化する。初期化済みCSとbus pinは通常のGPIO操作から変更できない。STM32F4はpackageに存在するpin、ESP32-C3はGPIO0〜10のうち予約と競合しないpinを使える。

STM32F4は各wordのTXE/RXNEを処理し、送信時にも受信側をdrainする。最後はTXEとBSY解除を確認する。ESP32-C3は64byteのdata buffer単位で同時送受信し、最終commandとTRANS_DONEを確認する。16bitはbit順に応じて明示的にbyteを配置し、host endiannessへ依存しない。

時刻の分解能は1μs。STM32F4はSysTickと単調増加ms、ESP32-C3は52bit SysTimerのwrapを延長して測る。`timeout_ms`は0〜`Int64`の最大値 / 1000を受け入れ、μsで扱えない値と負値を事前に拒否する。setup/holdが正なら量子化で短くならないよう追加の1μsを待つ。0では待たない。

controller停止は追加2msの期限で有界に行い、必要ならresetする。通常転送の失敗後は未設定となるため再configureが必要。停止またはCS解除が失敗したバスは`recover`が成功するまで`InvalidState`。単なるconfigureでは復旧しない。追加時間には時刻の量子化・有限のMMIO処理とIRQ latencyが加わり、任意のISRを含む最大実行時間の証明ではない。

## 実装と検証

stdlib、BoardSpi、固定descriptorへの変換、共通driver、両BSP、2デバイスの使用例を提供する。[検証手順](../spi-validation.md)にhost生成C・ASan/UBSan付きレジスタモデル・CLI/firmware buildの範囲を記録する。実機のclock/sample edge、電気的なbit順、GPIO matrixの入力遅延、最大stackは未確認である。

共有bridgeは基本転送の共通word範囲と返却長の上限を検査する。短いreceive/transferを成功として返さないことと、設定幅に収まることはBSPが保証する。

## SpiDeviceとtransactionによるCS管理

基本のSPI・GPIO APIを維持し、デバイスごとの設定とCSをまとめる標準APIを提供する。利用者が初期化時にCS用pinを指定し、通常の通信は接続先を表す`SpiDevice`と、その接続先への一連の通信を表す`transaction`を通して行えるようにする。基本SPI APIを直接使い、利用者が独自にCSを管理する経路も維持する。

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

例えば初期化済みの`sensor`へ`[Write([0x80]), Read(2), Transfer([0x00])]`を渡すと、成功結果は長さ2と長さ1の受信Listを順に持つ。`SpiDeviceConfig(bus, config, cs, active, setup_us, hold_us)`を`(use Spi).create_device(config)`へ渡し、初期化済みの`SpiDevice(slot)`を取得する。未知・未初期化のslotは拒否する。slotは別のdeviceの設定を表す値ではなく、BSPの固定表にある初期化済み資源を参照するhandleである。

CSを選択する前に、全操作の種類・方向・値の幅・size・timeout、操作数、送受信の総word数を検証する。容量は操作ごとだけでなくtransaction全体に適用し、上限を生成設定とheaderに記録する。総数計算の桁あふれも拒否する。操作数、合計送信word数、合計受信word数はそれぞれ`peripheral_buffer_capacity`を上限とする。headerの`EMELA_SPI_TRANSACTION_OPERATION_CAPACITY`と`EMELA_SPI_TRANSACTION_WORD_CAPACITY`へ記録する。deviceは固定4件（`EMELA_SPI_DEVICE_CAPACITY`）。固定容量のC descriptor、送信・受信buffer、結果のoffset/length領域を準備してから通信に入る。検証・容量不足の失敗ではCSもclockも操作しない。

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

発生元と併存をenum・固定サイズのfieldで区別できる型にし、後の解除失敗で最初の失敗を上書きしたり、両方を1つの`Other`へ潰したりしない。C境界にも主結果とcleanup結果を別のstatus欄として設ける。`primary`は`Option[SpiTransactionFailure]`（`SpiFailure(kind)` / `GpioFailure(kind)`）、`cleanup`は`Option[GpioErrorKind]`。CS解除だけの失敗では`primary = None`、`cleanup = Some(kind)`となる。

CS解除に失敗したバスは復旧が必要な状態へ置き、新たな転送を`InvalidState`で拒否する。`(use Spi).recover(bus)`でcontroller停止・予約済みの全CSの非選択を確認するまで再利用せず、単なるバスlock解放や再configureだけで復旧済みにしない。借用bufferへの参照は失敗時も終了する。部分的な受信結果を成功として返さず、自動再送もしない。

結果Listの確保はCS解除とバス解放の後に行うため、そこでのheap不足がCSを選択したままにすることはない。runtimeのdefect全般に通常のcleanupを保証するものではなく、defect時の出力方針はBSPの契約に従う。

#### BSPとISRへの接続

操作列を先に渡し、BSPは最終操作を判別できる形にする。途中のoperation完了でCSを解除せず、低レベルの1 operationの完了とCS解除を含むtransactionの完了を区別する。C境界には固定レイアウトのdescriptor・bufferを渡し、Emelaの`SpiOperation`やListの内部レイアウトを公開しない。

ISRでCSを操作する実装には、[BSP提供のISR専用handlerを静的に接続する方針](uart-interface.md#bspによるisr専用handlerの静的供給)を適用する。pin・操作列・bufferをmain側で準備し、ISRにListの構築・参照管理・解放を要求しない。初版の完成条件は同期transactionの契約であり、最終ISRからのCS解除を必須の実装方法にはしない。

上位APIと資源管理を両BSPへ接続した。BSP内部は同期pollingで実行し、ISRやfiberへ転送の進行を委ねない。失敗した初期化の予約も、安全なCS解除を確認したrecoverの後だけ解放する。

## その他の拡張候補

Hz指定のclock設定、可変dummy cycle、上記の操作列を超える任意の送受信phase列、非同期・DMAは別の拡張として扱う。現行の同期operationへ暗黙に追加しない。

## 参照

- [ST公式STM32F4 SPI定義](https://github.com/STMicroelectronics/stm32f4xx-hal-driver/blob/master/Inc/stm32f4xx_hal_spi.h)：mode、direction、data size、CPOL/CPHA、prescaler、bit順。
- [embedded-halのSPI busとdevice](https://docs.rs/embedded-hal/latest/embedded_hal/spi/index.html#bus-vs-device)：バスとCS付き接続先の区別、transaction中のCS保持とバスの排他、CSのtiming設定。
- [ESP-IDF ESP32-C3 SPI Master](https://docs.espressif.com/projects/esp-idf/en/stable/esp32c3/api-reference/peripherals/spi_master.html)：SPI modeと信号、CSとtransactionの区別。Emelaの現行APIはCSをGPIOで制御する。

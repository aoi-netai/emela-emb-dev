# I²Cの機能一覧と操作契約

状態：同期master操作、I²C専用エラー、静的割り込みbindingをstdlib・C bridge・STM32F4/ESP32-C3 BSPへ実装済み。検証は[host・レジスタモデル検証](i2c.md#検証)を参照する。実機の電気特性と通信timingの確認は未実施。以下の「〜する」「〜しない」はhandlerとBSPの操作契約である。

共通の型変換・所有権・容量検査は[標準周辺機器bridge](common.md#c-bridge)、アプリとボードの責務は[組み込みインターフェース](README.md)に従う。[機能一覧](README.md)と[共通の操作契約](common.md)も参照する。この文書はI²C固有の契約を追加するものであり、UART・SPIの意味を変更しない。

## 機能一覧

| 機能 | API | 内容 |
| --- | --- | --- |
| バス設定 | `configure` | clockとduty cycleを設定する。デバイスアドレスは未シフトの7bit値 |
| master送受信 | `master_transmit`／`master_receive` | 1つの接続先へ同期転送する |
| レジスタアクセス | `memory_write`／`memory_read` | 8/16bitのレジスタアドレス、readのRepeated START |
| 応答確認 | `is_device_ready` | 試行全体で共通の期限を使い、address ACK/NACKを確認する |
| callbackの接続 | `bind_interrupt` | バスIDと名前付き関数を静的に対応付ける |
| 割り込み制御 | `enable_interrupt`／`disable_interrupt` | 転送全体の成功・失敗の通知を制御する |

## 確定したアドレス方針

1. デバイスアドレスは、R/W bitを含まない未シフトの7bit値に固定する。
2. 初版のAPIに`I2cAddressing`と`I2cConfig.addressing`を設けない。
3. 10bit対応は、具体的に必要な部品が出てきた時点で追加する。

デバイスアドレスと、機器内部のレジスタ・メモリアドレスは別である。`MemoryAddressSize.Bits8`と`Bits16`は維持する。

## 対象範囲

- controller（API名ではmaster）による同期送受信、8/16bitのレジスタアドレスを使うアクセス、接続先の応答確認を扱う。
- Standard-modeとFast-modeを対象にする。target/slave動作、非同期・DMA API、任意のメッセージ列、一般コール等の予約アドレス用途は対象外とする。
- デバイスアドレスは7bitのみとする。BSPが対応するバス、速度、duty cycleの組合せを文書化し、非対応の設定をエラーにする。
- 既存の同期operationはmain contextから呼び出す。呼び出し中にEmelaの別fiberへ制御を渡さず、ISRから呼び出さない。BSPのsoftware IRQからユーザー関数を直接呼ぶ、転送完了・通信異常の通知を提供する。

## Emela API

宣言元は[`Std.Embedded.I2c`](../../../emela-core/lib/std/embedded/i2c.emel)、共有handlerは[`Std.Embedded.Board.BoardI2c`](../../../emela-core/lib/std/embedded/board.emel)である。同期転送6operationに加え、`enable_interrupt(id, priority)`と`disable_interrupt(id)`を提供する。bindingとエラー型は後述する。

```emela
import Std.Embedded.Interrupt.{InterruptPriority}

pub enum I2cId { I2c1, I2c2, I2c3 }
pub enum I2cDutyCycle { Standard, Fast16To9 }
pub enum MemoryAddressSize { Bits8, Bits16 }
pub type I2cConfig(clock_speed_hz: Int, duty_cycle: I2cDutyCycle)

pub effect I2c {
  fn configure(id: I2cId, config: I2cConfig) -> () fails I2cError
  fn master_transmit(
    id: I2cId, address: Int, data: List[Int], timeout_ms: Int64
  ) -> () fails I2cError
  fn master_receive(
    id: I2cId, address: Int, size: Int, timeout_ms: Int64
  ) -> List[Int] fails I2cError
  fn memory_write(
    id: I2cId, address: Int, memory_address: Int,
    address_size: MemoryAddressSize, data: List[Int], timeout_ms: Int64
  ) -> () fails I2cError
  fn memory_read(
    id: I2cId, address: Int, memory_address: Int,
    address_size: MemoryAddressSize, size: Int, timeout_ms: Int64
  ) -> List[Int] fails I2cError
  fn is_device_ready(
    id: I2cId, address: Int, trials: Int, timeout_ms: Int64
  ) -> Bool fails I2cError
  fn enable_interrupt(id: I2cId, priority: InterruptPriority) -> () fails I2cError
  fn disable_interrupt(id: I2cId) -> () fails I2cError
}
```

### バスと設定

`I2cId`は論理バスIDであり、MCUの周辺機器番号やC enumの整数値を直接表さない。BSPが各IDから物理controller、SCL/SDA pin、alternate function、clock sourceへの対応を定める。例えば、物理的にI2C0という名前のcontrollerを`I2c1`へ対応させてもよい。

`configure`は指定バスを初期化し、以降の操作に適用する設定を保持する。未設定のバスへの転送・応答確認、存在しないバス、予約pinとの競合は`I2cError`とする。再設定は許可する。引数検証で失敗した場合は既存設定を変更せず、ハードウェアの再設定途中で失敗した場合は未設定として扱う。

| 設定 | 契約 |
| --- | --- |
| `clock_speed_hz` | 正のHz値。100,000以下をStandard-mode、100,000超〜400,000以下をFast-modeとして扱う。範囲外・実現できない設定は拒否する。分周による丸めが必要な場合は要求値を超えない名目周波数を選び、丸め方をBSPに記載する。 |
| `Standard` | 現行名を維持するための互換定義。Standard-modeではそのモードのtimingを使用し、Fast-modeではSCLのLow:Highを2:1とする。100 kHz以下に限定する値ではない。 |
| `Fast16To9` | Fast-modeでSCLのLow:Highを16:9とする。Standard-modeとの組合せや、この設定を実現できないBSPでは拒否する。 |

`Standard`の名称は速度モードと紛らわしいため、将来のAPI変更候補とする。この仕様ではenumの名前・順序を変更せず、上の意味を明示する。SCL/SDAの配線、外付けpull-up等の条件はBSPごとに記載する。

### デバイスアドレス

`address`は、R/W bitを含まない、左シフトしていない7bitの接続先アドレスである。例えばアドレス`0x40`の機器には`64`を渡す。アプリや共有handlerが`0x80`へ変換しない。

通常の個別デバイスアドレス`0x08`〜`0x77`を受け入れる。`0x00`〜`0x07`と`0x78`〜`0x7f`は予約用途として拒否する。7bitに収まらない値も拒否し、値の大きさによって10bit形式へ切り替えない。

範囲に収まらない値をマスク・切り詰めて送信しない。R/W bitの生成、下位HALが要求するシフト等はBSP内部で処理する。全operationで同じ表現を使う。

### データとレジスタアドレス

- I²Cの転送要素は1 byteで、`data`と受信結果の各要素は0〜255とする。文字列として解釈しない。
- C ABIの転送領域は共通bridgeに合わせて`uint16_t`配列を使う。各wordに1 byteを格納し、上位8bitは0にする。wordを2 byteへ分解して送信しない。
- 送信要素の範囲をバス操作前に全て検証する。256以上を下位8bitへ切り詰めない。
- `size`は受信byte数で、0以上とする。成功時の受信結果は必ず`size`要素になる。途中まで受信して失敗した場合はエラーとし、短いListを成功として返さない。
- `memory_address`は`Bits8`なら0〜255、`Bits16`なら0〜65,535とする。`Bits16`は上位byte、下位byteの順に送信する。範囲外は拒否する。
- データ本体のbyte順、レジスタの自動increment、bank切り替え等は接続先デバイスの仕様に従い、標準handlerは変換しない。

共有bridgeは0〜65,535のword範囲と転送容量を検査する。I²C固有の8bit範囲、アドレス、設定、timeout等の検証はBSPが行う。独自のEmela handlerも、同じ公開契約に従う。

### timeoutとゼロ長転送

`timeout_ms`は各operation全体の待機期限をミリ秒で指定する。バスが空くまでの待機、address phase、データ転送、Repeated STARTを含め、byteやphaseごとに期限を延長しない。`is_device_ready`では全試行に共通の期限とする。

- 負値は不正な引数として拒否する。無期限待ちを表す特別な値は設けない。
- 0は待機しない指定とする。非空転送と`is_device_ready`は新たな通信を必要とするため、STARTを発行する前に`Timeout`を返す。空転送は下記の検証後に成功する。
- BSPは単調増加する時刻源で経過時間を測る。`Clock.now`のUnix epochやRTCの時刻補正に依存しない。
- 下位HALの整数幅やtick単位へ変換する場合、桁あふれや切り詰めで期限を変えない。表現できない値は操作前に拒否する。
- 期限切れ後のSTOP生成・controllerの停止等の後処理も待機を有界にし、その追加時間の上限をBSPに記載する。`timeout_ms`は物理的な最大実行時間の証明値ではない。

空の`data`を渡す送信と`size == 0`の受信は、ID・設定・アドレス・レジスタアドレス・timeoutを検証した後、バスを操作せず成功する。受信結果は`[]`とする。これは`memory_write`と`memory_read`にも適用し、レジスタアドレスだけを送信しない。応答確認には`is_device_ready`を使う。

## operationの通信手順

以下の図は7bit形式を示す。`S`はSTART、`Sr`はRepeated START、`P`はSTOP、`A`はACK、`N`はNACKを表す。`Addr(W/R)`は、7bitの接続先アドレスとR/W bitを組み合わせた1 byteである。

### master_transmit

```text
S Addr(W) A data[0] A ... data[n-1] A P
```

全byteの送信・ACK確認とSTOP生成まで成功して`()`を返す。接続先からのaddress/data NACKは`I2cError`とし、自動再送しない。

### master_receive

```text
S Addr(R) A data[0] A ... data[n-1] N P
```

controllerは最終byteにNACKを返して受信を終了する。この最終NACKは正常な手順であり、転送エラーではない。要求byte数を受信し、STOPを生成してからListを返す。

### memory_write

```text
S Addr(W) A register-address A data[0] A ... data[n-1] A P
```

`Bits16`の`register-address`は2 byteで、各byteの後にACKを確認する。レジスタアドレスとデータの間にSTOPを入れない。成功はバス上の転送完了を意味し、EEPROM等の内部書込み完了を保証しない。

### memory_read

```text
S Addr(W) A register-address A Sr Addr(R) A data[0] A ... data[n-1] N P
```

レジスタアドレスを送信し、STOPを挟まずRepeated STARTで受信へ切り替える。BSPはこれを1つの操作として扱い、途中で同じバスの別操作を実行しない。独立した`master_transmit`と`master_receive`をSTOP付きで呼ぶ実装では代用しない。

### is_device_ready

```text
ACK時:  S Addr(W) A P
NACK時: S Addr(W) N P
```

データを送らず、書込み方向のaddress phaseに対する応答を確認する。デバイス固有の読出し可否や、内部処理の完了を一般的に保証する操作ではない。

`trials`は正の最大試行回数である。最初の試行を1回と数え、address NACKの場合だけSTOP後に再試行する。各試行で期限を作り直さない。

| 結果 | Emelaでの結果 | C ABI |
| --- | --- | --- |
| いずれかの試行でACK | `True` | status 0、`*out = 1` |
| 全`trials`回でaddress NACKを確認 | `False` | status 0、`*out = 0` |
| 試行完了前の期限切れ、バス異常、不正な引数、非対応設定 | `I2cError(kind)` | 非0 status。`*out`は使わない |

無応答の原因をNACKとして確認できないtimeoutやbus busyを、`False`へまとめない。0以下の`trials`は試行前に拒否する。

## エラーと終了時の状態

全operationとbindingは`I2cError(kind)`で回復可能な失敗を返す。C status 0は成功、非0は失敗であり、共有handlerが次の共通分類へ変換する。未知の非0は`Other`になる。

| C status | 分類 |
| --- | --- |
| -1 / -2 | `InvalidArgument` / `CapacityExceeded` |
| -3 / -4 / -5 | `NotConfigured` / `Unsupported` / `Timeout` |
| -6 / -7 / -8 | `Busy` / `ResourceConflict` / `InvalidState` |
| -9 | `Other` |
| -13 / -14 / -15 | `Nack` / `BusError` / `ArbitrationLost` |

数値・C定数・Emela分類は[単一定義表](../../src/backend/board/peripheral_status.rs)から生成する。GPIO/UART固有のstatus -10〜-12はI²Cでは`Other`となる。共有bridgeで負のsize・共通word範囲外・容量超過を検出した場合はBSPを呼び出さない。I²C固有の不正引数・非対応・未設定もバス操作前に拒否する。

通信途中の失敗で、それまでに接続先へ送ったbyteや副作用は取り消せない。BSPと共有handlerは通常の転送を自動再送しない。BSPはバスの所有状態に応じてSTOP・controller停止・ライン解放を行う。arbitration lost時には他のcontrollerの通信を妨げるSTOPを発行しない。バスが回復しない場合は、次の操作も期限内にエラーを返す。独自のbus recoveryが必要なら、その手順をBSPに記載する。

### I²C専用エラー

UART・GPIOと同様に、`Std.Embedded.I2c`で定義する専用の`I2cError`を使う。ボード間で分類の意味を揃え、細かい機種固有エラーは`Other`へまとめる。HAL固有のcodeをそのまま利用者へ公開しない。

同期転送6operation、設定・binding・割り込み制御の回復可能な失敗を、次の型で表す。

```emela
pub enum I2cErrorKind {
  InvalidArgument
  CapacityExceeded
  NotConfigured
  Unsupported
  Timeout
  Busy
  ResourceConflict
  InvalidState
  Nack
  BusError
  ArbitrationLost
  Other
}
pub error I2cError(kind: I2cErrorKind)
```

共通分類は[エラー定義の共通化](common.md#エラー定義の共通化)に従う。`Nack`は転送中に接続先から受けたaddress/data NACK、`BusError`は不正なバス状態・通信手順の異常、`ArbitrationLost`はバス調停の喪失を表す。正常な最終受信byteへのNACKは含めない。`is_device_ready`で全試行のaddress NACKを確認した場合に`False`を返す契約は維持する。

共通statusの単一の定義表からC定数とEmela側の変換を生成し、stdlib・共有handler・生成C ABI・BSPを合わせて更新する。0は成功、-1は`InvalidArgument`、-2は`CapacityExceeded`とし、BSPも-1/-2を返せる。未知の非0 statusは`Other`へ変換する。

controller・pin・IRQの予約と状態は[共通の固定資源表と状態遷移](common.md#状態遷移と通知の確定)に従う。割り込み有効中の再設定は`InvalidState`、進行中の操作との競合は`Busy`、予約資源との競合は`ResourceConflict`とする。設定、binding、割り込み有効化の順に初期化し、再設定前には停止して実行中callbackとbuffer参照の終了を確認する。

## 割り込みとハンドラーバインド

同期APIを維持し、転送全体の完了・通信異常を静的binding先へ通知する。[UARTのハンドラーバインド](uart.md#割り込みとハンドラーバインド)と同じ方式で、BSPのドライバーのISRから、bindingしたユーザーのEmela関数を直接呼ぶ。mainへのqueue配送やfiberの起動・再開を経由しない。

### 転送手順の隠蔽と通知単位

| 内容 | 担当 |
| --- | --- |
| byteごとの送受信、ACK/NACK制御、通信段階の移行、Repeated START・STOP等の手順 | BSPのI²Cドライバー内部。利用者向けイベントとして公開しない。 |
| 要求した転送全体の完了、転送中のNACK・bus error・arbitration lost等の通信異常 | I²Cにbindingしたユーザー関数へ通知する。 |
| センサー等の「測定データができた」という外部通知端子 | [GPIOの割り込み](gpio.md#割り込みハンドラーの静的binding)として扱う。 |

利用者は接続先アドレス・送信データ・受信数等を指定し、割り込みごとの状態遷移を記述しない。1回の要求を処理するためにBSP内部で何度割り込みが発生しても、利用者へは転送全体の結果を通知する。`memory_read`はレジスタアドレス送信・Repeated START・受信をまとめて1件とし、中間段階の完了を別の転送完了として通知しない。

受信の最終byteにcontrollerが返す正常なNACKは、通信異常として通知しない。`is_device_ready`の試行ごとのaddress NACKも内部で処理し、試行ごとの通知を利用者へ要求しない。確認済みNACKによる`False`と、通信異常による失敗を区別する。

### イベントと最終結果

イベントとbindingは次の形を使う。`configure`は転送結果の通知対象に含めない。

```emela
pub enum I2cOperationKind {
  MasterTransmit
  MasterReceive
  MemoryWrite
  MemoryRead
  IsDeviceReady
}
pub enum I2cEvent {
  TransferComplete(operation: I2cOperationKind)
  TransferError(operation: I2cOperationKind, kind: I2cErrorKind)
  DeviceReady(ready: Bool)
}

pub fn bind_interrupt[R: Immediate](
  id: I2cId,
  handler: fn(I2cId, I2cEvent) -> () fails {} use R
) -> () fails I2cError
```

通知が有効な場合、1要求に対する最終結果を次のように通知する。`TransferComplete`は送受信・memory操作の4種類に使い、応答確認の正常終了は`DeviceReady`で表す。

| 条件 | 通知 |
| --- | --- |
| 引数・設定・容量・所有状態の検証失敗、未対応要求 | なし。同期呼び出しだけが失敗する。 |
| 空転送・受信size 0 | なし。検証後に同期呼び出しが成功する。 |
| `timeout_ms == 0`による開始前の拒否 | なし。同期呼び出しが`Timeout`で失敗する。 |
| 開始した送受信・memory操作の成功 | 対応するoperationの`TransferComplete`を1回。 |
| 応答確認のACK、または全試行で確認済みaddress NACK | 全試行をまとめて`DeviceReady(True)`または`DeviceReady(False)`を1回。 |
| 開始した要求のtimeout・通信異常・後処理の失敗 | 対応するoperationと同期結果と同じ分類の`TransferError`を1回。 |

要求の開始とは、検証とcontrollerの取得が成功し、バス空き待ちを含む通信処理へ進んだ時点を指す。バス空き待ち中のtimeoutも開始済み要求の失敗となる。試行ごと・phaseごとの通知は行わない。bindingがない、または通知を停止している場合はcallbackを呼ばず、同期APIだけで同じ成功・失敗を返す。

完了IRQとtimeoutは1つの完了状態を更新し、成功と失敗のどちらかへ一度だけ確定する。後処理を終えて同期結果を確定してから通知し、旧要求のpendingを次の要求へ混入させない。最初の通信失敗がある場合はその分類を保持し、後処理だけが失敗した場合は後処理の分類で失敗とする。ラインを安全に再利用できなければ、結果とは別にcontrollerを復旧が必要な状態へ置く。

timeout等に対応するハードウェア完了IRQがない場合は、BSPの固定サイズ完了recordとtimerまたはsoftware IRQを使ってISRへ通知する。recordは要求ごとに一度だけ消費し、mainからユーザーcallbackを直接呼んで代用しない。通知が有効な要求はcallback終了後に同期operationから返す。通知IRQの進行条件と停止・通知の追加時間をBSPに記録し、IRQを無効にしたまま完了を待つ構成を避ける。

### 同期呼び出しとISRの関係

同期operationの呼び出し元は、転送・必要な後処理・有効な最終通知のcallbackが終了するまで待つ。BSP内部で割り込みを使うことやユーザー関数をbindingすることだけでは、呼び出し元を転送中に進めるAPIへ変更しない。bindingした関数はISR内で直接動作できるため、転送完了を受けてGPIO出力を変更する等の処理に利用できる。

転送結果のList構築と同期operationの成功・失敗の返却は、通常の呼び出し側の経路で行う。割り込み通知を受け取ったことを理由に、同期operationが返すエラーを消費・無視しない。ISRへ受信Listや所有bufferを渡さず、イベントはID・数値・固定サイズの値型等で構成する。同期APIのbufferは呼び出し中だけ借用し、復帰までにISRを含むbuffer参照を終了する。

GPIO等のユーザーISRから既存の同期I²C転送を呼ぶことは、待機を伴うため引き続き禁止する。転送を開始してすぐ戻るAPIが必要になった場合は、完了・取消まで保持できる固定bufferとアクセス規則を含め、別の拡張として設計する。

### 静的bindingとISR制約

- binding専用の関数を設け、論理`I2cId`とcaptureのない名前付き関数を静的に解決する。UARTと同じく同じバスへの重複bindingを拒否し、動的な関数登録へ切り替えない。
- [BSPによるISR専用handlerの静的供給](uart.md#bspによるisr専用handlerの静的供給)を共通に適用する。必要な資源を割り込みの有効化前に準備し、無効化後も実行中のISRがアクセスを終えるまで保持する。
- ユーザー関数には`fails {}`と`IsrSafe`に相当する制約を適用する。heap・参照管理・再帰・suspend・Clock系effect・待機等を禁止し、BSPが提供するISR用操作だけを静的に接続する。
- 優先度はUART・GPIOと共通の`Low`・`Normal`・`High`の方針に従う。同じバスへの同期呼び出しや再設定をISRから再入させない。

[UARTと共通の静的binding](uart.md#割り込みとハンドラーバインド)をI²Cにも適用する。型検査後に指定を収集し、定数・不変な別名を辿って論理バスIDを解決する。プログラム全体で同じバスへの重複を検出し、通常コードから呼ばれないprivateなユーザー関数も保持してISR検査を自動適用する。BSP向けの生成入口は転送全体の通知契約に合わせて具体化する。

bindingの実行時検証が成功する前には通知を有効化しない。停止後はcallbackや借用buffer参照が残らず、既に停止している場合の停止は成功する。`enable_interrupt(id, priority)`と`disable_interrupt(id)`を使う。優先度は`Std.Embedded.Interrupt.InterruptPriority`の`Low`・`Normal`・`High`。`Immediate`は公開署名の制約であり、ISR安全性はbackendの到達検査で検証する。

## C ABI

生成された`emela_i2c.h`を宣言の正とする。引数名を補った現在のprototypeは次のとおりである。

```c
int32_t emela_i2c_configure(EmelaI2cId id, EmelaI2cConfig config);
int32_t emela_i2c_master_transmit(
    EmelaI2cId id, int32_t address,
    const uint16_t *data, uint16_t length, int64_t timeout_ms);
int32_t emela_i2c_master_receive(
    EmelaI2cId id, int32_t address, int32_t size, int64_t timeout_ms,
    uint16_t *out, uint16_t capacity, uint16_t *length);
int32_t emela_i2c_memory_write(
    EmelaI2cId id, int32_t address, int32_t memory_address,
    EmelaI2cMemoryAddressSize address_size,
    const uint16_t *data, uint16_t length, int64_t timeout_ms);
int32_t emela_i2c_memory_read(
    EmelaI2cId id, int32_t address, int32_t memory_address,
    EmelaI2cMemoryAddressSize address_size, int32_t size, int64_t timeout_ms,
    uint16_t *out, uint16_t capacity, uint16_t *length);
int32_t emela_i2c_is_device_ready(
    EmelaI2cId id, int32_t address, int32_t trials,
    int64_t timeout_ms, int32_t *out);
int32_t emela_i2c_bind_interrupt(int32_t id);
int32_t emela_i2c_enable_interrupt(EmelaI2cId id, EmelaInterruptPriority priority);
int32_t emela_i2c_disable_interrupt(EmelaI2cId id);
int32_t emela_i2c_has_binding(int32_t id);
void emela_i2c_transfer_complete(int32_t id, int32_t operation);
void emela_i2c_transfer_error(int32_t id, int32_t operation, int32_t status);
void emela_i2c_device_ready(int32_t id, int32_t ready);
```

operationの公開C定数は`EMELA_I2C_OPERATION_MASTER_TRANSMIT`=1、`MASTER_RECEIVE`=2、`MEMORY_WRITE`=3、`MEMORY_READ`=4、`IS_DEVICE_READY`=5。Emela enumの内部tagから独立して変換する。`TransferComplete`は1〜4、`TransferError`は1〜5と非0 status、`DeviceReady`は0/1だけを受け入れる。未binding IDへの配送は行わない。

- `length`・`size`・`capacity`は配列のword数であり、このI²C仕様では転送byte数と一致する。レジスタアドレスのprefixは転送Listやその容量に含めない。
- 配列は呼び出し中だけ借用する。BSPはpointerを保存せず、返却時には割り込み・DMAを含め配列を参照する処理を終了する。
- 受信成功時は`size <= capacity`を満たし、`out[0..size]`を初期化し、`*length = size`を設定する。各要素を0〜255にする。失敗時に出力をList化しない。
- Listの構築・固定heapでの所有権管理は共有bridgeが行う。BSPがEmelaのList nodeを確保・解放しない。List用heapの容量不足は共通runtimeのdefect契約に従う。

通知入口はUARTと同じくイベント別に生成し、転送種別・共通エラーstatus・応答確認のBoolを基本型引数で渡す。生成bridgeが論理`I2cId`を補い、固定サイズの`I2cEvent`を構築する。受信ListやEmela enumの内部レイアウトは公開C ABIへ渡さない。転送種別のC定数は生成headerで定義し、Emela enumの内部値を流用しない。

## 初期対応BSP

| BSP | 論理ID / controller | SCL / SDA | clock・対応設定 | 通知IRQ |
| --- | --- | --- | --- | --- |
| STM32F407VG/F412ZG/F429ZI/F446REとそのboard alias | `I2c1` / I2C1 | PB6 / PB7、AF4 open drain | APB1 16 MHz。Standard/Fast、2:1/16:9 | I2C1_EV（IRQ31）のsoftware pending |
| ESP32-C3 | `I2c1` / I2C0 | GPIO6 / GPIO7、GPIO matrix、open drain | XTAL 40 MHz。Standard/Fast、2:1/16:9 | FROM_CPU_INTR0（source 50）→CPU IRQ4 |

両BSPとも`I2c2`・`I2c3`は`Unsupported`。SCL/SDAには外付けpull-upと共通GNDが必要。配線容量・抵抗・接続先の速度制約はアプリの基板設計で確認する。pinは初期化後に予約し、GPIOで先に設定されている場合も後からGPIOへ変更する場合も`ResourceConflict`で拒否する。未復旧でもpin予約を維持する。

STM32F4はCCRを`ceil(16 MHz / (要求Hz × factor))`で選ぶ。factorはStandard-mode=2、Fast 2:1=3、Fast 16:9=25。CCRが12bitへ収まらない低速設定等は`Unsupported`。TRISEはStandard=17、Fast=5。ESP32-C3はXTALの整数分周（1〜256）とSCL Low/High期間を切り上げ、2:1または16:9を保つ。各registerに収まらない設定は`Unsupported`。どちらも名目速度は要求Hzを超えない。実際の立上り・glitch filter・配線・clock誤差による速度は別途実測が必要。

転送段階はmainでpollし、同じ期限でバス待機・address・全data・Repeated STARTを処理する。STM32F4は最終1/2/3 byteでACK/POS/STOPの手順を分ける。ESP32-C3は32byte FIFO単位のcommand batchを`END`で接続し、最後だけNACKとSTOPを生成する。HALへのtimeout幅変換を行わず、非負の`Int64`をそのまま扱う。

STOP・controller停止は追加2msの期限（ms量子化により実時間では3ms未満、IRQによる遅延を除く）で終了させる。既存の通信失敗があればその分類を保持し、後処理だけの失敗はその分類を返す。再利用できなければ`InvalidState`とし、通知停止後の`configure`で復旧を試みる。外部機器がラインを保持している場合にclock pulseを強制送出するbus recoveryは行わない。arbitration lostではSTOPを出さない。

通知には固定サイズのscalar recordを1個使い、buffer pointerを保存しない。後処理と同期結果の確定後にsoftware IRQをpendし、callback終了までmainで待つ。通知停止時にはIRQと旧pendingを消去する。main APIはISR内・IRQ mask中・event lock中の実行を禁止する。通知IRQを有効にし、上位IRQが継続してCPUを占有しないことが進行条件である。通知の追加時間はIRQ latencyとcallbackの実行時間であり、転送timeoutには含めない。ユーザーISRは有界に実行し、独自の`isr_safe_externals`もこの条件を守る。

## 現在の実装と確認

stdlib、BoardI2c、生成C ABI、静的binding、共通状態機械、両BSPのdriverを実装した。[検証範囲と再現手順](i2c.md#検証)に、host生成C、ASan/UBSan、実BSPソースを使うレジスタモデル、C構文検査を記録する。5 MCU × O0/O2のcross build/linkとimage生成も確認した。実機通信、SCL波形、割り込みlatency・最大stackは未確認であり、hostモデルの成功をその代わりにはしない。

共有bridgeは共通word範囲と返却lengthの上限を検査する。要求数すべての成功、8bitの送受信範囲はBSPが保証する。prefixだけの空転送や短いListの成功は返さない。

## 後続のAPI検討

専用エラーと転送単位の静的bindingは初版に含む。以下は追加の拡張候補である。

- 10bitデバイスアドレス。具体的に必要な部品が出てきた時点で、公開API・C ABI・BSPへの追加を検討する。
- 任意長のコマンド列を書いてRepeated STARTで読む`write_read`。現行`memory_read`が扱うprefixは8/16bitのレジスタアドレスに限定する。
- duty cycle設定の名称、またはハードウェア固有のtiming設定を共通configから分離する設計。
- 非同期・DMA向けの操作と、完了・取消まで保持できるbufferの契約。

これらの拡張候補は現行の公開operationやC ABIには追加しない。

## 参照

- [NXP UM10204 Rev. 7.0](https://www.nxp.com/docs/en/user-guide/UM10204.pdf)：byte転送、ACK/NACK、予約アドレス、combined format、7bitアドレス、Standard/Fast-modeのtiming。
- [ST公式STM32F4 HAL I²C header](https://github.com/STMicroelectronics/stm32f4xx-hal-driver/blob/master/Inc/stm32f4xx_hal_i2c.h)：Fast-modeの2:1・16:9設定。HAL自体は同梱BSPの依存ではない。
- [ST公式STM32F4 HAL I²C source](https://github.com/STMicroelectronics/stm32f4xx-hal-driver/blob/master/Src/stm32f4xx_hal_i2c.c)：下位APIが7bitアドレスの左シフトを要求する例。
- [CMSIS-Driver I²Cインターフェース](https://arm-software.github.io/CMSIS_6/main/Driver/group__i2c__interface__gr.html)：転送開始と完了イベントの分離、通信異常の通知、転送中のbuffer保持の契約。
- [ESP-IDF ESP32-C3 I²C](https://docs.espressif.com/projects/esp-idf/en/stable/esp32c3/api-reference/peripherals/i2c.html)：R/W bitを含まないアドレスと、STOPを挟まないwrite/readの例。

## 検証

### hostでの再現

```sh
cargo test --locked -p emela-codegen-emb
cargo test --locked -p emela-core
cargo test --locked -p emela-cli --test embedded_cli
```

I²C単独では`cargo test --locked -p emela-codegen-emb --test emit embedded_peripherals::i2c`を使う。C11・C++17のhost compilerとPython 3が必要。

| 試験 | 確認範囲 |
| --- | --- |
| 生成CとEmela | 専用エラー全分類・未知status、static ID/const/関数別名、private callback、scalar ABIからの3イベント、operation code変換、ISR内のGPIO write、ISRでのheap確保0 |
| 言語機能と共通driverの統合 | EmelaからNACK・Timeoutを`escape`で捕捉して次の通信を成功させ、ISR→固定queue→mainで4件の最終結果を取得。応答確認3試行は1通知、失敗時の受信prefixをList化しないこと |
| compiler拒否 | 動的ID、lambda、重複binding、同期I²C再入、heap、GPIO/UART/I²Cのqueue producer競合、main producer競合 |
| 共通driver | バス操作前のアドレス・data・size・memory・設定・timeout検証、空転送、設定維持、失敗後の未設定、容量、短い成功を返さないこと、最初の通信異常の保持、復旧状態、通知停止と旧pending排除 |
| 応答確認・期限 | 初回ACK、NACK後のACK、全試行NACKのFalse、通信異常、trials 0、負timeout、開始前timeout 0、全phase・全試行で共通の期限、最終NACK時の期限切れをFalseにしないこと、Int64上限 |
| 最終通知 | 検証・空転送・timeout 0では無通知、開始済み要求は1回、応答確認の全試行を1回に集約、同期結果との一致、callback終了後の復帰。hostのISR代替入口で確認 |
| STM32F4のレジスタモデル | 実BSPのC sourceを実行。7bit address、Bits8/16 prefix、Repeated START、1/2/3/4/32/33/64byte受信、ACK/POSと最終NACK、NACK時のSTOP、address/data NACKを再送しないこと、arbitration lost時のSTOP抑止、STOP失敗と再設定、100kHz/400kHzと16:9のregister設定 |
| ESP32-C3のレジスタモデル | 実BSPのC sourceを実行。FIFO32byte境界、command batch間のENDでbusを保持、Repeated START、最終byteだけの正常NACK、STOP、NACK/調停/後処理失敗と回復、16:9 timing、START前の期限切れで不要なSTOPを出さないこと、52bit時刻counterのwrap |
| C構文検査 | センサー使用例から生成したCと両BSP全translation unitを`-std=c11 -Wall -Wextra -Werror -fsyntax-only`で確認 |

共通driverとレジスタモデルはASan/UBSan付きで実行する。レジスタモデルではCPU assemblyと時刻・MMIOを置換する。STOP後のcallbackを本物のNVIC/RISC-V trapで実行したという意味ではない。共通bridgeの従来mockは16bit wordの移送も検査し、I²C固有の8bit検査とは分ける。

### CLIとfirmware build

[センサー使用例](../../../../examples/i2c_sensor/README.md)を通常のCLIで検査・生成し、配布BSPでcross compile/linkしてfirmware imageを作る。実機への書込みは行わない。

```sh
cargo build --locked -p emela-cli
export ARM_TOOLCHAIN_PATH=/path/to/arm-none-eabi-toolchain
export EMELA_ESP32C3_TOOLCHAIN=/path/to/riscv32-esp-elf
export EMELA_ESPTOOL_PYTHON=/path/to/python-with-esptool-4.8.1
python3 crates/emela-codegen-emb/tools/i2c_build_matrix.py \
  --emela target/debug/emela \
  --output target/i2c-validation
```

既定はSTM32F407VG/F412ZG/F429ZI/F446RE・ESP32-C3 × O0/O2の10 case。生成C、board資産、compile/link log、firmware、resource reportとhashを出力し、総合`report.json`へまとめる。`--boards`・`--optimizations`で範囲を指定できる。

### 実機確認との境界

実機のSCL/SDA波形、pull-upと配線容量、clock精度、clock stretching、複数controllerの電気的調停、MCU errata、IRQ latency・優先度ネスト、実行時stack high-waterは未確認。レジスタモデルはdriverの手順と境界条件を確認するもので、実デバイスの全レジスタとtimingの証明にはしない。

実機では通常のSTART/Repeated START/STOPと最終NACK、100/400kHz、16:9、通信異常と失敗後の次の要求、ISRからのGPIO変更と同期結果の一致をlogic analyzer等で確認する。callbackと独自のISR専用FFIは有界に実行し、通知IRQの進行を妨げない構成を使う。

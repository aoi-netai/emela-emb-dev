# I²Cインターフェース仕様

状態：BSP実装に向けた操作契約と採用した実装計画。7bit固定のデバイスアドレス方針を維持し、[I²C専用エラーへの移行](#i²c専用エラーへの移行方針)と[転送単位の割り込みハンドラーバインド](#割り込みとハンドラーバインドの採用方針)を初版に含める。これらの改訂は未実装である。以下の「〜する」「〜しない」は、この仕様に準拠するhandlerとBSPへの要求である。現行実装の対応状況は末尾に記載する。

共通の型変換・所有権・容量検査は[標準周辺機器bridge](../embedded-peripherals.md)、アプリとボードの責務は[組み込みインターフェース](../embedded-interfaces.md)に従う。[仕様一覧と共通の操作契約案](README.md)も参照する。この文書はI²C固有の契約を追加するものであり、UART・SPIの意味を変更しない。

## 確定したアドレス方針

1. デバイスアドレスは、R/W bitを含まない未シフトの7bit値に固定する。
2. 初版のAPIに`I2cAddressing`と`I2cConfig.addressing`を設けない。
3. 10bit対応は、具体的に必要な部品が出てきた時点で追加する。

デバイスアドレスと、機器内部のレジスタ・メモリアドレスは別である。`MemoryAddressSize.Bits8`と`Bits16`は維持する。

## 対象範囲

- controller（API名ではmaster）による同期送受信、8/16bitのレジスタアドレスを使うアクセス、接続先の応答確認を扱う。
- Standard-modeとFast-modeを対象にする。target/slave動作、非同期・DMA API、任意のメッセージ列、一般コール等の予約アドレス用途は対象外とする。
- デバイスアドレスは7bitのみとする。BSPが対応するバス、速度、duty cycleの組合せを文書化し、非対応の設定をエラーにする。
- 既存の同期operationはmain contextから呼び出す。呼び出し中にEmelaの別fiberへ制御を渡さず、ISRから呼び出さない。改訂では、BSPのISRからユーザー関数を直接呼ぶ、転送完了・通信異常の通知を追加する。

## Emela API

宣言元は[`Std.Embedded.I2c`](../../../emela-core/lib/std/embedded/i2c.emel)、共有handlerは[`Std.Embedded.Board.BoardI2c`](../../../emela-core/lib/std/embedded/board.emel)である。以下は現行の宣言であり、I²C専用エラーと割り込みbindingへの改訂はまだ含まない。

```emela
import Std.Embedded.Peripheral.{PeripheralError}

pub enum I2cId { I2c1, I2c2, I2c3 }
pub enum I2cDutyCycle { Standard, Fast16To9 }
pub enum MemoryAddressSize { Bits8, Bits16 }
pub type I2cConfig(clock_speed_hz: Int, duty_cycle: I2cDutyCycle)

pub effect I2c {
  fn configure(id: I2cId, config: I2cConfig) -> () fails PeripheralError
  fn master_transmit(
    id: I2cId, address: Int, data: List[Int], timeout_ms: Int64
  ) -> () fails PeripheralError
  fn master_receive(
    id: I2cId, address: Int, size: Int, timeout_ms: Int64
  ) -> List[Int] fails PeripheralError
  fn memory_write(
    id: I2cId, address: Int, memory_address: Int,
    address_size: MemoryAddressSize, data: List[Int], timeout_ms: Int64
  ) -> () fails PeripheralError
  fn memory_read(
    id: I2cId, address: Int, memory_address: Int,
    address_size: MemoryAddressSize, size: Int, timeout_ms: Int64
  ) -> List[Int] fails PeripheralError
  fn is_device_ready(
    id: I2cId, address: Int, trials: Int, timeout_ms: Int64
  ) -> Bool fails PeripheralError
}
```

### バスと設定

`I2cId`は論理バスIDであり、MCUの周辺機器番号やC enumの整数値を直接表さない。BSPが各IDから物理controller、SCL/SDA pin、alternate function、clock sourceへの対応を定める。例えば、物理的にI2C0という名前のcontrollerを`I2c1`へ対応させてもよい。

`configure`は指定バスを初期化し、以降の操作に適用する設定を保持する。未設定のバスへの転送・応答確認、存在しないバス、予約pinとの競合は`PeripheralError`とする。再設定は許可する。引数検証で失敗した場合は既存設定を変更せず、ハードウェアの再設定途中で失敗した場合は未設定として扱う。

| 設定 | 契約 |
| --- | --- |
| `clock_speed_hz` | 正のHz値。100,000以下をStandard-mode、100,000超〜400,000以下をFast-modeとして扱う。範囲外・実現できない設定は拒否する。分周による丸めが必要な場合は要求値を超えない名目周波数を選び、丸め方をBSPに記載する。 |
| `Standard` | 現行名を維持するための互換定義。Standard-modeではそのモードのtimingを使用し、Fast-modeではSCLのLow:Highを2:1とする。100 kHz以下に限定する値ではない。 |
| `Fast16To9` | Fast-modeでSCLのLow:Highを16:9とする。Standard-modeとの組合せや、この設定を実現できないBSPでは拒否する。 |

`Standard`の名称は速度モードと紛らわしいため、将来のAPI変更候補とする。この仕様案ではenumの名前・順序を変更せず、上の意味を明示する。SCL/SDAの配線、外付けpull-up等の条件はBSPごとに記載する。

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

全byteの送信・ACK確認とSTOP生成まで成功して`()`を返す。接続先からのaddress/data NACKは`PeripheralError`とし、自動再送しない。

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
| 試行完了前の期限切れ、バス異常、不正な引数、非対応設定 | `PeripheralError(code)` | 非0 status。`*out`は使わない |

無応答の原因をNACKとして確認できないtimeoutやbus busyを、`False`へまとめない。0以下の`trials`は試行前に拒否する。

## エラーと終了時の状態

現行の全operationは`PeripheralError(code)`で失敗を返す。C status 0は成功、非0は失敗であり、現行の共有handlerはcodeを変更しない。以下のcode契約は改訂前のものであり、I²C専用エラーへの移行時に更新する。

| 分類 | 契約 |
| --- | --- |
| 共有bridgeの不正引数 | code -1。負の受信size、共通word範囲外の要素等。BSPを呼び出さない。 |
| 共有bridgeの容量超過 | code -2。入力List・受信sizeの容量超過、返却lengthの超過。入力検証で検出した場合はBSPを呼び出さない。 |
| I²C固有の不正引数・非対応・未設定 | BSPが定める非0 code。バス操作前に検証する。 |
| 転送中のNACK・timeout・bus error・arbitration lost | BSPが定める非0 code。`is_device_ready`の確認済みaddress NACKだけは上のBool契約に従う。 |

BSPは現行契約の-1/-2を独自エラーcodeとして使わず、それ以外のcodeと原因の対応表を公開する。改訂後の共通分類とstatus対応は、次の移行方針に従って定める。

通信途中の失敗で、それまでに接続先へ送ったbyteや副作用は取り消せない。BSPと共有handlerは通常の転送を自動再送しない。BSPはバスの所有状態に応じてSTOP・controller停止・ライン解放を行う。arbitration lost時には他のcontrollerの通信を妨げるSTOPを発行しない。バスが回復しない場合は、次の操作も期限内にエラーを返す。独自のbus recoveryが必要なら、その手順をBSPに記載する。

### I²C専用エラーへの移行方針

2026-10-23時点の決定として、UART・GPIOと同様に、`Std.Embedded.I2c`で定義する専用の`I2cError`へ移行する。ボード間で分類の意味を揃え、細かい機種固有エラーは`Other`へまとめる。HAL固有のcodeをそのまま利用者へ公開しない。

既存の6operationと追加するbinding・割り込み制御の回復可能な失敗を、次の型で表す。これは改訂後の型であり、未実装である。

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

共通分類は[エラー定義の共通化](README.md#エラー定義の共通化)に従う。`Nack`は転送中に接続先から受けたaddress/data NACK、`BusError`は不正なバス状態・通信手順の異常、`ArbitrationLost`はバス調停の喪失を表す。正常な最終受信byteへのNACKは含めない。`is_device_ready`で全試行のaddress NACKを確認した場合に`False`を返す契約は維持する。

共通statusの単一の定義表からC定数とEmela側の変換を生成し、stdlib・共有handler・生成C ABI・BSPを合わせて更新する。0は成功、-1は`InvalidArgument`、-2は`CapacityExceeded`とし、改訂後はBSPも-1/-2を返せる。未知の非0 statusは`Other`へ変換する。

controller・pin・IRQの予約と状態は[共通の固定資源表と状態遷移](README.md#状態遷移と通知の確定)に従う。割り込み有効中の再設定は`InvalidState`、進行中の操作との競合は`Busy`、予約資源との競合は`ResourceConflict`とする。設定、binding、割り込み有効化の順に初期化し、再設定前には停止して実行中callbackとbuffer参照の終了を確認する。

## 割り込みとハンドラーバインドの採用方針

2026-10-23時点の決定として、初版は既存の同期APIを維持し、転送全体の完了・通信異常を通知する静的bindingを追加する。[UARTのハンドラーバインド](uart-interface.md#割り込みとハンドラーバインド)と同じ方式で、BSPのドライバーのISRから、bindingしたユーザーのEmela関数を直接呼ぶ。mainへのqueue配送やfiberの起動・再開を経由しない。

### 転送手順の隠蔽と通知単位

| 内容 | 担当 |
| --- | --- |
| byteごとの送受信、ACK/NACK制御、通信段階の移行、Repeated START・STOP等の手順 | BSPのI²Cドライバー内部。利用者向けイベントとして公開しない。 |
| 要求した転送全体の完了、転送中のNACK・bus error・arbitration lost等の通信異常 | I²Cにbindingしたユーザー関数へ通知する。 |
| センサー等の「測定データができた」という外部通知端子 | [GPIOの割り込み](gpio-interface.md#割り込みハンドラーの静的binding)として扱う。 |

利用者は接続先アドレス・送信データ・受信数等を指定し、割り込みごとの状態遷移を記述しない。1回の要求を処理するためにBSP内部で何度割り込みが発生しても、利用者へは転送全体の結果を通知する。`memory_read`はレジスタアドレス送信・Repeated START・受信をまとめて1件とし、中間段階の完了を別の転送完了として通知しない。

受信の最終byteにcontrollerが返す正常なNACKは、通信異常として通知しない。`is_device_ready`の試行ごとのaddress NACKも内部で処理し、試行ごとの通知を利用者へ要求しない。確認済みNACKによる`False`と、通信異常による失敗を区別する。

### イベントと最終結果

改訂後のイベントとbindingは次の形を採用する。`configure`は転送結果の通知対象に含めない。

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

fn bind_interrupt[R: IsrSafe](
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
- [BSPによるISR専用handlerの静的供給](uart-interface.md#bspによるisr専用handlerの静的供給)を共通に適用する。必要な資源を割り込みの有効化前に準備し、無効化後も実行中のISRがアクセスを終えるまで保持する。
- ユーザー関数には`fails {}`と`IsrSafe`に相当する制約を適用する。heap・参照管理・再帰・suspend・Clock系effect・待機等を禁止し、BSPが提供するISR用操作だけを静的に接続する。
- 優先度はUART・GPIOと共通の`Low`・`Normal`・`High`の方針に従う。同じバスへの同期呼び出しや再設定をISRから再入させない。

[静的bindingのコンパイラー実装計画](uart-interface.md#静的bindingのコンパイラー実装計画)をI²Cにも適用する。型検査後に指定を収集し、定数・不変な別名を辿って論理バスIDを解決する。プログラム全体で同じバスへの重複を検出し、通常コードから呼ばれないprivateなユーザー関数も保持してISR検査を自動適用する。BSP向けの生成入口は転送全体の通知契約に合わせて具体化する。

bindingの実行時検証が成功する前には通知を有効化しない。停止後はcallbackや借用buffer参照が残らず、既に停止している場合の停止は成功する。優先度指定・開始停止の正式な関数名とC symbolはABI実装段階で定め、上記の通知条件と共通の状態遷移を維持する。

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
```

- `length`・`size`・`capacity`は配列のword数であり、このI²C仕様では転送byte数と一致する。レジスタアドレスのprefixは転送Listやその容量に含めない。
- 配列は呼び出し中だけ借用する。BSPはpointerを保存せず、返却時には割り込み・DMAを含め配列を参照する処理を終了する。
- 受信成功時は`size <= capacity`を満たし、`out[0..size]`を初期化し、`*length = size`を設定する。各要素を0〜255にする。失敗時に出力をList化しない。
- Listの構築・固定heapでの所有権管理は共有bridgeが行う。BSPがEmelaのList nodeを確保・解放しない。List用heapの容量不足は共通runtimeのdefect契約に従う。

追加する通知入口はUARTと同じくイベント別に生成し、転送種別・共通エラーstatus・応答確認のBoolを基本型引数で渡す。生成bridgeが論理`I2cId`を補い、固定サイズの`I2cEvent`を構築する。受信ListやEmela enumの内部レイアウトは公開C ABIへ渡さない。転送種別のC定数は生成headerで定義し、Emela enumの内部値を流用しない。

## 現在の実装と必要な確認

I²C専用エラー・静的binding・ユーザー関数のISR内直接呼び出しは未実装である。以下は改訂前の実装状況を示す。

| 部分 | 現在の状態 |
| --- | --- |
| Emelaの型・6operation・BoardI2c | 宣言と接続がある。7bit固定の方針を反映し、`I2cAddressing`と`I2cConfig.addressing`を削除した。`I2cConfig`と生成Cの`EmelaI2cConfig`は周波数・duty cycleの2 fieldになる。 |
| C bridge | word配列への変換、共通範囲・容量検査、status伝播、受信Listの所有権管理がある。I²C固有の入力検証はBSPへ委ねる。 |
| STM32F4・ESP32-C3の同梱BSP | I²CのC driverは未実装。本仕様への準拠を確認した状態ではない。 |
| 既存host C mock | 6operationの値・エラー伝播、容量・返却長、List解放を確認する。65535を保持する共通word試験もあり、I²Cの8bit制約や通信波形を検証するものではない。 |

共有bridgeは返却lengthが要求size以下であることを検査するが、短い受信を拒否せず、返却wordの8bit範囲も検査しない。本仕様の「成功は要求byte数全て」「受信要素は0〜255」はBSPが保証する。timeoutの扱いをボードごとに定めていた共通説明に対し、本仕様に準拠するI²C BSPは上の期限契約を満たす。

BSP実装時には次を確認する。既存bridge試験の成功を、これらの検証の代わりにしない。

| 対象 | 確認内容 |
| --- | --- |
| 設定 | 未設定・非対応ID・速度・duty cycle、再設定、設定失敗後の状態。 |
| アドレス | 未シフトの値、7bitの予約範囲と範囲外の拒否、値によって10bit形式へ切り替えないこと。 |
| データ | 0/255の保存、-1/256/65535の拒否、全要素を検証してから開始すること。 |
| レジスタ | Bits8/Bits16の境界、Bits16の上位byte先行、Repeated START、途中にSTOPがないこと。 |
| 受信 | 1/2/3 byteと容量境界、最終byteのNACK、途中失敗と短い受信を成功にしないこと。 |
| ゼロ長 | 検証は行うが、STARTもレジスタprefixも送らないこと。 |
| 応答確認 | 初回ACK、NACK後のACK、全試行NACK、bus busy、timeout、0以下のtrials。 |
| 期限と失敗 | 負値・0・表現上限、phaseや試行ごとに期限を延長しないこと、失敗後の次の操作、エラーcodeの対応。 |
| 資源 | bufferの借用期間、成功・失敗後のheap解放、stack・転送容量。 |
| 専用エラー | 共通分類への変換、未知の機種固有エラーの`Other`への変換、`is_device_ready`の`False`と失敗の区別。 |
| 静的bindingと通知 | IDと関数の静的解決、重複bindingの拒否、ISR内の直接呼び出し、転送全体の通知、中間phase・正常なNACKを通信異常として通知しないこと。 |
| ISR安全性 | 禁止操作の拒否、BSP専用handlerへの接続、資源の寿命、同期転送・再設定への再入の拒否、復帰後にbufferを参照しないこと。 |
| 最終結果 | 検証失敗・空転送・timeout 0の無通知、応答確認の全試行で1通知、timeoutと完了IRQの競合、後処理失敗、callback終了後の同期返却。 |
| 通知経路と停止 | timer/software IRQ経由のtimeout通知、停止後の旧pending排除、buffer再利用と復旧が必要な状態の維持。 |
| 言語機能の統合 | Emelaソースからeffect/handlerをmockと実BSPへ切り替え、`fails`でNACK・timeoutを処理し、次の通信へ回復すること。 |

host試験で引数・status・所有権を、周辺機器モデルまたは実機でaddress/data byte列・ACK/NACK・START/STOP・SCL timingを確認する。実施した確認範囲をBSPの文書に記録する。

## 後続のAPI検討

[I²C専用エラー](#i²c専用エラーへの移行方針)と[転送単位の静的binding](#割り込みとハンドラーバインドの採用方針)は初版の実装計画に含める。両BSPの代表profileで、7bit master・8/16bitレジスタアクセス・応答確認・最終結果の通知を確認する。以下は、それ以外の拡張候補である。

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

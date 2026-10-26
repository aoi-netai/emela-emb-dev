# UARTの機能一覧と操作契約

状態：2026-10-24、UART専用エラー、同期送受信、受信ユーティリティ、静的割り込みbindingをSTM32F4・ESP32-C3へ実装。利用する言語構文は既存のenum・effect・`Immediate`・`escape`であり、一般のC FFIの型制限は変更しない。

## 機能一覧

| 機能 | API | 内容 |
| --- | --- | --- |
| UART設定 | `configure` | 通信速度、データ幅、parity、stop bit等を設定する。対応組合せはBSP表を参照 |
| 同期送信・固定長受信 | `transmit`／`receive` | 期限内の送信完了、要求数の受信 |
| 可変長受信・入力破棄 | `receive_some`／`discard_input` | 上限付き受信、取得済み入力の破棄 |
| 受信方式 | `set_receive_mode` | 通常受信と`InterruptReceive`を選択する |
| callbackの接続 | `bind_interrupt` | UART IDと名前付き関数を静的に対応付ける |
| 割り込み制御 | `enable_interrupt`／`disable_interrupt` | 受信・受信異常・送信完了の通知を制御する |

## 初期対応BSP

| MCU / board profile | UART / TX / RX | Console予約 | 対応設定 |
| --- | --- | --- | --- |
| STM32F407VG / `stm32f407vg`, `stm32f4-discovery` | `Uart1` = USART1 / PA9 (9) / PA10 (10)、AF7、APB2 16MHz | USART2 / PA2・PA3 | 8N1を初期検証。8bitはparityなし・偶数・奇数、7bitは偶数・奇数、9bitはparityなし。stop 1/2、oversampling 8/16（Nativeは16）、flow controlなし。 |
| STM32F412ZG / `stm32f412zg` | 同上 | USART6 / PC6・PC7 | 同じ共通driver。初期Renode検証の対象外。 |
| STM32F446RE / `stm32f446`, `stm32f446re`, `nucleo-f446re` | 同上 | USART2 / PA2・PA3 | 同じ共通driver。初期Renode検証の対象外。 |
| STM32F429ZI / `stm32f429zi` | 初版のアプリ用UART配線なし | USART1 / PA9・PA10 | `Uart1`は`ResourceConflict`。他IDは`Unsupported`。 |
| ESP32-C3 / `esp32c3` | `Uart1` = UART1 / GPIO4 / GPIO5、matrix signal 9、XTAL 40MHz | UART0 / GPIO21・GPIO20 | 8N1を初期検証。7/8bit、parityなし・偶数・奇数、stop 1/2、Native、flow controlなし、9600〜1,000,000 baud。 |

両BSPともReceive・Transmit・Bothを提供する。RTS/CTSのpinは割り当てず、flow control要求、Half/OneAndHalf、未対応の組合せは`Unsupported`。設定を黙って丸めない。OversamplingにはBy8・By16・Nativeを定義し、Nativeは倍率を要求せずBSP標準のsamplingを選ぶ。ESP32-C3 driverは倍率を設定しないためNativeだけを受け付け、By8/By16はUnsupported。分周器の1/16精度を16倍samplingと混同しない。STM32F4はdividerの実現範囲内でbaud誤差2%以下を検証する。ESP32-C3は指定範囲で1/16精度の分周を使う（最大baudでも誤差2%未満）。

UART設定成功時にTX/RXの両pinを予約する。既にGPIOで設定したpinとの競合は`ResourceConflict`。UARTが予約したpinのGPIO操作も同じ分類で拒否する。予約を解放するAPIは初版に含めない。Consoleのcontroller・pinを再設定しない。

## Emela API

宣言元は[`Std.Embedded.Uart`](../../../emela-core/lib/std/embedded/uart.emel)、handlerは[`BoardUart`](../../../emela-core/lib/std/embedded/board.emel)。設定型は従来の`UartConfig(baud_rate, data_bits, stop_bits, parity, direction, flow_control, oversampling)`を使う。

```emela
pub enum UartReceiveMode { Polling, InterruptReceive }
pub type UartReceiveErrors(parity: Bool, framing: Bool, overrun: Bool, other: Bool)
pub enum UartEvent {
  Received(value: Int)
  TransmitComplete
  ReceiveError(errors: UartReceiveErrors)
}

pub effect Uart {
  fn configure(id: UartId, config: UartConfig) -> () fails UartError
  fn transmit(id: UartId, data: List[Int], timeout_ms: Int64) -> () fails UartError
  fn receive(id: UartId, size: Int, timeout_ms: Int64) -> List[Int] fails UartError
  fn receive_some(id: UartId, max_size: Int, timeout_ms: Int64) -> List[Int] fails UartError
  fn discard_input(id: UartId, max_size: Int) -> Int fails UartError
  fn set_receive_mode(id: UartId, mode: UartReceiveMode) -> () fails UartError
  fn enable_interrupt(id: UartId, priority: InterruptPriority, notify_transmit: Bool) -> () fails UartError
  fn disable_interrupt(id: UartId) -> () fails UartError
}
```

`InterruptPriority`は`Std.Embedded.Interrupt`のLow・Normal・High。通常のoperationはmain文脈で同期実行し、待機中にEmelaの別fiberへ制御を渡さない。

## UARTエラー

`pub error UartError(kind: UartErrorKind)`を返す。`PeripheralError(code)`からの破壊的な変更であり、UARTの`escape`も変更する。

| UartErrorKind | 共通C status | 意味 |
| --- | ---: | --- |
| InvalidArgument | -1 | 不正ID・負のsize/timeout・データ幅超過等 |
| CapacityExceeded | -2 | bridgeの転送容量・返却lengthの超過 |
| NotConfigured | -3 | 未設定 |
| Unsupported | -4 | 非対応ID・設定・組合せ |
| Timeout | -5 | operation全体の期限切れ |
| Busy | -6 | 実行中操作との一時的な競合用（初版はmainで直列実行） |
| ResourceConflict | -7 | Console・GPIO等との予約競合 |
| InvalidState | -8 | 受信方式の違反、未bindingでの有効化、有効中の再設定、方向違反 |
| Other | -9 / 未知の非0 | 未分類の失敗、BSPの不正な返却長 |
| Parity | -10 | parity異常 |
| Framing | -11 | frame異常 |
| Overrun | -12 | FIFO / 通常受信ringのoverflow |

BSPはHAL固有codeをそのまま返さず、生成`emela_peripheral.h`の定数を使う。同期受信で同時に複数の異常を取得した場合はParity→Framing→Overrun→Otherの順で1分類を返し、その観測のflagを解除する。イベント側は複数の異常を保持する。

## 受信方式の選択と排他

`configure`後、最初の受信操作やbindingより前に`set_receive_mode`を1回だけ使う。省略時はPolling。方式選択時は既存の受信ring・ハードウェア入力を破棄する。通常受信・破棄の利用開始後やbinding後には方式を変更できない。再設定しても受信方式と選択済み状態を維持し、無効化をPollingへの切替として扱わない。

| 方式 | receive / receive_some / discard_input | Received / ReceiveError | main transmit |
| --- | --- | --- | --- |
| Polling | 使用可 | 通知しない | 使用可。送信完了通知だけのbindingも可 |
| InterruptReceive | size 0も無待機・無消費でInvalidState | 有効化したISR内で通知 | 使用可 |

PollingでもBSP内部のRX IRQが32wordの固定ringへ保存する。公開の受信operationはmainがこのringを読む同期APIであり、fiberは再開しない。STM32F4はハードウェアの1word受信レジスタ、ESP32-C3は128word FIFOからringへ転送する。割り込みが間に合わない入力やringの満杯はOverrunとなる。

## operationの完了条件

- `configure`：引数・組合せ・pin予約を検証してから適用する。検証失敗は既存設定を維持する。適用時は受信ring、FIFO、受信異常を破棄する。有効中はInvalidState。
- `transmit`：全wordの範囲を転送前に検査する。最終stop bitまでの物理送信完了を待つ。非空の成功送信に通知を有効化していれば、TransmitCompleteのISRとcallbackが終了してから成功を返す。空送信・失敗送信には通知しない。
- `receive`：ちょうどsize個を到着順に消費し、余りを次回へ残す。期限切れ・異常時は部分Listを成功として返さない。消費済み入力は戻らない。

Listの1要素はparityを除く1word。7/8/9bitに対して0..127/255/511を扱い、0・改行を保存する。C配列は`uint16_t`。文字列変換・改行追加・NUL終端は行わない。

### timeoutと失敗後の状態

timeoutは全wordと送信完了待ちに共通の単調時刻による期限。負数はInvalidArgument。空転送・size 0もID・設定・方向・方式・timeoutを検証する。

非空transmitのtimeout 0は送信前にTimeout。固定長receiveのtimeout 0は、要求数がringに揃っていなければ無消費でTimeout。揃っていれば直ちに返す。正のtimeoutでは受信済みのprefixを消費する場合がある。

送信失敗後、STM32F4のDR/shift register、ESP32-C3のFIFO/shift registerへ渡したwordは物理的に送信を続け得る。入力配列への参照は返却前に終了し、古いTC IRQから成功通知しない。次の送信は残っているwordの後へ続く。

STM32F4の受信異常はSR→DRの順で取得・解除する。parity/frame異常wordは捨てる。ESP32-C3はparity/frame異常を取得した時点のRX FIFOを破棄して復旧する。Overrunでは欠落を通知するが、正常に保持できたwordは残す。

## 通常受信のユーティリティ

`receive_some`は最初の1wordを期限まで待ち、その後は直ちに読める分だけmax_sizeまで返す。正のmax_sizeで0件ならTimeout。上限を埋めるための追加待機や、固定長受信のTimeoutを成功に読み替える処理はしない。max_size 0は検証後に空List。

`discard_input`は通常受信ringから最大max_size個だけを破棄し、個数を返す。待機せず、破棄先bufferも確保しない。正の上限を指定した場合は保存済みの受信異常も解除する。上限0は検証後に0を返し、異常を解除しない。IRQ未処理のハードウェア入力は、次のIRQで取り込む。

## 割り込みとハンドラーバインド

### binding専用関数

```emela
pub fn bind_interrupt[R: Immediate](
  id: UartId,
  callback: fn(UartId, UartEvent) -> () fails {} use R
) -> () fails UartError
```

UARTごとに1つの静的binding。IDとcallbackは定数・不変別名を辿って解決する。captureのない名前付き関数を使う。重複、動的ID、実行時のcallback選択、lambdaはコンパイルエラー。同じ関数を別UARTへbindingできるが、実際に利用できるcontrollerはBSPの表に従う。重複診断は現在関数名・対象ID単位であり、2箇所のソース位置を併記する改善は残る。

`configure → set_receive_mode → bind_interrupt → enable_interrupt`の順で準備する。静的C入口が存在しても、mainのbind実行が成功するまでは有効化できない。停止はmainから`disable_interrupt`。既に停止済みなら成功する。停止後の入力・旧pendingを再有効化時のイベントとして通知しない。InterruptReceiveの開始・停止時に残ったハードウェア入力を破棄する。

### 採用した利用者向けインターフェース

[`examples/uart_interrupt`](../../../../examples/uart_interrupt)はUSART1の受信値でLEDを操作し、mainから送信する完全なCLI project。

```emela
fn on_uart(id: Uart.UartId, event: Uart.UartEvent) -> () fails {} use {Gpio.Gpio} {
  event match {
    Uart.Received(value) -> {
      accepted = Interrupt.try_push(Interrupt.Channel0, value)
      # Falseなら満杯。必要ならGPIO等で欠落を記録する。
      ()
    }
    Uart.TransmitComplete -> ()
    Uart.ReceiveError(errors) -> ()
  }
}
```

### イベント型

Receivedは取得済みの1word。ユーザー関数が同じUARTを重ねて読み出さない。ReceiveErrorはparity/framing/overrun/otherの非空集合。parity/frame異常wordをReceivedとして通知しない。同じ観測の複数異常を保持し、異常通知後に正常に保持できたwordを通知する。

TransmitCompleteは成功した非空送信1回につき1回。RXと同時pendingなら、そのIRQでRX/errorを先に処理する。異なるIRQの通知順序・欠落個数・異常の正確な位置は保証しない。

### BSPによるISR専用handlerの静的供給

callbackと到達する補助関数を専用に具体化し、GPIO read/writeをBSPのISR専用C操作へ直接解決する。mainのhandler evidenceは捕捉しない。GPIOエラーはcallback内で捕捉する。`pub`指定は不要。

Console、Clockのnow/sleep、通常UART、configure/toggle/割り込み制御、State/Cell、heap確保・参照管理、再帰、suspend、未解決の動的呼出しを拒否する。数値・固定サイズenum・固定サイズエラーは利用可。検査はembedded build時に行う。現在のcheck/LSPだけではbackendのISR診断は出ない。

### 静的bindingのコンパイラー処理

実装はGPIOとUARTで収集・ID/関数解決・重複検査・ISR特殊化を共有する。callbackは通常呼出しがなくても保持し、BSP用C入口から専用関数を直接呼ぶ。runtime登録は設定検証として残し、関数pointerの動的登録は行わない。I²Cにも同じ静的bindingとISR検査を適用する。

### 割り込みイベントのC入口

生成`emela_uart.h`は次の基本型入口を宣言する。イベントenumやEmelaの関数値の内部レイアウトは公開しない。

```c
int32_t emela_uart_has_binding(int32_t id);
void emela_uart_received(int32_t id, int32_t value);
void emela_uart_transmit_complete(int32_t id);
void emela_uart_receive_error(int32_t id, int32_t parity, int32_t framing,
                              int32_t overrun, int32_t other);
```

生成bridgeがUartIdとUartEventを構築し、同じISR内でcallbackへ渡す。一般のpayload付き複数variant enumをC ABIへ追加する変更は不要。空のerror集合は通知しない。

### 優先度と情報共有

STM32F4はUSART1 IRQ37、Low/Normal/HighをNVIC 0xc0/0x80/0x40へ対応させる。ESP32-C3はUART1 source22→CPU IRQ3、優先度1/2/3。同じ優先度・低い優先度のユーザーIRQはネストしない。ESP32-C3はtrap内の再有効化を行わず、全ユーザーIRQでネストしない。

`Std.Embedded.Interrupt`のWordChannelはChannel0..3の4本、それぞれ16個のIntを保持する。`try_push(channel, value) -> Bool`は満杯ならFalse、`try_pop(channel) -> SharedWord(available, value)`は空なら(False, 0)。ISRはpush、mainはpopを使い、1本に1つのISR producer・1つのmain consumerを対応させる。consumerはアプリ側でも1経路に揃える。

channel保存領域は静的確保し、短いIRQ禁止区間で値をコピーする。heap、lock待機、scheduler起動はない。複数のGPIO/UART bindingが同じchannelへpushする場合、ISR所有channelへmainからpushする場合、ISRのchannelが静的に解決できない場合はbuildで拒否する。ISRのpopも禁止。失敗したpushのFalseを使ってアプリが欠落を観測する。

ISRが所有するchannelがある場合、mainの`try_push`も呼び出し先とchannelを静的に確認する。ローカル・定数の関数別名は辿るが、`try_push`そのものを高階関数へ渡す、record等へ保存する、動的な関数値として返す使い方は、競合を否定できないためbuildで拒否する。高階関数から送信する場合は、ISRが所有しないchannelへ`try_push`する名前付きラッパーを渡す。ISRによるchannel所有がないプログラムでは、この関数値の制限は適用しない。

## 現在の実装と必要な確認

[検証](#検証)にhost・Renodeの再現手順と確認範囲をまとめる。Renodeの8N1試験を、全設定の実機確認とみなさない。STM32F429の別UART配線、RTS/CTS、DMA、行/idle受信、break、RS-485は初版の対象外。

## 参照

- [ST公式USART定義](https://github.com/STMicroelectronics/stm32f4xx-hal-driver/blob/master/Inc/stm32f4xx_ll_usart.h)：parityを含むword length、BRR、TC。
- [ESP-IDF v5.4.1 UARTレジスタ](https://github.com/espressif/esp-idf/blob/v5.4.1/components/soc/esp32c3/register/soc/uart_reg.h)、[UART LL](https://github.com/espressif/esp-idf/blob/v5.4.1/components/hal/esp32c3/include/hal/uart_ll.h)：FIFO、error/TC、XTAL分周。
- [共通の操作契約](common.md)、[言語仕様](../../../../docs/spec/emela-0.20.md)。

## 検証

### 再現手順

開発checkoutのCLIをビルドする。PATH上にある古いEmelaでは追加した標準モジュール/APIを利用できないため、下記ではバイナリを明示する。

```sh
cargo build --locked -p emela-cli
export ARM_TOOLCHAIN_PATH=/path/to/arm-none-eabi-toolchain
export EMELA_ESP32C3_TOOLCHAIN=/path/to/riscv32-esp-elf
# BSPで指定されたesptool 4.8.1をPythonから利用できる状態にする。
python3 crates/emela-codegen-emb/tools/uart_matrix.py \
  --emela target/debug/emela \
  --renode /path/to/renode-1.16.1/renode \
  --output target/uart-validation
```

既定で2 MCU × 2受信方式 × O0/O2の8 caseを生成する。各caseは別のfirmware起動であり、UARTの受信方式を動作中に切り替えない。`--boards`・`--modes`・`--optimizations`で範囲を指定できる。

通常の`emela check`・`emela build`から生成し、BSPをクロスコンパイルして実行する。生成先は`<board>/<mode>/<optimization>/project`・`build`・`observed`。総合`report.json`にfirmware hash、モデル情報、受信方式、観測値、stack、資源量を記録する。PCの書換えやISR関数の直接呼出しによる代用はしない。

### 自動確認の範囲

| 試験 | 確認内容 |
| --- | --- |
| host生成C | 3イベント入口、複数error flag、正しいID、同じprivate関数の複数IDへのbinding、callback/ID/関数別名、ISRでのheap確保0、mainのList/String/errorとの共存、status分類 |
| compiler拒否 | 動的ID・lambda・重複binding、Console等の禁止操作、heap、ISRのpop、複数ISRの同一channel使用、main producerとの競合（高階関数・関数別名・record保存・closure経由を含む）、動的channel |
| 共通BSP state machine | 実装そのものをhostでコンパイルし、設定維持・方向/受信方式・空転送・負の引数・word幅・32word ring overflow・受信異常・prefix消費・ゼロ期限の無消費・全体期限・TC/timeout競合・失敗後の再利用を確認。ASan/UBSan付き |
| 共有queue | 16 Intの上限、満杯のFalse、FIFO、channel分離、slot再利用、IRQ lockの保存/復元 |
| Renode Polling | 0/改行/255の順序、RX IRQ→通常受信ring、要求数不足時の無消費、固定長/receive_some、上限付き破棄、期限切れ後の受信、9600から115200への再設定、Pollingと送信完了通知の併用、GPIO pin予約競合 |
| Renode InterruptReceive | TC通知後の同期返却、受信→GPIO、size 0も通常受信禁止、ISR→固定queue→main、停止後の入力抑止、旧pendingの除去、再設定時の方式維持、再開、ESP32-C3のparity+framing集合 |
| GPIO回帰 | 両BSP O2で既存の9 IRQ・18checkpoint、guardを確認 |

UART matrixの合否は8 caseすべてでアプリ結果42、defect 0、stack guard正常、送信wordが正確に`[0, 10, 255]`、イベント数・LED観測値の一致とする。PollingはTC 1回、InterruptReceiveはSTM32F4でTC 1回+RX 4回、ESP32-C3でさらにerror集合1回。空送信からは通知しない。

### モデルと実機確認の境界

STM32F4はRenode標準のUSART1・NVIC・GPIOを使い、Flash binaryからresetする。1.16.1のUSARTはPEIE/EIEを未実装tagとして報告するため、指定した2種類のwarningを各4回（Polling）/各3回（InterruptReceive）だけを照合して記録する。他のwarning/errorは失敗。parity/frame波形は未検証で、TCはモデル上即時になる。

ESP32-C3は`ESP32C3GpioTestSoC.cs`でGPIO/UART1/FIFO/TC・SysTimer・interrupt matrixの必要な部分をモデル化し、generic RV32のSRAMへnative ELFをロードする。TXは115200/8N1相当のword時間を経てTCを発生させる。ROM・Flash boot、実デバイスの全レジスタ、baud/divider・parityの波形、優先度競合・IRQネストを網羅しない。BSPはnative MMIOと通常のtrap vectorを実行する。

実機でのbaud誤差・stop bit・7/9bit・parity・高負荷overflow・割り込み遅延、非代表STM32F4 profileの通信は未確認。RTS/CTS等はUnsupportedとして拒否する。デジタルword注入による成功を、電気特性や全設定の確認済みという意味にはしない。

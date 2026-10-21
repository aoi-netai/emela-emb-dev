# STM32F446向けfirmware package

`stm32f446`はSTM32F446RE／NUCLEO-F446REを想定する。Flash 512 KiB、SRAM 128 KiB、HSI 16 MHz、Cortex-M4F hard-floatを使う。Emelaから生成したCに独立したstartup・linker・board runtimeを組み合わせ、Flashからreset起動する`firmware.elf`と`firmware.bin`を作る。STM32Cube・HAL・CMSISの取得は不要である。

## CLIから使う

プロジェクトの`Pome.toml`に設定する。各pathはプロジェクトdirectoryが基準になる。

```toml
[build]
target = "c-embedded"

[embedded]
board = "stm32f446"
config = "embedded.toml"
entry = "main"
optimization = "O2"
```

`embedded.toml`を次の内容で作る。標準handlerの環境を保持するため、この例では固定heapも予約する。

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

`src/main.emel`には、引数を持たず`Int`を返す公開関数を定義する。

```emel
import Std.Console.{Console}
import Std.Clock.{Clock}
import Std.Embedded.Board.{BoardConsole, BoardClock}

pub fn main() -> Int use Async {
  with BoardConsole, BoardClock {
    (use Console).print_line("booted")
    (use Clock).sleep(1000)
    (use Console).print_line("awake")
    0
  }
}
```

```sh
emela build .
emela flash . --dry-run
emela flash .
```

CLIは`main`の開始関数を`board_main_start`というC aliasへ接続し、package内の`main()`が結果の保存先とschedulerを管理する。アプリ側にCの`main`やstartupを書く必要はない。公開entryの署名・設定と生成先は[CLIガイド](../../docs/cli-embedded.md)を参照する。

ARM GNU toolchainの`arm-none-eabi-gcc`・`nm`・`objcopy`・`size`を`PATH`へ入れる。`ARM_TOOLCHAIN_PATH`にtoolchainの`bin`の親directoryを指定することもできる。Flashへの転送にはOpenOCDとST-Linkが必要で、`OPENOCD`で実行fileを指定できる。

## packageが提供する機能

| 機能 | 実装・制約 |
| --- | --- |
| GPIO | 論理ID 0〜15をPA0〜PA15へ対応付ける。USART2用PA2・PA3は予約する。Input・OutputPushPull・OutputOpenDrain・Analog、NoPull・Up・Downを設定できる |
| GPIO入出力 | readはIDRのpin level、writeはBSRR、toggleはODR latchを使う。writeは設定済みoutputに限定する |
| Console | USART2、PA2 TX・PA3 RXを初期化し、115200 baud、8N1で長さ付きUTF-8を送信、末尾へ改行する |
| Uart.configure | Uart2／115200／Bits8／One／NoParity／Transmit／NoFlowControl／By16を受け入れる。それ以外はPeripheralError(1) |
| Uart.transmit／receive | timeout付き転送はPeripheralError(2)。Consoleの送信と区別する |
| Clock.sleep | SysTickの1ms割り込みで1件の待機を進める。0以下は即時通知。ISRは完了を通知し、Emelaの再開はmain側で行う。取消済みtaskへの遅延通知は記録して無視する |
| Clock.now | 起動時を仮想Unix epoch 0とした経過ms。実際の日時・RTCではない。SysTickはアプリ終了まで連続して進む |
| GPIO割り込み | board defectとして停止する。I²C・SPI等はこのpackageにdriverを含めない |
| defect | SysTickを止め、packageが設定したGPIO outputをLowへし、失敗数を記録して停止する |

PA13・PA14をGPIOへ変更すると実機のSWD接続に影響する。用途に応じて予約pinとして扱う。GPIOの電気的制約、外部装置の駆動回路、センサdriverはアプリのボード設計に従う。

Clock待機を取り消しても物理timerの期限は残る。期限が終わる前に2件目の正の`Clock.sleep`を要求すると、単一待機の容量超過としてboard defectになる。複数timerや取消時の即時再利用が必要なprojectでは専用driverを用意する。

独自のセンサ等のC実装は`[embedded].sources = ["sensor.c"]`で追加できる。対応する`@external(c, ...)`をEmela moduleに定義する。温度調節器では[firmware project](../../examples/temperature-controller/firmware/Pome.toml)がこの方法を使う。模擬センサや制御判断はpackageへ含めない。

## Flash起動と書き込み

vector tableをFlash先頭`0x08000000`へ配置する。最初のwordはMSP `0x20020000`、次のwordはThumb状態の`Reset_Handler`である。reset handlerはFPUを有効にし、VTOR設定、FlashからSRAMへの`.data`コピー、`.bss`クリア、C初期化、アプリ起動を行う。PLLやHSEには依存しない。実機はmain Flashから起動するBOOT設定にする。[STM32F446 reference manual](https://www.st.com/resource/en/reference_manual/dm00135183.pdf)を参照する。

ST-LinkではOpenOCDの`interface/stlink.cfg`と`target/stm32f4x.cfg`を使い、ELFを`program ... verify reset exit`で書き込む。`--port`はST-Linkのadapter serialとして扱う。dry runでは実行する引数列のみを出力し、デバイスへ接続しない。[OpenOCD Flash Programming](https://openocd.org/doc/html/Flash-Programming.html)に書き込み・verify・resetの動作がある。

build開始時に古い`firmware.json`を無効化し、compile・link・資源・vector検査がすべて通った後にmanifestをatomicに公開する。manifestにはboard、成果物のSHA-256、Flash配置と資源を保存する。Flash実行前にこれらの一致を検査する。CLIは追加でソース・設定の鮮度を検査する。

## 資源と観測

固定heap・fiber等のpoolは生成Cの静的領域に含む。system allocatorの`malloc`等がlinkされている場合はbuildを失敗させる。stackは16 KiB、low guardは別に256 byteを予約し、静的RAMとの衝突をlink時に拒否する。

`resources.json`にはFlash image byte数、静的RAM、stack・guard、予約RAM合計を保存する。`objects/`にはGCCの`.su`、`firmware.map`には配置を残す。16 KiBのstack予約と実際のstack使用量は別の値である。

runtimeは`board_app_completed`・`board_app_result`・`board_app_failures`・`board_ticks`・`board_notifications`・`board_rejected_notifications`・`board_uart_bytes`と`emela_validation_stack_*`を公開し、Emela処理とISRを含むstack high-water、guardを観測できる。paint領域の走査はschedulerがworkを実行した後と終了時に行い、各1msのIRQではSPの記録だけを行う。

Flash起動の確認では、Renode上で同一Flash memoryを`0x08000000`とBOOT0=0のalias `0x00000000`へ登録し、`LoadBinary firmware.bin 0x08000000`のみを使う。RAMの`board_boot_data`・`board_boot_bss`を意図的に汚してからCPUをresetし、vectorからSP／PCを取り込ませる。monitorによるPC・SP設定やELFのSRAM初期化を使わない。startupが成功すると`board_boot_data=0x454d4244`、`board_boot_bss=0x4253534f`、`board_boot_initialized=1`になる。

この確認はFlash imageからのresetとアプリ動作の根拠であり、実機のST-Link転送の実測ではない。温度調節器の実行結果は[example README](../../examples/temperature-controller/README.md)へ記載する。

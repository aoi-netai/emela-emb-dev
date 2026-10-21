# 温度調節器

温度を1秒ごとに読み、低温なら加熱、高温なら冷却するexampleである。同じ`controller.emel`をSTM32F4の各profileとESP32-C3で使用する。温度と運転状態はUARTへ表示し、加熱・冷却装置への指示を2本のGPIOで出力する。

温度だけで次の出力を決めると、境界付近で装置が頻繁に切り替わる。ここでは「現在の運転状態」を保持し、開始温度と停止温度を分ける。

| 装置 | 開始条件 | 停止条件 |
| --- | --- | --- |
| 加熱 | 18℃以下 | 20℃以上 |
| 冷却 | 28℃以上 | 26℃以下 |

例えば19℃では、停止中なら停止を続け、加熱中なら加熱を続ける。加熱中に29℃へ上昇した場合は、加熱を停止して冷却へ切り替える。温度が取得できない場合は両方OFFにして`fault`へ移り、次の有効な入力で制御を再開する。

## CLIでfirmwareを作り、書き込む

[firmware/Pome.toml](firmware/Pome.toml)を使うと、Emelaの型検査から各CPU向けのcompile・link、Flash imageの作成までCLIで行える。workspace rootから実行する。

```sh
emela build crates/emela-codegen-emb/examples/temperature-controller/firmware \
  --board stm32f446 --out-dir /tmp/emela-temperature-stm32-flash
emela flash crates/emela-codegen-emb/examples/temperature-controller/firmware \
  --board stm32f446 --out-dir /tmp/emela-temperature-stm32-flash --dry-run
```

`--dry-run`を外すとST-Link／OpenOCDを使って実機へ書き込み、verify後にresetする。`--port`にはST-Linkのadapter serialを指定できる。ARM GNU toolchainは`PATH`または`ARM_TOOLCHAIN_PATH`、OpenOCDは`PATH`または`OPENOCD`で指定する。STM32Cubeはこのfirmware経路には不要である。

ボードを`--board esp32c3`へ変えると同じEmelaアプリをESP32-C3向けに作る。ボードの既定値・固定容量・独自C sourceは`Pome.toml`と[firmware/embedded.toml](firmware/embedded.toml)へまとめ、`[embedded.modules]`が`{board}`に応じて`Board` moduleの配線を選ぶ。ESP32-C3のtoolchain・Flash imageと書き込み手順は[packageの説明](../../boards/esp32c3/README.md)を参照する。

共通の`controller.emel`はそのままで、[firmware/src/main.emel](firmware/src/main.emel)が`Controller.run(24)`を呼ぶ。packageの汎用runtimeがstartup、標準GPIO・Clock・Console、schedulerを提供する。独自の模擬センサだけを[firmware/sensor.c](firmware/sensor.c)から追加するため、アプリ側にCの`main`は不要である。

出力先には`generated/`、展開された`bsp/`、`firmware/firmware.elf`・`.bin`・`.map`、資源とhashを含むmanifestを保存する。書き込み前にはboard、生成物とソース・設定の鮮度を検査し、変更後にrebuildしていない場合は失敗する。

### STM32F4のボードを切り替える

`stm32f407vg`・`stm32f412zg`・`stm32f429zi`・`stm32f446re`のMCU profileと、`stm32f4-discovery`・`nucleo-f446re`の基板profileを選べる。既存の`stm32f446`も互換IDとして使える。アプリ本体は共通で、`boards/{board}/board.emel`が配線とConsole UARTを選ぶ。

```sh
emela build crates/emela-codegen-emb/examples/temperature-controller/firmware \
  --board stm32f412zg --out-dir /tmp/emela-temperature-f412
python3 crates/emela-codegen-emb/examples/temperature-controller/firmware/renode_stm32.py \
  --firmware-dir /tmp/emela-temperature-f412/firmware \
  --platform cpu-f412 --output /tmp/emela-temperature-f412-observed \
  --renode /path/to/renode --toolchain /path/to/arm-toolchain
```

F412はUSART6 PC6/PC7、F429はUSART1 PA9/PA10を使う。F407・F412・F429・Discoveryの加熱/冷却出力はPD12/PD13、F446・NucleoはPA5/PA6である。論理GPIO IDは`port * 16 + pin`で、PD12は60となる。Console用pinとMCU packageにないpinは利用時に診断する。

[STM32F4の対象と一括確認](../../docs/stm32f4-boards.md)に全profileの容量・配線、Renode 1.16.1の全F4定義とfragmentの分類、O0/O2 matrixの手順をまとめている。Discovery fragmentは基板へ合成して検証する。温度アプリの起動・GPIO・UART・Clockの確認であり、Ethernet等の全周辺機器を網羅するものではない。

### STM32のFlash resetをRenodeで確認する

```sh
python3 crates/emela-codegen-emb/examples/temperature-controller/firmware/renode_stm32.py \
  --firmware-dir /tmp/emela-temperature-stm32-flash/firmware \
  --output /tmp/emela-temperature-stm32-flash-observed \
  --renode /path/to/renode --toolchain /path/to/arm-toolchain
```

`RENODE`・`ARM_TOOLCHAIN_PATH`環境変数でも指定できる。空のdirectoryまたはこの検証runnerが所有する出力先を使う。空白を含むpathにも対応する。

runnerはCLIが作成した`firmware.bin`のみをFlash `0x08000000`へloadする。同一Flashを`0x00000000`にも登録してBOOT0=0をmodel化し、CPU resetでvectorからMSP／PCを取り込む。ELFはsymbolのaddressを得るためだけに使い、Renodeへはloadしない。monitorでPC・SP・VTORを設定する処理もない。

`.data`と`.bss`の観測wordを事前に`0xdeadbeef`で汚してからresetし、startup自身が初期化することを確認する。さらに24行のUARTの温度と運転状態、1秒間隔で観測する実GPIO出力、23回のtask通知、正常終了後の両出力OFF、stack high-waterとguardを検査する。`run.resc`・platform・`renode.log`・`uart.log`・`report.json`を残す。GPIOはsample時の出力registerに加えて、外部LEDと独立receiverへ接続する。receiverが記録した全遷移を期待するOFF/ON順序と照合し、両装置の同時ONと終了時の状態も検査する。ConsoleのTX/RXのAF設定とRCC enable bitもreadbackする。これは電気的特性を検証するものではない。

現行のF4共通BSPによる機種別の資源は[STM32F4の確認結果](../../docs/stm32f4-boards.md#確認結果)を参照する。以下は共通化前のSTM32F446専用BSPをRenode 1.16.1・Arm GNU GCC 13.2.1で確認した履歴である。単位はbyte。

| 最適化 | Flash BIN | 静的RAM | RAM予約合計 | 観測MSP |
| --- | ---: | ---: | ---: | ---: |
| O0 | 41,232 | 7,032 | 23,672 | 1,980 |
| O2 | 28,268 | 7,048 | 23,688 | 1,360 |

予約RAM合計は静的RAM、stack 16,384byte、guard 256byteの合計である。固定heap 2,048byteと各poolは静的RAMに含む。両構成とも24sampleの温度・mode・GPIOが一致し、結果24、実SysTick割り込み23,000回、task通知23回、stale通知0、UART 944byte、失敗0、終了時両出力OFF、guard正常だった。観測MSPはこのプログラムと入力での値であり、任意のアプリの最大値ではない。

別途、独自C sourceを持たず`BoardConsole`で2行を出力し、`BoardClock`で10ms待機して`Int`を返すEmelaだけのprojectもCLIでbuildし、Flash BINからのreset、待機後の再開と終了を確認した。標準APIだけを使うアプリにCやRustの補助mainは必要ない。実機のST-Link転送は実測しておらず、書き込みcommandはdry runで確認した。

このFlash経路と、以下の詳細な履歴付きのボード別runnerは、別の確認手順として残している。

## Cを生成する

このdirectoryで次を実行する。Rustのexampleプログラムは必要なく、通常の`emela build`で組み込みCを生成できる。

```sh
emela build controller.emel --target c-embedded \
  --embedded-config embedded.toml --out-dir generated
```

[embedded.toml](embedded.toml)で固定heap・各poolの容量、ボード側から呼ぶ公開関数の名前を設定する。生成するファイルや設定の詳細は[組み込み向けCLI](../../docs/cli-embedded.md)を参照する。

workspace上で開発中のCLIを使う場合は、workspace rootから次を実行する。

```sh
cargo run --locked -q -p emela-cli -- build \
  crates/emela-codegen-emb/examples/temperature-controller/controller.emel \
  --target c-embedded \
  --embedded-config crates/emela-codegen-emb/examples/temperature-controller/embedded.toml \
  --out-dir /tmp/emela-temperature-generated
```

上記のboard指定を使わない直接生成commandはCとheaderを出力し、既定のSTM32用`board.emel`を選ぶ。以下のボード別runnerはこの直接生成経路を使い、独自の観測付きboard Cと組み合わせてcompile・linkとRenode起動を行う。

## STM32F446／Renodeで実行する

workspace rootから実行する。

```sh
bash crates/emela-codegen-emb/examples/temperature-controller/renode/run.sh \
  --output /tmp/emela-temperature-controller
```

runnerがCLIでEmelaソースからCを生成し、Cortex-M4F向けにビルドしてRenodeで実行する。既定では24回測定したあと両装置を停止して終了する。途中で停止した処理や前回の実行結果を成功として扱わないよう、今回のUART出力・GPIO履歴・終了状態を確認する。

出力例：

```text
sample=1 temperature=21.0 mode=idle
sample=2 temperature=19.0 mode=idle
sample=3 temperature=18.0 mode=heating
sample=4 temperature=19.0 mode=heating
sample=5 temperature=19.99 mode=heating
sample=6 temperature=20.0 mode=idle
```

Floatの小数部の表記は生成された変換処理に従う。全24行は出力先の`uart.log`に保存する。結果は`report.json`、生成Cは`generated/`、firmwareは`firmware.elf`に残る。

ARM GNU toolchain、STM32CubeF4、Renodeが必要である。この環境の既定の配置と`STM32_RENODE_ROOT`、`ARM_TOOLCHAIN_PATH`、`STM32CUBE_F4`、`RENODE`などの指定は[Renodeの準備](../../docs/renode.md#必要なもの)を参照する。outputには空のdirectoryか、このrunnerが作成したdirectoryを指定する。

## ESP32-C3／Renodeで実行する

[ESP32-C3の準備](../../docs/esp32c3-renode.md)に従い、Renodeと固定版model・toolchainを用意する。`RENODE`には実行fileを指定する。依存物は既定で`/tmp/emela-esp32c3-cache`に置き、`--cache`または`EMELA_ESP32C3_CACHE`で変更できる。

```sh
bash crates/emela-codegen-emb/examples/temperature-controller/renode-esp32c3/run.sh \
  --verify-scenario --optimization O2 --output /tmp/emela-temperature-esp32c3-O2
```

runnerは`controller.emel`をbyte単位で同一のまま出力先の`source/`へcopyし、[ESP32-C3用board.emel](boards/esp32c3/board.emel)を組み合わせて通常のCLIへ渡す。[ESP32-C3用embedded.toml](boards/esp32c3/embedded.toml)を用い、生成Cを加工せずRISC-V向けにcompile・linkする。アプリの状態遷移・effect・標準handlerの実装はSTM32と同じである。この詳細観測runnerのボード選択はrunner内で行い、上のfirmware projectではCLIの`--board`を使う。

| 入出力 | ESP32-C3での割り当て |
| --- | --- |
| 加熱 | GPIO3、HighでON |
| 冷却 | GPIO4、HighでON |
| ログ | UART0、115200 baud相当、8N1、TX |
| 周期 | SYSTIMER target0の実割り込みで1msを数え、1000msでtaskを通知 |
| 温度 | STM32と同じ0.01℃単位の模擬入力系列 |

GPIOは独自の小さなRenode modelから外部LED endpointへ接続する。実GPIO出力の履歴とUARTを照合し、加熱・冷却の同時ONを検査する。GPIOのINをOUTへcopyする上流modelの内部loopbackを、外部装置の動作として扱わない。UART0とSYSTIMER・interrupt matrixは固定版の外部modelを使う。

このexampleの`board.c`が提供する範囲はmodelの機能全体とは異なる。GPIO設定は3・4の`OutputPushPull`／`NoPull`に限定し、readは出力latchを読み、write・toggleもこの2本を操作する。GPIO割り込みはboard defectで停止する。UART0は上記のconfigureとConsole出力を提供し、timeout付き`Uart.transmit`・`Uart.receive`は`PeripheralError(2)`で明示的に失敗する。`Clock.now`は1970-01-01T00:00:00Zを仮想の起点とするデモ用時刻で、実際の日時を示さない。温度調節器は`Clock.sleep`だけを使用する。

`--continuous`で継続運転を26秒相当観察する。`--optimization O0`も利用できる。`report.json`、`metrics.json`、`resources.json`、`uart.log`、`firmware.elf`とmap、生成ソースを出力先に保存する。空のdirectoryか、このESP32-C3 runnerが作成したdirectoryだけを出力先にできる。別processと同時利用した出力先や、以前のUART logを成功として再利用しない。

この詳細観測runnerの構成はboot ROM・ESP-IDF・flashを使わないSRAM実行である。Renode monitorが実行前にentry PC、`MTVEC`とCLIC line1を設定し、その後は`SYSTIMER → interrupt matrix → CLIC → trap → ISR → task通知 → main scheduler`で進む。ISRを呼び出すPC hookはない。Flash向けの成果物と書き込みcommandは上のfirmware projectで扱う。I²C・SPI・Wi-Fi・BLE等の動作確認もこのexampleには含めない。

### ESP32-C3の資源

既定の24sampleをRenode 1.16.1・Espressif GCC 14.2.0で観測した。単位はbyte。

| 最適化 | code・定数のSRAM割当 | 静的data SRAM | stack予約 | SRAM割当合計 | 観測stack |
| --- | ---: | ---: | ---: | ---: | ---: |
| O0 | 53,428 | 7,396 | 16,384 | 77,208 | 2,140 |
| O2 | 33,748 | 7,404 | 16,384 | 57,536 | 1,400 |

stack予約には256byteのguardを含み、実際に使用できるstackは16,128byteである。固定Emela heap 2,048byte、fiber・frame・task・event等のpoolは静的dataに含む。C libraryの`malloc`等はlinkされていない。heap使用ピークや終了後のheap残量はこのrunnerでは測定しない。

modelの128 KiBはSRAM sliceの上限で、合計欄の使用量とは異なる。code用64 KiB・data用48 KiB・stack用16 KiBを同じ物理SRAMの非重複領域へ配置し、IRAM/DRAM aliasの二重計上がないことも確認する。codeもSRAMに置くため、STM32のFlash/RAM内訳とそのまま比較はできない。Flash使用量0はこのexampleがflash起動に対応したという意味ではない。

有限運転では24行のUART・24sampleのGPIO履歴・23回の待機と23,000回のSYSTIMER割り込みが一致した。独立した外部pin遷移は26回で、両出力の同時ONはなく、終了時は両LEDがOFF、stack guardも正常だった。観測stackはこの入力系列での値であり、任意のプログラムの最大値やWCETではない。

O2の継続運転は26秒相当で26sample、120秒相当で120sampleまで進行し、外部pin遷移・UART・入力系列の一致とguard正常を確認した。両方とも同じ固定容量で、観測stackは1,400byteのままだった。120sampleの観察も停止時は次の待機中であり、プログラムの正常終了やheap残量0を示すものではない。

## アプリケーションを読む

```text
temperature-controller/
├── controller.emel
├── board.emel
├── embedded.toml
├── README.md
├── firmware/
│   ├── Pome.toml
│   ├── embedded.toml
│   ├── src/main.emel
│   ├── sensor.c
│   └── renode_stm32.py
├── boards/{stm32f407vg,stm32f412zg,stm32f429zi,stm32f446re}/board.emel
├── boards/{stm32f4-discovery,nucleo-f446re,stm32f446}/board.emel
├── boards/esp32c3/
│   ├── board.emel
│   └── embedded.toml
├── renode/
│   ├── board.c
│   ├── run.sh
│   └── run.py
└── renode-esp32c3/
    ├── board.c
    ├── startup.S
    ├── firmware.ld
    ├── validation_stack.c
    ├── validation_stack.h
    ├── run.sh
    └── run.py
```

| ファイル | 役割 |
| --- | --- |
| [controller.emel](controller.emel) | 状態遷移、状態の保持、温度取得と出力、周期待機 |
| [board.emel](board.emel) | 論理ピンとUART設定、温度センサeffectとボード固有の接続 |
| [embedded.toml](embedded.toml) | 固定容量、ボード用の公開名の設定。標準APIのadapter指定は不要 |
| [firmware/Pome.toml](firmware/Pome.toml)、[src/main.emel](firmware/src/main.emel) | CLIでボードを選び、汎用packageから起動するprojectと引数なしentry |
| [firmware/sensor.c](firmware/sensor.c) | projectが追加する独自の模擬センサ。制御判断とCのmainは持たない |
| [firmware/renode_stm32.py](firmware/renode_stm32.py) | CLI作成済みのFlash BINをreset起動し、UART・sample時GPIO・startup初期化・stackを確認する |
| `boards/{board}/board.emel` | 選択したSTM32F4 MCU・基板の論理GPIOとConsole UARTの配線。センサeffectとlayerの契約は共通 |
| [renode/board.c](renode/board.c) | 模擬温度入力、GPIO、USART2、SysTick、schedulerの駆動 |
| [renode/run.sh](renode/run.sh)、[run.py](renode/run.py) | ビルド、Renode起動、実行ログと資源の確認 |
| [boards/esp32c3/board.emel](boards/esp32c3/board.emel) | ESP32-C3向けの論理ピン・UART0設定。アプリ共通のセンサeffectとlayer |
| [renode-esp32c3/run.py](renode-esp32c3/run.py)、[board.c](renode-esp32c3/board.c) | 同一アプリのCLI生成、RISC-V build、GPIO・UART0・SYSTIMER・schedulerの接続と観測 |

Emela側は、現在の状態と温度から次の状態を返す純粋関数と、`State`による状態の保持を組み合わせる。装置の起動・停止を表す`Thermostat` effectはアプリの運転状態を受け取り、`Actuators` handlerが標準`Gpio`の操作へ対応付ける。センサの読み取りは`board.emel`の`TemperatureSensor`、待機は標準`Clock`、ログは標準`Console`を使用する。

`board.emel`の`Peripherals` layerが`Std.Embedded.Board`の`BoardGpio`・`BoardUart`・`BoardClock`・`BoardConsole`とセンサhandlerをまとめ、`run`の`with Peripherals`で適用すると、標準effectが共通のC ABIへ接続される。GPIO・UART等の個別`@external`やadapter対応表をアプリで定義する必要はない。独自の模擬センサへの`@external`だけを`board.emel`に置く。標準インターフェースとボードの役割は[共通APIのガイド](../../docs/embedded-interfaces.md)を参照する。

1回分の処理は`take_sample`で完了させ、その後に待機する。ログ用Stringは待機をまたいで保持せず、`Console`へ直接渡すためbyte列のListを作らない。`Actuators`の`release`は正常終了・失敗の巻き戻し時に両装置を停止する。UART設定で`PeripheralError`が発生した場合も出力を停止し、`run`は−1を返す。board defect時の停止はC側のhookで行う。

CLIは`.emel`を通常のソース読み込み・型検査に通し、多相Coreを組み込みbackendへ渡す。backend内で単相化してFlow→Cへ変換する。Flow IRを手作業で作成・差し替えする経路は使わない。`pub fn run`に対して設定した`temperature_controller_run_start`は生成された`emela_embedded_exports.h`に定義され、ボードから呼び出せる。生成関数名の末尾の番号をボード側で調べる必要はない。

### ボード入出力

| 入出力 | 内容 |
| --- | --- |
| 温度 | `board.c`の時系列を返す模擬センサ。単位は0.01℃、`INT32_MIN`は取得失敗 |
| 加熱 | PA5、HighでON |
| 冷却 | PA6、HighでON |
| ログ | USART2 TX、PA2、115200 baud相当 |
| 周期 | HSI 16MHzとSysTickの実割り込みによる1000ms待機 |

温度入力は物理センサのモデルではない。GPIO、USART2、SysTickはRenodeのSTM32モデル上で動作させる。実際のセンサに接続する場合は、Emela側の制御を保ち、`board.emel`のセンサhandlerとボード実装をADCやI²C等へ置き換える。論理ピンとUARTの指定も`board.emel`へまとめてある。

GPIOの切り替えはEmelaの`Actuators`内で行い、まず両方をOFFにしてから必要な側だけONにする。C側のGPIO関数は指定した1本を操作し、運転状態の判断は持たない。起動時・正常終了時・取得失敗時は両方OFFになる。センサ値の有効範囲は−40〜125℃とし、範囲外も取得失敗として扱う。

`Uart.configure`を`BoardUart`で処理し、USART2を実際に設定する。未対応の設定は`PeripheralError(1)`で失敗する。ログは`BoardConsole`から実USART2へ送信する。timeout付きの標準`Uart.transmit`と`Uart.receive`はこのデモでは提供せず、要求されると`PeripheralError(2)`で明示的に失敗する。GPIO割り込みは未実装で、利用時はboard defectとして停止する。

`Clock.now`を`BoardClock`で使うためのC実装も提供する。これは1970-01-01T00:00:00Zを仮想の起点とし、待機中のSysTickで進めるデモ用時計で、実際の日時を示さない。制御プログラムは`Clock.sleep`だけを使用する。0以下の待機時間は即時に完了通知を送り、main側の次のscheduler処理で再開する。`BoardConsole`は文字列のbyte長でUSART2へ書き込み、末尾に改行を1つ追加する。

### 模擬温度の流れ

最初は適温→低温→適温→高温→適温と進める。18℃／20℃／28℃／26℃の境界と、その内側での状態保持も含む。後半は加熱から冷却への直接切り替え、冷却から加熱への切り替え、温度取得失敗、失敗後の再開を観察する。取得失敗の入力は`INT32_MIN`、上限超過の125.01℃、下限未満の−40.01℃の3種類を用意する。

閾値は`controller.emel`先頭の定数、温度の系列は`renode/board.c`の`temperatures`を変更すると試せる。通常のrunnerはUARTと実際の温度入力・GPIOの整合などを確認し、既定の閾値や温度の系列への一致は要求しない。

### 連続運転

既定の測定回数24は有限デモのための設定である。`run(0)`は継続して測定する。次のコマンドではボード側の呼び出しを0にしてビルドし、26秒相当を観察したところでRenodeを終了する。

```sh
bash crates/emela-codegen-emb/examples/temperature-controller/renode/run.sh \
  --continuous --output /tmp/emela-temperature-controller-continuous
```

温度系列は繰り返す。Emelaの処理はこの時点でも動作中であり、正常終了を示す`completed`は0のままとなる。Cの`TEMPERATURE_DEMO_SAMPLES`を0にすることが連続運転の切り替えに相当する。

## 資源と確認範囲

`--optimization O0`または`O2`で最適化を選べる。既定は`O2`。

```sh
bash crates/emela-codegen-emb/examples/temperature-controller/renode/run.sh \
  --optimization O0 --output /tmp/emela-temperature-controller-O0
```

runnerはFlash、静的RAM、予約stack、実行中に観測したMSP使用量を`resources.json`へ記録する。stackは16KiB、low guardは256byteを共通linkerで予約する。固定Emela heapとscheduler poolは静的RAMに含め、C libraryの動的allocatorがlinkされていないことも確認する。現在の設定はheap 2048byte、String生成容量96byte、fiber 4、suspend frame 8個×payload 384byte、task 1、通知event 1である。fiberにはアプリ本体に加え、suspend handlerの実行に使う内部fiberも含む。

このexampleでは生成Cを書き換えてheap使用量を計測しない。heapの固定容量と静的RAM配置を確認するもので、heap使用ピークの測定値は示さない。観測したstack使用量はこの入力系列での値であり、任意の変更後の最大値やWCETを示すものではない。

ISRは時刻の更新と外部taskの完了通知を行い、Emela処理の再開とUART出力はmain側で行う。実行の確認には実UART、測定時のGPIO履歴、両装置の同時ONがないこと、待機とSysTick割り込み、有限運転の終了時の両装置OFF、stack guardを使う。

既定シナリオの24回の入力とヒステリシスの判断を独立した期待値で照合する場合は、`--verify-scenario`を付ける。これはexampleを編集した後の通常利用には不要である。

```sh
bash crates/emela-codegen-emb/examples/temperature-controller/renode/run.sh \
  --verify-scenario --optimization O2 --output /tmp/emela-temperature-controller-O2
```

この場合は24行のUARTと実GPIO、23回の待機、23000回のSysTick割り込みを確認する。開始・停止の境界、ヒステリシスによる保持、加熱と冷却の直接切り替え、異常入力と復帰を含む。

### このexampleの実行結果

標準effectと共有board handlerへ移行後、既定の24回の測定をSTM32F446／Renodeで確認した。単位はbyte。

| 最適化 | Flash割当 | 静的RAM | RAM予約合計 | 観測MSP使用量 |
| --- | ---: | ---: | ---: | ---: |
| O0 | 41,212 | 7,320 | 23,960 | 1,980 |
| O2 | 28,176 | 7,328 | 23,968 | 1,360 |

Flash割当は各ALLOC領域の合計で、image spanはO0が41,212byte、O2が28,180byteである。RAM予約合計には静的RAMに加え、stack 16,384byteとguard 256byteを含む。静的RAMには固定heap 2048byteとruntime poolを含む。両構成で24行のUART、24回の状態遷移とGPIO履歴、23回の待機と23000回のSysTick割り込みが一致し、正常終了後は両装置OFF、故障・同時ONは0、stack guardは正常だった。C libraryの動的allocatorもlinkされていない。

共有API導入前のCLI版は、O0がFlash 28,640／静的RAM 6,016／観測MSP 1,964byte、O2がFlash 20,592／静的RAM 6,016／観測MSP 1,376byteだった。共有handler・UART設定の失敗経路等を導入した結果、O2のFlashは7,584byte、静的RAMは1,312byte増えた。RAMの主な増分は論理ピン等のconst初期化に使う診断buffer 1,024byteと緊急領域177byteで、heapやpoolの予約容量は増やしていない。

`--continuous --verify-scenario`でも26秒相当実行し、26回の測定まで進んだ。25回の待機完了後も`completed=0`で次の待機中となり、故障・同時ONは0、stack guardは正常だった。このときのFlash・静的RAM・観測MSPは上表のO2と同じだった。

別の64bit host用の手動harnessでは、同じEmelaソースをCLIで生成し、有限運転を1000回（合計24000回の測定）実行した。各運転の解放後に固定heap使用量0を確認し、負の測定回数とUART設定失敗時の両装置停止、継続運転2400回の測定も確認した。観測したfiberは3、suspend frameは5だった。64bitのpointerと値配置に合わせ、このhost確認だけはheapを8192byte、frame payloadを1024byteに増やした。STM32の設定・メモリ測定値とは区別し、hostの結果をSTM32のheap使用ピークや最大stackの根拠には使わない。

このexampleは言語仕様全体を網羅する検証ではない。既存のSTM32F446受入fixtureは回帰確認として残す。

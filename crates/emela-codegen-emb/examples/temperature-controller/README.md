# 温度調節器

温度を1秒ごとに読み、低温なら加熱、高温なら冷却するSTM32F446向けのexampleである。温度と運転状態はUARTへ表示し、加熱・冷却装置への指示を2本のGPIOで出力する。

温度だけで次の出力を決めると、境界付近で装置が頻繁に切り替わる。ここでは「現在の運転状態」を保持し、開始温度と停止温度を分ける。

| 装置 | 開始条件 | 停止条件 |
| --- | --- | --- |
| 加熱 | 18℃以下 | 20℃以上 |
| 冷却 | 28℃以上 | 26℃以下 |

例えば19℃では、停止中なら停止を続け、加熱中なら加熱を続ける。加熱中に29℃へ上昇した場合は、加熱を停止して冷却へ切り替える。温度が取得できない場合は両方OFFにして`fault`へ移り、次の有効な入力で制御を再開する。

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

CLIはCとheaderの生成までを担当する。ARM向けのcompile・linkとRenode起動は、以下のボード用runnerで行う。

## Renodeで実行する

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

## アプリケーションを読む

```text
temperature-controller/
├── controller.emel
├── board.emel
├── embedded.toml
├── README.md
└── renode/
    ├── board.c
    ├── run.sh
    └── run.py
```

| ファイル | 役割 |
| --- | --- |
| [controller.emel](controller.emel) | 状態遷移、状態の保持、温度取得と出力、周期待機 |
| [board.emel](board.emel) | 論理ピンとUART設定、温度センサeffectとボード固有の接続 |
| [embedded.toml](embedded.toml) | 固定容量、ボード用の公開名の設定。標準APIのadapter指定は不要 |
| [renode/board.c](renode/board.c) | 模擬温度入力、GPIO、USART2、SysTick、schedulerの駆動 |
| [renode/run.sh](renode/run.sh)、[run.py](renode/run.py) | ビルド、Renode起動、実行ログと資源の確認 |

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

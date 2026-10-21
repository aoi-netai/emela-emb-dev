# ESP32-C3のRenode導入

STM32以外への移植を進めるため、RISC-VのESP32-C3を対象にする。[独立したC probe・setup・runner](../examples/renode-esp32c3/README.md)に加え、[共通Emela温度調節器](../examples/temperature-controller/README.md#esp32-c3renodeで実行する)をCLIから生成し、標準effect・schedulerを接続するrunnerを提供する。

## 基礎probeの境界

- CLI、core、syntax、Flow IR、既存STM32 backendを変更しない。
- Renode本体は既存installを使い、外部周辺機器モデルを外部cacheからロードする。
- 既定probeはboot ROM・ESP-IDFを使わず、64 KiBのSRAM内でCPU・UART・GPIO register操作・実timer割り込みを確認する。
- 独立probeはEmelaコード生成・schedulerを含まない。これらの接続は下記の温度調節器runnerで確認する。

## 構成

```text
setup.py
  ├─ 固定commitの外部C# modelを外部cacheへ取得
  └─ SHA256固定のEspressif toolchainを外部cacheへ取得
run.py
  ├─ startup.S + probe.c + firmware.ld → ELF・map
  ├─ 必要な4 model + 64 KiB SRAM + CPU/CLIC → 最小platform
  └─ headless Renode → UART・ELF symbolの観測 → report
```

IRAM `0x40380000`とDRAM `0x3fc80000`は同じ64 KiBへのaliasとし、code 16 KiB・data 32 KiB・stack 16 KiBを物理的に重ならない領域へ分ける。[ESP-IDF v5.4.1のSoC定義](https://github.com/espressif/esp-idf/blob/v5.4.1/components/soc/esp32c3/include/soc/soc.h)に対応する範囲を使う。stack内には256 byteのguardを含める。

実割り込み経路は`SYSTIMER target0 → interrupt matrix source37 → CLIC line1 → CPU trap → C ISR → mret → main`。タイマーは4回、10 msずつone-shotで再設定する。runnerがPCを書き換えてISRを呼び出したり、pending flagを注入したりしない。

CLIC registerと`MTVEC`の初期設定はrunnerが行う。RenodeのCLICモデルに合わせた設定であり、実チップのreset・controller全体の再現を主張しない。`mcause`は[CLIC仕様](https://github.com/riscv/riscv-fast-interrupt/blob/v0.10/src/clic.adoc#changes-to-mcause-csr)のMPP/MPIE付きの値として記録し、interrupt bitとinterrupt IDを検査する。

## 今回の実行結果

2026-10-21、Renode `1.16.1 (d66b0c2a-202602160923)`、Espressif GCC `14.2.0`で確認した。

| 確認 | 結果 |
| --- | --- |
| 空cacheからのsetup | 固定Git commit、toolchain archive、optional ROM archive/ELFを取得・検証して成功 |
| O0 probe | UART開始/4進捗/完了、IRQ 4回、main 4回、GPIO trace 8step、stack guard正常、Renode warning 0 |
| O2 probe | 同じ観測条件で成功、Renode warning 0 |
| 空白を含む出力path | 独立したO2実行で同じ成功条件を確認 |
| 失敗と再実行 | 未管理directoryを拒否して既存fileを保持、変更済みmodelを拒否、短すぎるsimulationを失敗とし、同じ出力先の再実行で新しい成功reportへ更新 |
| SDK Hello | `Calling app_main()`・`Hello world!`・`silicon revision`を確認。既知RTC warning 6行のみ許容 |

| C probeの資源 | O0 | O2 |
| --- | ---: | ---: |
| ELF `text` | 2,520 byte | 2,004 byte |
| ELF `data` | 0 byte | 0 byte |
| ELF `bss`（下記stack予約を含む） | 16,540 byte | 16,540 byte |
| 観測stack | 232 byte | 196 byte |

`bss`は通常の156 byteと16,384 byteのstack予約を合わせた値。stack予約には256 byteのguardを含む。heap allocator・C library heapは使わない。SRAMのmapは64 KiBであり、ELF sectionがその全域を消費するという意味ではない。これは独立probeの値であり、後続のEmelaアプリの必要量ではない。

生の`mcause`は4回とも`0xb8000001`で、CLICの状態bitを除いたinterrupt IDは1。16 MHzのtimer tickが各回160,000以上増え、割り込まれたPCがprobeのIRAM内であることを確認した。UART完了だけで成功にせず、停止後のELF symbolから同じ結果を読み取る。

再現用のcommandは[example README](../examples/renode-esp32c3/README.md)を参照。実行時の完全なcommand、SHA256、観測値は各出力directoryの`report.json`・`metrics.json`に保存する。compile失敗・timeout・Renode command errorと、probeのwarning・未処理MMIOを失敗扱いにする。SDK Helloは記録済みRTC offset `0x80`のwarningだけを許容する。

## 共通Emelaアプリの接続

温度調節器の`controller.emel`をbyte単位で同一のまま使用し、`boards/esp32c3/board.emel`でGPIO3・GPIO4・UART0を選ぶ。runnerはこの2fileを出力先の`source/`へ置き、通常の`emela build --target c-embedded`で生成する。AST・Core・Flow IRや生成Cの書き換えは行わない。CLI、stdlib、codegenの変更もこの移植には不要だった。

```sh
bash crates/emela-codegen-emb/examples/temperature-controller/renode-esp32c3/run.sh \
  --verify-scenario --optimization O2 --output /tmp/emela-temperature-esp32c3-O2
```

`RENODE`・`EMELA_ESP32C3_CACHE`や`--cache`・`--toolchain`の指定は基礎probeと同じ。`--optimization O0`、`--continuous`、`--seconds`で条件を変えられる。完全なcommandとsource/model SHA、UART、RAM上の観測値、section別の資源を出力先へ保存する。

標準`BoardGpio`・`BoardUart`・`BoardClock`・`BoardConsole`の共通C ABIへC3固有のMMIO操作を接続する。模擬温度入力と独自sensor effectの接続だけはアプリ用board moduleに残す。1ms SYSTIMER割り込みは残り時間を更新し、1000回で外部task完了を通知する。Emelaの再開・状態遷移・文字列変換・UART出力はmainで行う。mainは待機中WFIで休止する。

温度調節器用`board.c`のGPIOは3・4の`OutputPushPull`／`NoPull`に限定し、readは出力latch、write・toggleもこの2本を扱う。GPIO割り込みはboard defectで停止する。UART0は115200/8N1/TXのconfigureとConsole出力だけを提供し、timed transmit/receiveは`PeripheralError(2)`で失敗する。`Clock.now`はvirtual epochを1970-01-01T00:00:00Zとしたデモ時計で、実日時ではない。modelにregisterやendpointが存在することと、このboard実装がすべての標準操作を提供することを区別する。

GPIOには[`EmelaESP32C3_GPIO`](../examples/renode-esp32c3/models/EmelaESP32C3_GPIO.cs)を追加した。software output routeと出力enableを検査し、GPIO3・4を外部LEDと独立observerへfanout接続する。observerは実endpointの変化だけを記録する。各UART行のmodeから期待したOFF→ON遷移を求め、全外部pin遷移、最終LED、register値、C側sample履歴を照合する。同時ONが一瞬でも生じた場合は失敗とする。

温度調節器は128 KiBのSRAM sliceを使う。IRAM `0x40380000`からcode用64 KiB、同じSRAMのDRAM alias `0x3fc90000`からdata用48 KiB、`0x3fc9c000`からstack用16 KiBへ分け、物理offsetを重ねない。初期のcode32 KiB構成ではO0が20,660byte超過したため拡張した。stackの16 KiBには256byteのguardを含む。128 KiBはmodelが扱う上限であり、実際の消費量は`text`・静的data・stack予約の合計で別に示す。

boot ROM・flash・ESP-IDFは使わない。CLIC/MTVECのmonitor初期化は基礎probeと同じで、実行中にPCを変更してISRを呼ぶhookはない。これはSRAM実行でのbackend/board境界の確認であり、実機のreset・flash起動・書き込みを確認したものではない。

### 温度調節器の観測結果

2026-10-21、Renode 1.16.1と固定Espressif GCC 14.2.0で、O0・O2とも24sample、UART 944byte、23回のtask通知、23,000回のtimer割り込みを確認した。外部GPIO observerの26遷移はsampleの運転状態から求めた順序と完全一致し、両LEDの最終状態はOFF、同時ON・board failureは0、stack guardは正常だった。

| 温度調節器の資源（byte） | O0 | O2 |
| --- | ---: | ---: |
| code・定数のSRAM | 53,428 | 33,748 |
| 静的data SRAM | 7,396 | 7,404 |
| stack予約（guard 256byteを含む） | 16,384 | 16,384 |
| SRAM割当合計 | 77,208 | 57,536 |
| 観測stack | 2,140 | 1,400 |

`size`の`bss`はO0が23,780byte、O2が23,788byteで、上記の静的dataとstack予約を合算した値である。固定heap 2,048byte、fiber 4、frame 8×payload 384byte、task 1、event 1の設定を維持した。O2の実体はheap storage 2,048byte、frame pool 3,200byte、fiber pool 240byte、task pool 28byte、event ring 16byteで、これらを静的dataへ含める。C libraryの動的allocatorはlinkされていない。heap使用ピーク・解放後残量は測定していない。

128 KiBをすべて消費したという結果ではなく、IRAM/DRAM aliasを同じSRAMとして照合したALLOC sectionの合計が上表のSRAM割当合計である。stackの値は今回の入力系列での観測値であり、任意のアプリでの最大値や割り込み応答時間を保証しない。

O2継続運転は26秒相当で26sample、120秒相当で120sampleまで確認し、いずれも固定heap・poolの設定と観測stack 1,400byteを維持した。UART・外部pin遷移・input/GPIO履歴は一致し、guard正常、failure・同時ONは0。停止時は次の待機中で`completed=0`である。mainの待機はMIEをmaskしたevent確認とWFIを組み合わせ、`WfiAsNop`等でbusy loopへ置き換えていない。

共有の期待値checkerはpin maskと出力名を引数化し、STM32では従来値を既定にした。同じ変更後にSTM32F446のO2温度調節器と旧ESP32-C3の独立O2 probeを再実行し、従来の観測条件が通ることも確認した。

## 制限と次の作業

| 項目 | 今回の状態 | 次の作業 |
| --- | --- | --- |
| RISC-V CPU / UART0 / SYSTIMER IRQ | 独立probeと共通Emela温度調節器で観測する | 他のアプリ・割り込み条件の追加 |
| GPIO | 独自modelの外部LED・observerで温度調節器の出力を検査 | 他pin・入力・GPIO割り込み等を必要に応じて追加 |
| I²C / 汎用SPI | probeへmodelを含めない。上流はplaceholder | model・実driverの対応範囲を固めてから追加 |
| Wi-Fi / BLE / ADC / DMA等 | 今回の対象外 | 必要な機能ごとに実装・検証 |
| ESP-IDF SDK Hello | 別caseで上流prebuiltを実行 | 固定PC hookやRAM拡張を量産用board構成へ持ち込まない |
| CLIのboard選択 | 温度調節器runnerでboard moduleとconfigを選ぶ | configの`board`によるパッケージ選択・build/書き込み契約を設計 |

SDK HelloはROM entryから上流の固定PC hookでfirmwareへ移り、固定interrupt dispatch hookを使う。480 KiB RAMと23 placeholderを含むため、通常probeと分ける。SDK Helloが起動しても、未実装の周辺機器が動く根拠にはしない。

次はこの[共通周辺機器ABI](embedded-peripherals.md)への接続をボード支援パッケージに整理し、board選択・配線設定・toolchainによる最終build・flash書き込みの契約を固める。I²C・SPI等はmodelとdriverの双方が動く根拠を揃えてから対応を広げる。

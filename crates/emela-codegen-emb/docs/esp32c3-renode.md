# ESP32-C3のRenode導入

STM32以外への移植を進めるため、RISC-VのESP32-C3を追加候補とし、まずエミュレーター実行環境を作る。今回の成果物は[独立したC probe・setup・runner](../examples/renode-esp32c3/README.md)である。

## 今回の境界

- CLI、core、syntax、Flow IR、既存STM32 backendを変更しない。
- Renode本体は既存installを使い、外部周辺機器モデルを外部cacheからロードする。
- 既定probeはboot ROM・ESP-IDFを使わず、64 KiBのSRAM内でCPU・UART・GPIO register操作・実timer割り込みを確認する。
- Emelaコード生成、scheduler、標準`Std.Embedded.*`のESP32-C3実装はまだ接続しない。

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

## 制限と次の作業

| 項目 | 今回の状態 | 次の作業 |
| --- | --- | --- |
| RISC-V CPU / UART0 / SYSTIMER IRQ | 独立firmwareで観測する | Emela runtimeを載せて同じ経路を再確認 |
| GPIO | MMIOと内部loopbackのみ。上流modelに外部pin endpointがない | 外部pin modelまたは明示的なadapterを設計し、温度調節器の出力を検査 |
| I²C / 汎用SPI | probeへmodelを含めない。上流はplaceholder | model・実driverの対応範囲を固めてから追加 |
| Wi-Fi / BLE / ADC / DMA等 | 今回の対象外 | 必要な機能ごとに実装・検証 |
| ESP-IDF SDK Hello | 別caseで上流prebuiltを実行 | 固定PC hookやRAM拡張を量産用board構成へ持ち込まない |
| CLIのboard選択 | 今回変更なし | ボード支援パッケージのbuild/書き込み契約を設計 |

SDK HelloはROM entryから上流の固定PC hookでfirmwareへ移り、固定interrupt dispatch hookを使う。480 KiB RAMと23 placeholderを含むため、通常probeと分ける。SDK Helloが起動しても、未実装の周辺機器が動く根拠にはしない。

次の移植では、[共通周辺機器ABI](embedded-peripherals.md)に対するESP32-C3 board実装を追加し、同じEmela温度制御ロジックを使う。board選択・配線設定・SDK/toolchainによる最終buildを切り分け、coreやFlow IRの変更が必要かを実際の生成Cで判断する。

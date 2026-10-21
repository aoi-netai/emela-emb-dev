# STM32F4の共通BSPとRenode対象一覧

STM32F4の実装は`boards/stm32/common/`と`boards/stm32/stm32f4/`へまとめる。core・syntax・Flow IRは変更しない。シリーズ共通のレジスタ操作、MCUの容量・bonded pin・IRQ表、基板の配線を分離し、CLIへBSPを同梱する。

## CLIの対象

| ボードID | MCU | 通常SRAM | Flash | ConsoleのTX/RX | 温度exampleの加熱/冷却 |
| --- | --- | ---: | ---: | --- | --- |
| `stm32f407vg` | STM32F407VG | 128 KiB | 1 MiB | USART2 PA2/PA3 AF7 | PD12/PD13 |
| `stm32f412zg` | STM32F412ZG | 256 KiB | 1 MiB | USART6 PC6/PC7 AF8 | PD12/PD13 |
| `stm32f429zi` | STM32F429ZI | 192 KiB | 2 MiB | USART1 PA9/PA10 AF7 | PD12/PD13 |
| `stm32f446re` | STM32F446RE | 128 KiB | 512 KiB | USART2 PA2/PA3 AF7 | PA5/PA6 |
| `stm32f4-discovery` | STM32F407VG | 128 KiB | 1 MiB | USART2 PA2/PA3 AF7 | PD12/PD13（基板LED） |
| `nucleo-f446re` | STM32F446RE | 128 KiB | 512 KiB | USART2 PA2/PA3 AF7 | PA5/PA6 |
| `stm32f446` | STM32F446RE | 128 KiB | 512 KiB | USART2 PA2/PA3 AF7 | PA5/PA6（既存ID互換） |

MCU名のIDはチップprofileであり、特定の基板の搭載部品や配線全体を示さない。DiscoveryとNucleoのIDは基板profileである。NucleoのPA6は追加配線する冷却出力で、加熱出力PA5だけが標準LEDへつながる。DiscoveryのConsoleは上記pinへ外部UARTを接続する想定である。

SRAMは`0x20000000`から連続する通常SRAMに限定する。F407/F429のCCMや外付けSDRAMを通常SRAMへ足さない。初期clockはHSI 16 MHzのまま使い、PLL・HSE・基板水晶へ依存しない。GPIO論理IDは`port * 16 + pin`で、A=0、B=1、…となる。packageに存在しないpinとConsole予約pinを検査する。

```sh
emela build crates/emela-codegen-emb/examples/temperature-controller/firmware \
  --board stm32f4-discovery --out-dir /tmp/emela-discovery
emela flash crates/emela-codegen-emb/examples/temperature-controller/firmware \
  --board stm32f4-discovery --out-dir /tmp/emela-discovery --dry-run
```

`--dry-run`を外すとST-Link/OpenOCDで書き込み・verify・resetする。実機への転送結果は、この文書のRenode確認には含まない。MCUの追加時は正しいMCUの容量・pin・IRQ表と必要な基板配線profileを追加する。F4という名前だけで全MCUを同一設定にしない。

## 固定したRenodeの範囲

Renode **1.16.1**の`platforms/`を監査した。F4に該当する`.repl`は7ファイルで、CPU定義3、基板定義1、合成kit1、単体でloadできないfragment2である。

| matrix case | 使用する上流platform | CLI profile | 分類 |
| --- | --- | --- | --- |
| `cpu-f407` | `cpus/stm32f4.repl` | `stm32f407vg` | 汎用CPUモデルを実F407VG容量へ制限 |
| `cpu-f412` | `cpus/stm32f412.repl` | `stm32f412zg` | CPU |
| `cpu-f429` | `cpus/stm32f429.repl` | `stm32f429zi` | CPU |
| `discovery` | `boards/stm32f4_discovery.repl` | `stm32f4-discovery` | 基板 |
| `discovery-additional-gpios` | Discovery + `stm32f4_discovery-additional_gpios.repl` | `stm32f4-discovery` | 基板へfragmentを合成 |
| `discovery-bb` | Discovery + `stm32f4_discovery-bb.repl` | `stm32f4-discovery` | 基板へfragmentを合成 |
| `discovery-kit` | `boards/stm32f4_discovery-kit.repl` | `stm32f4-discovery` | 上記3つを含むkit |
| `cpu-f446re` | `cpus/stm32f4.repl` | `stm32f446re` | 汎用モデルでのMCU profile確認 |
| `cpu-f446-compat` | `cpus/stm32f4.repl` | `stm32f446` | 旧CLI ID互換 |
| `nucleo-f446re` | `cpus/stm32f4.repl` | `nucleo-f446re` | 汎用モデルでの基板profile確認 |

F446専用のCPUモデルやNucleo基板モデルがRenodeにある、という意味ではない。`bb`はEthernet PHYの追加、`additional_gpios`は外部button/LEDの追加であり、それぞれを独立した基板数へ数えない。合成platformがloadされ、温度アプリと共存することを確認する。Ethernet通信、button入力、その他の全周辺機器の検証は含まない。

## 一括確認

Python、Arm GNU toolchain、Renode 1.16.1を用意し、workspace rootから実行する。STM32Cubeは不要である。

```sh
python3 crates/emela-codegen-emb/tools/run_stm32f4_matrix.py \
  --output /tmp/emela-stm32f4-matrix \
  --renode /path/to/renode --toolchain /path/to/arm-toolchain
```

`RENODE`・`ARM_TOOLCHAIN_PATH`でも指定できる。CLIを`cargo build --locked -p emela-cli`で作成し、7 CLI profileをO0/O2でbuildする。同じ成果物を必要なplatform合成で再利用し、10 case×2最適化の20回を実行する。Renodeの同時実行数は最大2、各processの`TMPDIR`は別directoryである。

`--emela /path/to/emela`ではインストール済みまたはcopyしたCLIを使用する。各projectは出力先の`standalone project/`へ自己完結する形でcopyし、同じ`controller.emel`のSHA-256を照合する。projectには空白を含むpathを使い、CLIはproject directoryを作業directoryとして起動する。通常のmodule overlay・C生成・同梱BSP・Flashビルド経路を通り、生成Cを書き換えない。

部分確認は`--platform cpu-f412 --optimization O2`などで絞れる。部分確認はreportの対象一覧で全件確認と区別する。空またはrunner所有のdirectoryのみを出力先に指定でき、同じ出力先の同時使用は拒否する。`report.json`は今回の開始時点で`running`へ更新する。

### 確認する内容

- 上流F4 platformのファイル一覧とSHA-256を記録する。追加・欠落したファイルがある場合は対象一覧を見直すため失敗する。
- `.repl`の変更はネット取得専用の`ApplySVD`メタデータと除去後に空になるinit blockの除去、BOOT0=0のFlash alias、実MCU容量、HSI用SysTick周波数、独立GPIO観測endpointの追加である。命令・UART・IRQ・schedulerをPC hookへ置換しない。
- ELFは`nm`でsymbol addressと固定pool容量を調べるだけに使う。Flash BINを`0x08000000`へloadし、同一Flashを`0x00000000`へaliasする。`.data`/`.bss`の観測wordを汚染した後、CPU resetでvectorから起動する。monitorでPC・SP・VTORを設定しない。
- UART全24行の温度とmode、24回のGPIO出力状態、実SysTick 23,000回、task通知23回、終了結果24を照合する。ConsoleのTX/RXのGPIO MODER、AFRL/AFRH、GPIOとUARTのRCC enable bitもprofileへ照合する。電気的配線の検証とは区別する。
- GPIOの独立receiverで全出力遷移を記録し、期待する切り替え順序と照合する。加熱・冷却の同時ONがなく、最後にLED endpointも両方OFFであることを検査する。
- stack high-water、guard、linkerの実容量制限、固定heap 2,048 byte、pool symbol、system allocator非依存を記録する。heapピークは測定しない。
- Renodeのwarning/errorを失敗として扱う。未対応MMIOや存在しない周辺機器を成功扱いしない。
- 各buildに対し`emela flash --dry-run`の鮮度検査とOpenOCD argv生成まで確認する。実機へは接続しない。

単体のFlash起動確認には[温度exampleのrunner](../examples/temperature-controller/firmware/renode_stm32.py)で`--platform`を指定する。旧`stm32f446`向けの呼び出しは引数追加なしで互換にする。

## 確認結果

2026-10-21、Renode 1.16.1・Arm GNU GCC 13.2.1で上記commandを実行した。7 CLI profileのO0/O2の**14 buildすべて成功**、Flash resetの**20 caseすべて成功**した。

| matrix case | O0 | O2 |
| --- | --- | --- |
| `cpu-f407` | 成功 | 成功 |
| `cpu-f412` | 成功 | 成功 |
| `cpu-f429` | 成功 | 成功 |
| `discovery` | 成功 | 成功 |
| `discovery-additional-gpios` | 成功 | 成功 |
| `discovery-bb` | 成功 | 成功 |
| `discovery-kit` | 成功 | 成功 |
| `cpu-f446re` | 成功 | 成功 |
| `cpu-f446-compat` | 成功 | 成功 |
| `nucleo-f446re` | 成功 | 成功 |

全caseで24行のUART（944 byte）、24sampleの温度・mode・GPIO、実SysTick 23,000回、task通知23回、終了結果24が一致した。失敗・stale通知は0、外部GPIOは期待する26遷移で同時ONはなく、最後は両LEDがOFF、stack guardは正常だった。USART1/2/6の選択、APB1/2 clock、TX/RXのAF7/AF8もregister readbackが一致した。Renodeのwarning/errorは0だった。

### 資源

単位はbyte。Discoveryと各fragment/kitはF407VGの値、旧`stm32f446`とNucleoはF446REの値と同じだった。

| MCU | 最適化 | Flash BIN | 静的RAM | RAM予約合計 | 観測MSP |
| --- | --- | ---: | ---: | ---: | ---: |
| F407VG | O0 | 41,800 | 7,064 | 23,704 | 1,980 |
| F407VG | O2 | 28,544 | 7,072 | 23,712 | 1,360 |
| F412ZG | O0 | 41,860 | 7,064 | 23,704 | 1,980 |
| F412ZG | O2 | 28,644 | 7,072 | 23,712 | 1,360 |
| F429ZI | O0 | 41,836 | 7,064 | 23,704 | 1,980 |
| F429ZI | O2 | 28,564 | 7,072 | 23,712 | 1,360 |
| F446RE | O0 | 41,860 | 7,064 | 23,704 | 1,980 |
| F446RE | O2 | 28,608 | 7,072 | 23,712 | 1,360 |

RAM予約合計は静的RAM、stack 16,384 byte、guard 256 byteの合計である。固定Emela heapは全caseで2,048 byteのまま、各poolとともに静的RAMへ含む。system allocatorはlinkされていない。共通化前F446のO2と比べると、静的RAMは24 byte、Flashは340 byte増え、観測MSPは1,360 byteで同じだった。GPIO port別の状態保持を追加したことによる小さな増分で、heapやstack予約を拡大していない。

観測stackはこの入力系列の値で、任意のアプリの最大stackやWCETを示すものではない。heap使用ピークや全言語機能の網羅もこのmatrixの測定対象ではない。

### 手動で確認した境界

- 空白を含むproject/outputを通常のCLIでcheck/buildし、14 buildすべてのOpenOCD dry-runが成功した。
- checkout外へcopyしたCLIからも4 MCU profileをcheck/build/dry-runできた。元の温度exampleの`{board}` overlayでもF412への切り替えが成功した。
- 同じSHA-256のCLIをcheckout外の空白pathへcopyした独立確認でも、Discovery・F412・旧F446互換のO2を再check/build/dry-runし、別outputからのFlash resetが3/3成功した。
- F429 projectで`USART1_IRQHandler`をstrong定義し、Flash BINのIRQ37 vectorがそのhandlerのThumb addressへ解決されることを確認した。
- 既存の`firmware/renode_stm32.py`も`--platform`を追加せずに実行でき、旧`stm32f446`のO2で24sample・26 GPIO遷移が成功した。
- 未管理の非空outputを拒否し、既存の利用者file/reportを変更しなかった。
- 以前成功した観測outputへ1秒timeoutで再実行すると、古い成功を残さず今回の`failed`と`timed_out: true`を記録した。

対象・platform SHA-256・共通観測値と資源は結果の要約JSONにも保存する。ELF・BIN・生成C・raw logはcommitしない。

## 関連資料

- [シリーズ共通BSP](../boards/stm32/stm32f4/README.md)
- [CLIの設定と鮮度検査](cli-embedded.md)
- [共通の温度調節器](../examples/temperature-controller/README.md)
- 既存F446の言語runtime受入：本matrixは言語仕様全体を再検証するものではない。

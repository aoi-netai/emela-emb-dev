# ESP32-C3ボードpackage

Emela生成CとこのpackageをPythonの`build.py`でRISC-Vへcompileし、`firmware.elf`・`firmware.bin`・`firmware.map`・`resources.json`を生成する。標準entryは引数なし・`Int`戻り値のsuspend関数で、`generated/emela_embedded_exports.h`の`board_main_start` aliasから呼び出す。schedulerはmain contextで実行し、SysTimer ISRは完了通知だけを行う。通常のprojectは[Emela CLIの開発手順](../../../emela-cli/README.md)でboardを選び、生成・build・flashを行える。以下はRust APIを使うstandalone手順である。

## 前提ツール

- Espressif RISC-V GCC。検証版は`esp-14.2.0_20241119`。`riscv32-esp-elf-gcc`をPATHへ置くか、toolchain rootを`EMELA_ESP32C3_TOOLCHAIN`へ指定する。
- Python 3と公式`esptool==4.8.1`。buildを実行するPython環境へ`python3 -m pip install esptool==4.8.1`で導入する。別のPythonを使う場合は`EMELA_ESPTOOL_PYTHON`を指定する。
- 実機への書き込み時だけ、UART download modeに入れるESP32-C3とserial portが必要。`--dry-run`はデバイスへ接続しない。

## Rust APIから生成してstandalone buildする

repo rootから次のように生成する。`board_probe`はGPIO3をHighにし、Consoleへ`board probe: high`を出力、10ms待機、GPIOをLowにし、`board probe: low`を出力して42を返す。出力先に`generated/`と`bsp/`ができる。

```sh
cargo run -p emela-codegen-emb --example board_probe -- \
  --board esp32c3 --out-dir /tmp/emela-board-esp32c3

python3 /tmp/emela-board-esp32c3/bsp/build.py build \
  --generated-dir /tmp/emela-board-esp32c3/generated \
  --output-dir /tmp/emela-board-esp32c3/artifacts \
  --project-dir /tmp/emela-board-esp32c3 \
  --optimization O2
```

`--optimization`は`O0`／`O2`を受け付ける。追加のC／assembly sourceは`--source`で指定する。独自のEmelaソースを使う場合は既存frontendでCoreを作り、`compile`／`compile_flow`とboard package APIを組み合わせる。[ボード接続と確認](../../README.md#同梱bsp)に生成例と観測方法を記載する。

## Flash起動方式

標準ESP imageをFlash offset **0x0**へ書き込む。ROMがIRAM・DRAMのsegmentsをSRAMへloadしてentryへ移る。全コードがSRAMへ収まる構成なのでESP-IDFのsecond-stage bootloader・partition tableは使わない。[Espressif公式の説明](https://github.com/espressif/esp32c3-direct-boot-example#alternatives-to-direct-boot)がこの方法を案内している。rev3専用のmagicによるXIP direct bootとは異なる。

書き込み設定はDIO・40MHz・4MB Flash。SPI Flashへの書き込みとresetは公式esptoolへ任せる。secure boot・Flash暗号化・OTA・rollbackはこのpackageの対象に含めない。

SRAM1の128KiBだけを使う。物理aliasの重複をlinkerで検査し、ROM loaderのstack（ESP-IDF v5.4.1の`SOC_ROM_STACK_START=0x3fcde710`、サイズ`0x2000`）より下に置く。

| 用途 | CPU address | 予約量 |
| --- | --- | ---: |
| コード・定数 | `0x40380000` | 64KiB |
| data・bss・Emela固定pool | `0x3fc90000` | 48KiB |
| stack（256byte guardを含む） | `0x3fc9c000` | 16KiB |

startup自身がgp・sp・bss・guardを初期化し、256byte整列した32entryのvector tableを`mtvec` mode 1へ登録する。RTC/TG watchdogとsuper watchdogを停止し、IO_MUX、UART0の40MHz XTAL clock、SysTimer、interrupt matrixを設定する。libcの動的heap allocatorをlinkした場合はbuildを失敗させる。

## 標準APIの範囲

- GPIO番号は物理GPIO番号。0〜10でInput・OutputPushPull・OutputOpenDrain、NoPull・Up・Down、出力初期Low/Highを実装する。11〜17はFlash、18/19はUSB Serial/JTAG、20/21はConsoleの予約として`GpioError(ResourceConflict)`で拒否する。Analogは`Unsupported`。静的binding、Rising/Falling/Both、ISR専用read/writeにも対応し、GPIO全pinで共有するCPU IRQ2の優先度競合を検査する。
- ConsoleはUART0 TX=GPIO21 / RX=GPIO20、115200/8N1。通常UARTは`Uart1`=UART1、TX=GPIO4 / RX=GPIO5に分離し、同期送受信・受信ユーティリティ・静的割り込みbindingを提供する。UART0は`UartError(ResourceConflict)`で拒否する。[対応設定・IRQ・復旧](../../docs/embedded-interfaces/uart.md)、[検証](../../docs/embedded-interfaces/uart.md#検証)を参照する。
- `Clock.now`は起動からの単調増加millisecondを仮想Unix epochへ対応させる。実日時のRTCではない。`Clock.sleep`は同時1件をSysTimer割り込みで完了する。
- I²Cは`I2c1`で7bit master・8/16bitレジスタ・応答確認・転送全体のISR通知を提供する。SCL/SDAはGPIO6/GPIO7、I2C0。外付けpull-upが必要。GPIOとのpin競合を検査し、共通期限と有界なSTOP/abortを使う。[対応設定・IRQ・復旧](../../docs/embedded-interfaces/i2c.md)、[検証](../../docs/embedded-interfaces/i2c.md#検証)を参照する。
- SPIは`Spi2`=SPI2、SCK=GPIO0 / MISO=GPIO10 / MOSI=GPIO1、GPIO matrix、XTAL 40MHz。Master/TwoLine・8/16bit・mode 0〜3・両bit順を実装する。CSは基本APIではアプリが制御し、上位APIでは固定4件のdeviceへ予約してtransaction/recoverで管理する。SPI・GPIO・UART・I²C間のpin競合を拒否する。[対応設定と復旧](../../docs/embedded-interfaces/spi.md)、[検証](../../docs/embedded-interfaces/spi.md#検証)を参照する。ADC等のdriverは対象外である。

Emela heapは設定された固定領域の中だけを使う。`resources.json`の静的RAMには固定heap・poolが含まれる。stack予約量と実行で観測した使用量は区別する。

Clock taskを取消した場合も物理timerは旧期限まで保持する。その間の新たなsleepは容量超過としてdefectになる。期限のstale通知はruntimeのgeneration検査で拒否し、`board_rejected_notifications`へ数えて無視する。Emelaの再開や解放はISRから行わない。

## standaloneの書き込み

```sh
python3 /tmp/emela-board-esp32c3/bsp/build.py flash \
  --output-dir /tmp/emela-board-esp32c3/artifacts \
  --port /dev/ttyUSB0 --dry-run

python3 /tmp/emela-board-esp32c3/bsp/build.py flash \
  --output-dir /tmp/emela-board-esp32c3/artifacts \
  --port /dev/ttyUSB0
```

`flash`は成功したbuildのmanifestと成果物hashを照合する。失敗したbuildの古いmanifestや改変されたimageは書き込まない。standaloneの照合対象はbuild成果物である。Emelaソースや生成設定の鮮度まで照合する場合は`emela build`／`emela flash`を使う。ソースを変更した場合はC生成とbuildをやり直す。

実行前にGPIO9をLowにしてresetするUART download modeが必要になるボードもある。詳細は[公式boot mode説明](https://docs.espressif.com/projects/esptool/en/latest/esp32c3/advanced-topics/boot-mode-selection.html)を参照する。

## emulatorとの境界

このpackageは実チップの`mtvec` mode 1とROMからのFlash起動を対象にし、CLIC modeやCPU dispatch hookには依存しない。Flash起動の検証にはEspressif公式QEMU `esp-develop-9.2.2-20260417`を使う。必要なQEMUとライブラリはrepo外に用意する。

[QEMU runner](../../tools/board_probe_esp32c3.py)はbuild manifestとimageを検査し、Flash imageの末尾だけを`0xff`で4MBへ補完してSPI Flashへ接続する。`-kernel`・`-bios`によるアプリ注入を行わず、QEMU内蔵ROMのpower-onから起動し、Console・10msのIRQ待機再開・結果42・stack guardを観測する。再実行手順と確認範囲は[ボード接続と確認](../../README.md#同梱bsp)を参照する。

この版の[QEMU GPIO model](https://github.com/espressif/qemu/blob/esp-develop-9.2.2-20260417/hw/gpio/esp32_gpio.c)はstrap以外のreadを0にし、writeを無視する。このためQEMUはGPIO3の物理出力遷移を検証する根拠にはしない。実機の配線・serial転送は別の確認範囲である。

レジスタの根拠は[ESP-IDF v5.4.1 ESP32-C3 headers](https://github.com/espressif/esp-idf/tree/v5.4.1/components/soc/esp32c3/register/soc)、[起動処理](https://docs.espressif.com/projects/esp-idf/en/v5.4.1/esp32c3/api-guides/startup.html)。Flash imageのchecksum・SHA-256・chip ID・entry・load範囲はbuild時に独立に検査し、公式`esptool image_info`でも確認する。

GPIOの新API・status ABI・ISR制約は[GPIO仕様](../../docs/embedded-interfaces/gpio.md)、native BSPを使う限定Renodeモデルでの試験は[確認手順](../../docs/embedded-interfaces/gpio.md#検証)を参照する。Renode試験はSRAMから開始し、この文書のQEMUによるROM/Flash確認とは別の範囲を扱う。

## QEMUでの起動確認

上記のstandalone build後、workspace rootから実行する。

```sh
python3 crates/emela-codegen-emb/tools/board_probe_esp32c3.py \
  --firmware-dir /tmp/emela-board-esp32c3/artifacts \
  --output /tmp/emela-c3-observation --qemu /path/to/qemu-system-riscv32
```

固定版はEspressif QEMU `esp_develop_9.2.2_20260417`。必要なshared libraryの探索pathは利用側の環境で設定する。runnerは4 MiBのraw Flashへimageを配置し、QEMUの内蔵ROMによるreset起動を使う。ELF loadやPC/SP設定は行わず、UARTのROM Flash boot列・load先・entryが1回だけ現れることも確認する。

実際のSYSTIMER/CPU IRQを通したtick 10回、task通知1回、結果42、guard/stack、`mtvec` とtrap causeを検査する。applicationの2行とbyte数はROM自身のログと分けて照合する。

**この固定QEMU版のGPIO modelは出力writeを無視するため、GPIO遷移は検証できない。** reportには `gpio_verification.status = unavailable` として記録する。UARTがHigh/Lowという文字列を出したことをもって、C3のGPIO出力確認として扱わない。F4の外部receiverによる確認とは範囲が異なる。


runnerは空のdirectoryか同じrunnerが所有するmarkerのあるdirectoryを使い、lockで排他する。開始時に古い成功reportを無効化し、失敗時もreportとlogを残す。manifestのhashと資源量を検査してから起動し、timeout時には子process groupを停止する。stack値はprobeの観測値であり、任意のアプリの最大値ではない。

runnerの入出力・lock・manifest・timeoutのhost確認は、workspace rootで`python3 crates/emela-codegen-emb/tools/test_board_probe.py`を実行する。firmware起動の検証には各emulatorも必要になる。

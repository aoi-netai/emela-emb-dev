# ESP32-C3ボードpackage

`emela build --board esp32c3`は、Emela生成CとこのpackageをRISC-Vへcompileし、`firmware.elf`・`firmware.bin`・`firmware.map`・`resources.json`を生成する。標準entryは引数なし・`Int`戻り値のsuspend関数で、生成export名`board_main_start`から呼び出す。schedulerはmain contextで実行し、SysTimer ISRは完了通知だけを行う。

## 前提ツール

- Espressif RISC-V GCC。検証版は`esp-14.2.0_20241119`。`riscv32-esp-elf-gcc`をPATHへ置くか、toolchain rootを`EMELA_ESP32C3_TOOLCHAIN`へ指定する。
- Python 3と公式`esptool==4.8.1`。CLIを実行するPython環境へ`python3 -m pip install esptool==4.8.1`で導入する。別のPythonを使う場合は`EMELA_ESPTOOL_PYTHON`を指定する。
- 実機への書き込み時だけ、UART download modeに入れるESP32-C3とserial portが必要。`--dry-run`はデバイスへ接続しない。

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

- GPIO番号は物理GPIO番号。0〜10と18〜21でInput・OutputPushPull、NoPull・Up・Downを実装する。11〜17はFlash電源・SPI Flash用として拒否する。GPIO interrupt・OpenDrain・Analogは明示的なdefectになる。
- ConsoleはUART0 TX=GPIO21、115200 baud/8N1。RX=GPIO20を設定するが、標準UARTのtimed transmit/receiveは`PeripheralError(2)`を返す。
- `Clock.now`は起動からの単調増加millisecondを仮想Unix epochへ対応させる。実日時のRTCではない。`Clock.sleep`は同時1件をSysTimer割り込みで完了する。
- I²C・SPI・ADC・無線・USB consoleは未実装。GPIO18/19を使う場合、USB用途との競合をアプリ側で避ける。

Emela heapは設定された固定領域の中だけを使う。`resources.json`の静的RAMには固定heap・poolが含まれる。stack予約量と実行で観測した使用量は区別する。

Clock taskを取消した場合も物理timerは旧期限まで保持する。その間の新たなsleepは容量超過としてdefectになる。期限のstale通知はruntimeのgeneration検査で拒否し、`board_rejected_notifications`へ数えて無視する。Emelaの再開や解放はISRから行わない。

## 書き込み

```sh
emela build . --board esp32c3
emela flash . --board esp32c3 --port /dev/ttyUSB0 --dry-run
emela flash . --board esp32c3 --port /dev/ttyUSB0
```

`flash`は成功したbuildのmanifestと成果物hashを照合する。失敗したbuildの古いmanifestや改変されたimageは書き込まない。実行前にGPIO9をLowにしてresetするUART download modeが必要になるボードもある。詳細は[公式boot mode説明](https://docs.espressif.com/projects/esptool/en/latest/esp32c3/advanced-topics/boot-mode-selection.html)を参照する。

## emulatorとの境界

従来の[Renode温度調節器](../../examples/temperature-controller/renode-esp32c3/)はCLIC mode 3を使うSRAM実行確認である。このFlash packageは実チップのmode 1を使用し、CLIC設定やCPU dispatch hookに依存しない。

Flash起動の検証にはEspressif公式QEMU `esp-develop-9.2.2-20260417`を使う。Linux x86_64用の取得例は次のとおり。cacheはrepoの外に置く。

```sh
mkdir -p /tmp/emela-esp32c3-qemu
curl --fail --location \
  https://github.com/espressif/qemu/releases/download/esp-develop-9.2.2-20260417/qemu-riscv32-softmmu-esp_develop_9.2.2_20260417-x86_64-linux-gnu.tar.xz \
  --output /tmp/emela-esp32c3-qemu/qemu.tar.xz
printf '%s  %s\n' \
  547f03e04701a92cbb699f7f7d015adc1f5b5ef93cbb94c0dd9b7107e2d84e77 \
  /tmp/emela-esp32c3-qemu/qemu.tar.xz | sha256sum --check
tar -xf /tmp/emela-esp32c3-qemu/qemu.tar.xz -C /tmp/emela-esp32c3-qemu
```

SHAは[公式release asset](https://github.com/espressif/qemu/releases/tag/esp-develop-9.2.2-20260417)の値と一致する。Linux版はOSの`libslirp.so.0`も必要である。検証環境ではUbuntuの`libslirp0=4.6.1-1ubuntu0.2`（debのSHA256 `b313de55b17cf8974996706b9cc70e4de387a39decf0bc3fa3a4256734476ee8`）を`/tmp`へ展開し、`LD_LIBRARY_PATH`で参照した。

温度調節器の[firmware project](../../examples/temperature-controller/firmware/)をCLIでbuild後、次のように実行する。

```sh
python3 crates/emela-codegen-emb/examples/temperature-controller/firmware/qemu_esp32c3.py \
  --firmware-dir /tmp/emela-temperature-c3/firmware \
  --qemu /tmp/emela-esp32c3-qemu/qemu/bin/qemu-system-riscv32 \
  --output /tmp/emela-temperature-c3-qemu \
  --expect-result 24 --expect-ticks 23000 --expect-notifications 23 --verify-temperature
```

runnerはbuild manifestのhashを検査し、image末尾だけを`0xff`で4MBへ補完してQEMUのSPI Flashへ接続する。`-kernel`・`-bios`によるapp注入は行わず、QEMU内蔵の実ROMのpower-onから実行する。UARTの`SPI_FAST_FLASH_BOOT`、RAM segment load、entry、完了・戻り値・実IRQ・通知回数・guard・stackを検査する。QMPはstdioで状態を読むために使い、CPUのPC・CSR・pendingを変更しない。観測と全commandは出力先の`report.json`へ記録する。

`--verify-temperature`は24行の連番・温度値・運転mode・アプリUART byte数を独立した期待値と照合する。100msごとのGPIO register値も記録するが、この版の[QEMU GPIO model](https://github.com/espressif/qemu/blob/esp-develop-9.2.2-20260417/hw/gpio/esp32_gpio.c)はstrap以外のreadを0にし、writeを無視する。従ってGPIOはreportで`unavailable`とし、出力や同時ONを確認した根拠には使わない。外部GPIO endpointの確認は既存Renode runnerの結果を参照する。

最小のGPIO/Console/10ms sleepアプリで、ROMからのload→開始ログ→IRQ10回・通知1回→完了ログ→結果42まで確認した。温度調節器では24sample・23,000tick・通知23回・UART944byte・結果24を確認した。実機の電源・pad電気特性・配線・serial書込みは検証していない。Renodeは外部GPIO endpointの遷移、QEMUはROMとFlashの起動経路をそれぞれ確認する。

レジスタの根拠は[ESP-IDF v5.4.1 ESP32-C3 headers](https://github.com/espressif/esp-idf/tree/v5.4.1/components/soc/esp32c3/register/soc)、[起動処理](https://docs.espressif.com/projects/esp-idf/en/v5.4.1/esp32c3/api-guides/startup.html)。Flash imageのchecksum・SHA-256・chip ID・entry・load範囲は独立に検査し、公式`esptool image_info`でも確認する。

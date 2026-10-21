# ESP32-C3のRenode実行環境

ESP32-C3を次の組み込みtargetへ追加するための、独立したエミュレーター確認用example。既存のRenodeへ外部の周辺機器モデルを読み込み、RISC-VのC firmwareをビルドして動かす。EmelaのC生成・標準board handlerとの接続は次の作業である。

## 必要なもの

- Linux x86_64、Python 3.10以降、Git
- Renode 1.16.1（この版で確認）。`renode`をPATHへ置くか、`RENODE`または`--renode`でexecutableを指定する
- 初回の依存取得にHTTPS接続。RISC-V toolchainのarchiveは約298 MBで、展開後の空き容量も必要

通常のprobeはSDK・boot ROM・ESP-IDFを必要としない。RISC-V toolchainはEspressif公式の固定版をSHA256検証して取得する。Renode本体は自動installしない。既存のRenode directoryへモデルを書き込まない。

## 実行

workspace rootから実行する。

```sh
bash crates/emela-codegen-emb/examples/renode-esp32c3/run.sh
```

Renodeの場所・cache・出力を変える例：

```sh
RENODE=/opt/renode/renode \
bash crates/emela-codegen-emb/examples/renode-esp32c3/run.sh \
  --cache /tmp/emela-esp32c3-cache \
  --output /tmp/emela-esp32c3-probe
```

初回は固定commitのモデルとtoolchainを外部cacheへ取得する。次回からは同じcacheを使い、モデルのcommit・変更の有無を確認して再利用する。自動更新はしない。

既存の依存を指定する場合：

```sh
bash crates/emela-codegen-emb/examples/renode-esp32c3/run.sh \
  --models /opt/renode-espemu \
  --toolchain /opt/riscv32-esp-elf \
  --renode /opt/renode/renode \
  --optimization O0 \
  --output /tmp/emela-esp32c3-probe-O0
```

`--models`は後述のcommitと一致するcleanなGit checkoutに限る。`--toolchain`は`bin/riscv32-esp-elf-gcc`等を含むroot。ユーザー指定toolchainのarchiveは検証済みとは扱わず、compiler versionと指定元をreportへ残す。環境変数`RISCV32_ESP_TOOLCHAIN`でも指定できる。

依存取得だけを行う場合：

```sh
python3 crates/emela-codegen-emb/examples/renode-esp32c3/setup.py \
  --cache /tmp/emela-esp32c3-cache
```

## probeで確認するもの

| 対象 | 成功条件 |
| --- | --- |
| CPU・startup | ELF entryから起動し、BSS・stackを初期化してCへ入る |
| UART0 | firmwareから出力された開始・4回の進捗・完了markerを取得する |
| GPIO3 | enable、W1TS/W1TC、OUT/IN readbackを検査し、`[8,0]`を4回観測する |
| タイマーと割り込み | SYSTIMERの10 ms one-shotを4回発火し、interrupt matrix→CLIC→CPU trap→C ISR→`mret`→mainを通る |
| CPUの観測 | 生の`mcause`・割り込まれたPC・timer tickを記録し、interrupt IDと時刻増加を検査する |
| メモリ | 64 KiBのSRAMだけをmap。16 KiBのstack予約内の観測使用量と256 byte guardを検査する |

UART完了markerは次の形になる。

```text
ESP32C3_PROBE_PASS irq=4 main=4 gpio=8
```

GPIOモデルは外部pin endpointを提供せず、INは内部loopbackである。外部LEDやセンサーの電気的な接続確認ではない。timer発火やPC変更をrunnerが合成するhookは使わない。

CLICはRenodeの共通モデルを利用し、runnerがCLICのline 1設定と`MTVEC = ELFのtrap_entry | 3`を実行前に設定する。これは現行モデルに合わせたadapterであり、ESP32-C3の割り込みcontroller・reset起動を完全再現したという意味ではない。CPUが保存するCLIC形式の`mcause`にはMPP/MPIEも含まれるため、interrupt bitと下位のinterrupt IDを検査する。

## 出力と失敗判定

既定の出力先は`/tmp/emela-esp32c3-probe-O2`。空directory、またはrunnerの管理markerがあるdirectoryを指定する。既存の無関係なdirectoryは拒否する。同じ出力先の同時実行も拒否する。

| ファイル | 内容 |
| --- | --- |
| `report.json` | 今回の状態、依存の版、実行command、エラー、観測値 |
| `firmware.elf` / `firmware.map` | 今回ビルドしたfirmwareと配置 |
| `objects/*.su` | compilerの関数単位stack見積り |
| `size.log` | sectionサイズ |
| `machine.repl` / `run.resc` | 最小platformと今回の実行script |
| `uart.log` / `renode.log` | 実UART出力とmonitorログ |
| `metrics.json` | 停止時にELF symbolから読み取った今回の観測値 |

開始時にreportを`running`へ更新し、旧metrics・UARTを除去する。timeout、toolの異常終了、Renode command error、観測不一致を成功にしない。通常probeはRenodeのwarningも失敗とし、SDK Helloだけ既知RTC warningを許容する。成功は終了code 0と`report.json`の`status: passed`で確認する。各実行に専用の`TMPDIR`とRenode設定directoryを用意する。

既定は10 MIPSで100 msのsimulation、wall timeoutは300秒。`--seconds`・`--mips`・`--timeout`で変更できる。simulation中の時間とhost上の実行時間を区別する。

## SDK Helloを別に確認する

上流のprebuilt ESP-IDF Helloも起動できる。こちらだけ公式boot ROM ELFを取得する。

```sh
bash crates/emela-codegen-emb/examples/renode-esp32c3/run.sh \
  --case hello \
  --output /tmp/emela-esp32c3-sdk-hello
```

`--rom`で同じSHA256のROM ELFを渡すこともできる。`setup.py --with-rom`は事前取得用。

このcaseは上流の`hello_world/setup.resc`と`tests/clic_setup.resc`をそのまま使う。ROM mainから固定firmware entryへのPC hookと、固定interrupt dispatch hookに依存する。RAMは物理400 KiBを超える480 KiBへ拡張されており、23個のplaceholder peripheralも含む。SDK起動とUARTの`Hello world!`等3 markerを確認するための参考caseであり、メモリ上限・全周辺機器・Emela動作の証拠には使わない。既知のRTC offset `0x80`の未処理read/write warningがある。

## 固定する外部依存

| 依存 | 固定値・検証 |
| --- | --- |
| [renode-espemu](https://github.com/mithro/renode-espemu/tree/9c9bdf6bb9c80d40efeaa8e4e398668e60c7007e) | commit `9c9bdf6bb9c80d40efeaa8e4e398668e60c7007e`、cleanなcheckoutを要求 |
| [Espressif RISC-V toolchain](https://github.com/espressif/crosstool-NG/releases/tag/esp-14.2.0_20241119) | `esp-14.2.0_20241119`、archive SHA256 `7faaa86d272f3e43c233f8a5ffeba327673224a752c2eb72394655d5e7950000` |
| [ROM ELF release](https://github.com/espressif/esp-rom-elfs/releases/tag/20240305) | optional、archive SHA256 `a26609b415710f0163d785850c769752717004059c129c472e9a0cbd54e0422c` |
| ROM `esp32c3_rev3_rom.elf` | SHA256 `19ac22e08707df926fb0cf4c54795d4067b983fae8635f396ded173a6d78fc3c` |

toolchainの版とSHAは[ESP-IDF v5.4.1のmanifest](https://github.com/espressif/esp-idf/blob/v5.4.1/tools/tools.json)による。外部モデルの当該commit内にはlicense指定が見当たらないため、このrepoへソースをvendorせず外部cacheへ取得する。再配布を伴うpackagingは別途licenseを確認する。ROM ELFの配布条件は[公式license案内](https://github.com/espressif/esp-rom-elfs#copyrights-and-license)を参照する。

次に行うボード移植の設計・確認範囲は[ESP32-C3 Renode導入記録](../../docs/esp32c3-renode.md)を参照。

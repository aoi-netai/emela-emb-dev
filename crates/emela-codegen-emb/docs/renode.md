# Renodeでのembedded backend smoke確認

利用者向けのアプリケーション例は[温度調節器](../examples/temperature-controller/README.md)を参照する。Emelaで運転状態を保持し、周期的な温度入力に応じて加熱・冷却GPIOを制御し、USART2へ表示する。C生成は[組み込み向けCLI](cli-embedded.md)の`emela build --target c-embedded`で行い、exampleの`renode/run.sh`はCLIによる生成からARMビルド、Renodeでの実行までをまとめて行う。利用者がRustの生成プログラムを書く必要はない。

この手順では、現行の `emela-codegen-emb` が生成したCをSTM32F446向けにクロスビルドし、Renode上で実行する。const・同期非scalar ABI・generic/成長再帰型をまとめた新しい`completion` fixtureは統合確認を参照。生成Cのentry、整数演算、比較、`Let`、`If`の両分岐、内部関数呼び出し、ゼロ除算時のboard defect hook、外部C関数までを確認する。

過去の [`emela-fix-259/examples/embedded-bus`](../../../../emela-fix-259/examples/embedded-bus) は、ボード設定やRenode構成を調べる参考資料として使う。その `run.sh` は当時の `emela-cli` と旧backendをビルドするため、現行 `emela-codegen-emb` の確認には使わない。このcrateのsmoke fixtureは現在のbackend APIを直接呼び出す。

最終受入のhost検査・O0/O2・容量不足を含む一括実行は完了ロードマップ5を参照する。以下は単独smokeの手順である。

## 必要なもの

- Rust workspaceのbuildに必要な環境
- ARM GNU toolchain（`arm-none-eabi-gcc` / `arm-none-eabi-size`）
- STM32CubeF4のCMSIS device/core headers、`system_stm32f4xx.c`、F446 startup assembly
- RenodeとSTM32F4 platform description

この環境で使っている既定の場所は `/home/aoi_nemui/bareruby/stm32_renode` 以下である。別の場所にある場合は、実行時に環境変数で指定する。

| 環境変数 | 既定値 | 用途 |
| --- | --- | --- |
| `STM32_RENODE_ROOT` | `/home/aoi_nemui/bareruby/stm32_renode` | toolchain、Renode、STM32CubeF4をまとめたroot |
| `ARM_TOOLCHAIN_PATH` | `$STM32_RENODE_ROOT/.tools/common/arm/xpack-arm-none-eabi-gcc-13.2.1-1.1` | ARM GNU toolchain |
| `STM32CUBE_F4` | `$STM32_RENODE_ROOT/.tools/stm32cube/STM32CubeF4-1.28.3` | STM32CubeF4 |
| `RENODE` | `$STM32_RENODE_ROOT/.tools/stm32cube/renode-1.16.1/renode` | Renode executable |
| `RENODE_PLATFORM` | `$(dirname "$RENODE")/platforms/cpus/stm32f4.repl` | STM32F4 platform description |
| `EMELA_EMB_RENODE_OUT` | `/tmp/emela-codegen-emb-renode-scalar` | 生成物とログの出力先 |

## 実行

workspace rootから次を実行する。

```sh
bash crates/emela-codegen-emb/examples/renode-f446/run.sh
```

実行内容は [`run.sh`](../examples/renode-f446/run.sh) にある。引数なしまたは`scalar`が従来の確認、`completion`がロードマップ2〜4の統合確認を選ぶ。

ツールの場所を変更する場合は、例えば次のように指定する。

```sh
STM32_RENODE_ROOT=/opt/stm32-renode \
ARM_TOOLCHAIN_PATH=/opt/arm-gnu-toolchain \
STM32CUBE_F4=/opt/STM32CubeF4 \
RENODE=/opt/renode/renode \
bash crates/emela-codegen-emb/examples/renode-f446/run.sh
```

スクリプトは次の順に処理する。

1. [`examples/renode_smoke.rs`](../examples/renode_smoke.rs) を `cargo run` し、CoreからFlow IRを経由してCを生成する。
2. 生成C、board fixture、CMSIS system source、F446 startupをCortex-M4向けにcompileし、ELFとmap fileを作る。
3. STM32F4 platformでRenodeを起動し、ELFを500 ms相当実行する。
4. `GPIOA_ODR` のPA5・PA6・PA7 bitがHighになったことを確認する。PA5は条件の真側で42、PA6は偽側で34を受け取ったこと、PA7はdivision-by-zero defect hookが期待したkindとmessageを受け取ったことを示す。異常値はPA8で示す。

主な出力は `EMELA_EMB_RENODE_OUT` 以下に保存される。

| ファイル | 内容 |
| --- | --- |
| `generated/emela_embedded.c` | embedded backendの生成C |
| `firmware.elf` | STM32F446向けfirmware |
| `firmware.map` | linker map |
| `renode.log` | Renode実行ログ |

正常終了時には `PASSED: scalar/O2: .../report.json` が表示される。`metrics.json`の`gpioa_odr`が224（`0xe0`）であることと、共通stack/resource検査の成功をrunnerが確認する。

## 確認範囲と制約

このfixtureのEmela `main` は内部関数を21と10で呼び、内部関数は比較結果に応じてそれぞれ42と34を返す。その値を外部C関数 `renode_smoke_signal` に渡す。board側はPA5・PA6をGPIO outputに設定し、受け取った値と順序が期待どおりなら各pinをBSRR経由でHighにする。不一致時にはPA8をHighにする。続けて実行時のゼロ除算を起こし、`emela_board_defect` がkindとmessageを検査してPA7をHighにし、停止する。GPIOA ODRの値から両分岐とdefect hookの結果を確認できる。

CMSIS startupの `Reset_Handler` はCの `main()` を呼ぶ一方、embedded backendは `emela_embedded_start()` を生成する。そのためfixtureのboard Cは `main()` shimを持ち、そこから `emela_embedded_start()` を呼ぶ。このshimはアプリケーション統合側の役割である。

以前のfixtureはC heapを512 byte、stackを4096 byte予約しており、過去の`arm-none-eabi-size`記録にはその予約分も含む。現在のlinker scriptは実MSPの領域をSRAM上端へ合わせ、測定用のstack 16384 byteとlow guard 256 byteをNOLOADで予約する。C library heapの暗黙予約は0、Emela固定heapは生成CのBSSへ別途含む。予約設定と観測した使用量を分けたSTM32F446資源検査を参照する。smokeの実行結果から任意のアプリケーションの最大stackやheap使用量を推定してはならない。

Renodeで確認できるのはモデル上の実行結果であり、実チップの電気的動作、周辺回路の全挙動、割り込み遅延やWCETはこのprobeから判断しない。本計画のtarget統合確認はユーザー方針によりemulatorを実機の代替として扱う。生成Cのcompile/linkとRenode実行は、実際に行った範囲を分けて報告する。

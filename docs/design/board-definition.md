# 組み込みボード定義の草案

ボード定義は Emela の論理ピン・周辺機器 ID を MCU の実リソースへ結び付ける。Emela の標準インターフェースには物理 GPIO ポートや STM32 HAL の型を出さない。

最初の例は [`NUCLEO-F401RE の定義案`](../../boards/stm32f4/nucleo-f401re.toml) とする。現時点では定義形式の草案であり、コンパイラによる読み込み、検査、コード生成にはまだ対応していない。

## 役割

- `Std.Embedded.Gpio` の `GpioPin(id)` を、ボード端子名と MCU のポート・ピン番号へ対応付ける。
- `Uart0` などの論理周辺機器 ID を、USART インスタンスと信号ピンへ対応付ける。
- 周辺機器が占有するピンを記録し、GPIO と UART などの競合をビルド時に検査できるようにする。
- HAL 固有の設定値は HAL バックエンドの設定として保持し、Emela の共通インターフェースには漏らさない。

## 形式案

- TOML とする。
- `format_version` で定義形式の版を示す。
- `[board]` にボード、MCU、HAL を記す。
- `[logical_pins."<id>"]` に論理ピン ID、ボード上のラベル、MCU 端子を記す。
- `[peripheral_routes.<id>]` に周辺機器インスタンス、送受信ピン、代替機能などを記す。
- `[backend.<name>]` に HAL 固有の設定を書く。

プロジェクトから選択する方法は、たとえば次の形を想定する。

```toml
[build]
target = "c-embedded"
board = "boards/stm32f4/nucleo-f401re.toml"
```

## NUCLEO-F401RE の例

この定義では論理ピン ID `0`〜`13` を Arduino の D0〜D13 として扱う。たとえば Emela の `GpioPin(id: 13)` はボード端子 D13、MCU の PA5 へ対応し、基板上の LED LD2 に接続される。

USART のルート例も同じファイルに置いた。NUCLEO-F401RE では D0/D1 は PA3/PA2 の USART2_RX/TX、USART1 は D2/D8（PA10/PA9、AF7）に対応する。したがって、このボードでは「D0/D1 が USART1」ではない。USART2 を D0/D1 へ接続するには SB62/SB63 を ON、SB13/SB14 を OFF にする必要がある。これらのブリッジ設定もボード定義に記録した。

最初に実装する GPIO ターゲットが読むのは `[logical_pins]` と GPIO 用 HAL 設定だけとする。`[peripheral_routes]` は UART バインドの形式案であり、UART 操作をまだ実装したことを意味しない。

## 静的バインドの方針

組み込みターゲットでは、初期段階の `GpioPin(id)` は静的に分かる ID に限る。選択したボード定義に ID がなければ、C 出力前にその ID とボード名を示してエラーにする。値が実行時まで分からないピン ID は、対応する動的ディスパッチ設計を追加するまでは拒否する。

周辺機器ルートを使う段階では、次も静的に検査する。

- 同じ論理ピンを GPIO と周辺機器の両方で同時に使っていないこと。
- 同じ物理周辺機器を複数の論理 ID に割り当てていないこと。
- 指定した MCU ピンがその周辺機器信号と代替機能に対応していること。
- ボード端子への配線やはんだブリッジなど、基板上の条件が満たされること。

## 将来のビルド段階

ボードを選んだビルドでは、Emela の論理 ID をボード定義で静的に解決し、ボード用 C アダプターを生成または選択する。その C と HAL、起動コード、リンカースクリプトをターゲット用ツールチェーンでリンクする。ボード未指定の C ソース生成段階では、物理リソースを決めず、論理 API の C 呼び出しまでを出力する。

ボード定義には将来、MCU の能力表と基板固有の端子・配線を分離して持たせる。最初の草案では参照ボード1つに限るため、両者を同じファイルに置く。

## 参照資料

- [STM32 Nucleo-64 boards user manual UM1724](https://www.st.com/resource/en/user_manual/dm00105823.pdf)
- [STM32F401xD/xE datasheet](https://www.st.com/resource/en/datasheet/stm32f401ve.pdf)

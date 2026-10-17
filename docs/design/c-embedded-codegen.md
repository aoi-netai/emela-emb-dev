# 組み込みCの初期出力

`emela build --target c-embedded` は、リンク前のCソースを `target/emela/c-embedded/` に出力します。生成物は `main.c` と `emela_gpio.h` です。既存の `--target c` が使うヒープランタイム、POSIX API、ccによるリンクは含みません。

## GPIO 割り込みハンドラーの設計例

```emel
import Std.Embedded.Gpio.{GPIO, GpioPin, GpioEvent, GpioMode, Pull, PinState, Edge}

fn button_changed(event: GpioEvent) use { GPIO } {
  gpio = use GPIO
  gpio.write(GpioPin(13), event.level)
}

fn main() use { GPIO } {
  gpio = use GPIO
  gpio.configure(GpioPin(13), GpioMode.OutputPushPull, Pull.NoPull)
  gpio.configure(GpioPin(2), GpioMode.Input, Pull.Up)
  gpio.enable_interrupt(GpioPin(2), Edge.Both)
}
```

割り込みハンドラーは HAL コールバックから ISR 内で呼び出す方針です。ボードバインドが論理ピン、エッジ、Emela 関数を静的に結び付け、生成 C が中継関数を出します。`main` からポーリングする設計ではありません。この記法と生成処理は未実装です。現行プロトタイプには旧案の `poll_interrupt` が残っており、この設計に合わせて置き換える必要があります。

上の例のバインド関係は `GpioPin(2), Edge.Both -> button_changed` です。バインド設定の具体的な構文はまだ決めていません。

## ボード側の接続

`emela_gpio.h` が宣言する `emela_gpio_*` 関数をボード側のCソースで実装し、そのソースと `main.c` を同じファームウェアへリンクします。ヘッダーには論理ピン番号と各GPIO enumの値を定義しています。たとえば `GpioPin(13)` は `EmelaGpioPin` の値 `13` として呼び出し境界を越えます。

現段階では論理ピンから STM32 のポート／ピンへの対応も、割り込みハンドラーのバインドも生成していません。`boards/stm32f4/nucleo-f401re.toml` の読み込みと、対応を C へ静的に出力する処理は次の段階です。ISR 到達範囲のヒープ禁止や、ボード側の ISR 安全性もまだ検査しません。

## 対応範囲

最初の出力器はGPIO操作と、整数・真偽値・GPIO enum・論理ピン・GPIOイベントの値を使う関数を扱います。`if`、基本的な整数演算、ガードなしの `Some(event)` / `None` による割り込みイベント照合を出力できます。文字列、関数値、ループ、一般のパターン照合、失敗処理、中断処理、明示的な `with` handler、他のeffectは診断で拒否します。`run` と `test` は実行ファイルを作らないターゲットのため利用できません。

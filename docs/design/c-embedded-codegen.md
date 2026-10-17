# 組み込みCの初期出力

`emela build --target c-embedded` は、リンク前のCソースを `target/emela/c-embedded/` に出力します。生成物は `main.c`、GPIO境界の `emela_gpio.h`、ボードから公開関数を呼ぶための `emela_embedded.h` です。既存の `--target c` が使うヒープランタイム、POSIX API、ccによるリンクは含みません。

## GPIO 割り込みハンドラー

`pub fn` で公開され、`GpioEvent` を引数に取る関数は、組み込みCでは割り込み入口として扱います。関数は単相化対象に含まれ、ボード側は `emela_embedded.h` の宣言を使って静的コールバックから直接呼び出せます。

```emel
import Std.Embedded.Gpio.{GPIO, GpioPin, GpioEvent, GpioMode, Pull, PinState, Edge}

fn button_level(event: GpioEvent) -> Option[PinState] {
  if event.pin == GpioPin(2) { Some(event.level) } else { None }
}

fn is_pressed(level: PinState) -> Bool {
  level == PinState.Low
}

fn led_state(pressed: Bool) -> PinState {
  if pressed { PinState.High } else { PinState.Low }
}

pub fn button_changed(event: GpioEvent) use { GPIO } {
  gpio = use GPIO
  output = button_level(event)
    |> Option.map(is_pressed)
    |> Option.map(led_state)

  output match {
    Some(state) -> gpio.write(GpioPin(13), state)
    None -> ()
  }
}

fn main() use { GPIO } {
  gpio = use GPIO
  gpio.configure(GpioPin(13), GpioMode.OutputPushPull, Pull.NoPull)
  gpio.configure(GpioPin(2), GpioMode.Input, Pull.Up)
  gpio.enable_interrupt(GpioPin(2), Edge.Both)
}
```

`Option[A]` はペイロード型ごとの固定サイズC構造体として表現します。`Option.map` は、コールバック関数が静的に分かる場合に専用の直接呼び出し関数へ具体化します。Cの関数ポインターやヒープ確保は使いません。

割り込み入口から到達する関数には、失敗・中断・外部関数・間接呼び出し・組み込み関数・リスト・クロージャー・fiber等を許可しません。effect呼び出しは GPIO の `read`、`write`、`toggle` に限定します。ボード側の実装も ISR 内で安全かつ非ブロッキングにする必要があります。

## ボード側の接続

`emela_gpio.h` が宣言する `emela_gpio_*` 関数をボード側のCソースで実装し、そのソースと `main.c` を同じファームウェアへリンクします。公開 Emela 関数は `emela_embedded.h` に宣言されます。ボード側の HAL コールバックからその関数を直接呼べます。

生成する `main` は Emela の `main` を一度呼び、その後は戻らずに待機します。マイコン用スタートアップから `main` が戻ることを前提にせず、割り込みコールバックを継続して受けられるようにします。

論理ピンから STM32 のポート／ピンへの対応、ピン競合検査、ピンとエッジからハンドラーを選ぶボードバインド、HAL コールバックの生成は未実装です。バインドの記法と設定場所は後続の作業で決めます。したがって、この出力には `button_changed` の関数本体とC ABIが含まれますが、`GpioPin(2), Edge.Both` から HAL コールバックまでの経路はまだ自動生成しません。

## 対応範囲

初期出力器は GPIO 操作と、整数・真偽値・GPIO enum・論理ピン・GPIOイベント、および `Option` の値を扱います。`if`、基本的な整数演算、`Some` / `None` の照合、静的関数を使う `Option.map` を出力できます。文字列、ループ、一般のパターン照合、失敗処理、中断処理、明示的な `with` handler、他のeffectは診断で拒否します。`run` と `test` は実行ファイルを作らないターゲットのため利用できません。

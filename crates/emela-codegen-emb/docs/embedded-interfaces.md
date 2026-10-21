# 組み込みの標準インターフェースとボード実装

組み込みアプリは、周辺機器の操作を`Std.Embedded.*`のeffectとして記述する。ボードで実行するときは`Std.Embedded.Board`のhandlerを選び、ボードのC実装へ接続する。アプリごとにGPIO・UART等の`@external`やadapter対応表を定義する必要はない。

## アプリ、handler、ボードの役割

| 場所 | 役割 |
| --- | --- |
| アプリの`.emel` | 計算、状態の保持、操作する機器とタイミングの判断 |
| `Std.Embedded.Gpio`等 | 操作の型とeffectを宣言する共通インターフェース |
| `Std.Embedded.Board` | 標準effectをボードABIへ接続する共有handler |
| ボードの`.emel`モジュール | 論理ピン・機器IDの割り当て、機器の設定、独自センサ等の接続 |
| ボードのCコード | レジスタやHALの操作、割り込み、非同期処理の完了通知 |
| `embedded.toml` | 固定容量、公開関数のC名、独自の外部関数に必要な追加設定 |

例えばGPIOは次の経路で動く。

```text
(use Gpio).write(pin, High)
        ↓ with BoardGpio
標準handlerとbackendの共有bridge
        ↓ emela_gpio_write
ボードのC実装 → GPIOレジスタ
```

共通handlerの内部にもCとの境界はある。そのABI変換とadapter登録はbackendが行う。独自デバイス・既存Cライブラリに使う一般の`@external`とadapter設定も引き続き利用できる。

## 利用できるeffect

| importするモジュール | 主な操作 | ボード用handler |
| --- | --- | --- |
| `Std.Embedded.Gpio` | ピン設定、読み書き、反転、割り込み元の設定 | `BoardGpio` |
| `Std.Embedded.Uart` | UART設定、データの送受信 | `BoardUart` |
| `Std.Embedded.I2c` | 設定、master送受信、メモリアクセス、応答確認 | `BoardI2c` |
| `Std.Embedded.Spi` | 設定、送信、受信、同時送受信 | `BoardSpi` |
| `Std.Clock` | `sleep`、`now` | `BoardClock` |
| `Std.Console` | 通常・エラーの行出力 | `BoardConsole` |

handlerは`Std.Embedded.Board`からimportし、`with`で適用する。GPIOの論理ピン番号とUART等のIDがどの物理機器に対応するかは、ボード実装が定める。GPIOの割り込み設定は割り込み元の設定までで、任意のEmela関数をISRで実行するAPIではない。

UART・I²C・SPIのエラーは`Std.Embedded.Peripheral.PeripheralError(code)`で扱う。C境界では0が成功、−1（不正な引数）と−2（転送buffer容量超過）が共有bridgeの予約codeである。その他の非0 codeの意味はボード実装が定める。GPIOの操作には回復可能な失敗型を定義していない。不正な機器指定等をどう診断するかは、ボード実装の契約に従う。

## 最初の利用例

[温度調節器](../examples/temperature-controller/README.md)は、アプリを`controller.emel`、論理ピンとセンサの定義を`board.emel`、STM32F446の実装を`renode/board.c`に分ける。

- `transition`が現在の運転状態と温度から次の状態を返す。
- `State[Mode]`が運転状態を保持する。
- `Actuators` handlerが`Gpio`で両装置を停止してから必要な側を起動し、`release`でも停止する。
- `BoardGpio`と`BoardUart`がGPIOとUARTの設定をC実装へ渡す。
- `BoardClock`が1秒の待機をSysTickの完了通知へ接続する。
- `BoardConsole`が長さ付きStringのログをUARTへ出力する。

アプリは標準effectを使い、ボードを変更するときは論理ピンの定義とC実装を交換する。独自の温度センサの`@external`は`board.emel`だけに置く。全ての物理ボード用driverを標準ライブラリが提供するわけではない。

## 転送データと容量

UART・I²C・SPIの転送データは`List[Int]`で表す。共有bridgeは固定長の`uint16_t`転送bufferを使い、capacityと要素の範囲0〜65535を確認してからボードへ渡す。実際に使用できる要素の幅は機器の設定に依存し、8bit設定のUARTなら0〜255である。設定に対して不正な値はボード側で診断する。転送要素はUTF-8の文字列やUnicodeのcode pointと同じものではない。

`embedded.toml`の`peripheral_buffer_capacity`は、このbufferに保持できる要素数を指定する。既定は256、0は使えない。大きな転送にはアプリでの分割や容量の調整が必要である。具体的な生成ABIと所有権・エラーcodeの契約は[標準周辺機器bridge](embedded-peripherals.md)と生成headerを参照する。

ログを出す目的では`Console.print_line`を使える。長さ付きStringを直接ボードへ渡すため、ログを一度`List[Int]`へ変換する必要がない。

## 待機と割り込み

`BoardClock.sleep`は外部taskとして開始し、ISRからの完了通知後、main側のschedulerでEmelaの処理を再開する。0以下の待機時間は0として扱い、ボードはtaskの完了通知だけを送り、その呼び出し中にアプリを直接再開しない。ボード側は生成されたtask API・event lockの契約に従う。ISRでString生成、Emela heap操作、アプリの再開を行わない。

`Clock.now`はUnix epochからのミリ秒を表す`Instant`を返す。実時間の分からないボードでは、起動からの経過時間を黙ってUnix時刻として扱わず、RTCや設定したepochとの対応をボード側で定める。温度調節器は`Clock.sleep`だけを使用する。

## ビルドと確認範囲

C生成は通常のCLIで行う。固定容量と公開名の設定、生成ファイルは[組み込み向けCLI](cli-embedded.md)を参照する。ボード側は、生成されたGPIO・UART等のheaderに合わせてC関数を実装し、アプリと一緒にcompile・linkする。

STM32F446／Renodeで確認する温度調節器は、模擬温度入力、GPIO出力、UARTログ、SysTick待機を組み合わせた利用例である。UART・I²C・SPIの全動作や全てのボードの物理機器動作までを保証するものではない。

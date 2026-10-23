# GPIOインターフェース仕様

状態：2026-10-24、`GpioConfig`、`GpioError`、静的な割り込みbinding、ISR専用read/writeをstdlib・codegen・STM32F4/ESP32-C3 BSPに実装。従来の`GpioMode`とstatusを返さないC ABIから移行した。旧header・旧BSPとの混在には対応せず、Cとfirmwareを再生成する。

[仕様一覧](README.md)、[共通bridge](../embedded-peripherals.md)、[Renode試験と再実行手順](../gpio-validation.md)も参照する。UART・I²C・SPIの拡充は引き続き別の実装段階である。

## Emela API

宣言元は[`Std.Embedded.Gpio`](../../../emela-core/lib/std/embedded/gpio.emel)、mainの共有handlerは[`BoardGpio`](../../../emela-core/lib/std/embedded/board.emel)である。優先度は共通module [`Std.Embedded.Interrupt`](../../../emela-core/lib/std/embedded/interrupt.emel)に置く。

```emela
pub type GpioPin(id: Int)
pub enum PinState { Low, High }
pub enum Pull { NoPull, Up, Down }
pub enum GpioConfig {
  Input(pull: Pull)
  OutputPushPull(pull: Pull, initial: PinState)
  OutputOpenDrain(pull: Pull, initial: PinState)
  Analog(pull: Pull)
}
pub enum Edge { Rising, Falling, Both }
pub type GpioEvent(pin: GpioPin, level: PinState)
pub enum GpioErrorKind {
  InvalidArgument
  NotConfigured
  Unsupported
  Busy
  ResourceConflict
  InvalidState
  Other
}
pub error GpioError(kind: GpioErrorKind)

pub effect Gpio {
  fn configure(pin: GpioPin, config: GpioConfig) -> () fails GpioError
  fn read(pin: GpioPin) -> PinState fails GpioError
  fn write(pin: GpioPin, state: PinState) -> () fails GpioError
  fn toggle(pin: GpioPin) -> () fails GpioError
  fn enable_interrupt(pin: GpioPin, edge: Edge, priority: InterruptPriority) -> () fails GpioError
  fn disable_interrupt(pin: GpioPin) -> () fails GpioError
}

pub fn bind_interrupt[R: Immediate](
  pin: GpioPin,
  callback: fn(GpioEvent) -> () fails {} use R
) -> () fails GpioError
```

`InterruptPriority`のvariantは別moduleの`Low`・`Normal`・`High`であり、`PinState`と名前を共有しないようにqualified importで指定する。

```emela
import Std.Embedded.Gpio
import Std.Embedded.Interrupt
import Std.Embedded.Board.{BoardGpio}

const OUTPUT = Gpio.GpioPin(60)

fn on_edge(event: Gpio.GpioEvent) -> () fails {} use {Gpio.Gpio} {
  event.level match {
    Gpio.High -> (use Gpio.Gpio).write(OUTPUT, Gpio.High) escape { Gpio.GpioError(_) -> () }
    Gpio.Low -> (use Gpio.Gpio).write(OUTPUT, Gpio.Low) escape { Gpio.GpioError(_) -> () }
  }
}

pub fn initialize() -> () fails Gpio.GpioError {
  with BoardGpio {
    gpio = use Gpio.Gpio
    gpio.configure(OUTPUT, Gpio.OutputPushPull(Gpio.NoPull, Gpio.High))
    gpio.configure(Gpio.GpioPin(6), Gpio.Input(Gpio.Down))
    Gpio.bind_interrupt(Gpio.GpioPin(6), on_edge)
    gpio.enable_interrupt(Gpio.GpioPin(6), Gpio.Both, Interrupt.Normal)
  }
}
```

これはSTM32F407VGの例である。割り込みの有効期間はアプリと固定資源を保持し、終了前にmainから`disable_interrupt`する。callback内では通常のmain handlerを選ばず、compilerがread/writeをBSPのISR専用実装へ直接接続する。

## 対応MCU・profile・pin・設定

STM32のIDは`port番号 × 16 + pin番号`で、A=0、B=1である。package上のpinの存在は各chip JSONの`gpio_pin_masks`を用いる。C3はGPIO番号をIDとする。全MCUで同じIDを同じ物理pinとは解釈しない。

| 対象MCU | board profile | GPIO controller / IRQ | 初期確認の出力 | 入力 | 予約pin |
| --- | --- | --- | --- | --- | --- |
| STM32F407VG | `stm32f407vg`、`stm32f4-discovery` | GPIO A〜E/H、SYSCFG/EXTI/NVIC | PD12 / 60 | PA6 / 6、PA7 / 7 | Console PA2/PA3 |
| STM32F412ZG | `stm32f412zg` | chip表のGPIO、SYSCFG/EXTI/NVIC | PD12 / 60 | PA6 / 6、PA7 / 7 | Console PA9/PA10 |
| STM32F429ZI | `stm32f429zi` | chip表のGPIO、SYSCFG/EXTI/NVIC | PD12 / 60 | PA6 / 6、PA7 / 7 | Console PA9/PA10 |
| STM32F446RE | `stm32f446re`、`nucleo-f446re`、互換ID `stm32f446` | chip表のGPIO、SYSCFG/EXTI/NVIC | PA5 / 5 | PA6 / 6、PA7 / 7 | Console PA2/PA3 |
| ESP32-C3 | `esp32c3` | GPIO、matrix source16 → CPU IRQ2 | GPIO3 / 3 | GPIO6 / 6、GPIO7 / 7 | Flash電源/Flash 11〜17、USB Serial/JTAG 18/19、Console 20/21 |

| 設定 | STM32F4 | ESP32-C3 |
| --- | --- | --- |
| Input、PushPull、OpenDrain | 対応 | GPIO0〜10で対応 |
| Analog | pinをAnalog modeに変更。ADC/DACの操作は含まない | `Unsupported`、既存の設定を保持 |
| NoPull / Up / Down | 対応 | 対応 |
| 出力初期Low / High | PushPull・OpenDrainで指定 | PushPull・OpenDrainで指定 |
| Rising / Falling / Both | EXTIで対応 | GPIO edge検出で対応 |
| Low / Normal / High | NVIC priority 0xC0 / 0x80 / 0x40 | CPU IRQ2 priority 1 / 2 / 3 |

実行を伴う初期受け入れprofileは`stm32f407vg`と`esp32c3`。他profileは同じdriverとchip/package表を使うが、確認方法は[試験記録](../gpio-validation.md)に分ける。SWD/JTAG、水晶、基板上の外付け回路等の用途は実基板に応じて追加予約する必要がある。

## operationの意味

### configureと出力初期値

指定のmode/pullを適用する。全引数・予約・状態を検証した後、以前の出力を解放し、出力latchと出力型を設定してから出力駆動を有効にする。初期Highを指定したpinを一度Lowとして駆動しない。open-drainのHighは出力の解放であり、実際の電圧はpull-upや外部回路に依存する。

Inputは出力駆動を無効にしデジタル入力を有効にする。Analogはデジタルread/write/toggleと割り込みに使わない。pullの指定を実現できなければ別の設定へ丸めず拒否する。引数・対応設定・資源の検証失敗では、以前の設定や出力を変更しない。

対象pinの割り込み有効中の再設定は`InvalidState`。先に停止し、必要なら再び有効化する。他pinの設定・出力を変更しない。現在の資源表はpackageの存在とConsole等の予約を扱い、将来のUART/I²C/SPI driverの所有権も同じ検査へ統合する。

### read・write・toggle

全operationは先にconfigureする。readはデジタル入力の実レベルを返す。出力pinも入力回路を有効にし、latchではなくSTM32のIDR、C3のGPIO_INを読む。open-drainを解放していても外部がLowならreadはLowである。

writeとtoggleは出力pinのみ。toggleは入力レベルではなくODR/GPIO_OUTのlatchを反転する。同じportの他bitはBSRRまたはW1TS/W1TCで保持する。同じpinを複数経路から操作する場合の論理的な排他は利用側が管理する。toggleはISRへ公開しない。

### binding・enable・disable

設定 → `bind_interrupt`の実行時成功 → `enable_interrupt`の順に開始する。bindingは有効化とは別であり、既に停止しているbindingの再実行は設定を変更せず成功する。

未bindingでの有効化、既に有効なpinの再有効化は`InvalidState`。edge・優先度の変更は停止後に行う。過去のpendingをclearしてから新たな検出を許可する。

停止はmainから呼ぶ。既に停止済みなら成功する。対象pinだけをmaskしてpendingをclearし、他pinと共有するIRQは、ほかの有効な発生元があれば停止しない。単一coreのmainは実行中callbackの終了後にしか再開せず、停止のcritical sectionとbarrierにより返却後の旧pending通知を防ぐ。mainへ既に保存したアプリの値は消さない。

STM32では同じEXTI番号を異なるportが同時に所有できない。共有IRQはEXTI0〜4が各1本、EXTI5〜9とEXTI10〜15が各1本であり、同じIRQに異なる優先度を要求すると`ResourceConflict`。C3のGPIOは1つのIRQを共有するため、同時に有効な全pinで優先度を揃える。停止後は別のportや優先度へ割り当て直せる。

同じ優先度以下はISRへネストさせない。STM32のpreemptionはNVICが管理し、SysTickはGPIOより高いシステム優先度を使う。C3はtrap中に割り込みを再有効化せず、優先度によるISRネストを提供しない。優先度だけで応答期限を保証しない。

## 割り込みハンドラーの静的binding

compilerは型検査・具体化後にbindingを集める。pinのliteral、固定整数式、const、不変な別名を辿り、captureのない名前付き関数を解決する。runtimeの値・lambda・条件分岐で選ぶ関数やbinding関数の動的な持ち出しを拒否する。同じpinへの宣言は同じ関数でも重複エラーとなり、異なる条件分岐に置いても重複と数える。同じ関数を異なるpinへbindingすることは可能である。

通常の到達性からはprivateなcallbackも保持し、ISR向けに関数と呼び先を特殊化する。名前付きcallbackは公開する必要がない。BSPが発生元を確認してacknowledgeし、そのISR内で生成入口を直接呼ぶ。入口は基本型のpin/levelを受け、内部で`GpioEvent`を構築する。queue、fiberの起動・再開、schedulerでのcallback実行は介さない。

`GpioEvent.level`はISR内で観測したレベルである。発生時点の厳密な値、edgeの種類・順序・回数ではない。Both時のlevelだけで発生edgeを断定しない。debounceや全edgeの欠落しない配送は保証しない。

### ISR専用handlerと資源

公開署名の`Immediate`は言語既存の非suspend制約を使う。計画中の`IsrSafe`という新しい言語構文は追加せず、embedded backendが具体化した呼び先を検査する。

許可するGPIO operationはread/writeのみ。BSPのISR専用関数へ直接接続し、通常のfiber handler・evidence table・可変状態を持ち込まない。configure、toggle、binding、有効化・停止、Clock、Console等の別effect、heapを使う値・文字列付きエラー、参照管理、再帰（最適化された末尾再帰を含む）、suspend、動的呼び出しをISRから拒否する。

read/writeは`GpioError`で失敗し得るためcallback内で捕捉する。固定エラーの失敗経路を辿り、mainだけで使う文字列付きエラーを理由に固定エラーまでheap必須とは判定しない。mainのList/StringとISRのenum/match・read/writeが共存する。固定のpin値等のconstは、ISR用の純粋な定義を通し、mainの遅延初期化cellへ触れない。

BSPの固定領域とpinはmainで準備する。ISRからmainのList/String/Cell/Stateへの参照を渡さない。固定容量の単一producer・単一consumer受け渡しAPIは、GPIOのこの改訂には含めず、UART等の次段階で実装する。

## 不正な操作とdefect

| 分類 | 条件 |
| --- | --- |
| InvalidArgument | pinが範囲外・packageに存在しない、C引数やenum値が不正 |
| NotConfigured | 未設定pinへのread/write/toggle/bind/enable/disable |
| Unsupported | C3のAnalog等、実現しない設定 |
| ResourceConflict | Console・Flash・USB予約、EXTI入力、共有IRQ優先度の競合 |
| InvalidState | Input/Analogへのwrite、Analogのread/interrupt、未bindingでの有効化、既に有効、割り込み有効中の再設定 |
| Busy | 一時的使用中のための共通分類。現在の単一core driverは待機やspin lockを行わない |
| Other | 未知の非0 status |

回復可能な失敗を無視せず、失敗時の出力値を成功の代わりに返さない。runtimeのdefectと、C側がmain専用操作をISRから直接誤用した場合は引き続き停止契約であり、cleanupを実行する保証はない。

## C ABI

生成された`emela_gpio.h`を宣言の正とする。

```c
int32_t emela_gpio_configure(EmelaGpioPin pin, EmelaGpioMode mode,
                            EmelaGpioPull pull, EmelaGpioPinState initial);
int32_t emela_gpio_read(EmelaGpioPin pin, EmelaGpioPinState *out);
int32_t emela_gpio_write(EmelaGpioPin pin, EmelaGpioPinState state);
int32_t emela_gpio_toggle(EmelaGpioPin pin);
int32_t emela_gpio_bind_interrupt(EmelaGpioPin pin);
int32_t emela_gpio_enable_interrupt(EmelaGpioPin pin, EmelaGpioEdge edge,
                                   EmelaInterruptPriority priority);
int32_t emela_gpio_disable_interrupt(EmelaGpioPin pin);
int32_t emela_gpio_isr_read(EmelaGpioPin pin, EmelaGpioPinState *out);
int32_t emela_gpio_isr_write(EmelaGpioPin pin, EmelaGpioPinState state);

/* 生成側が実装し、BSPが呼ぶ。 */
int32_t emela_gpio_has_binding(int32_t pin);
void emela_gpio_dispatch(int32_t pin, int32_t level);
```

Intとenumはint32_t、`EmelaGpioPin`は`int32_t id`のstruct。C modeはInput=0、PushPull=1、OpenDrain=2、Analog=3。Input/Analogのinitialは未使用値0。Pull、PinState、Edge、InterruptPriorityも生成定数を使い、BSPのレジスタ値へ変換する。read失敗時のoutは使わない。

Cには`GpioConfig`・`GpioEvent`の内部tag/union layoutを公開しない。headerはC境界の型のみを生成し、payload付きenumがmodule内に存在するだけで拒否しない。一般の`@external(c)`の型規則は変更しない。

statusは[単一定義表](../../src/peripheral_status.rs)から共通header定数とGpioErrorKindへの変換を生成する。0=成功、-1=InvalidArgument、-3=NotConfigured、-4=Unsupported、-6=Busy、-7=ResourceConflict、-8=InvalidState、-9および未知値=Other。-2（容量）と-5（timeout）は通信向けの予約で、GPIOには対応する分類を追加しない。

## 確認範囲

host C試験では全config variantの分解、既知/未知status、private callbackとconst/alias、別pinのread/write、固定エラーの捕捉、ISRでheap確保が増えないこと、禁止経路のコンパイル診断を確認する。

Renodeでは外部入力から実IRQへ入り、2pinの共有IRQ、片方だけの停止、Rising/Falling、停止後の再設定、未binding・不正pin・未設定・予約・優先度競合・Analogの対応差を確認する。STM32のopen-drain電気動作は同梱モデルが実装しない。C3はrepositoryの限定モデルであり、ROM/Flash起動・アナログ電圧・IRQ応答時間・優先度の実機での競合を確認したとは扱わない。再実行方法・資源量・実測結果は[試験記録](../gpio-validation.md)に記載する。

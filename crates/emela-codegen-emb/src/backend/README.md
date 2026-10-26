# Flow IR backend

通常の `emela_codegen_emb::compile_flow` は、このbackendを使う。公開API、生成file名、C ABI、固定容量の設定は従来の契約を引き継ぐ。

```text
公開compile_flow
  → compiler::compile_flow
  → CompiledModule
  → board::link
      → LinkPreparation（元の生成物のcopyを制限付きAPIで編集）
      → finish(roots)による検証
      → LinkedModule
      → compiler::emitによる配置・runtime計画・最終C生成
```

`compiler/`は具体的な接続先をimportせず、`Target`の契約を使う。`board/`は周辺機器・ISRの特殊化と検査、ABI接続、起動処理を担当する。`cnative/`は同じ共通生成物をホストから実行するlibrary test用の接続先であり、単一threadのplatform実装を持つ。

## 配置と責任

- `compiler/mod.rs`: 入力の正規化・検証、closure・coerce・handler等の準備、生成物の管理、共有の生成処理。
- `compiler/emit.rs`: 検査済み生成物の配置・runtime計画と最終C出力。
- `compiler/features/`: Flow IRの機能ごとの入口。生成と、その機能に固有のbuffer・長さ・所有権・heap・runtime要求の判断を集める。
- `compiler/foundation/`: 配置・所有権・接続準備・共有計画。`traversal.rs`と`reachability.rs`を、Coreからの準備や最終C生成でも共有する。
- `tests/separation.rs`: 同じ生成物からのboard/native接続、出力buffer、managed値、接続側への依存を確認する内部試験。
- crate直下の`tests/`: 公開API経由の回帰・生成C実行・対応表の試験。

enum全体を受け取る解析dispatcherはvariantを網羅し、処理不要なものも明示する。子のIRは共有の走査・dispatcherへ戻す。

`CompiledModule`のIRとside tableは接続側から変更できない。`LinkPreparation`は型と関数の追加、検査可能なcallback ABIの変更だけを許し、`finish`がID・capture・const metadata、関数の整合性を確認する。captureは検査用copyで暗黙の定義として扱い、実際のABIには追加しない。新rootを含むDeferred poolの要求もここで再計画する。

## 実装・テストの対応表

設定はcrate直下の`inventory/owners.json`、`containers.json`、`tests.json`にある。variantに専用入口と関連テストを明示し、宣言・fieldは担当との関係を記録する。fieldを独立した実装数やテスト数には加算しない。

[生成された対応表](../../docs/generated/flow-backend-inventory.md)は入口・追加の段階・関連テストへのリンクを持つ。テストはターゲットと完全な名前で識別する。関連付け・入口・テストの存在は、実装やテストの正しさ・条件の網羅性を保証しない。review状態は自動更新しない。

生成・検査コマンドと設定の更新方法は[crateのREADME](../../README.md#対応表の生成と検査)を参照する。

## 実装境界と入力

中心となるAPIは `compile_flow(&emela_lir::Module, &Options)` である。入力は `Stage::Flow` の構造体であり、dump文字列を解析しない。型・制御・失敗辺を検証し、closure変換、値レイアウト、所有権、suspend継続、必要なruntime生成をこのcrateで行う。`emela_lir::to_heap` およびその後のHeap IRパスは呼ばない。

型やvariantの定義は [LIR](../../../emela-lir/src/ir.rs)、[Coreのbuiltin](../../../emela-core/src/intrinsics.rs) を参照する。標準moduleはCoreの同梱ソースへ登録する。組み込みの生成C ABIはホスト向けC backendとは独立している。

| API | 入力・役割 | 現在の境界 |
| --- | --- | --- |
| `compile_flow` | Flowを正規化・検証してCを生成 | 未解決の型・effect集合、不整合なABI、Heap IR専用要素は拒否する。外部C境界とISRには後述の制約がある。 |
| `verify_flow` | Flowの型・stage・制御辺を検証 | 共通LIR verifierに加え、Heap専用RuntimeOpと未lowerのsource builtinを拒否する。資源設定・C ABIの検査は出力段でも行う。 |
| `flow_from_core` | 既存のCore単相化と `emela_lir::from_core` を使う便宜API | Core入力・既存lowering経路の制約を受ける。正常Flowを受け取るAPIの範囲と同一ではない。 |
| `compile` | `flow_from_core` と `compile_flow` を順に呼ぶ | Core便宜APIと同じ制約を受ける。Emelaソースを解析するAPIではない。 |
| `derived_binding` | 閉じたFlow型の構造比較・構造表示のbindingを作る | `DerivedOperation::Eq` / `Show` を指定し、producerがFlowへ追加して呼ぶ。sourceのtrait実装を選択・書き換える機能ではない。 |

### Core便宜APIとsourceの制約

正常なFlow入力への対応から、任意のsourceが既存frontend経由でそのFlowになることは導けない。特に次の境界がある。

- **成長する再帰型**：`Grow[A]` のフィールドに `Grow[List[A]]` が現れるような型族を含むCore moduleは、未使用の定義でも `flow_from_core` / `compile` が `Unsupported` として拒否する。[`derived::check_core_input`](../derived.rs) と [`type_expansion`](../type_expansion.rs) による入口検査である。Flow producerが既存IRで有限の入力を作り、必要な構造操作を `derived_binding` で表せば、`compile_flow` の対応対象になる。sourceからそのFlowを自動生成する接続は提供していない。
- **直接の関数値const**：現在の本体 [`emela-core/src/lower.rs`](../../../emela-core/src/lower.rs) の `signature` は、constが返すfn型を通常関数の宣言と同じように扱うケースがあり、`const APPLY = fn(x: Int) { x + 1 }` を正しいCoreへlowerできない。値として返す関数のeffectをconst initializerのevidence引数にしない処理も必要になる。sourceの回帰試験では関数値を `(関数, ())` のtupleへ格納して呼び出す。backendのconst cache・closure対応は有効なCore/Flow入力に対するものであり、このsource loweringには別途修正が必要になる。
- **汎化effectのAsyncへの具体化**：現在の本体 [`emela-core/src/mono.rs`](../../../emela-core/src/mono.rs) には、未確定effect集合をAsyncへ具体化した際にLambdaと間接Callの `suspends` の印を更新できないケースがある。例は `const APPLY = fn(callback) { callback() }` を `Fiber.join` するcallbackへ適用する場合である。Flow producerは確定した型・suspend/failure/evidence ABIを与える必要がある。source汎化constの回帰試験は同期・failure・通常effectを扱い、このAsync具体化の成功を保証しない。

`derived_binding` は既存のexternal metadataを使うbackend専用契約である。型はclosedかつ有効で、Unit/Neverは引数表現がないため対象外。Eqは関数値を比較せず、Showは関数値を `<fn>` と表示する。sourceのユーザー定義Eq/Showを使う場合は、その呼出しをproducer側で残す。

## 値表現・メモリ・所有権

| 基盤 | 実装される値・操作 | 資源と実行文脈 |
| --- | --- | --- |
| 静的なC値と定数 | scalar、固定レイアウトのtuple/enum、String literal/view、静的に解決できるclosure/handler | 使用する値・helperだけを出力する。大きなinline値や同期再帰もC stackを使用する。 |
| 固定heap | List、再帰型の箱、escapeしたString、動的closure/handler/evidence、Cell、Anyのbox | moduleごとに上限を確保し、システムの `malloc` を呼ばない。main側で参照管理・解放を行う。 |
| 機能別runtimeとpool | Fiber、suspend frame、task/event、Deferred、resume、失敗・取消の継続 | 到達可能な機能に応じて出力する。heapとは別の固定領域も持ち、各poolを独立に設定する。 |

- `Int` / `Bool` はCの32 bit整数、`Int64` は64 bit整数、`Float` はbinary64の `double`。Unit/Neverには通常のC値を割り当てない。
- nullary enumは整数、payloadを持つenumとtupleはinlineの構造体を基本とする。List等の参照フィールドがあるだけでは構造体全体をheapへ移さない。再帰する辺に箱を置き、成長する型族の辺では型付きの動的表現を使う。
- 所有参照の追加・解放はbackendが挿入する。借用したフィールドの読取り、分岐の結果、並列loop代入、capture、失敗値、suspend保持も対象になる。結果を使わない操作でも必要な副作用・容量検査・所有権解放を維持する。
- 最後の参照からの深い値の解放は、解放待ちリストを使い反復する。Cell・closure・Any等によるcycleを扱う経路では固定heap内のcycle回収も行う。回収やデータ量に比例する操作を含むため、固定容量であることから一定の実行時間は保証しない。
- `@external type` は不透明なhandleとして保持する。`Any` / `EmelaError` はタグ付きdescriptorを使い、一般値のboxとerror payloadを区別する。Cellは共有するtyped contentを所有し、creatorのstack frameから独立した寿命を持つ。source capability制約をheap化によって緩めない。

### 設定値

| `Options` の設定方法 | 既定値 | 意味 |
| --- | --- | --- |
| `with_heap_bytes` | 0 byte | heapを使用する入力には正の容量が必要。0ではheapが必要な経路を診断する。 |
| `with_peripheral_buffer_capacity` | 256 word | 標準UART/I²C/SPI bridgeの1方向の転送buffer。1〜65,535 wordで、1 wordはuint16_t。C stack上の配列を使う。 |
| `with_string_capacity` | 256 byte | 動的String生成の上限。設定範囲は1〜512 byte。 |
| `with_const_diagnostic_bytes` | 1024 byte | runtime constの保存するdefect message領域。必要な場合だけ出力し、heapからは確保しない。 |
| `with_fiber_capacity` | 8 | 実行中または完了後に未解放のFiber handle数。 |
| `with_frame_capacity` | 32 | 同時に生存するsuspend frame数。 |
| `with_frame_bytes` | 256 byte | headerを含む1 frameの上限。target依存の型付きframeサイズを生成Cのstatic assertで検査する。 |
| `with_task_capacity` | 8 | 外部task数。 |
| `with_event_capacity` | 8 | 完了event queueの長さ。 |
| `with_deferred_capacity` | 8 | 生存するDeferred数。 |

設定範囲外は `InvalidOptions`、実行中の容量不足はdefectとする。heap容量には管理情報・alignmentも含まれ、指定byte数すべてがpayloadに使えるわけではない。stack総量の証明やtargetのlink map評価はこのライブラリでは行わない。

### String・数値・Unicode

`EmelaString` はデータpointer・uint16のbyte長・backing ownerを持つdescriptorであり、各変数に256 byte配列を埋め込まない。NUL終端を必要とせず、埋込みNULを保持する。literalは読み取り専用領域を参照する。

`StringBufferPlan` は生成箇所ごとの容量と生存期間を計画し、連結の中間bufferを抑え、短命なscratchを再利用する。String戻り値には必要に応じcallerが渡す出力bufferを使う。長寿命の値・capture・suspend保持では、元のstackを参照し続けず所有backingまたはframeへ保持する。容量が足りなければ切り詰めずdefectにする。任意の制御フローで最小のscratch量になる保証はない。

Int / Int64の十進出力は最大11 / 20 byte、動的Float出力は最大25 byteをproducerの容量計画へ登録する。Floatの短い往復可能な表記、±0、非有限値、subnormal、固定/指数表記の切替を実装する。`-0.0000012345678901234567` は25 byteを使う。Float.parseはpointer+長さで文法を検査し、固定作業領域でbinary64へ丸める。不正形式はNone、overflow/underflowは符号付きInfinity/0をSomeで返す。動的変換はhost libcの文字列変換や動的確保に依存しない。targetのdoubleにはIEEE 754 binary64を要求する。

書記素処理は同梱するUnicode 18.0.0のtableとextended grapheme clusterの規則を使う。データ出典は [`unicode_grapheme.rs`](../unicode_grapheme.rs)、公式vectorは [`GraphemeBreakTest-18.0.0.txt`](../../tests/data/GraphemeBreakTest-18.0.0.txt) にある。String長、chars、空separatorでのsplitはこの境界を使い、byte長・部分文字列検索・code point操作とは区別する。

### const

静的に評価できるscalar/String等は定数へ畳む。評価のfuel・深さ・出力サイズに上限を設け、畳めない場合はruntimeの初回参照時に評価してcacheする。再帰helper、外部呼出し、closureやaggregateを返すconstも、有効な入力IRと必要な資源があればこの経路で扱う。通常のdefectはcontext付きでcacheし、自己参照は専用の診断を行う。const初期化とその診断に必要な記憶領域も固定容量である。

汎化したconstは `const_origins` が既存Core単相化APIのoriginを追跡し、既存Flowのprivate metadataへcanonical値と型別specializationの対応を保存する。`const_bridge` が型別レイアウトとcallableの橋渡しを生成し、source上の初期化を型ごとに繰り返さない。origin metadataのない直接Flow入力は、異なるconst IDを独立したcacheとして扱う。source/Coreの関数値constとAsync具体化には[Core便宜APIの制約](#core便宜apiとsourceの制約)がある。

## 失敗・defect・suspendの契約

`fails` は回復可能な失敗辺として扱い、所有するerrorをcatch・上位関数・FiberのCauseへ渡す。通常のreturn/fail/break、取消、scopeの終了では必要なProtect cleanupを実行する。cleanup自身がsuspendする場合も、その完了を待って元の結果や失敗を配送する。永久に完了しないcleanupを時間だけで強制終了する契約はない。

**defectは即時停止契約である。** kind・messageのpointer・byte長を `_Noreturn emela_board_defect` へ渡す。Protect cleanupや参照解放を行わず、Fiber.attemptでもDieとして捕捉しない。通常のEmelaのdefect unwindと異なるtarget固有の意味論であり、hookが安全な出力状態への移行・停止/resetを担当する。公開入口まで届いた未処理のfailsは `EMELA_DEFECT_UNHANDLED_ERROR` と静的message `unhandled error` にする。

suspendは固定pool上の継続と協調schedulerで実装する。Scope/Fork/Join/Await/Interrupt、Deferred、Cause、suspend closure、handlerのresume、managed値の保持・解放を扱う。schedulerはmain文脈で呼ぶ。`emela_scheduler_run(budget)` はdispatchまたはeventの処理数を制限するが、同期の同一fiber内resumeや次のsuspendまでの処理時間を分割する上限ではない。WCET・任意の応答時間・全stack使用量の保証は提供しない。

## 外部C・ISR・ボードとの境界

基本の生成ファイルは `emela_embedded.h`、`emela_board.h`、`emela_embedded.c` の3つ。標準handlerを使う場合は、到達する機能に応じてGPIO/UART/I²C/SPI/Clock/ConsoleのC headerと共通 `emela_peripheral.h` も出力する。必要なhookとtask ABIだけをheaderに宣言する。C側の宣言は生成headerを正とし、ホスト向けC backendの型定義やABIを流用しない。

| 境界 | 契約 |
| --- | --- |
| scalarの同期external | 通常の `@external(c, ...)` bindingを利用する。非scalar、fails付きの同期externalには明示的な `with_external_adapter` を使う。 |
| `ExternalResult::Borrowed` | adapterの戻り値をEmela側でownしてから返る。C側は取得完了まで値を生存させる。C calleeのstackへのpointerは返さない。 |
| `ExternalResult::Owned` | 全managed fieldを含む所有値をEmelaへ移す。ownerは当該moduleの固定heapに属し、mallocやC stack由来ではない。 |
| `ExternalResult::StringBuffer` | callerから追加で渡すbufferとbyte容量へ書き、ownerなしStringを返す。String戻り値専用。書込み前に容量を検査する。 |
| 同期adapterの引数 | 呼出し中だけの借用。保持するなら独立した所有参照を取得する。ownerなしStringのbyte列はcopyする。 |
| suspend external | `with_task_adapter` でC symbolを明示mappingする。開始時に `EmelaTask` を渡し、完了を通知してmain側で継続を再開する。開始引数を後で使うならC側でsnapshot/retainする。 |
| managed task結果 | 型付きproducer/context leaseを使う。producerとrelease callbackはmain側・event lock外で呼び、ISRでは呼ばない。scheduler・Emela入口へ再入しない。 |
| C callback | Emela関数値を通常のC関数pointerへ自動変換するthunkは提供しない。runtime capabilityを含む外部署名にも制約があり、条件外は診断する。 |
| moduleの実行文脈 | scheduler・heap・生成Emela入口はmain側で扱う。複数thread/coreからの同時呼出し・再入を自動で同期するAPIではない。 |

| 生成hook/API | C側の責任 |
| --- | --- |
| `emela_board_defect` | 使用するプログラムで要求される場合に実装する。戻らず、messageは長さ付きの借用として扱う。ISRから到達する場合も安全に停止する。 |
| `emela_board_debug` | DebugPrintが使う長さ付きbyte列を呼出し中に読み切るかcopyする。行末の出力を担当する。 |
| `emela_board_init` / `emela_board_idle` | main起動wrapperを生成する場合の初期化・idle処理を担当する。 |
| `emela_board_event_lock` / `emela_board_event_unlock` | task/event経路で必要な割込み状態の保存・復元と排他を実装する。 |
| `emela_task_complete_*` | 固定event queueへ通知するAPI。ISRから呼べるが、その場でEmelaを再開しない。stale/重複通知は拒否し、queue不足はdefectにする。 |

ISRとして公開するEmela関数は `with_isr_entry` で指定する。到達先にheap値・参照管理・再帰・suspend・runtime const初期化・動的call・Hold/Unholdがある場合は拒否する。外部C symbolは `with_isr_safe_external` で明示し、非scalar adapter等の追加制約も満たす必要がある。この検査は外部関数の実装や実行時間を証明しない。ISRのstack総量は算出しない。

標準 `Std.Embedded` effect・共有handlerとC bridgeは[共通インターフェース](../../docs/embedded-interfaces/README.md)と[ABI契約](../../docs/embedded-interfaces/common.md#c-bridge)で提供する。これらの標準bindingでは汎用adapterの明示mappingは不要である。[ボードpackage](../../README.md#同梱bsp)はSTM32F4とESP32-C3のstartup/runtime、standalone build/flash script、Rustのboard probeとFlash起動の観測runnerを提供する。`Output.exports` から公開C symbolとsuspend入口かどうかを取得できる。[CLIとdriver](../../../emela-cli/README.md)はこのCore便宜APIを利用し、target/board選択、TOML設定、module置換、firmware buildとflash時の鮮度照合へ接続する。ツールチェーンの自動取得は行わない。BSPはGPIO・Console・Clock等の対応部分を実装し、未提供のdriverは利用側が補う。

## APIの最小利用例

既にFlow producerを持つ場合は `compile_flow` へ渡す。最小のFlowを直接作る例を示す。

```rust
use emela_codegen_emb::{Options, compile_flow};
use emela_core::TypeId;
use emela_lir::{Const, FnBuilder, Module, Operand, Term};

let mut flow = Module::new(emela_core::Module::new());
let mut function = FnBuilder::new(&flow.core, "answer");
function.set_ret(TypeId::INT).set_exported();
function.term(Term::Return(Some(Operand::Const(Const::Int(42)))));
flow.add(function.finish());
let output = compile_flow(&flow, &Options::default()).expect("compile Flow IR");
let directory = std::path::Path::new("generated");
std::fs::create_dir_all(directory).expect("create output directory");
for file in output.files {
    std::fs::write(directory.join(file.path), file.contents).expect("write generated C");
}
```

利用側のCプログラムは生成headerをincludeして公開入口を呼び、要求されたhookを実装して生成Cとlinkする。入口名・引数・String出力buffer・所有権操作は生成headerに従う。heapを使うプログラムでは、例えば `Options::default().with_heap_bytes(4096)` とする。これは4096 byteで任意のプログラムが動くという推奨値ではない。

## Flowの入力

入力は`emela_lir::Module`の`Stage::Flow`で、dump文字列を解析するAPIではない。型・error/effect集合を確定させ、呼び出しとfailure・suspend・evidenceのABIを一致させる。組み立てには[`FnBuilder`](../../../emela-lir/src/build.rs)、構造検証には`verify_flow`、C ABI・資源設定まで含む検証には`compile_flow`を使う。

`Module`はCoreの型定義とLIRの関数群を持つ。各関数のarenaをIDで接続し、変数は一度だけ定義する。`Seq`は文列と終端からなり、compound文の結果は`Term::Yield`で渡す。結果が不要でも副作用とdefectを維持する。`Repr::Ref`は必ずheap pointerへ配置する指定ではなく、Core型からinline構造体・descriptor・参照を選ぶ。`Const::Unit`はtupleや構成子の添字、`List[()]`の長さを保つための、C値を持たない枠としても使う。

定義は[`ir.rs`](../../../emela-lir/src/ir.rs)と[`repr.rs`](../../../emela-lir/src/repr.rs)を参照する。個々のvariantと担当・関連テストは[生成された対応表](../../docs/generated/flow-backend-inventory.md)へ集約する。

producerは次の条件も満たす。

- 関数・closureの引数にはC表現のある値だけを並べ、evidence表を取る場合は最後へ置く。`fails`・`suspends`・evidenceの有無を定義・型・呼び出しで一致させる。operation自身の型引数は[LIR verifier](../../../emela-lir/src/verify.rs)のboxed表現に従う。
- 失敗可能な呼び出しとFiber操作には`on_fail`を付け、失敗しないものには付けない。`CauseRaise`は常にfailure辺を要求する。`flow_from_core`は失敗しない関数内の辺を正規化するが、直接Flow入力は局所blockへの`OnFail::Break`等で受ける必要がある。
- `Field`は構成子の一致、`ListHead`・`ListTail`は非空をproducer側で保証する。構成子の引数とhandlerのoperationは定義順に並べる。
- literal switchは`i32`・`i64`でdefaultが必要。tag switchはdefaultがなければ全構成子を覆う。Float・Stringのliteralパターンは比較と`If`で表す。
- 関数・closure本体を`Yield`で終えない。cleanupの正常終了は値なし`Yield`とし、cleanup外へのreturn・break・recurを行わない。`Recur`は最内側loopの引数を並列に更新し、Protect・closure境界を越えない。

整数の加減乗算・符号反転は32/64bitでwrapする。0除算はdefect、MIN/-1はMIN、MIN%-1は0とする。FloatはIEEE除算と`fmod`、String比較はbyte列の内容比較を使う。論理反転は`i32`だけで、比較・論理反転の結果も`i32`。String連結には`StringAppend`を使い、構造的な比較は型ごとの関数を呼ぶ。FloatからIntへの丸めは非有限値・範囲外でdefectとなる。Hold/UnholdはFiberの取消延期であり、hardwareの割り込みmaskではない。

FlowとHeapはRustのデータ型を共有するが、次の要素はこのbackendのFlow入口で拒否する。

| 対象 | Flowでの表現・役割分担 |
| --- | --- |
| `Stage::ClosuresConverted` / `Stage::DefectsExplicit` / `Stage::StateMachines` / `Stage::RefCounted` | `Stage::Flow` を渡す。後段のHeapパスは実行しない |
| `Value::MakeClosure` / `Value::EnvGet` | `Value::FnRef` / `Value::Closure` を渡す。環境の配置はbackendが決める |
| `Stmt::Dup` / `Stmt::Drop` | 明示RCを渡さない。backendが所有権処理を挿入する |
| `Term::Unwind` / `OnFail::Handle` | `Term::Defect`、`OnFail::Break` / `OnFail::Propagate` と `Protect` で表す |
| `Stmt::Let` の `on_defect` / `on_suspend` | ともに `None`。Heap用のlanding・`Suspend` のstate/saved情報を付けない |
| `RuntimeOp::Defect` / `RuntimeOp::DefectTake` / `RuntimeOp::DefectResume` | 保留defectの明示unwindはHeap専用 |
| `RuntimeOp::Yield` / `RuntimeOp::YieldPerform` | Heapの公平性yield。組み込みschedulerが必要な実行制御を生成する |

また、次のsource builtinはFlowへ変換するまでに置き換える。生の `Value::Builtin` として渡すと拒否される。

| builtin | Flowでの表現 |
| --- | --- |
| `Builtin::Panic` / `Builtin::Todo` | message付き `Term::Defect` |
| `Builtin::Dbg` / `Builtin::Inspect` / `Builtin::ShowNested` | 単相化で選んだShowや構造表示の関数呼び出し、必要なruntime操作。dbgは元の値を返す |
| `Builtin::ShowString` / `Builtin::DebugPrint` | `RuntimeOp::ShowString` / `RuntimeOp::DebugPrint` |
